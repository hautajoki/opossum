//! `CxBackend` over the transform state (DESIGN §9, §11.3): one [`HookBackend`] per hook call, anchored at the node
//! being dispatched, with the phase table of `rbxts_plugin::CxBackend` enforced.

use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use rbxts_luau::{AnyIdentifier, Effects, Expr, Stmt, create};
use rbxts_plugin::erased::DynInstance;
use rbxts_plugin::{
    Aborted, AccessPosition, Arg, BinOp, CallKind, CallSite, Callee, Cx, CxBackend, FileInfo, GlobMatcher, JsDocTag,
    LocalCounts, LuauIdent, ObjectEdits, PResult, PluginDiagnostic, Utf16Pos, text_literal,
};
use rustc_hash::FxHashMap;
use rbxts_project::{PackageInfo, ProjectServices, ResolvedModuleInfo};
use tsrs_ast::{Diagnostic, DiagnosticExt, Kind, Node, SourceFile, Symbol, new_diagnostic};
use tsrs_checker::Checker;
use tsrs_compiler::Program;
use tsrs_core::{CompilerOptions, P};
use tsrs_diagnostics::{Category, new_ad_hoc_message};

use super::HookKind;
use super::dispatch;
use super::facts;
use super::hoist_check;
use super::file_state::{EffectMark, HookFrame, SpillableHoist, SpillableLocal, declared_locals};
use super::host::PluginHost;
use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_call_expression::{
    PropertyKey, create_ordered_property_call, fix_void_arguments_for_roblox_functions,
};
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_super_keyword::transform_super_keyword;
use crate::nodes::statements::transform_expression_statement::{
    is_statement_lowered, transform_expression_statement_inner,
};
use crate::nodes::transform_optional_chain::{ChainItem, transform_chain_item};
use crate::shared::diagnostics::node_range;
use crate::util::add_one_if_array_type::add_one_if_array_type;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::ensure_transform_order::ensure_transform_order_with;
use crate::util::evaluation::effects::{
    effects_commute, get_effects, get_effects_of_statements, is_late_read, join_effects,
};
use crate::util::is_method::{is_method, is_method_from_type};
use crate::util::traversal::{skip_downwards, skip_upwards};
use crate::util::types::{get_first_construct_symbol, get_first_defined_symbol};
use crate::util::validate_not_any::validate_not_any_type;
use crate::util::value_to_id_str::value_to_id_str;
use crate::util::wrap_return_if_lua_tuple::wrap_return_if_lua_tuple;

/// Which column of the phase table a hook is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Claim hooks and `class`: everything.
    Claim,
    /// `begin_file`, `end_file`: no lowering primitives.
    File,
    /// `classify_symbol`: facts only.
    Classify,
    /// `explain_checker_diagnostic`: facts and reports.
    Explain,
}

impl Phase {
    pub(crate) fn of(hook: HookKind) -> Phase {
        match hook {
            HookKind::Call | HookKind::Assignment | HookKind::Class | HookKind::Expression | HookKind::Statement => {
                Phase::Claim
            }
            // property_key answers with a key: it lowers nothing
            HookKind::BeginFile | HookKind::EndFile | HookKind::PropertyKey => Phase::File,
            HookKind::Classify => Phase::Classify,
            HookKind::Explain | HookKind::Setup | HookKind::Finish => Phase::Explain,
        }
    }
}

/// The rows of the phase table that can refuse (`report` is refused only in `classify_symbol`, with its own
/// message).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Lowering,
    FileLevel,
}

/// The syntactic shape of a claimed call, for `compile_receiver` and `compile_call`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CallShape {
    /// `f(…)`: the callee is compiled as a whole.
    Call,
    /// `super(…)`
    Super,
    /// `obj.name(…)`
    Property { access: P<Node>, name: &'static str },
    /// `obj[argument](…)`
    Element { access: P<Node>, argument: P<Node> },
    /// `new C(…)`
    New,
    /// `@D` (`call` is `None`) or `@D(…)`.
    Decorator { call: Option<P<Node>> },
}

/// What roblox-ts calls for an optional call (`o.m?.(…)`, `o[k]?.(…)`), once the chain lowering has read the
/// callee into a local and nil-checked it: the call itself is lowered inside that check.
#[derive(Clone, Debug)]
pub(crate) struct OptionalCall {
    /// The callee's value (the chain's temporary, known not to be nil here).
    pub function: Expr,
    /// The argument roblox-ts passes first: the receiver of a method call (bound to a local), `self` for
    /// `super.m?.()`; `None` for a call that is not a method call.
    pub self_arg: Option<Expr>,
    /// The receiver for `compile_receiver`.
    pub receiver: OptionalReceiver,
}

/// The receiver of an optional call, for `compile_receiver`.
#[derive(Clone, Debug)]
pub(crate) enum OptionalReceiver {
    /// A method call's receiver, already in a local (`_self`).
    Bound(Expr),
    /// A non-method call's receiver as compiled, in no local: the callee is read from it before the nil check,
    /// so `compile_receiver` binds it to a local declared before that read (`PluginFileState::optional_receivers`).
    Unbound(Expr),
    /// `super.m?.()`: `super` is not a value.
    Super,
}

/// How an expression hook's own node is lowered when it was offered after its base was compiled (a chain link
/// inside an optional chain, see `PluginFileState::eager_offers`).
#[derive(Clone, Debug)]
pub(crate) enum EagerLowering {
    /// A chain link on its compiled base.
    Item { base: Expr, item: ChainItem },
    /// An optional call inside its nil check.
    OptionalCall { call: OptionalCall, item: ChainItem },
}

/// A call being offered to plugins.
#[derive(Clone)]
pub(crate) struct CallContext {
    pub site: CallSite,
    pub shape: CallShape,
    /// The compiled base: the receiver of a property or element call, else the callee. Given by the chain lowering
    /// (eager), or compiled on first need (lazy).
    pub base: Option<Expr>,
    /// The effect marks right before and after a lazily compiled base, measured against the offer's own prereq
    /// scope: a declining plugin may leave what compiling the base left (the next plugin, or roblox-ts, reuses the
    /// base), but nothing from before it. `None` when the base was not compiled lazily into that scope.
    pub base_marks: Option<(EffectMark, EffectMark)>,
    /// The base was compiled lazily inside a capture: its prereqs are the plugin's, so it is never reused.
    pub base_captured: bool,
    /// The chain link of a chain call, to lower the call on its compiled base.
    pub item: Option<ChainItem>,
    /// An optional call (`o.m?.()`): lowered inside the chain's nil check on its callee.
    pub optional: Option<OptionalCall>,
}

impl CallContext {
    pub(crate) fn new(site: CallSite, shape: CallShape, base: Option<Expr>) -> CallContext {
        CallContext { site, shape, base, base_marks: None, base_captured: false, item: None, optional: None }
    }
}

/// One hook call's backend.
pub(crate) struct HookBackend<'s, 'a> {
    pub state: &'s mut TransformState<'a>,
    pub host: &'a PluginHost,
    prereqs: Option<&'s mut Prereqs>,
    captures: Vec<Prereqs>,
    call: Option<&'s mut CallContext>,
    plugin: usize,
    hook: HookKind,
    phase: Phase,
    anchor: P<Node>,
}

/// Runs one hook of `plugin`. `prereqs` is the scope `prereq` / `push_to_var` / `compile_*` write to (claim hooks
/// only); `call` the call being claimed. A hook's `Err` without a new error diagnostic is reported as an internal
/// error naming the plugin; `Err(())` means the hook failed (its error is recorded).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_hook<R>(
    state: &mut TransformState<'_>,
    prereqs: Option<&mut Prereqs>,
    call: Option<&mut CallContext>,
    plugin: usize,
    hook: HookKind,
    anchor: P<Node>,
    f: impl FnOnce(&dyn DynInstance, &mut Cx<'_>) -> PResult<R>,
) -> Result<R, ()> {
    run_hook_framed(state, prereqs, call, plugin, hook, anchor, f).0
}

/// [`run_hook`], also returning the hook's frame ([`HookFrame`]): what it did that its node can no longer be
/// declined after, and the hooks that ran inside it. The frame is pushed before the hook and popped after it; the
/// enclosing hook's frame counts this hook as nested (`classify_symbol`, memoised, does not count).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_hook_framed<R>(
    state: &mut TransformState<'_>,
    prereqs: Option<&mut Prereqs>,
    call: Option<&mut CallContext>,
    plugin: usize,
    hook: HookKind,
    anchor: P<Node>,
    f: impl FnOnce(&dyn DynInstance, &mut Cx<'_>) -> PResult<R>,
) -> (Result<R, ()>, HookFrame) {
    let host = state.plugins.expect("the build has plugins");
    let diagnostics_before = state.diagnostics.diagnostics.len();
    if hook != HookKind::Classify
        && let Some(parent) = state.plugin_file.frames.last_mut()
    {
        parent.nested_hooks += 1;
    }
    state.plugin_file.frames.push(HookFrame { node: Some(anchor), ..HookFrame::default() });
    let approved_before = state.plugin_file.approved_count();
    host.enter(Some(plugin), hook);
    let result = {
        let mut backend = HookBackend {
            state: &mut *state,
            host,
            prereqs,
            captures: Vec::new(),
            call,
            plugin,
            hook,
            phase: Phase::of(hook),
            anchor,
        };
        let mut cx = Cx::new(&mut backend);
        f(&*host.plugins[plugin].instance, &mut cx)
    };
    host.exit();
    let mut frame = state.plugin_file.frames.pop().expect("this hook's frame");
    // an approval (`is_usable_local_name`, a `&self` query) changes later naming: it touches state
    frame.touched |= state.plugin_file.approved_count() != approved_before;
    let result = match result {
        Ok(value) => Ok(value),
        Err(_) => {
            if !has_errors_since(state, diagnostics_before) {
                let message = format!(
                    "plugin `{}` returned Err from {} without reporting an error",
                    host.name(plugin),
                    hook.label()
                );
                state.plugin_internal_error(anchor, &message);
            }
            Err(())
        }
    };
    (result, frame)
}

/// Runs `classify_symbol` (no `PResult`; diagnostics are refused there).
///
/// What it reports (the internal errors of refused primitives, anchored at the symbol's declaration) leaves the
/// file's list for [`PluginFileState::classify_diagnostics`](super::file_state::PluginFileState): each checker asks
/// about a symbol once, from whichever of its files reaches it first, so the build reports these once per diagnostic,
/// apart from any file, whatever the checker count.
pub(crate) fn run_classify(
    state: &mut TransformState<'_>,
    plugin: usize,
    anchor: P<Node>,
    f: impl FnOnce(&dyn DynInstance, &mut Cx<'_>) -> Option<rbxts_plugin::Tag>,
) -> Option<rbxts_plugin::Tag> {
    let before = state.diagnostics.diagnostics.len();
    let tag = run_hook(state, None, None, plugin, HookKind::Classify, anchor, |instance, cx| Ok(f(instance, cx)))
        .ok()
        .flatten();
    let reported: Vec<P<Diagnostic>> = state.diagnostics.diagnostics.drain(before..).collect();
    state.plugin_file.classify_diagnostics.extend(reported);
    tag
}

pub(crate) fn has_errors_since(state: &TransformState<'_>, index: usize) -> bool {
    state.diagnostics.diagnostics[index..].iter().any(|d| d.category() == Category::Error)
}

/// The sources of plugin diagnostics, by the address of their ad hoc message (each diagnostic gets its own, leaked,
/// so an address is never reused).
static SOURCES: Mutex<Option<FxHashMap<usize, &'static str>>> = Mutex::new(None);

fn message_address(diagnostic: P<Diagnostic>) -> Option<usize> {
    diagnostic.message().map(|message| std::ptr::from_ref(message) as usize)
}

/// The source a plugin diagnostic names (`PluginDiagnostic::source`, else its plugin's `diagnostic_source()`), for
/// the reporter's code slot: `error TS <source>: …`. `None` for every other diagnostic.
pub fn diagnostic_source(diagnostic: P<Diagnostic>) -> Option<&'static str> {
    let address = message_address(diagnostic)?;
    SOURCES.lock().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(&address).copied()
}

/// A plugin diagnostic as a tsrs diagnostic: the node's `getStart()` to its end, the hint on a line of its own,
/// related information attached. `plugin_source` is the plugin's `diagnostic_source()`; the diagnostic's own
/// source wins over it.
pub(crate) fn to_diagnostic(diagnostic: &PluginDiagnostic, plugin_source: Option<&'static str>) -> P<Diagnostic> {
    let at = |node: P<Node>, message: String, category: Category| {
        let (file, range) = node_range(node);
        let d = new_diagnostic(file, range, new_ad_hoc_message(&message), &[]);
        if category != Category::Error {
            d.set_category(category);
        }
        d
    };
    let d = at(diagnostic.node, message_text(diagnostic), diagnostic.category);
    if let Some(source) = diagnostic.source.or(plugin_source)
        && let Some(address) = message_address(d)
    {
        SOURCES.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(FxHashMap::default).insert(address, source);
    }
    if !diagnostic.related.is_empty() {
        let related: Vec<P<Diagnostic>> =
            diagnostic.related.iter().map(|(node, message)| at(*node, message.clone(), Category::Message)).collect();
        d.set_related_info(&related);
    }
    d
}

/// A plugin diagnostic's message text: the message, and the hint on a line of its own.
fn message_text(diagnostic: &PluginDiagnostic) -> String {
    match &diagnostic.hint {
        Some(hint) => format!("{}\n{hint}", diagnostic.message),
        None => diagnostic.message.clone(),
    }
}

impl HookBackend<'_, '_> {
    fn name(&self) -> &'static str {
        self.host.name(self.plugin)
    }

    /// Something the running hook did that its node can no longer be declined after (on its own frame).
    fn note_effect(&mut self, what: &'static str) {
        note_effect(self.state, what);
    }

    /// A primitive that changes compiler state ran (see [`HookFrame::touched`]).
    fn touch(&mut self) {
        self.state.plugin_file.emitted = true;
        if let Some(frame) = self.state.plugin_file.frames.last_mut() {
            frame.touched = true;
        }
    }

    /// A hook compiling its own node: an expression hook's node is lowered (it cannot be declined any more; a call
    /// hook's goes through `compile_claimed_call`, which notes it), a second time is an internal error (it would be
    /// lowered twice), and a statement cannot be compiled as an expression.
    fn check_self_compile(&mut self, nodes: &[P<Node>]) -> Result<(), Aborted> {
        let compiled_base = self.compiled_base();
        for &node in nodes {
            // everything else inside an ancestor would be offered and lowered a second time
            if node != self.anchor && is_ancestor(node, self.anchor) {
                return Err(self.misuse(
                    "compiled an ancestor of its own node (everything else inside it would be lowered a second time)",
                ));
            }
            // the base the chain lowering (or compile_receiver) compiled already would be evaluated a second time
            if node != self.anchor
                && let Some(base) = compiled_base
                && (node == base || is_ancestor(base, node) || is_ancestor(node, base))
            {
                return Err(self.misuse(
                    "compiled the receiver or callee of its node, which is compiled already (it would be evaluated a \
                     second time): use compile_receiver, or compile_expr of the node itself",
                ));
            }
            // roblox-ts lowers the operand of `delete` as the deletion itself, not as a read
            if self.hook == HookKind::Assignment && is_delete_operand(node) {
                return Err(self.misuse(
                    "compiled the operand of `delete`, which roblox-ts lowers as the deletion (it would delete it, not \
                     read it)",
                ));
            }
        }
        let own = nodes.iter().filter(|&&n| n == self.anchor).count();
        if own == 0 {
            return Ok(());
        }
        if self.hook == HookKind::Statement {
            return Err(self.misuse(
                "a statement hook cannot compile its own statement (compile_* takes expressions; lower the statement's \
                 parts)",
            ));
        }
        if self.hook == HookKind::Class {
            return Err(self.misuse(
                "a class hook cannot compile its own class (roblox-ts lowers it already; it would be lowered twice)",
            ));
        }
        if matches!(self.hook, HookKind::Expression | HookKind::Assignment) {
            let twice = own > 1 || self.state.plugin_file.frames.last().is_some_and(|f| f.self_compiled);
            if twice {
                return Err(self.misuse("compiled its own node a second time (it would be lowered twice)"));
            }
            if let Some(frame) = self.state.plugin_file.frames.last_mut() {
                frame.self_compiled = true;
            }
            self.note_effect("lowering it with compile_expr");
        } else if own > 1 {
            return Err(self.misuse("compiled its own node a second time (it would be lowered twice)"));
        }
        Ok(())
    }

    /// The node of the hook's offer whose value is compiled already, so compiling it (or a node inside or around it)
    /// again would evaluate it twice: a call's receiver or callee once the chain lowering or `compile_receiver`
    /// compiled it, an optional call's receiver, and the base of a chain link offered to an expression hook after
    /// its base was compiled.
    fn compiled_base(&self) -> Option<P<Node>> {
        if let Some(context) = self.call.as_deref() {
            if context.optional.is_some() {
                return receiver_node(context.shape);
            }
            context.base.as_ref()?;
            return match context.shape {
                CallShape::Property { .. } | CallShape::Element { .. } => receiver_node(context.shape),
                CallShape::Call | CallShape::New | CallShape::Decorator { .. } => Some(context.site.callee),
                CallShape::Super => None,
            };
        }
        if self.hook != HookKind::Expression {
            return None;
        }
        let (_, lowering) = self.state.plugin_file.eager_offers.iter().rev().find(|(n, _)| *n == self.anchor)?;
        let item = match lowering {
            EagerLowering::Item { item, .. } | EagerLowering::OptionalCall { item, .. } => item,
        };
        chain_base_node(item)
    }

    /// A temporary identifier no user local of the file can capture ([`PluginFileState::seam_hint`]).
    fn seam_temp(&mut self, hint: &str) -> LuauIdent {
        temp_id(self.state.plugin_file.seam_hint(self.state.source_file, hint))
    }

    fn hoist_unchecked(&mut self, value: Expr, hint: &str) -> Expr {
        self.note_effect("calling hoist");
        self.touch();
        let id = self.seam_temp(hint);
        self.state.plugin_file.top_level_temps.insert(id.id);
        self.state.plugin_file.main_chunk_locals += 1;
        self.state.plugin_file.root_inserts.push(create::local(id.clone(), Some(value)));
        Expr::TemporaryIdentifier(id)
    }

    /// Refuses a value to hoist that uses something the current top-level statement declares, which it would not read
    /// before that statement ([`hoist_check`]): an internal error naming the plugin, and `true`.
    fn refuse_hoist(&mut self, what: &str, value: &Expr) -> bool {
        let start = self.state.plugin_file.root_temp_start;
        let top_level = std::mem::take(&mut self.state.plugin_file.top_level_temps);
        let problem = hoist_check::hoist_problem(self.state, self.anchor, value, start, &top_level);
        self.state.plugin_file.top_level_temps = top_level;
        let Some(problem) = problem else { return false };
        self.misuse(&format!(
            "{what}: the value {problem}; a hoist is declared before the top-level statement around its node, where \
             it would not read the same (hoist only values that need nothing that statement declares)"
        ));
        true
    }

    /// Temporaries `stmts` declare at the file's top level, before every later statement (for [`hoist_check`]).
    fn note_top_level_temps(&mut self, stmts: &[Stmt]) {
        for statement in stmts {
            match statement {
                Stmt::VariableDeclaration(node) => {
                    for id in &node.left {
                        if let AnyIdentifier::Temporary(t) = id {
                            self.state.plugin_file.top_level_temps.insert(t.id);
                        }
                    }
                }
                Stmt::FunctionDeclaration(node) if node.localize => {
                    if let Expr::TemporaryIdentifier(t) = &node.name {
                        self.state.plugin_file.top_level_temps.insert(t.id);
                    }
                }
                _ => {}
            }
        }
    }

    /// Records the internal error for a call the phase table refuses, and says whether it was refused.
    fn refused(&mut self, what: &str, row: Row) -> bool {
        let allowed = match (self.phase, row) {
            (Phase::Claim, _) => true,
            (Phase::File, Row::Lowering) => false,
            (Phase::File, _) => true,
            (Phase::Classify | Phase::Explain, _) => false,
        };
        if !allowed {
            let message = format!("plugin `{}` called {what} in {}", self.name(), self.hook.label());
            self.state.plugin_internal_error(self.anchor, &message);
        }
        !allowed
    }

    /// An internal error at the anchor, naming the plugin.
    fn misuse(&mut self, message: &str) -> Aborted {
        let message = format!("plugin `{}`: {message}", self.name());
        self.state.plugin_internal_error(self.anchor, &message);
        Aborted
    }

    /// The prereq scope the lowering primitives write to.
    fn scope(&mut self) -> &mut Prereqs {
        match self.captures.last_mut() {
            Some(capture) => capture,
            None => self.prereqs.as_deref_mut().expect("a claim hook has a prereq scope"),
        }
    }

    /// Runs compiler code for the plugin, after the caller's phase check (`refused`): timed as the compiler's, and
    /// `Err` when it reported an error.
    fn lower_unchecked<T>(
        &mut self,
        f: impl FnOnce(&mut TransformState<'_>, &mut Prereqs, Option<&mut CallContext>) -> Result<T, String>,
    ) -> Result<T, Aborted> {
        self.touch();
        let before = self.state.diagnostics.diagnostics.len();
        self.host.enter(None, self.hook);
        let prereqs = match self.captures.last_mut() {
            Some(capture) => capture,
            None => self.prereqs.as_deref_mut().expect("a claim hook has a prereq scope"),
        };
        let result = f(self.state, prereqs, self.call.as_deref_mut());
        self.host.exit();
        match result {
            Ok(value) if !has_errors_since(self.state, before) => Ok(value),
            Ok(_) => Err(Aborted),
            Err(message) => Err(self.misuse(&message)),
        }
    }

    /// The call context for a `CallSite` the plugin passed: it must be the call being claimed.
    fn check_site(&mut self, call: &CallSite, what: &str) -> Result<(), Aborted> {
        match self.call.as_deref() {
            Some(context) if context.site.node == call.node => Ok(()),
            _ => Err(self.misuse(&format!("{what} needs the CallSite of the call being claimed"))),
        }
    }
}

impl CxBackend for HookBackend<'_, '_> {
    fn program(&self) -> &'static Program {
        self.state.program
    }

    fn options(&self) -> P<CompilerOptions> {
        self.state.program.options()
    }

    fn file(&self) -> &FileInfo {
        self.state.plugin_file.info.as_ref().expect("a file being transformed for plugins")
    }

    fn checker(&mut self) -> &mut Checker {
        &mut *self.state.checker
    }

    fn project(&self) -> &ProjectServices {
        self.state.services
    }

    fn ts5_properties_of_type(&mut self, t: P<tsrs_checker::Type>) -> Vec<P<Symbol>> {
        crate::util::ts5_property_order::ts5_property_order(&mut *self.state.checker, t)
    }

    fn resolve_module(&self, specifier: &str, from: P<SourceFile>) -> Option<ResolvedModuleInfo> {
        self.host.resolve_module(specifier, from.get().file_name())
    }

    fn declaring_package(&self, symbol: P<Symbol>) -> Option<Arc<PackageInfo>> {
        PluginHost::declaring_package(self.state.services, symbol)
    }

    // --- lowering ---

    fn compile_expr(&mut self, node: P<Node>) -> Result<Expr, Aborted> {
        if self.refused("compile_expr", Row::Lowering) {
            return Err(Aborted);
        }
        self.check_self_compile(&[node])?;
        let own = self.anchor;
        self.lower_unchecked(|state, prereqs, context| compile_node(state, prereqs, context, node, own))
    }

    fn compile_in_order(&mut self, nodes: &[P<Node>]) -> Result<Vec<Expr>, Aborted> {
        if self.refused("compile_in_order", Row::Lowering) {
            return Err(Aborted);
        }
        self.check_self_compile(nodes)?;
        let own = self.anchor;
        self.lower_unchecked(|state, prereqs, mut context| {
            let mut error = None;
            let values =
                ensure_transform_order_with(state, prereqs, nodes, &mut |state, prereqs, node| match compile_node(
                    state,
                    prereqs,
                    context.as_deref_mut(),
                    node,
                    own,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        error.get_or_insert(message);
                        Expr::None
                    }
                });
            match error {
                Some(message) => Err(message),
                None => Ok(values),
            }
        })
    }

    fn compile_receiver(&mut self, call: &CallSite) -> Result<Expr, Aborted> {
        if self.refused("compile_receiver", Row::Lowering) {
            return Err(Aborted);
        }
        self.check_site(call, "compile_receiver")?;
        let in_capture = !self.captures.is_empty();
        self.lower_unchecked(|state, prereqs, context| {
            let context = context.expect("checked above");
            compile_receiver(state, prereqs, context, in_capture)
        })
    }

    fn compile_call(&mut self, call: &CallSite, callee: Callee, args: Vec<Arg>) -> Result<Expr, Aborted> {
        if self.refused("compile_call", Row::Lowering) {
            return Err(Aborted);
        }
        self.check_site(call, "compile_call")?;
        let frame = self.state.plugin_file.frames.last().copied().unwrap_or_default();
        if frame.called {
            return Err(self.misuse("compile_call ran a second time on its call (it would be lowered twice)"));
        }
        if frame.self_compiled {
            return Err(
                self.misuse("compile_call after the call was lowered with compile_expr (it would be lowered twice)")
            );
        }
        self.lower_unchecked(|state, prereqs, context| {
            let context = context.expect("checked above");
            if let Some(frame) = state.plugin_file.frames.last_mut() {
                frame.called = true;
            }
            note_effect(state, "lowering it with compile_call");
            lower_call(state, prereqs, context, callee, args)
        })
    }

    fn compile_object_literal(&mut self, node: P<Node>, edits: ObjectEdits) -> Result<Expr, Aborted> {
        if self.refused("compile_object_literal", Row::Lowering) {
            return Err(Aborted);
        }
        self.check_self_compile(&[node])?;
        self.lower_unchecked(|state, prereqs, _| super::lowering::lower_object_literal(state, prereqs, node, edits))
    }

    fn compile_binary(&mut self, op: BinOp, left: P<Node>, right: P<Node>) -> Result<Expr, Aborted> {
        if self.refused("compile_binary", Row::Lowering) {
            return Err(Aborted);
        }
        if left == self.anchor || right == self.anchor {
            return Err(self.misuse("compile_binary takes operands, not the hook's own node"));
        }
        self.check_self_compile(&[left, right])?;
        let anchor = self.anchor;
        self.lower_unchecked(|state, prereqs, _| super::lowering::lower_binary(state, prereqs, op, left, right, anchor))
    }

    // --- prereqs and placement ---

    fn prereq(&mut self, stmt: Stmt) {
        if !self.refused("prereq", Row::Lowering) {
            self.touch();
            self.scope().push(stmt);
        }
    }

    fn push_to_var(&mut self, value: Expr, hint: &str) -> Expr {
        if self.refused("push_to_var", Row::Lowering) {
            return create::nil();
        }
        self.touch();
        // an empty hint is the one roblox-ts derives from the value, made capture-safe like any other
        let hint = if hint.is_empty() { value_to_id_str(&value) } else { hint.to_owned() };
        let hint = self.state.plugin_file.seam_hint(self.state.source_file, &hint);
        self.scope().push_to_var(value, &hint)
    }

    fn begin_capture(&mut self) {
        // a refused capture still opens a scope, so begin and end stay paired
        self.refused("capture", Row::Lowering);
        self.touch();
        self.captures.push(Prereqs::new());
    }

    fn end_capture(&mut self) -> Vec<Stmt> {
        self.captures.pop().expect("end_capture after begin_capture").statements
    }

    fn insert_before_root_statement(&mut self, stmts: Vec<Stmt>) {
        if !self.refused("insert_before_root_statement", Row::Lowering) {
            self.note_effect("calling insert_before_root_statement");
            self.touch();
            self.state.plugin_file.main_chunk_locals += stmts.iter().map(declared_locals).sum::<u32>();
            self.note_top_level_temps(&stmts);
            self.state.plugin_file.root_inserts.extend(stmts);
        }
    }

    fn insert_before_statement(&mut self, stmts: Vec<Stmt>) {
        if self.refused("insert_before_statement", Row::Lowering) {
            return;
        }
        let Some(statement) = dispatch::enclosing_statement(self.anchor) else {
            self.misuse("insert_before_statement: its node is in no statement");
            return;
        };
        self.note_effect("calling insert_before_statement");
        self.touch();
        // locals before a top-level statement are the main chunk's (a function body counts its statements' own)
        if statement.parent().is_some_and(|p| p.kind() == Kind::SourceFile) {
            self.state.plugin_file.main_chunk_locals += stmts.iter().map(declared_locals).sum::<u32>();
        }
        let file = &mut self.state.plugin_file;
        let position = file.before_statement_position(statement, self.anchor);
        file.before_statement_count += stmts.len();
        file.before_statement.entry(statement).or_default().push((position, stmts));
    }

    fn header(&mut self, stmts: Vec<Stmt>) {
        if !self.refused("header", Row::FileLevel) {
            self.note_effect("calling header");
            self.touch();
            self.state.plugin_file.main_chunk_locals += stmts.iter().map(declared_locals).sum::<u32>();
            self.note_top_level_temps(&stmts);
            self.state.plugin_file.header.extend(stmts);
        }
    }

    fn hoist(&mut self, value: Expr, hint: &str) -> Expr {
        if self.refused("hoist", Row::Lowering) || self.refuse_hoist("hoist", &value) {
            return create::nil();
        }
        self.hoist_unchecked(value, hint)
    }

    fn hoist_spillable(&mut self, value: Expr, hint: &str) -> Expr {
        if self.refused("hoist_spillable", Row::Lowering) || self.refuse_hoist("hoist_spillable", &value) {
            return create::nil();
        }
        // spilled into one table local at the end of the file if the main chunk is past Luau's limit (`spill`)
        let hoisted = self.hoist_unchecked(value, hint);
        if let Expr::TemporaryIdentifier(id) = &hoisted {
            let hoist = SpillableHoist { local: SpillableLocal::Temporary(id.id), hint: hint.to_owned() };
            self.state.plugin_file.spillable.push(hoist);
        }
        hoisted
    }

    fn hoist_spillable_named(&mut self, value: Expr, name: &str) -> Expr {
        if self.refused("hoist_spillable_named", Row::Lowering) || self.refuse_hoist("hoist_spillable_named", &value) {
            return create::nil();
        }
        if !rbxts_luau::is_valid_identifier(name) {
            self.misuse(&format!("hoist_spillable_named: `{name}` is not a valid Luau identifier"));
            return create::nil();
        }
        self.note_effect("calling hoist");
        self.touch();
        // TypeScript's optimistic unique name: `name`, else `name_1`, … (capture-safe: nothing in the file has it)
        let name = self.state.plugin_file.unique_plain_name(self.state.source_file, name);
        self.state.plugin_file.main_chunk_locals += 1;
        self.state.plugin_file.root_inserts.push(create::local(create::any_id(name.clone()), Some(value)));
        let hoist = SpillableHoist { local: SpillableLocal::Named(name.clone()), hint: name.clone() };
        self.state.plugin_file.spillable.push(hoist);
        create::id(name)
    }

    fn import_named(&mut self, specifier: &str, export: &str) -> Result<Expr, Aborted> {
        if self.refused("import_named", Row::FileLevel) {
            return Err(Aborted);
        }
        self.note_effect("calling import_named");
        self.touch();
        let anchor = self.anchor;
        let before = self.state.diagnostics.diagnostics.len();
        match dispatch::import_named(self.state, self.host, anchor, specifier, export) {
            Ok(value) if !has_errors_since(self.state, before) => {
                // declared with the imports, at the top of the file
                if let Expr::TemporaryIdentifier(t) = &value {
                    self.state.plugin_file.top_level_temps.insert(t.id);
                }
                Ok(value)
            }
            Ok(_) => Err(Aborted),
            Err(message) => Err(self.misuse(&message)),
        }
    }

    fn runtime_member(&mut self, object: &str, member: &str) -> Expr {
        if self.refused("runtime_member", Row::FileLevel) {
            return create::nil();
        }
        for name in [object, member] {
            if !rbxts_luau::is_valid_identifier(name) {
                self.misuse(&format!("runtime_member: `{name}` is not a valid Luau identifier"));
                return create::nil();
            }
        }
        self.note_effect("calling runtime_member");
        self.touch();
        self.state.plugin_file.runtime_members += 1;
        // as roblox-ts compiles a reference to a global the runtime library provides: `TS.<object>`, counted
        let anchor = self.anchor;
        create::property(self.state.ts(anchor, object), member)
    }

    fn mark_import_used(&mut self, alias_declaration: P<Node>) {
        if !self.refused("mark_import_used", Row::FileLevel) {
            self.note_effect("calling mark_import_used");
            self.touch();
            self.state.plugin_file.forced_imports.insert(alias_declaration);
        }
    }

    fn request_import_elision(&mut self, alias_declaration: P<Node>) {
        if self.refused("request_import_elision", Row::FileLevel) {
            return;
        }
        if !matches!(
            alias_declaration.kind(),
            Kind::ImportSpecifier | Kind::ImportClause | Kind::NamespaceImport | Kind::ImportEqualsDeclaration
        ) {
            self.misuse(&format!(
                "request_import_elision takes an import alias declaration, not a {:?}",
                alias_declaration.kind()
            ));
            return;
        }
        // harmless on a decline: an elision request is only honoured when nothing compiled uses the import
        self.touch();
        let requests = &mut self.state.plugin_file.elision_requests;
        if !requests.contains(&alias_declaration) {
            requests.push(alias_declaration);
        }
    }

    fn temp_id(&mut self, hint: &str) -> LuauIdent {
        self.refused("temp_id", Row::FileLevel);
        self.touch();
        self.seam_temp(hint)
    }

    /// A valid identifier, not a name roblox-ts's output uses itself (`luau.globals`), not a name a plugin import
    /// took, and not a name the file's source uses anywhere, which covers every global (Roblox's or declared) the
    /// output refers to and every local.
    fn is_usable_local_name(&self, name: &str) -> bool {
        self.state.plugin_file.approve_name(self.state.source_file, name)
    }

    fn locals(&self) -> LocalCounts {
        let main_chunk = self.state.plugin_file.main_chunk_locals;
        // the innermost function's body: the locals its statements declared at its top level so far
        let function = match self
            .anchor
            .parent()
            .and_then(|p| crate::util::traversal::get_ancestor(p, tsrs_ast::is_function_like))
        {
            Some(function) => {
                function.body().and_then(|body| self.state.plugin_file.function_locals.get(&body).copied()).unwrap_or(0)
            }
            None => main_chunk,
        };
        LocalCounts { function, main_chunk }
    }

    fn late_string(&mut self, key: &str) -> Expr {
        if self.refused("late_string", Row::FileLevel) {
            return create::nil();
        }
        self.note_effect("calling late_string");
        self.touch();
        let key = super::lifecycle::late_key(self.name(), key);
        self.state.plugin_file.use_late_key(self.plugin, key.clone());
        create::late_string(key)
    }

    fn comment(&mut self, text: &str) {
        if !self.refused("comment", Row::Lowering) {
            self.touch();
            self.scope().push(create::comment(text));
            // leading its statement's Luau, it goes above the statement's leading comments (`before_statement`)
            if self.captures.is_empty()
                && let Some(statement) = dispatch::enclosing_statement(self.anchor)
            {
                self.state.plugin_file.statement_comments.entry(statement).or_default().push(text.to_owned());
            }
        }
    }

    // --- source facts ---

    fn utf16_pos(&self, node: P<Node>) -> Utf16Pos {
        let Some(file) = tsrs_ast::get_source_file_of_node(node) else { return Utf16Pos::default() };
        let Some((start, _)) = facts::span(node, file) else { return Utf16Pos::default() };
        facts::utf16_pos_of(file, &self.host.lines(file), start)
    }

    fn utf16_width(&self, node: P<Node>) -> u32 {
        let Some(file) = tsrs_ast::get_source_file_of_node(node) else { return 0 };
        let Some((start, end)) = facts::span(node, file) else { return 0 };
        facts::utf16_len(&self.host.lines(file), start, end)
    }

    fn source_text(&self, node: P<Node>) -> &'static str {
        let Some(file) = tsrs_ast::get_source_file_of_node(node) else { return "" };
        let Some((start, end)) = facts::span(node, file) else { return "" };
        &file.get().text()[start as usize..end as usize]
    }

    fn jsdoc_tags(&mut self, declaration: P<Node>) -> Rc<[JsDocTag]> {
        self.host.jsdoc_tags(self.state.checker, declaration)
    }

    fn file_diagnostics(&self) -> &[P<Diagnostic>] {
        let file = self.file().source;
        self.host.file_diagnostics.get(&file).map_or(&[], Vec::as_slice)
    }

    fn file_identifier_texts(&self) -> Rc<[&'static str]> {
        self.state.plugin_file.identifier_texts(self.state.source_file)
    }

    // --- diagnostics ---

    fn report(&mut self, diagnostic: PluginDiagnostic) {
        if self.phase == Phase::Classify {
            let message =
                format!("plugin `{}` reported a diagnostic from classify_symbol: {}", self.name(), diagnostic.message);
            self.state.plugin_internal_error(diagnostic.node, &message);
            return;
        }
        self.touch();
        // one of each in the build: `compile::drop_plugin_duplicates`
        let diagnostic = to_diagnostic(&diagnostic, self.host.sources[self.plugin]);
        self.state.plugin_file.plugin_diagnostics.push(diagnostic);
        self.state.add_diagnostic(diagnostic);
    }

    // --- build inputs ---

    fn env(&mut self, name: &str) -> Option<String> {
        // which file first asks about a symbol depends on the checker count: no inputs recorded there
        if self.phase == Phase::Classify {
            let message = format!("plugin `{}` called env in {}", self.name(), self.hook.label());
            self.state.plugin_internal_error(self.anchor, &message);
            return None;
        }
        let value = self.host.env.get(name);
        if crate::build::plugin_inputs::valid_env_name(name) {
            self.state.plugin_file.inputs.record_env(self.plugin, name, &value);
        }
        value
    }

    fn record_glob(&mut self, root: &Path, pattern: &str, matcher: Arc<dyn GlobMatcher>) -> Vec<String> {
        if self.phase == Phase::Classify {
            let message = format!("plugin `{}` called record_glob in {}", self.name(), self.hook.label());
            self.state.plugin_internal_error(self.anchor, &message);
            return Vec::new();
        }
        let text = root.to_string_lossy();
        let root = if rbxts_project::nodepath::is_absolute(&text) {
            crate::build::plugin_inputs::plain(&text)
        } else {
            crate::build::plugin_inputs::plain(&rbxts_project::nodepath::resolve(&[
                &self.state.services.data.project_path,
                &text,
            ]))
        };
        let glob = crate::build::plugin_inputs::RecordedGlob::record(self.plugin, root, pattern, matcher);
        let answer = glob.answer.clone();
        self.state.plugin_file.inputs.record_glob(glob);
        answer
    }
}

// --- compile_receiver and compile_call ---

/// The receiver node of a property or element call.
fn receiver_node(shape: CallShape) -> Option<P<Node>> {
    match shape {
        CallShape::Property { access, .. } => Some(access.as_property_access_expression().expression),
        CallShape::Element { access, .. } => Some(access.as_element_access_expression().expression),
        _ => None,
    }
}

/// Records on the running hook's frame something its node can no longer be declined after.
fn note_effect(state: &mut TransformState<'_>, what: &'static str) {
    if let Some(frame) = state.plugin_file.frames.last_mut() {
        frame.lowered.get_or_insert(what);
    }
}

/// `compile_receiver`: the receiver as the chain lowering compiled it, or compiled now (once). Compiled into the
/// offer's own scope, it is kept for the next plugin (or roblox-ts) to reuse if this one declines; compiled inside a
/// capture, its prereqs are the plugin's to place, so the call cannot be declined any more.
fn compile_receiver(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    context: &mut CallContext,
    in_capture: bool,
) -> Result<Expr, String> {
    const SUPER: &str = "compile_receiver: `super` is not a value (roblox-ts calls `super.m(self, …)`); lower the \
                         call with compile_expr or compile_call, or pass `self` yourself";
    if let Some(optional) = &mut context.optional {
        let node = context.site.node;
        return match &optional.receiver {
            OptionalReceiver::Bound(value) => Ok(value.clone()),
            OptionalReceiver::Super => Err(SUPER.to_owned()),
            OptionalReceiver::Unbound(value) => {
                // the callee is read from the receiver before the nil check: the local goes before that read
                let (id, _) = state.plugin_file.optional_receivers.entry(node).or_insert_with(|| {
                    let id = temp_id("self");
                    (Expr::TemporaryIdentifier(id.clone()), create::local(id, Some(value.clone())))
                });
                let id = id.clone();
                optional.receiver = OptionalReceiver::Bound(id.clone());
                Ok(id)
            }
        };
    }
    let Some(receiver) = receiver_node(context.shape) else {
        return Err(format!("compile_receiver: a {:?} call has no receiver", context.site.kind));
    };
    if tsrs_ast::is_super_property(context.site.callee) {
        return Err(SUPER.to_owned());
    }
    if let Some(base) = &context.base {
        return Ok(base.clone());
    }
    let before = state.plugin_file.effect_mark(prereqs.statements.len());
    let nested_before = state.plugin_file.frames.last().map_or(0, |f| f.nested_hooks);
    let value = transform_expression(state, prereqs, receiver);
    context.base = Some(value.clone());
    if in_capture {
        context.base_captured = true;
        note_effect(state, "compiling the receiver inside a capture");
    } else {
        context.base_marks = Some((before, state.plugin_file.effect_mark(prereqs.statements.len())));
        if let Some(frame) = state.plugin_file.frames.last_mut() {
            frame.receiver_nested_hooks += frame.nested_hooks - nested_before;
        }
    }
    Ok(value)
}

/// A node a hook compiles: the claimed call itself through [`compile_claimed_call`], the hook's own node
/// (`own`) through [`lower_own_node`], anything else as roblox-ts lowers it (a node being offered right now is
/// lowered without asking the plugins of that kind again).
fn compile_node(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    context: Option<&mut CallContext>,
    node: P<Node>,
    own: P<Node>,
) -> Result<Expr, String> {
    match context {
        Some(context) if context.site.node == node => compile_claimed_call(state, prereqs, context),
        _ if tsrs_ast::is_statement(node) => {
            Err(format!("compile_* takes expressions, not a statement ({:?})", node.kind()))
        }
        _ if node == own => Ok(lower_own_node(state, prereqs, node)),
        _ => Ok(transform_expression(state, prereqs, node)),
    }
}

/// An expression or assignment hook's own node, lowered as roblox-ts lowers it there: on its compiled base when it
/// was offered after the base (a link inside an optional chain); as a statement when roblox-ts lowers it as one
/// (an assignment, `++` / `--` or logical assignment whose value is unused), with no value (`nil`);
/// else as an expression.
fn lower_own_node(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    if let Some(lowering) =
        state.plugin_file.eager_offers.iter().rev().find(|(n, _)| *n == node).map(|(_, l)| l.clone())
    {
        return match lowering {
            EagerLowering::Item { base, item } => transform_chain_item(state, prereqs, base, &item),
            EagerLowering::OptionalCall { call, item } => {
                dispatch::lower_optional_call(state, prereqs, &item, &call).unwrap_or(Expr::None)
            }
        };
    }
    if is_statement_form(node) {
        // no value: `nil`, which the claim point drops when the hook returns it as it is
        let statements = transform_expression_statement_inner(state, node);
        prereqs.push_list(statements);
        if let Some(frame) = state.plugin_file.frames.last_mut() {
            frame.statement_form = true;
        }
        return Expr::Nil;
    }
    transform_expression(state, prereqs, node)
}

/// roblox-ts lowers `node` as a statement of its own, not as a value: it is an expression statement's expression,
/// or a `for` statement's initializer or incrementor, and an assignment (not a destructuring one), a logical
/// assignment, `++` or `--` (`transformExpressionStatementInner`).
pub(crate) fn is_statement_form(node: P<Node>) -> bool {
    if !is_statement_lowered(node) {
        return false;
    }
    let Some(parent) = skip_upwards(node).parent() else { return false };
    match parent.kind() {
        Kind::ExpressionStatement => parent.expression().map(skip_downwards) == Some(node),
        Kind::ForStatement => {
            let header = parent.as_for_statement();
            header.initializer == Some(node) || header.incrementor == Some(node)
        }
        _ => false,
    }
}

/// `ancestor` is a proper ancestor of `node`.
fn is_ancestor(ancestor: P<Node>, node: P<Node>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// `node` is the operand of a `delete` (parentheses and type assertions skipped).
pub(crate) fn is_delete_operand(node: P<Node>) -> bool {
    skip_upwards(node).parent().is_some_and(|p| p.kind() == Kind::DeleteExpression)
}

/// `compile_expr` of the claimed call itself: the call lowered as roblox-ts lowers it, without asking the plugins
/// again. A chain call whose base the chain lowering (or `compile_receiver`) already compiled is lowered on that
/// base, so the receiver is not evaluated twice; a call nothing of which is compiled yet is lowered from scratch.
fn compile_claimed_call(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    context: &mut CallContext,
) -> Result<Expr, String> {
    if state.plugin_file.frames.last().is_some_and(|f| f.called) {
        return Err("compile_expr of the claimed call after compile_call lowered it".to_owned());
    }
    if state.plugin_file.frames.last().is_some_and(|f| f.self_compiled) {
        return Err("compiled its own call a second time (it would be lowered twice)".to_owned());
    }
    if let Some(frame) = state.plugin_file.frames.last_mut() {
        frame.self_compiled = true;
    }
    note_effect(state, "lowering it with compile_expr");
    let node = context.site.node;
    if let Some(optional) = &context.optional {
        // inside the chain's nil check on the callee: roblox-ts's optional call lowering
        let item = context.item.expect("an optional call is a chain link");
        return Ok(crate::nodes::transform_optional_chain::transform_optional_call_value(
            state, prereqs, &item, optional,
        )
        .unwrap_or(Expr::None));
    }
    if let CallShape::Decorator { call } = context.shape {
        // the decorator's expression, its call (offered as this decorator) not asked about again
        if let Some(call) = call {
            state.plugin_file.offered.insert(call);
        }
        let expression = node.expression().expect("a decorator has an expression");
        return Ok(transform_expression(state, prereqs, expression));
    }
    // the call is not offered again on the way down
    state.plugin_file.offered.insert(node);
    match (&context.base, context.item) {
        (Some(base), Some(item)) => Ok(transform_chain_item(state, prereqs, base.clone(), &item)),
        (Some(_), None) => Err("compile_expr of the claimed call after its callee was compiled".to_owned()),
        (None, _) => Ok(transform_expression(state, prereqs, node)),
    }
}

/// The original callee's base, compiled now unless the chain lowering or `compile_receiver` already did.
fn ensure_base(state: &mut TransformState<'_>, prereqs: &mut Prereqs, context: &mut CallContext) -> Expr {
    if let Some(base) = &context.base {
        return base.clone();
    }
    let value = match context.shape {
        CallShape::Call => transform_expression(state, prereqs, context.site.callee),
        CallShape::Super => transform_super_keyword(),
        CallShape::Property { .. } | CallShape::Element { .. } => {
            transform_expression(state, prereqs, receiver_node(context.shape).expect("a receiver"))
        }
        CallShape::New => {
            validate_not_any_type(state, context.site.callee);
            convert_to_indexable_expression(transform_expression(state, prereqs, context.site.callee))
        }
        CallShape::Decorator { .. } => transform_expression(state, prereqs, context.site.callee),
    };
    context.base = Some(value.clone());
    value
}

/// A roblox-ts macro that would lower this call: `compile_call` with the original callee cannot pass edited
/// arguments to it.
fn original_macro_name(state: &mut TransformState<'_>, context: &CallContext) -> Option<&'static str> {
    match context.shape {
        CallShape::New => {
            let symbol = get_first_construct_symbol(state, context.site.callee)?;
            state.macro_manager.get_constructor_macro(symbol).map(|m| m.name)
        }
        CallShape::Call | CallShape::Property { .. } | CallShape::Element { .. } => {
            let callee = context.site.callee;
            let callee_type = state.get_type(callee);
            let exp_type = state.checker.get_non_optional_type(callee_type);
            let symbol = get_first_defined_symbol(state, exp_type)?;
            match context.shape {
                CallShape::Call => state.macro_manager.get_call_macro(state.checker, symbol).map(|m| m.name),
                _ => state.macro_manager.get_property_call_macro(state.checker, symbol).map(|m| m.name),
            }
        }
        CallShape::Super | CallShape::Decorator { .. } => None,
    }
}

/// Compiled argument slots: the values, the slot nodes (`None` for Luau and `nil` slots) and the prereqs.
type Slots = (Vec<Expr>, Vec<Option<P<Node>>>, Prereqs);

/// The argument slots, compiled in evaluation order: each `Original` node with its own prereqs, then the values
/// bound to locals where a later slot's prereqs could change them (`ensureTransformOrder` over every slot, ready
/// Luau values included). Returns the values, the slot nodes (`None` for Luau and `nil` slots) and the prereqs.
fn compile_slots(state: &mut TransformState<'_>, context: &mut CallContext, args: Vec<Arg>) -> Result<Slots, String> {
    let mut infos: Vec<(Expr, Vec<Stmt>, Option<P<Node>>)> = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            Arg::Original(node) => {
                let mut node_prereqs = Prereqs::new();
                let own = context.site.node;
                let value = compile_node(state, &mut node_prereqs, Some(&mut *context), node, own)?;
                infos.push((value, node_prereqs.statements, Some(node)));
            }
            Arg::Luau(value) => infos.push((value, Vec::new(), None)),
            Arg::Nil => infos.push((create::nil(), Vec::new(), None)),
        }
    }
    let mut captures = vec![false; infos.len()];
    let mut suffix = Effects::none();
    for i in (0..infos.len()).rev() {
        let (expression, statements, _) = &infos[i];
        let effects = get_effects(expression);
        captures[i] = !effects_commute(&effects, &suffix);
        if captures[i] {
            suffix = join_effects(effects, &suffix);
        }
        suffix = join_effects(get_effects_of_statements(statements), &suffix);
    }
    let mut prereqs = Prereqs::new();
    let mut values = Vec::with_capacity(infos.len());
    let mut nodes = Vec::with_capacity(infos.len());
    for ((expression, statements, node), capture) in infos.into_iter().zip(captures) {
        prereqs.push_list(statements);
        values.push(if capture { prereqs.push_to_var(expression, "exp") } else { expression });
        nodes.push(node);
    }
    Ok((values, nodes, prereqs))
}

/// `fixVoidArgumentsForRobloxFunctions` on the `Original` slots.
fn fix_void_slots(
    state: &mut TransformState<'_>,
    context: &CallContext,
    values: &mut [Expr],
    nodes: &[Option<P<Node>>],
) {
    let callee_type = state.get_type(context.site.callee);
    let exp_type = state.checker.get_non_optional_type(callee_type);
    for (value, node) in values.iter_mut().zip(nodes) {
        if let Some(node) = node {
            fix_void_arguments_for_roblox_functions(state, exp_type, std::slice::from_mut(value), &[*node]);
        }
    }
}

/// `compile_call`: the call with another callee and/or argument slots, lowered as roblox-ts lowers calls.
fn lower_call(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    context: &mut CallContext,
    callee: Callee,
    args: Vec<Arg>,
) -> Result<Expr, String> {
    let node = context.site.node;
    match callee {
        Callee::Original => {
            if let CallShape::Decorator { call: None } = context.shape {
                return Err("compile_call: `@decorator` without a call has no call to lower".to_owned());
            }
            let super_access = tsrs_ast::is_super_property(context.site.callee);
            // roblox-ts applies no macro to `super.m()`
            if !super_access && let Some(name) = original_macro_name(state, context) {
                return Err(format!(
                    "compile_call: `{name}` is a roblox-ts macro; Callee::Original cannot lower it with argument \
                     slots (lower the node with compile_expr, or call another function)"
                ));
            }
            if let Some(optional) = context.optional.clone() {
                // inside the chain's nil check: `function(self, args)`, as roblox-ts's optional call lowering
                let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
                prereqs.push_list(arg_prereqs.statements);
                let mut call_args: Vec<Expr> = optional.self_arg.into_iter().collect();
                call_args.extend(values);
                return Ok(wrap_return_if_lua_tuple(state, node, create::call(optional.function, call_args)));
            }
            // the validations of roblox-ts's call lowerings, in their order
            match context.shape {
                CallShape::Property { access, .. } => {
                    validate_not_any_type(state, access.as_property_access_expression().expression);
                }
                CallShape::Element { access, .. } => {
                    let element = access.as_element_access_expression();
                    validate_not_any_type(state, element.expression);
                    validate_not_any_type(state, element.argument_expression);
                }
                _ => {}
            }
            if matches!(context.shape, CallShape::Call | CallShape::Property { .. } | CallShape::Element { .. }) {
                validate_not_any_type(state, context.site.callee);
            }
            let mut base = ensure_base(state, prereqs, context);
            match context.shape {
                CallShape::Property { access, name } if super_access => {
                    let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
                    prereqs.push_list(arg_prereqs.statements);
                    let key = dispatch::rewritten_key(state, access, AccessPosition::Callee);
                    let callee = keyed_access(base, key.as_deref().unwrap_or(name));
                    Ok(create::call(callee, with_super_self(state, access, values)))
                }
                CallShape::Element { access, argument } if super_access => {
                    let callee = match dispatch::rewritten_key(state, access, AccessPosition::Callee) {
                        Some(key) => {
                            // a computed argument is still evaluated (`super` has no effects to order it after)
                            prereqs.push_list(dispatch::replaced_key_statements(state, access));
                            keyed_access(base, &key)
                        }
                        None => {
                            let index = transform_expression(state, prereqs, argument);
                            create::computed_index(convert_to_indexable_expression(base), index)
                        }
                    };
                    let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
                    prereqs.push_list(arg_prereqs.statements);
                    Ok(create::call(callee, with_super_self(state, access, values)))
                }
                CallShape::Super => {
                    let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
                    prereqs.push_list(arg_prereqs.statements);
                    let mut with_self = vec![rbxts_luau::globals::self_()];
                    with_self.extend(values);
                    Ok(create::call(create::property(convert_to_indexable_expression(base), "constructor"), with_self))
                }
                CallShape::New => {
                    let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
                    // as `transform_new_expression` (stockfix): the constructor is read before the arguments run, so
                    // it is captured when their prerequisites could change it (`new C((C = B, x))`)
                    if !effects_commute(&get_effects(&base), &get_effects_of_statements(&arg_prereqs.statements)) {
                        base = prereqs.push_to_var(base, "class");
                    }
                    prereqs.push_list(arg_prereqs.statements);
                    Ok(create::call(create::property(base, "new"), values))
                }
                CallShape::Call | CallShape::Decorator { .. } => {
                    let (mut values, nodes, arg_prereqs) = compile_slots(state, context, args)?;
                    let callee_node = context.site.callee;
                    fix_void_slots(state, context, &mut values, &nodes);
                    if !effects_commute(&get_effects(&base), &get_effects_of_statements(&arg_prereqs.statements)) {
                        base = prereqs.push_to_var(base, "fn");
                    }
                    prereqs.push_list(arg_prereqs.statements);
                    let callee_type = state.get_type(callee_node);
                    let exp_type = state.checker.get_non_optional_type(callee_type);
                    if is_method_from_type(state, callee_node, exp_type) {
                        values.insert(0, rbxts_luau::Expr::Nil);
                    }
                    let call = create::call(convert_to_indexable_expression(base), values);
                    Ok(match context.shape {
                        CallShape::Call => wrap_return_if_lua_tuple(state, node, call),
                        _ => call,
                    })
                }
                CallShape::Property { access, name } => {
                    let (mut values, nodes, arg_prereqs) = compile_slots(state, context, args)?;
                    fix_void_slots(state, context, &mut values, &nodes);
                    let method = is_method(state, access);
                    let key = match dispatch::rewritten_key(state, access, AccessPosition::Callee) {
                        Some(key) => property_key_of(&key),
                        None => PropertyKey::Name(name.to_owned()),
                    };
                    let call = create_ordered_property_call(
                        state,
                        prereqs,
                        access,
                        base,
                        key,
                        values,
                        arg_prereqs.statements,
                        method,
                    );
                    Ok(wrap_return_if_lua_tuple(state, node, call))
                }
                CallShape::Element { access, argument } => {
                    let rewritten = dispatch::rewritten_key(state, access, AccessPosition::Callee);
                    let mut key_prereqs = Prereqs::new();
                    let argument_value = match &rewritten {
                        Some(key) => {
                            // a computed argument is still evaluated, after the receiver
                            key_prereqs.push_list(dispatch::replaced_key_statements(state, access));
                            text_literal(key)
                        }
                        None => transform_expression(state, &mut key_prereqs, argument),
                    };
                    // as `transformElementCallExpressionInner`: a late-read base is captured before a key with effects
                    let later = join_effects(
                        get_effects_of_statements(&key_prereqs.statements),
                        &(if is_late_read(&base) { get_effects(&argument_value) } else { Effects::none() }),
                    );
                    if !effects_commute(&get_effects(&base), &later) {
                        base = prereqs.push_to_var(base, "exp");
                    }
                    prereqs.push_list(key_prereqs.statements);
                    let (mut values, nodes, arg_prereqs) = compile_slots(state, context, args)?;
                    fix_void_slots(state, context, &mut values, &nodes);
                    let key = match rewritten {
                        Some(key) => property_key_of(&key),
                        None => {
                            let receiver_type = state.get_type(access.as_element_access_expression().expression);
                            let receiver_type = state.checker.get_non_optional_type(receiver_type);
                            PropertyKey::Index(add_one_if_array_type(state, receiver_type, argument_value))
                        }
                    };
                    let method = is_method(state, access);
                    let call = create_ordered_property_call(
                        state,
                        prereqs,
                        access,
                        base,
                        key,
                        values,
                        arg_prereqs.statements,
                        method,
                    );
                    Ok(wrap_return_if_lua_tuple(state, node, call))
                }
            }
        }
        Callee::Function(function) => {
            let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
            let function =
                if effects_commute(&get_effects(&function), &get_effects_of_statements(&arg_prereqs.statements)) {
                    function
                } else {
                    prereqs.push_to_var(function, "fn")
                };
            prereqs.push_list(arg_prereqs.statements);
            Ok(create::call(convert_to_indexable_expression(function), values))
        }
        Callee::Method { object, name } => {
            if !rbxts_luau::is_valid_identifier(&name) {
                return Err(format!(
                    "compile_call: Callee::Method name `{name}` is not a valid Luau identifier (use Callee::Function \
                     with the object as the first argument)"
                ));
            }
            let (values, _, arg_prereqs) = compile_slots(state, context, args)?;
            let object = if effects_commute(&get_effects(&object), &get_effects_of_statements(&arg_prereqs.statements))
            {
                object
            } else {
                prereqs.push_to_var(object, "self")
            };
            prereqs.push_list(arg_prereqs.statements);
            Ok(create::method_call(convert_to_indexable_expression(object), name, values))
        }
    }
}

/// The `CallKind` of a property or element call: a method call when roblox-ts calls it with `:`.
pub(crate) fn access_call_kind(state: &mut TransformState<'_>, access: P<Node>) -> CallKind {
    if is_method(state, access) { CallKind::Method } else { CallKind::Call }
}

/// `super.m(…)` passes `self` first when `m` is a method (roblox-ts's super property call).
fn with_super_self(state: &mut TransformState<'_>, access: P<Node>, values: Vec<Expr>) -> Vec<Expr> {
    if is_method(state, access) {
        let mut with_self = vec![rbxts_luau::globals::self_()];
        with_self.extend(values);
        with_self
    } else {
        values
    }
}

/// `base.key` when the key is a valid Luau identifier, else `base[text_literal(key)]` (a `property_key` answer).
pub(crate) fn keyed_access(base: Expr, key: &str) -> Expr {
    if rbxts_luau::is_valid_identifier(key) {
        create::property(convert_to_indexable_expression(base), key)
    } else {
        create::computed_index(convert_to_indexable_expression(base), text_literal(key))
    }
}

/// The key of a method callee from a `property_key` answer: `obj:key(…)` when it is a valid Luau identifier, else
/// `obj[text_literal(key)](obj, …)`.
pub(crate) fn property_key_of(key: &str) -> PropertyKey {
    if rbxts_luau::is_valid_identifier(key) {
        PropertyKey::Name(key.to_owned())
    } else {
        PropertyKey::Index(text_literal(key))
    }
}

/// The node a chain link's base was compiled from: the expression it accesses or calls.
fn chain_base_node(item: &ChainItem) -> Option<P<Node>> {
    use crate::nodes::transform_optional_chain::ChainItemKind;
    match item.kind {
        ChainItemKind::PropertyAccess { .. } | ChainItemKind::ElementAccess { .. } | ChainItemKind::Call => {
            item.node.expression()
        }
        ChainItemKind::PropertyCall { expression: access, .. } => {
            Some(access.as_property_access_expression().expression)
        }
        ChainItemKind::ElementCall { expression: access, .. } => Some(access.as_element_access_expression().expression),
    }
}
