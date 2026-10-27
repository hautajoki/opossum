//! The claim points and broadcast hooks the transform calls (DESIGN §9b "Plugin dispatch", §11.5). Each returns at
//! once when the build has no plugins (`state.plugins` is `None`) or no plugin asked for that kind of node.

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_luau::{AnyIdentifier, Expr, Stmt, create};
use rbxts_plugin::{
    AccessPosition, AccessSite, AssignOp, AssignSite, CallKind, CallSite, Claim, ClassFilter, ClassSite, SymbolRole,
    Tag,
};
use tsrs_ast::{Kind, ModifierFlags, Node, SourceFile, Symbol, SymbolFlags};
use tsrs_core::P;

use super::HookKind;
use super::backend::{
    CallContext, CallShape, EagerLowering, OptionalCall, access_call_kind, is_delete_operand, run_classify, run_hook,
    run_hook_framed,
};
// (run_hook: the broadcast hooks; run_hook_framed: the claim hooks, whose frame decides a decline)
use super::file_state::{EffectMark, HookFrame, ImportOverrides, PluginImport, RootRange, declared_locals};
use super::host::PluginHost;
use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::statements::transform_import_declaration::transform_import_declaration;
use crate::nodes::statements::transform_import_equals_declaration::transform_import_equals_declaration;
use crate::nodes::transform_optional_chain::{ChainItem, ChainItemKind, transform_chain_item};
use crate::util::create_import_expression::{create_import_expression_of_file, get_source_file_from_module_specifier};
use crate::util::symbols::is_used_as_statement;
use crate::util::traversal::{has_syntactic_modifier, skip_downwards, skip_upwards};
use crate::util::wrap_expression_statement::wrap_expression_statement;

// --- imports seen by the compiler ---

/// The compiler compiled a reference to `symbol` at `node` (`transformIdentifier`): when `symbol` is an import
/// alias, a plugin's elision request cannot drop that import.
#[inline]
pub fn note_alias_reference(state: &mut TransformState<'_>, node: P<Node>, symbol: P<Symbol>) {
    if state.plugins.is_none() || !symbol.flags().intersects(SymbolFlags::Alias) {
        return;
    }
    // the alias's own name in its declaration is not a reference
    if node.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            Kind::ImportSpecifier | Kind::ImportClause | Kind::NamespaceImport | Kind::ImportEqualsDeclaration
        )
    }) {
        return;
    }
    state.plugin_file.compiled_aliases.extend(symbol.declarations().iter().copied());
}

/// The elision decision for an import alias while its declaration is transformed again at the end of the file:
/// `Some(false)` elided, `Some(true)` kept, `None` as the checker says.
#[inline]
pub fn import_override(state: &TransformState<'_>, alias_declaration: P<Node>) -> Option<bool> {
    let overrides = state.plugin_file.import_overrides.as_ref()?;
    if overrides.elided.contains(&alias_declaration) {
        Some(false)
    } else if overrides.forced.contains(&alias_declaration) {
        Some(true)
    } else {
        None
    }
}

// --- calls ---

/// `get_property_of_type(get_apparent_type(non_nullable(t)), marker)` is `Some`.
fn has_marker(state: &mut TransformState<'_>, t: tsrs_core::P<tsrs_checker::Type>, marker: &str) -> bool {
    let t = state.checker.get_non_nullable_type(t);
    let t = state.checker.get_apparent_type(t);
    state.checker.get_property_of_type_exported(t, marker).is_some()
}

/// The symbol at a callee node, aliases skipped.
fn callee_symbol(state: &mut TransformState<'_>, node: Option<P<Node>>) -> Option<P<Symbol>> {
    let symbol = state.checker.get_symbol_at_location_exported(node?)?;
    Some(state.checker.skip_alias(symbol))
}

/// `classify_symbol`, memoised per checker and `(symbol, role)`.
fn classify(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    plugin: usize,
    symbol: P<Symbol>,
    role: SymbolRole,
    anchor: P<Node>,
) -> Option<Tag> {
    if let Some(&tag) = host.plugins[plugin].memo.borrow().get(&(symbol, role)) {
        return tag;
    }
    // anchored at the symbol's declaration, not at the first site that asked: which site is first depends on the
    // checker count and the file order, so a diagnostic there (a refused primitive) would move with them
    let anchor = symbol.value_declaration().or_else(|| symbol.declarations().first().copied()).unwrap_or(anchor);
    let tag = run_classify(state, plugin, anchor, |instance, cx| instance.classify_symbol(cx, symbol, role));
    host.plugins[plugin].memo.borrow_mut().insert((symbol, role), tag);
    tag
}

/// The plugins whose call interests match, with their tags, in registration order: the symbol classification,
/// then receiver interests, then callee markers.
fn call_candidates(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    site: &CallSite,
    method: Option<&str>,
) -> Vec<(usize, Tag)> {
    let mut out: Vec<(usize, Tag)> = Vec::new();
    let push = |out: &mut Vec<(usize, Tag)>, candidate: (usize, Tag)| {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    };
    let wants_method = method.is_some_and(|m| host.receiver_methods.contains(m)) && site.receiver.is_some();
    let mut receiver_type = None;
    let mut callee_type = None;
    for (plugin, entry) in host.plugins.iter().enumerate() {
        if !state.plugin_file.begun[plugin] {
            continue;
        }
        if entry.calls_by_symbol
            && let Some(symbol) = site.symbol
            && let Some(tag) = classify(state, host, plugin, symbol, SymbolRole::Callee, site.node)
        {
            push(&mut out, (plugin, tag));
        }
        if wants_method {
            let method = method.expect("checked above");
            for interest in &entry.receivers {
                if interest.methods.contains(&method) {
                    let receiver = site.receiver.expect("checked above");
                    let t = *receiver_type.get_or_insert_with(|| state.get_type(receiver));
                    if has_marker(state, t, interest.marker) {
                        push(&mut out, (plugin, interest.tag));
                    }
                }
            }
        }
        for &(marker, tag) in &entry.callee_markers {
            let t = *callee_type.get_or_insert_with(|| state.get_type(site.callee));
            if has_marker(state, t, marker) {
                push(&mut out, (plugin, tag));
            }
        }
    }
    out
}

/// Offers a call to the candidates in order; the first `Take` wins. A hook's `Err` claims the call as `nil` (its
/// error is recorded). `None` when every plugin declined.
fn offer_call(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    context: &mut CallContext,
    candidates: &[(usize, Tag)],
) -> Option<Expr> {
    let site = context.site;
    // the node is offered once: a dispatch of it while this offer runs, or after every plugin declined it, lowers it
    // without asking again
    if state.plugin_file.call_declined.contains(&site.node) || !state.plugin_file.in_offer.insert(site.node) {
        return None;
    }
    let value = offer_call_inner(state, host, prereqs, context, candidates);
    state.plugin_file.in_offer.remove(&site.node);
    if value.is_none() {
        state.plugin_file.call_declined.insert(site.node);
    }
    value
}

fn offer_call_inner(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    context: &mut CallContext,
    candidates: &[(usize, Tag)],
) -> Option<Expr> {
    let start = state.plugin_file.effect_mark(prereqs.statements.len());
    let site = context.site;
    for (position, &(plugin, tag)) in candidates.iter().enumerate() {
        let (result, frame) =
            run_hook_framed(state, Some(prereqs), Some(context), plugin, HookKind::Call, site.node, |instance, cx| {
                instance.call(cx, &site, tag)
            });
        match result {
            Ok(Claim::Take(value)) => {
                state.plugin_file.emitted = true;
                if host.claims_all {
                    report_double_claims(state, host, context, plugin, &candidates[position + 1..]);
                }
                return Some(unused_value(prereqs, site.node, value, site.value_used));
            }
            Ok(Claim::Decline) => {
                if check_decline(state, host, prereqs, &mut *context, plugin, start, frame) {
                    // already an error: no other plugin, and not roblox-ts, gets the node (nothing runs twice)
                    return Some(create::nil());
                }
            }
            Err(()) => return Some(create::nil()),
        }
    }
    None
}

/// A claimed value of `node` whose value is unused (statement position): evaluated as a statement now (a call
/// statement, or `local _ = value`), and `Expr::None` returned, so that no lowering that drops unused values (the
/// optional chain's) drops its effects. Used values, calls and temporaries are returned as they are, and so is a
/// value inside parentheses roblox-ts lowers (a `for` head's `(x = 0)`): the parentheses wrap it, and the statement
/// around them evaluates it (`local _ = (x)`), as without plugins.
fn unused_value(prereqs: &mut Prereqs, node: P<Node>, value: Expr, value_used: bool) -> Expr {
    if value_used
        || in_lowered_parentheses(node)
        || value.is_none()
        || matches!(value, Expr::Call(_) | Expr::MethodCall(_) | Expr::TemporaryIdentifier(_))
    {
        return value;
    }
    prereqs.push_list(wrap_expression_statement(value));
    Expr::None
}

/// `node` is inside parentheses that roblox-ts lowers as such before its value is dropped: a `for` statement's
/// parenthesized initializer or incrementor. (An expression statement's parentheses are skipped.)
fn in_lowered_parentheses(node: P<Node>) -> bool {
    let mut parenthesized = false;
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            Kind::ParenthesizedExpression => parenthesized = true,
            Kind::AsExpression
            | Kind::NonNullExpression
            | Kind::SatisfiesExpression
            | Kind::TypeAssertionExpression => {}
            Kind::ForStatement => return parenthesized,
            _ => return false,
        }
        current = parent;
    }
    false
}

/// A hook that lowered its own node as a statement (roblox-ts lowers it as one, its value unused) got `nil` for its
/// value: returned as it is, that `nil` is no value at all (the statement is in the prereqs).
fn statement_form_value(frame: &HookFrame, value: Expr) -> Expr {
    if frame.statement_form && matches!(value, Expr::Nil) { Expr::None } else { value }
}

/// What makes a decline wrong, from the declining hook's frame and the effect marks: the hook lowered the node or
/// left something of its own (`frame.lowered`), something grew in the offer's scope or the file (its own effects
/// or a nested hook's), or a hook ran inside it (it would run again when the node is lowered as usual). What a
/// reused receiver left is allowed (`allowed`, `allowed_nested`).
fn decline_problem(
    frame: &HookFrame,
    now: &EffectMark,
    allowed: &EffectMark,
    allowed_nested: u32,
) -> Option<&'static str> {
    frame.lowered.or_else(|| now.growth_since(allowed)).or_else(|| {
        (frame.nested_hooks > allowed_nested)
            .then_some("a node inside it was offered to a plugin hook (it would be offered again)")
    })
}

/// A declined call must leave nothing behind (its own effects or those of hooks nested in it, through
/// `compile_expr` of an argument) but a receiver the next plugin or roblox-ts reuses. Reports and returns `true`
/// when it did.
fn check_decline(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &Prereqs,
    context: &mut CallContext,
    plugin: usize,
    start: EffectMark,
    frame: HookFrame,
) -> bool {
    // a receiver compiled first into the offer's scope (compile_receiver) stays for the next plugin, or roblox-ts
    let (allowed, allowed_nested) = match context.base_marks {
        Some((base_start, base_end)) if base_start == start => (base_end, frame.receiver_nested_hooks),
        _ => (start, 0),
    };
    let now = state.plugin_file.effect_mark(prereqs.statements.len());
    let problem = decline_problem(&frame, &now, &allowed, allowed_nested);
    if let Some(problem) = problem {
        let message = format!("plugin `{}` declined a call after {problem}", host.name(plugin));
        state.plugin_internal_error(context.site.node, &message);
    }
    problem.is_some()
}

/// `RBXTS_PLUGIN_CLAIMS=all`: asks the remaining candidates too, in a scratch scope, and reports each that also
/// takes the call. A debug switch must not change the output: a candidate whose hook changed any compiler state
/// before declining (it compiled, placed, imported, named or reported anything) fails the build instead, so a
/// build with the switch either matches the build without it or writes nothing. (The hook's own instance state is
/// out of reach: hooks should decide from the AST and types before doing anything, as the decline rule asks.)
fn report_double_claims(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    context: &CallContext,
    winner: usize,
    rest: &[(usize, Tag)],
) {
    let site = context.site;
    for &(plugin, tag) in rest {
        if plugin == winner {
            continue;
        }
        let mut scratch = Prereqs::new();
        let mut copy = context.clone();
        let (result, frame) = run_hook_framed(
            state,
            Some(&mut scratch),
            Some(&mut copy),
            plugin,
            HookKind::Call,
            site.node,
            |instance, cx| instance.call(cx, &site, tag),
        );
        let message = match result {
            Ok(Claim::Take(_)) => Some(format!(
                "plugins `{}` and `{}` both claimed this call (RBXTS_PLUGIN_CLAIMS=all)",
                host.name(winner),
                host.name(plugin)
            )),
            _ if frame.touched || frame.nested_hooks > 0 => Some(format!(
                "plugin `{}` changed compiler state when asked about a call plugin `{}` claimed \
                 (RBXTS_PLUGIN_CLAIMS=all fails such a build rather than change its output)",
                host.name(plugin),
                host.name(winner)
            )),
            _ => None,
        };
        if let Some(message) = message {
            state.plugin_internal_error(site.node, &message);
        }
    }
}

/// The string text of an element access key, for receiver interests, symbols and `property_key`.
fn literal_key(argument: P<Node>) -> Option<&'static str> {
    matches!(argument.kind(), Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral).then(|| argument.text())
}

/// A property or element access link (not a call).
fn is_access(item: &ChainItem) -> bool {
    matches!(item.kind, ChainItemKind::PropertyAccess { .. } | ChainItemKind::ElementAccess { .. })
}

/// An optional call `o.m?.()` / `o[k]?.()` (its callee is nil-checked by the chain lowering).
fn is_optional_call(item: &ChainItem) -> bool {
    matches!(
        item.kind,
        ChainItemKind::PropertyCall { call_optional: true, .. }
            | ChainItemKind::ElementCall { call_optional: true, .. }
    )
}

/// The call site of a chain call: the site (kind still `Call` for property calls) with its shape, and the method
/// name for receiver interests. `None` for links that are not calls, and for `import()`.
fn chain_site(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    item: &ChainItem,
) -> Option<(CallContext, Option<&'static str>)> {
    let node = item.node;
    let (shape, callee, symbol_node, receiver, method) = match item.kind {
        ChainItemKind::Call => {
            if tsrs_ast::is_import_call(node) {
                return None;
            }
            let callee = node.expression()?;
            let inner = skip_downwards(callee);
            let shape = if inner.kind() == Kind::SuperKeyword { CallShape::Super } else { CallShape::Call };
            (shape, callee, Some(inner), None, None)
        }
        ChainItemKind::PropertyCall { expression: access, name, .. } => {
            let receiver = access.as_property_access_expression().expression;
            (CallShape::Property { access, name }, access, Some(access), Some(receiver), Some(name))
        }
        ChainItemKind::ElementCall { expression: access, argument_expression, .. } => {
            let receiver = access.as_element_access_expression().expression;
            let key = literal_key(argument_expression);
            let shape = CallShape::Element { access, argument: argument_expression };
            (shape, access, key.map(|_| argument_expression), Some(receiver), key)
        }
        ChainItemKind::PropertyAccess { .. } | ChainItemKind::ElementAccess { .. } => return None,
    };
    let symbol = if host.any_symbol { callee_symbol(state, symbol_node) } else { None };
    let kind = if matches!(shape, CallShape::Super) { CallKind::SuperCall } else { CallKind::Call };
    let site = CallSite {
        node,
        kind,
        callee,
        args: node.arguments(),
        symbol,
        receiver,
        in_optional_chain: tsrs_ast::is_optional_chain(node),
        value_used: !is_used_as_statement(node),
    };
    let mut context = CallContext::new(site, shape, None);
    context.item = Some(*item);
    Some((context, method))
}

/// Fills in the method-call kind of a property or element call, once some plugin wants the call.
fn finish_kind(state: &mut TransformState<'_>, context: &mut CallContext) {
    if let CallShape::Property { access, .. } | CallShape::Element { access, .. } = context.shape {
        context.site.kind = access_call_kind(state, access);
    }
}

/// The offers made before anything of a chain is compiled, in the order a recursive lowering would make them: from
/// the outermost link in, each link up to the first `?.` link or optional call (which mean the same lowered on
/// their own) to the expression hooks of its kind, and the chain's first call reached through plain accesses
/// only (`f()`, `ns.f()`, `a.b.m()`) to the call plugins, after the expression hooks. Returns the chain index to
/// continue from and the value so far (a claimed value, or the first call lowered as usual when a plugin compiled
/// its receiver and then declined); `None` to lower the chain as usual (the links offered here are not offered
/// again).
#[inline]
pub fn claim_chain_lazy(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    chain: &[ChainItem],
) -> Option<(usize, Expr)> {
    state.plugins?;
    claim_chain_lazy_slow(state, prereqs, chain)
}

#[inline(never)]
fn claim_chain_lazy_slow(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    chain: &[ChainItem],
) -> Option<(usize, Expr)> {
    let host = state.plugins?;
    let lazy_end = chain.iter().position(|item| item.optional || is_optional_call(item)).unwrap_or(chain.len());
    let start =
        if host.any_calls { chain.iter().position(|item| !is_access(item)).filter(|&k| k < lazy_end) } else { None };
    let any_fallback = chain[..lazy_end].iter().any(|item| host.expressions.contains_key(&item.node.kind()));
    if !any_fallback && start.is_none() {
        return None;
    }
    for i in (0..lazy_end).rev() {
        let item = &chain[i];
        if any_fallback && let Some(value) = claim_expression(state, prereqs, item.node) {
            return Some((i + 1, value));
        }
        if start == Some(i)
            && let Some(value) = claim_chain_start(state, host, prereqs, item)
        {
            return Some((i + 1, value));
        }
    }
    None
}

/// The chain's first call, offered to the call plugins before its callee is compiled. The claimed value, or the
/// call lowered as usual when a plugin compiled its receiver and then declined; `None` to lower it as usual (it is
/// then not offered again).
fn claim_chain_start(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    item: &ChainItem,
) -> Option<Expr> {
    if state.plugin_file.in_offer.contains(&item.node) {
        return None;
    }
    if state.plugin_file.offered.contains(&item.node) {
        // `compile_expr` of the claimed call itself: the item lowering skips it
        return None;
    }
    let (mut context, method) = chain_site(state, host, item)?;
    let candidates = call_candidates(state, host, &context.site, method);
    // asked now or never: the chain lowering does not ask again
    state.plugin_file.offered.insert(item.node);
    if candidates.is_empty() {
        return None;
    }
    finish_kind(state, &mut context);
    if let Some(value) = offer_call(state, host, prereqs, &mut context, &candidates) {
        state.plugin_file.offered.remove(&item.node);
        return Some(value);
    }
    // (an erroneous decline returned nil above, so the node is not lowered or offered again)
    // declined: a `compile_expr` of the call in a hook consumed the mark, which keeps it from being offered again
    state.plugin_file.offered.insert(item.node);
    if context.base_captured {
        // its prereqs were the plugin's (and the decline is an error): lowered again as usual
        return None;
    }
    let base = context.base.take()?;
    Some(transform_chain_item(state, prereqs, base, item))
}

/// A chain link reached by the chain lowering, its base (receiver or callee) compiled and nil-checked inside an
/// optional chain: offered to the expression hooks of its kind (a hook compiling its own node lowers it on that
/// base), then, for a call, to the call plugins. `Ok` with the claimed value, or `Err(base)` to lower it as usual.
#[inline]
pub fn claim_chain_item(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    item: &ChainItem,
    base: Expr,
) -> Result<Expr, Expr> {
    if state.plugins.is_none() {
        return Err(base);
    }
    claim_chain_item_slow(state, prereqs, item, base)
}

#[inline(never)]
fn claim_chain_item_slow(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    item: &ChainItem,
    base: Expr,
) -> Result<Expr, Expr> {
    let Some(host) = state.plugins else { return Err(base) };
    if wants_expression_offer(state, host, item.node) {
        let lowering = EagerLowering::Item { base: base.clone(), item: *item };
        if let Some(value) = claim_expression_eager(state, prereqs, item.node, lowering) {
            return Ok(value);
        }
    }
    claim_chain_call(state, host, prereqs, item, base)
}

/// A chain call offered to the call plugins with its base compiled (`claim_chain_item`).
fn claim_chain_call(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    item: &ChainItem,
    base: Expr,
) -> Result<Expr, Expr> {
    if !host.any_calls || is_access(item) {
        return Err(base);
    }
    if state.plugin_file.in_offer.contains(&item.node) || state.plugin_file.offered.remove(&item.node) {
        return Err(base);
    }
    let Some((mut context, method)) = chain_site(state, host, item) else { return Err(base) };
    let candidates = call_candidates(state, host, &context.site, method);
    if candidates.is_empty() {
        return Err(base);
    }
    finish_kind(state, &mut context);
    context.base = Some(base);
    match offer_call(state, host, prereqs, &mut context, &candidates) {
        Some(value) => Ok(value),
        None => Err(context.base.take().expect("the chain's base")),
    }
}

/// An optional call (`o.m?.()`, `o[k]?.()`), offered inside the chain's nil check on its callee: to the expression
/// hooks of its kind, then to the call plugins, then lowered as roblox-ts lowers it (`None` after roblox-ts's error
/// for a macro).
pub(crate) fn lower_optional_call(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    item: &ChainItem,
    call: &OptionalCall,
) -> Option<Expr> {
    if let Some(value) = claim_optional_call(state, prereqs, item, call) {
        return Some(value);
    }
    crate::nodes::transform_optional_chain::transform_optional_call_value(state, prereqs, item, call)
}

fn claim_optional_call(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    item: &ChainItem,
    call: &OptionalCall,
) -> Option<Expr> {
    let host = state.plugins?;
    if wants_expression_offer(state, host, item.node) {
        let lowering = EagerLowering::OptionalCall { call: call.clone(), item: *item };
        if let Some(value) = claim_expression_eager(state, prereqs, item.node, lowering) {
            return Some(value);
        }
    }
    if !host.any_calls || state.plugin_file.in_offer.contains(&item.node) {
        return None;
    }
    let (mut context, method) = chain_site(state, host, item)?;
    let candidates = call_candidates(state, host, &context.site, method);
    if candidates.is_empty() {
        return None;
    }
    finish_kind(state, &mut context);
    context.optional = Some(call.clone());
    offer_call(state, host, prereqs, &mut context, &candidates)
}

/// `new C(…)`, before constructor macros.
#[inline]
pub fn claim_new(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Option<Expr> {
    state.plugins?;
    claim_new_slow(state, prereqs, node)
}

#[inline(never)]
fn claim_new_slow(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Option<Expr> {
    let host = state.plugins?;
    if !host.any_calls || state.plugin_file.in_offer.contains(&node) || state.plugin_file.offered.remove(&node) {
        return None;
    }
    let callee = node.expression()?;
    let symbol = if host.any_symbol { callee_symbol(state, Some(skip_downwards(callee))) } else { None };
    let site = CallSite {
        node,
        kind: CallKind::New,
        callee,
        args: node.arguments(),
        symbol,
        receiver: None,
        in_optional_chain: false,
        value_used: !is_used_as_statement(node),
    };
    let candidates = call_candidates(state, host, &site, None);
    if candidates.is_empty() {
        return None;
    }
    let mut context = CallContext::new(site, CallShape::New, None);
    offer_call(state, host, prereqs, &mut context, &candidates)
}

/// A decorator (`@D`, `@D(…)`), before its expression is compiled. The claimed value replaces the decorator's
/// expression, which roblox-ts then applies as usual.
#[inline]
pub fn claim_decorator(state: &mut TransformState<'_>, prereqs: &mut Prereqs, decorator: P<Node>) -> Option<Expr> {
    state.plugins?;
    claim_decorator_slow(state, prereqs, decorator)
}

#[inline(never)]
fn claim_decorator_slow(state: &mut TransformState<'_>, prereqs: &mut Prereqs, decorator: P<Node>) -> Option<Expr> {
    let host = state.plugins?;
    if !host.any_calls || state.plugin_file.in_offer.contains(&decorator) {
        return None;
    }
    let expression = decorator.expression()?;
    let inner = skip_downwards(expression);
    let (call, callee, args): (Option<P<Node>>, P<Node>, &'static [P<Node>]) = if inner.kind() == Kind::CallExpression {
        (Some(inner), inner.expression()?, inner.arguments())
    } else {
        (None, expression, &[])
    };
    let symbol = if host.any_symbol { callee_symbol(state, Some(skip_downwards(callee))) } else { None };
    let site = CallSite {
        node: decorator,
        kind: CallKind::Decorator,
        callee,
        args,
        symbol,
        receiver: None,
        in_optional_chain: false,
        value_used: true,
    };
    let candidates = call_candidates(state, host, &site, None);
    if candidates.is_empty() {
        return None;
    }
    // offered as a decorator, not again as the call inside it
    if let Some(call) = call {
        state.plugin_file.offered.insert(call);
    }
    let mut context = CallContext::new(site, CallShape::Decorator { call }, None);
    let value = offer_call(state, host, prereqs, &mut context, &candidates);
    if value.is_some()
        && let Some(call) = call
    {
        state.plugin_file.offered.remove(&call);
    }
    value
}

// --- by-kind fallbacks ---

/// The `expressions(kind)` fallback, before roblox-ts's own lowering of the node.
#[inline]
pub fn claim_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Option<Expr> {
    state.plugins?;
    claim_expression_slow(state, prereqs, node)
}

#[inline(never)]
fn claim_expression_slow(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Option<Expr> {
    let host = state.plugins?;
    let list = host.expressions.get(&node.kind())?;
    // asked once per node; a `delete` operand is lowered as the deletion, not as a value
    if state.plugin_file.fallback_declined.contains(&node) || is_delete_operand(node) {
        return None;
    }
    if !state.plugin_file.in_fallback_offer.insert(node) {
        return None;
    }
    let value = claim_expression_offer(state, host, prereqs, node, list);
    state.plugin_file.in_fallback_offer.remove(&node);
    match value {
        Some(value) => Some(unused_value(prereqs, node, value, !is_used_as_statement(node))),
        None => {
            state.plugin_file.fallback_declined.insert(node);
            None
        }
    }
}

/// Whether the expression hooks would be asked about `node` now (so its eager lowering is worth preparing).
fn wants_expression_offer(state: &TransformState<'_>, host: &PluginHost, node: P<Node>) -> bool {
    host.expressions.contains_key(&node.kind())
        && !state.plugin_file.fallback_declined.contains(&node)
        && !state.plugin_file.in_fallback_offer.contains(&node)
}

/// The expression hooks' offer of a chain link whose base is compiled: a hook compiling its own node lowers it
/// with `lowering`.
fn claim_expression_eager(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    lowering: EagerLowering,
) -> Option<Expr> {
    state.plugin_file.eager_offers.push((node, lowering));
    let value = claim_expression(state, prereqs, node);
    state.plugin_file.eager_offers.pop();
    value
}

fn claim_expression_offer(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    node: P<Node>,
    list: &[(usize, Tag)],
) -> Option<Expr> {
    for &(plugin, tag) in list {
        if !state.plugin_file.begun[plugin] {
            continue;
        }
        let start = state.plugin_file.effect_mark(prereqs.statements.len());
        let (result, frame) =
            run_hook_framed(state, Some(prereqs), None, plugin, HookKind::Expression, node, |instance, cx| {
                instance.expression(cx, node, tag)
            });
        match result {
            Ok(Claim::Take(value)) => {
                state.plugin_file.emitted = true;
                return Some(statement_form_value(&frame, value));
            }
            Ok(Claim::Decline) => {
                let now = state.plugin_file.effect_mark(prereqs.statements.len());
                if let Some(what) = decline_problem(&frame, &now, &start, 0) {
                    let message = format!("plugin `{}` declined an expression after {what}", host.name(plugin));
                    state.plugin_internal_error(node, &message);
                    // already an error: no other plugin, and not roblox-ts, gets the node
                    return Some(create::nil());
                }
            }
            Err(()) => return Some(create::nil()),
        }
    }
    None
}

/// The `statements(kind)` fallback, before roblox-ts's own lowering of the statement. A claim's prereqs come first.
#[inline]
pub fn claim_statement(state: &mut TransformState<'_>, node: P<Node>) -> Option<Vec<Stmt>> {
    state.plugins?;
    claim_statement_slow(state, node)
}

#[inline(never)]
fn claim_statement_slow(state: &mut TransformState<'_>, node: P<Node>) -> Option<Vec<Stmt>> {
    let host = state.plugins?;
    let list = host.statements.get(&node.kind())?;
    if !state.plugin_file.in_fallback_offer.insert(node) {
        return None;
    }
    let value = claim_statement_offer(state, host, node, list);
    state.plugin_file.in_fallback_offer.remove(&node);
    value
}

fn claim_statement_offer(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    node: P<Node>,
    list: &[(usize, Tag)],
) -> Option<Vec<Stmt>> {
    for &(plugin, tag) in list {
        if !state.plugin_file.begun[plugin] {
            continue;
        }
        let mut prereqs = Prereqs::new();
        let start = state.plugin_file.effect_mark(0);
        let (result, frame) =
            run_hook_framed(state, Some(&mut prereqs), None, plugin, HookKind::Statement, node, |instance, cx| {
                instance.statement(cx, node, tag)
            });
        match result {
            Ok(Claim::Take(statements)) => {
                state.plugin_file.emitted = true;
                let mut out = prereqs.statements;
                out.extend(statements);
                return Some(out);
            }
            Ok(Claim::Decline) => {
                let now = state.plugin_file.effect_mark(prereqs.statements.len());
                if let Some(what) = decline_problem(&frame, &now, &start, 0) {
                    let message = format!("plugin `{}` declined a statement after {what}", host.name(plugin));
                    state.plugin_internal_error(node, &message);
                    return Some(Vec::new());
                }
            }
            Err(()) => return Some(Vec::new()),
        }
    }
    None
}

// --- property keys ---

/// Above this many `accesses_by_marker` registrations, a receiver's members are matched against the markers
/// (one lookup per member) instead of each marker against the receiver (one lookup per marker).
const MARKER_SCAN_LIMIT: usize = 16;

/// The plugins whose access interests match an access on `receiver`, with their tags, in plugin then registration
/// order.
fn access_candidates(state: &mut TransformState<'_>, host: &PluginHost, receiver: P<Node>) -> Vec<(usize, Tag)> {
    let t = state.get_type(receiver);
    let t = state.checker.get_non_nullable_type(t);
    let apparent = state.checker.get_apparent_type(t);
    let mut matched: Vec<usize> = if host.access_markers.len() <= MARKER_SCAN_LIMIT {
        (0..host.access_markers.len())
            .filter(|&i| state.checker.get_property_of_type_exported(apparent, host.access_markers[i].1).is_some())
            .collect()
    } else {
        let mut matched = Vec::new();
        for &member in state.checker.get_properties_of_type_exported(apparent) {
            if let Some(indices) = host.access_marker_index.get(member.name()) {
                matched.extend(indices.iter().copied());
            }
        }
        matched.sort_unstable();
        matched.dedup();
        // each confirmed as a single-marker check would find it
        matched.retain(|&i| state.checker.get_property_of_type_exported(apparent, host.access_markers[i].1).is_some());
        matched
    };
    matched.retain(|&i| state.plugin_file.begun[host.access_markers[i].0]);
    matched.into_iter().map(|i| (host.access_markers[i].0, host.access_markers[i].2)).collect()
}

/// The key a plugin's `property_key` gave the access `access` (a PropertyAccessExpression or an
/// ElementAccessExpression), when it differs from the site's key. `None` lowers the access as written. Asked once
/// per access node; every lowering of it (a read, a write, a callee) gets the same answer. A caller that uses a
/// rewritten key in place of a computed argument still evaluates the argument ([`replaced_key_statements`]).
#[inline]
pub fn rewritten_key(state: &mut TransformState<'_>, access: P<Node>, position: AccessPosition) -> Option<String> {
    let host = state.plugins?;
    if host.access_markers.is_empty() {
        return None;
    }
    rewritten_key_slow(state, host, access, position)
}

#[inline(never)]
fn rewritten_key_slow(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    access: P<Node>,
    position: AccessPosition,
) -> Option<String> {
    if let Some(answer) = state.plugin_file.key_answers.get(&access) {
        return answer.clone();
    }
    let answer = property_key_offer(state, host, access, position);
    state.plugin_file.key_answers.insert(access, answer.clone());
    answer
}

fn property_key_offer(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    access: P<Node>,
    position: AccessPosition,
) -> Option<String> {
    let (receiver, key_node) = access_parts(access)?;
    let candidates = access_candidates(state, host, receiver);
    if candidates.is_empty() {
        return None;
    }
    let key = access_key(state, access, key_node);
    let site = AccessSite { node: access, receiver, key, key_node, position };
    for (plugin, tag) in candidates {
        let start = state.plugin_file.effect_mark(0);
        let (result, frame) =
            run_hook_framed(state, None, None, plugin, HookKind::PropertyKey, access, |instance, cx| {
                instance.property_key(cx, &site, tag)
            });
        match result {
            Ok(Claim::Take(_)) if key.is_none() => {
                // a computed key has no text to rewrite: the access stays as written, and the build fails
                let message = format!(
                    "plugin `{}` took a property key whose key is computed (AccessSite.key is None): such a site can \
                     only be declined or failed",
                    host.name(plugin)
                );
                state.plugin_internal_error(access, &message);
                return None;
            }
            // the key as it is lowers the access as written
            Ok(Claim::Take(new_key)) => {
                let changed = Some(new_key.as_str()) != key;
                state.plugin_file.emitted |= changed;
                return changed.then_some(new_key);
            }
            Ok(Claim::Decline) => {
                let now = state.plugin_file.effect_mark(0);
                if let Some(what) = decline_problem(&frame, &now, &start, 0) {
                    let message = format!("plugin `{}` declined a property key after {what}", host.name(plugin));
                    state.plugin_internal_error(access, &message);
                    return None;
                }
            }
            Err(()) => return None,
        }
    }
    None
}

/// The receiver and key node of a property access (its name) or element access (its argument).
fn access_parts(access: P<Node>) -> Option<(P<Node>, P<Node>)> {
    match access.kind() {
        Kind::PropertyAccessExpression => {
            let property = access.as_property_access_expression();
            Some((property.expression, property.name))
        }
        Kind::ElementAccessExpression => {
            let element = access.as_element_access_expression();
            Some((element.expression, element.argument_expression))
        }
        _ => None,
    }
}

/// `AccessSite.key` / `AssignSite.key`: a property access's name; an element access's argument when it is a string
/// literal, else the value of its type when that is a single string literal, through casts and identifiers
/// (`obj[k as "a"]`, `obj[k]` with `k: "a"`, a string enum member; wider than Flamework's TS transformer, which
/// knows only `obj.k` and `obj["k"]`). Numeric keys are `None`.
fn access_key(state: &mut TransformState<'_>, access: P<Node>, key_node: P<Node>) -> Option<&'static str> {
    if access.kind() == Kind::PropertyAccessExpression {
        return Some(key_node.text());
    }
    if let Some(text) = literal_key(key_node) {
        return Some(text);
    }
    let t = state.get_type(key_node);
    crate::util::types::string_literal_value(t)
}

/// A rewritten key replaced the computed argument of the element access `access` (not compiled yet): the argument
/// is still evaluated for its effects, its value dropped. The statements that do it, to place after the receiver is
/// evaluated (empty for a literal argument, a property access, or an argument without effects).
pub(crate) fn replaced_key_statements(state: &mut TransformState<'_>, access: P<Node>) -> Vec<Stmt> {
    if access.kind() != Kind::ElementAccessExpression {
        return Vec::new();
    }
    let argument = access.as_element_access_expression().argument_expression;
    if matches!(
        skip_downwards(argument).kind(),
        Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral | Kind::NumericLiteral
    ) {
        return Vec::new();
    }
    let mut prereqs = Prereqs::new();
    let value = crate::nodes::expressions::transform_expression::transform_expression(state, &mut prereqs, argument);
    let mut statements = prereqs.statements;
    statements.extend(discarded_key_statements(state, access, value));
    statements
}

/// The compiled argument `index` of the element access `access` whose key a plugin rewrote: evaluated as a
/// statement for its effects, or nothing when reading it has none. That is an argument without effects in Luau, or
/// one that only reads (an identifier, `this`, a literal, an enum member such as `E.A`): Flamework's TS
/// transformer drops the argument with the key, and reading a name or a member of an enum's table does nothing.
pub(crate) fn discarded_key_statements(state: &mut TransformState<'_>, access: P<Node>, index: Expr) -> Vec<Stmt> {
    use crate::util::evaluation::effects::{can_discard, get_effects};
    if access.kind() == Kind::ElementAccessExpression
        && only_reads(state, access.as_element_access_expression().argument_expression)
    {
        return Vec::new();
    }
    let mut index = index;
    while let Expr::Parenthesized(parenthesized) = index {
        index = parenthesized.expression;
    }
    if can_discard(&get_effects(&index)) { Vec::new() } else { wrap_expression_statement(index) }
}

/// Whether evaluating the TS expression `node` only reads: a literal, `this`, an identifier, or a member of an enum
/// (through namespaces), behind parentheses and casts.
fn only_reads(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    use crate::util::expression_might_mutate::is_enum_member_access;
    let node = skip_downwards(node);
    match node.kind() {
        Kind::StringLiteral
        | Kind::NoSubstitutionTemplateLiteral
        | Kind::NumericLiteral
        | Kind::TrueKeyword
        | Kind::FalseKeyword
        | Kind::NullKeyword
        | Kind::ThisKeyword
        | Kind::Identifier => true,
        Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
            is_enum_member_access(state, node)
                && reads_namespace_path(state, node.expression().expect("an access has an expression"))
        }
        _ => false,
    }
}

/// Whether `node` names an enum or a namespace by identifiers and property accesses only (`E`, `NS.E`).
fn reads_namespace_path(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let node = skip_downwards(node);
    match node.kind() {
        Kind::Identifier => true,
        Kind::PropertyAccessExpression => {
            let name = node.as_property_access_expression().name;
            let symbol = state.checker.get_symbol_at_location_exported(name);
            symbol.is_some_and(|s| {
                state.checker.skip_alias(s).flags().intersects(SymbolFlags::Enum | SymbolFlags::ValueModule)
            }) && reads_namespace_path(state, node.as_property_access_expression().expression)
        }
        _ => false,
    }
}

/// Places the statements that evaluate a replaced key ([`replaced_key_statements`], [`discarded_key_statements`])
/// after the receiver `base`: `base` is bound to a local first when the key's effects could change it.
pub(crate) fn place_key_statements(prereqs: &mut Prereqs, base: Expr, statements: Vec<Stmt>) -> Expr {
    use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements};
    if statements.is_empty() {
        return base;
    }
    let base = if effects_commute(&get_effects(&base), &get_effects_of_statements(&statements)) {
        base
    } else {
        prereqs.push_to_var(base, "exp")
    };
    prereqs.push_list(statements);
    base
}

/// The position of an access as roblox-ts lowers it for a write: the target of `=` (or of a destructuring or a
/// `for … of` head), or of a compound assignment, `++` or `--`.
pub fn write_position(access: P<Node>) -> AccessPosition {
    match tsrs_ast::get_assignment_target(skip_upwards(access)) {
        Some(target) if target.kind() == Kind::BinaryExpression => {
            if target.as_binary_expression().operator_token.kind() == Kind::EqualsToken {
                AccessPosition::Write
            } else {
                AccessPosition::ReadWrite
            }
        }
        Some(target) if matches!(target.kind(), Kind::PrefixUnaryExpression | Kind::PostfixUnaryExpression) => {
            AccessPosition::ReadWrite
        }
        _ => AccessPosition::Write,
    }
}

// --- assignments ---

/// `op=` as the binary operator it applies (`+=` is `PlusToken`).
fn compound_operator(kind: Kind) -> Option<Kind> {
    Some(match kind {
        Kind::PlusEqualsToken => Kind::PlusToken,
        Kind::MinusEqualsToken => Kind::MinusToken,
        Kind::AsteriskEqualsToken => Kind::AsteriskToken,
        Kind::AsteriskAsteriskEqualsToken => Kind::AsteriskAsteriskToken,
        Kind::SlashEqualsToken => Kind::SlashToken,
        Kind::PercentEqualsToken => Kind::PercentToken,
        Kind::LessThanLessThanEqualsToken => Kind::LessThanLessThanToken,
        Kind::GreaterThanGreaterThanEqualsToken => Kind::GreaterThanGreaterThanToken,
        Kind::GreaterThanGreaterThanGreaterThanEqualsToken => Kind::GreaterThanGreaterThanGreaterThanToken,
        Kind::AmpersandEqualsToken => Kind::AmpersandToken,
        Kind::BarEqualsToken => Kind::BarToken,
        Kind::CaretEqualsToken => Kind::CaretToken,
        Kind::BarBarEqualsToken => Kind::BarBarToken,
        Kind::AmpersandAmpersandEqualsToken => Kind::AmpersandAmpersandToken,
        Kind::QuestionQuestionEqualsToken => Kind::QuestionQuestionToken,
        _ => return None,
    })
}

/// The target, operation and value of an assignment, update or `delete` whose target is a property or element
/// access (destructuring assignments are not: their targets are pattern elements).
fn assignment_parts(node: P<Node>) -> Option<(P<Node>, AssignOp, Option<P<Node>>)> {
    let (target, op, value) = match node.kind() {
        Kind::BinaryExpression => {
            let binary = node.as_binary_expression();
            let operator = binary.operator_token.kind();
            let op = if operator == Kind::EqualsToken {
                AssignOp::Assign
            } else {
                AssignOp::Compound(compound_operator(operator)?)
            };
            (binary.left, op, Some(binary.right()))
        }
        Kind::PrefixUnaryExpression => {
            let unary = node.as_prefix_unary_expression();
            let op = match unary.operator {
                Kind::PlusPlusToken => AssignOp::PreInc,
                Kind::MinusMinusToken => AssignOp::PreDec,
                _ => return None,
            };
            (unary.operand, op, None)
        }
        Kind::PostfixUnaryExpression => {
            let unary = node.as_postfix_unary_expression();
            let op = match unary.operator {
                Kind::PlusPlusToken => AssignOp::PostInc,
                Kind::MinusMinusToken => AssignOp::PostDec,
                _ => return None,
            };
            (unary.operand, op, None)
        }
        Kind::DeleteExpression => (node.expression()?, AssignOp::Delete, None),
        _ => return None,
    };
    let target = skip_downwards(target);
    matches!(target.kind(), Kind::PropertyAccessExpression | Kind::ElementAccessExpression)
        .then_some((target, op, value))
}

/// The `assignment` hook: an assignment, compound or logical assignment, `++` / `--` or `delete` whose target is
/// `receiver.key` (or `receiver["key"]`) and whose receiver's symbol a plugin classified (role `AssignReceiver`).
/// Offered once, before roblox-ts lowers the node, after the expression hooks.
#[inline]
pub fn claim_assignment(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Option<Expr> {
    let host = state.plugins?;
    if !host.any_assignments {
        return None;
    }
    claim_assignment_slow(state, host, prereqs, node)
}

#[inline(never)]
fn claim_assignment_slow(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Option<Expr> {
    if state.plugin_file.in_assign_offer.contains(&node) || state.plugin_file.assign_declined.contains(&node) {
        return None;
    }
    let (target, op, value) = assignment_parts(node)?;
    let (receiver, key_node) = access_parts(target)?;
    let symbol = callee_symbol(state, Some(skip_downwards(receiver)))?;
    let mut candidates = Vec::new();
    for (plugin, entry) in host.plugins.iter().enumerate() {
        if entry.assignments
            && state.plugin_file.begun[plugin]
            && let Some(tag) = classify(state, host, plugin, symbol, SymbolRole::AssignReceiver, node)
        {
            candidates.push((plugin, tag));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let key = access_key(state, target, key_node);
    let site = AssignSite { node, target, receiver, key, key_node, op, value, value_used: !is_used_as_statement(node) };
    state.plugin_file.in_assign_offer.insert(node);
    let value = claim_assignment_offer(state, host, prereqs, &site, &candidates);
    state.plugin_file.in_assign_offer.remove(&node);
    if value.is_none() {
        state.plugin_file.assign_declined.insert(node);
    }
    value
}

fn claim_assignment_offer(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    prereqs: &mut Prereqs,
    site: &AssignSite,
    candidates: &[(usize, Tag)],
) -> Option<Expr> {
    for &(plugin, tag) in candidates {
        let start = state.plugin_file.effect_mark(prereqs.statements.len());
        let (result, frame) =
            run_hook_framed(state, Some(prereqs), None, plugin, HookKind::Assignment, site.node, |instance, cx| {
                instance.assignment(cx, site, tag)
            });
        match result {
            Ok(Claim::Take(value)) => {
                state.plugin_file.emitted = true;
                return Some(unused_value(prereqs, site.node, statement_form_value(&frame, value), site.value_used));
            }
            Ok(Claim::Decline) => {
                let now = state.plugin_file.effect_mark(prereqs.statements.len());
                if let Some(what) = decline_problem(&frame, &now, &start, 0) {
                    let message = format!("plugin `{}` declined an assignment after {what}", host.name(plugin));
                    state.plugin_internal_error(site.node, &message);
                    // already an error: no other plugin, and not roblox-ts, gets the node
                    return Some(create::nil());
                }
            }
            Err(()) => return Some(create::nil()),
        }
    }
    None
}

/// An expression statement's expression that roblox-ts lowers as a statement of its own (an assignment, logical
/// assignment, `++` or `--`): offered to the expression hooks of its kind, then to the assignment hook, since it
/// does not go through the expression lowering. The claimed value is evaluated as a statement.
#[inline]
pub fn claim_statement_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Option<Expr> {
    state.plugins?;
    claim_expression(state, prereqs, node).or_else(|| claim_assignment(state, prereqs, node))
}

// --- classes ---

/// The class, a member, or a parameter of the constructor or a method has a decorator.
fn is_decorated(node: P<Node>) -> bool {
    !node.decorators().is_empty()
        || node.members().iter().any(|&member| {
            !member.decorators().is_empty()
                || (matches!(member.kind(), Kind::MethodDeclaration | Kind::Constructor)
                    && member.parameters().iter().any(|p| !p.decorators().is_empty()))
        })
}

/// The `class` hook: every plugin whose filter admits the class, in registration order, statements concatenated
/// (each plugin's prereqs before its statements). They go at the static-block position: after the statics, before
/// the decorators.
#[inline]
pub fn class_statements(state: &mut TransformState<'_>, node: P<Node>, luau_name: &AnyIdentifier) -> Vec<Stmt> {
    if state.plugins.is_none() {
        return Vec::new();
    }
    class_statements_slow(state, node, luau_name)
}

#[inline(never)]
fn class_statements_slow(state: &mut TransformState<'_>, node: P<Node>, luau_name: &AnyIdentifier) -> Vec<Stmt> {
    let Some(host) = state.plugins else { return Vec::new() };
    if !host.any_classes {
        return Vec::new();
    }
    if !state.plugin_file.in_class_offer.insert(node) {
        return Vec::new();
    }
    let decorated = is_decorated(node);
    let site = ClassSite { node, luau_name: luau_name.clone().into(), decorated };
    let mut out = Vec::new();
    for plugin in 0..host.len() {
        let admitted = match host.plugins[plugin].class_filter {
            Some(ClassFilter::All) => true,
            Some(ClassFilter::Decorated) => decorated,
            None => false,
        };
        if !admitted || !state.plugin_file.begun[plugin] {
            continue;
        }
        let mut prereqs = Prereqs::new();
        if let Ok(statements) =
            run_hook(state, Some(&mut prereqs), None, plugin, HookKind::Class, node, |instance, cx| {
                instance.class(cx, &site)
            })
        {
            out.extend(prereqs.statements);
            state.plugin_file.emitted |= !statements.is_empty();
            out.extend(statements);
        }
    }
    state.plugin_file.in_class_offer.remove(&node);
    out
}

// --- files ---

/// `begin_file` of every plugin, before the file's statements are transformed.
#[inline]
pub fn begin_file(state: &mut TransformState<'_>) {
    if state.plugins.is_none() {
        return;
    }
    begin_file_slow(state)
}

#[inline(never)]
fn begin_file_slow(state: &mut TransformState<'_>) {
    let Some(host) = state.plugins else { return };
    let root = state.source_file.as_node();
    for plugin in 0..host.len() {
        let begun =
            run_hook(state, None, None, plugin, HookKind::BeginFile, root, |instance, cx| instance.begin_file(cx))
                .is_ok();
        state.plugin_file.begun[plugin] = begun;
    }
}

/// `end_file` of every plugin (their parts are kept for `finish`), then what was decided only now: import elision
/// and kept imports, and the placement of plugin imports and header statements.
#[inline]
pub fn end_file(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    if state.plugins.is_none() {
        return;
    }
    end_file_slow(state, statements)
}

#[inline(never)]
fn end_file_slow(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    let Some(host) = state.plugins else { return };
    let root = state.source_file.as_node();
    for plugin in 0..host.len() {
        if let Ok(part) =
            run_hook(state, None, None, plugin, HookKind::EndFile, root, |instance, cx| instance.end_file(cx))
        {
            state.plugin_file.parts[plugin] = Some(part);
        }
    }
    // only claim hooks insert before root statements, and they run inside statements; nothing should be left
    if !state.plugin_file.root_inserts.is_empty() {
        statements.append(&mut state.plugin_file.root_inserts);
    }
    // a statement roblox-ts lowered outside a statement list would lose its insertions: never silent
    let mut unplaced: Vec<P<Node>> = state.plugin_file.before_statement.drain().map(|(node, _)| node).collect();
    unplaced.sort_by_key(|node| node.pos().source_offset());
    for node in unplaced {
        state.plugin_internal_error(
            node,
            "insert_before_statement: this statement was not lowered in a statement list, so the statements a plugin \
             placed before it were dropped",
        );
    }
    apply_import_decisions(state, statements);
    place_imports_and_header(state, statements);
}

/// Statements inserted at `index` into the file's list after end_file (`local exports`, the header comment and
/// `local TS`), so the plugins' root ranges can be found in the final list.
#[inline]
pub fn note_front_insert(state: &mut TransformState<'_>, index: usize, count: usize) {
    if state.plugins.is_some() {
        state.plugin_file.front_inserts.push((index, count));
    }
}

/// The complete file's main chunk against Luau's local limit, when a plugin added locals to it (`spill`).
#[inline]
pub fn check_main_chunk(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    if state.plugins.is_some() {
        super::spill::check_main_chunk(state, statements);
    }
}

/// Where a top-level statement's transform began.
#[derive(Clone, Copy, Debug)]
pub struct RootMark {
    diagnostics_start: usize,
    runtime_lib_uses: u32,
}

/// Starts a statement of the file's top level (`parent` is the source file); `None` otherwise or without plugins.
#[inline]
pub fn root_statement_start(state: &mut TransformState<'_>, parent: Option<P<Node>>) -> Option<RootMark> {
    state.plugins?;
    if parent?.kind() != Kind::SourceFile {
        return None;
    }
    state.plugin_file.root_temp_start = crate::classes::prereqs::next_temp_id();
    Some(RootMark { diagnostics_start: state.diagnostics.diagnostics.len(), runtime_lib_uses: state.runtime_lib_uses })
}

/// What plugins put before a statement of a statement list (`transformStatementList`), into `result`, in this
/// order (as TypeScript prints a TS transformer's output):
/// - for the file's first top-level statement, the file's header comment (TypeScript's detached comments, which its
///   printer keeps at the top of the file), taken from the front of `leading`;
/// - for a top-level statement (`root`), the root insertions (`hoist`, `insert_before_root_statement`);
/// - the `insert_before_statement` statements for this statement;
/// - the plugin comments (`comment`) leading the statement's own Luau, taken from the front of `transformed`.
///
/// The statement's leading comments (what is left of `leading`) follow.
#[inline]
pub fn before_statement(
    state: &mut TransformState<'_>,
    root: bool,
    statement: P<Node>,
    result: &mut Vec<Stmt>,
    leading: &mut Vec<Stmt>,
    transformed: &mut Vec<Stmt>,
) {
    if state.plugins.is_some() {
        before_statement_slow(state, root, statement, result, leading, transformed);
    }
}

#[inline(never)]
fn before_statement_slow(
    state: &mut TransformState<'_>,
    root: bool,
    statement: P<Node>,
    result: &mut Vec<Stmt>,
    leading: &mut Vec<Stmt>,
    transformed: &mut Vec<Stmt>,
) {
    if root {
        if state.plugin_file.root_ranges.is_empty() && !leading.is_empty() {
            let detached = detached_comment_count(state, statement).min(leading.len());
            state.plugin_file.detached_comments = detached;
            result.extend(leading.drain(..detached));
        }
        result.append(&mut state.plugin_file.root_inserts);
    }
    let file = &mut state.plugin_file;
    if let Some(statements) = file.take_before_statement(statement) {
        result.extend(statements);
    }
    if let Some(texts) = file.statement_comments.remove(&statement) {
        let mut texts = texts.iter();
        let mut next = texts.next();
        let mut moved = 0;
        while let (Some(Stmt::Comment(text)), Some(expected)) = (transformed.get(moved), next) {
            if text != expected {
                break;
            }
            moved += 1;
            next = texts.next();
        }
        result.extend(transformed.drain(..moved));
    }
}

/// The `insert_before_statement` statements for an `else if` (`statement`), which roblox-ts lowers outside any
/// statement list: they go first in `prereqs`, which then makes the `else` a block (`else <statements> if … end
/// end`), as roblox-ts lowers the `else { …; if … }` Flamework's TS transform makes of it.
#[inline]
pub fn else_if_before_statement(state: &mut TransformState<'_>, statement: P<Node>, prereqs: &mut Vec<Stmt>) {
    if state.plugins.is_some()
        && let Some(mut statements) = state.plugin_file.take_before_statement(statement)
    {
        statements.append(prereqs);
        *prereqs = statements;
    }
}

/// The TS statement `insert_before_statement` and `comment` place before: `node` itself when it is a statement,
/// else its nearest ancestor that is one (TypeScript's `isStatement`, as Flamework's transform finds the statement a
/// `prereq` goes before). A block counts only through its statements.
pub(crate) fn enclosing_statement(node: P<Node>) -> Option<P<Node>> {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == Kind::SourceFile {
            return None;
        }
        if n.kind() != Kind::Block && tsrs_ast::is_statement(n) {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

/// How many leading comments of the file's first statement TypeScript's printer treats as the file's header
/// (`emitDetachedComments`): the first comments up to a blank line between two of them, when a blank line follows
/// the last one before the statement.
fn detached_comment_count(state: &TransformState<'_>, statement: P<Node>) -> usize {
    let file = state.source_file;
    let Some(pos) = statement.pos().source_offset() else { return 0 };
    let line_starts = tsrs_scanner::get_ecma_line_starts(&*file);
    let line = |offset: u32| tsrs_scanner::compute_line_of_position(line_starts, offset);
    let mut count = 0;
    let mut last_end = None;
    for range in tsrs_scanner::get_leading_comment_ranges(file.get().text(), pos) {
        let (Some(start), Some(end)) = (range.text_range.pos().source_offset(), range.text_range.end().source_offset())
        else {
            return 0;
        };
        if let Some(last) = last_end
            && line(start) >= line(last) + 2
        {
            break;
        }
        count += 1;
        last_end = Some(end);
    }
    match last_end {
        Some(last) if line(crate::util::traversal::get_start(statement)) >= line(last) + 2 => count,
        _ => 0,
    }
}

/// Ends a top-level statement whose own Luau is `result[start..]`.
pub fn root_statement_end(
    state: &mut TransformState<'_>,
    mark: RootMark,
    statement: P<Node>,
    result: &[Stmt],
    start: usize,
) {
    state.plugin_file.main_chunk_locals += result[start..].iter().map(declared_locals).sum::<u32>();
    let range = RootRange {
        node: statement,
        start,
        end: result.len(),
        diagnostics_start: mark.diagnostics_start,
        diagnostics_end: state.diagnostics.diagnostics.len(),
        runtime_lib_uses: state.runtime_lib_uses - mark.runtime_lib_uses,
    };
    state.plugin_file.root_ranges.push(range);
}

/// `parent` when it is the body of a function-like node and the build has plugins (its locals are counted for
/// `locals().function`).
#[inline]
pub fn function_body_of(state: &TransformState<'_>, parent: Option<P<Node>>) -> Option<P<Node>> {
    state.plugins?;
    let body = parent?;
    let function = body.parent()?;
    (tsrs_ast::is_function_like(function) && function.body() == Some(body)).then_some(body)
}

/// Adds the locals a statement of a function body declared (`statements`, its Luau) to the body's count.
pub fn count_function_locals(state: &mut TransformState<'_>, body: P<Node>, statements: &[Stmt]) {
    let count: u32 = statements.iter().map(declared_locals).sum();
    *state.plugin_file.function_locals.entry(body).or_default() += count;
}

/// The alias declarations of an import statement.
fn alias_declarations(statement: P<Node>) -> Vec<P<Node>> {
    match statement.kind() {
        Kind::ImportDeclaration => {
            let Some(clause_node) = statement.as_import_declaration().import_clause else { return Vec::new() };
            let clause = clause_node.as_import_clause();
            let mut out = Vec::new();
            if clause.name.is_some() {
                out.push(clause_node);
            }
            if let Some(bindings) = clause.named_bindings {
                if bindings.kind() == Kind::NamespaceImport {
                    out.push(bindings);
                } else {
                    out.extend(bindings.elements().iter().copied());
                }
            }
            out
        }
        Kind::ImportEqualsDeclaration => vec![statement],
        _ => Vec::new(),
    }
}

/// The local name an alias declaration binds.
fn alias_name(alias: P<Node>) -> Option<&'static str> {
    match alias.kind() {
        Kind::ImportClause => alias.as_import_clause().name.map(|n| n.text()),
        Kind::NamespaceImport => Some(alias.as_namespace_import().name.text()),
        Kind::ImportEqualsDeclaration => Some(alias.as_import_equals_declaration().name.text()),
        _ => alias.name().map(|n| n.text()),
    }
}

/// The local names a file re-exports with `export { a, b as c }` (no module specifier).
fn reexported_names(file: P<SourceFile>) -> FxHashSet<&'static str> {
    let mut names = FxHashSet::default();
    for &statement in file.get().statements.nodes() {
        if statement.kind() != Kind::ExportDeclaration {
            continue;
        }
        let declaration = statement.as_export_declaration();
        if declaration.module_specifier.is_some() {
            continue;
        }
        let Some(clause) = declaration.export_clause else { continue };
        if clause.kind() != Kind::NamedExports {
            continue;
        }
        for &specifier in clause.elements() {
            let export = specifier.as_export_specifier();
            let local = export.property_name.unwrap_or(export.name);
            if local.kind() == Kind::Identifier {
                names.insert(local.text());
            }
        }
    }
    names
}

/// Import elision decided after the whole file is lowered: an alias a plugin asked to elide goes unless the
/// compiler compiled a reference to it, a plugin kept it (`mark_import_used`, a reused import), or the file
/// re-exports it; a kept alias is emitted though the checker saw no value use. The affected import declarations
/// are transformed again, and their Luau and diagnostics replace the first transform's.
fn apply_import_decisions(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    if state.plugin_file.elision_requests.is_empty() && state.plugin_file.forced_imports.is_empty() {
        return;
    }
    let reexported = reexported_names(state.source_file);
    for i in (0..state.plugin_file.root_ranges.len()).rev() {
        let range = state.plugin_file.root_ranges[i];
        let aliases = alias_declarations(range.node);
        // `export import X = …` re-exports X: never elided
        if aliases.is_empty() || has_syntactic_modifier(range.node, ModifierFlags::Export) {
            continue;
        }
        let file = &state.plugin_file;
        let mut overrides = ImportOverrides::default();
        for alias in aliases {
            if file.forced_imports.contains(&alias) {
                overrides.forced.insert(alias);
            } else if file.elision_requests.contains(&alias)
                && !file.compiled_aliases.contains(&alias)
                && !alias_name(alias).is_some_and(|name| reexported.contains(name))
            {
                overrides.elided.insert(alias);
            }
        }
        if overrides.elided.is_empty() && overrides.forced.is_empty() {
            continue;
        }
        state.plugin_file.import_overrides = Some(overrides);
        let diagnostics_before = state.diagnostics.diagnostics.len();
        // the first transform's uses of `TS` are taken back; the new one counts its own
        state.runtime_lib_uses -= range.runtime_lib_uses;
        let uses_before = state.runtime_lib_uses;
        let replacement = if range.node.kind() == Kind::ImportDeclaration {
            transform_import_declaration(state, range.node)
        } else {
            transform_import_equals_declaration(state, range.node)
        };
        state.plugin_file.import_overrides = None;
        state.plugin_file.root_ranges[i].runtime_lib_uses = state.runtime_lib_uses - uses_before;
        state.uses_runtime_lib = state.runtime_lib_uses > 0;
        let new_diagnostics: Vec<_> = state.diagnostics.diagnostics.drain(diagnostics_before..).collect();
        let diagnostics_delta =
            new_diagnostics.len() as isize - (range.diagnostics_end - range.diagnostics_start) as isize;
        state.diagnostics.diagnostics.splice(range.diagnostics_start..range.diagnostics_end, new_diagnostics);
        let delta = replacement.len() as isize - (range.end - range.start) as isize;
        statements.splice(range.start..range.end, replacement);
        let ranges = &mut state.plugin_file.root_ranges;
        ranges[i].end = (range.end as isize + delta) as usize;
        ranges[i].diagnostics_end = (range.diagnostics_end as isize + diagnostics_delta) as usize;
        for later in &mut ranges[i + 1..] {
            later.start = (later.start as isize + delta) as usize;
            later.end = (later.end as isize + delta) as usize;
            later.diagnostics_start = (later.diagnostics_start as isize + diagnostics_delta) as usize;
            later.diagnostics_end = (later.diagnostics_end as isize + diagnostics_delta) as usize;
        }
    }
}

/// Plugin imports go first, after any `--!` directives and the file's header comment (as Flamework's added imports
/// did ahead of the user's), ordered as Flamework's TS transformer adds them ([`plugin_import_statements`]);
/// header statements after the imports: after the last import of the file's leading run of imports.
fn place_imports_and_header(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    let imports = plugin_import_statements(state, statements);
    let header = std::mem::take(&mut state.plugin_file.header);
    if imports.is_empty() && header.is_empty() {
        return;
    }
    let directives =
        statements.iter().take_while(|s| matches!(s, Stmt::Comment(text) if text.starts_with('!'))).count();
    let mut after_imports = 0;
    for range in &state.plugin_file.root_ranges {
        match range.node.kind() {
            Kind::ImportDeclaration | Kind::ImportEqualsDeclaration => after_imports = range.end,
            _ if range.start == range.end => {}
            _ => break,
        }
    }
    // after the file's header comment, which TypeScript's printer keeps above every statement
    let imports_at = directives.max(state.plugin_file.detached_comments);
    let header_at = after_imports.max(imports_at) + imports.len();
    state.plugin_file.front_inserts.push((imports_at, imports.len()));
    state.plugin_file.front_inserts.push((header_at, header.len()));
    statements.splice(imports_at..imports_at, imports);
    statements.splice(header_at..header_at, header);
}

/// The plugin imports' statements, as roblox-ts compiles the import declarations Flamework's TS transformer adds
/// (`fileImports`): one declaration per path, the paths in the order their first import was asked for, each listing
/// its imports in the order they were asked for ([`PluginImport::order`]); a declaration of several imports binds
/// the module to a local first (`local _core = TS.import(…)`, `local a = _core.a`), as roblox-ts does.
///
/// TypeScript names the imports' unique identifiers (`t`, `t_1`, …) as it prints them, so in that order; they were
/// named when asked for, so names of one export are handed back out in print order, the file renamed to match.
fn plugin_import_statements(state: &mut TransformState<'_>, statements: &mut [Stmt]) -> Vec<Stmt> {
    use crate::classes::prereqs::{as_any_identifier, temp_id_expr};
    use crate::util::clean_module_name::clean_module_name;
    use super::luau_walk::{Visit, walk_statements};

    let imports = &mut state.plugin_file.imports;
    let mut sorted: Vec<usize> = (0..imports.len()).collect();
    sorted.sort_by_key(|&i| imports[i].order);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in sorted {
        match groups.iter_mut().find(|group| imports[group[0]].specifier == imports[i].specifier) {
            Some(group) => group.push(i),
            None => groups.push(vec![i]),
        }
    }

    // the names of one export in request order, handed out in print order
    let plain_name = |import: &PluginImport| match &import.local {
        AnyIdentifier::Identifier(id) => Some(id.name.clone()),
        AnyIdentifier::Temporary(_) => None,
    };
    let printed: Vec<usize> = groups.iter().flatten().copied().collect();
    let mut renames: FxHashMap<String, String> = FxHashMap::default();
    let mut handed: FxHashMap<&str, usize> = FxHashMap::default();
    for &i in &printed {
        let Some(old) = plain_name(&imports[i]) else { continue };
        let export = imports[i].export.as_str();
        let k = handed.entry(export).or_default();
        let new = imports.iter().filter(|import| import.export == export).filter_map(plain_name).nth(*k);
        *k += 1;
        if let Some(new) = new
            && new != old
        {
            renames.insert(old, new);
        }
    }
    if !renames.is_empty() {
        let mut rename = |visit: Visit<'_>| {
            if let Visit::Expr(Expr::Identifier(id)) = visit
                && let Some(new) = renames.get(&id.name)
            {
                id.name.clone_from(new);
            }
        };
        walk_statements(statements, &mut rename);
        walk_statements(&mut state.plugin_file.header, &mut rename);
    }

    let mut out = Vec::new();
    for group in groups {
        let imports = &mut state.plugin_file.imports;
        let import = imports[group[0]].import.take().expect("an import is placed once");
        let module = if group.len() > 1 {
            let specifier = &imports[group[0]].specifier;
            let module_name = specifier.rsplit('/').next().unwrap_or(specifier);
            let id = temp_id_expr(clean_module_name(module_name));
            out.push(create::local(as_any_identifier(id.clone()), Some(import)));
            id
        } else {
            import
        };
        for i in group {
            let import = &mut imports[i];
            import.import = None;
            let local = match &import.local {
                AnyIdentifier::Identifier(id) => match renames.get(&id.name) {
                    Some(new) => create::any_id(new.clone()),
                    None => import.local.clone(),
                },
                AnyIdentifier::Temporary(_) => import.local.clone(),
            };
            out.push(create::local(local, Some(create::computed_index(module.clone(), create::string(&import.export)))));
        }
    }
    out
}

/// Whether the local name of an import alias could be shadowed where a plugin uses it: some identifier of the file
/// with that text resolves to anything but the alias (a parameter, a local, a property), so the name does not mean
/// the import everywhere. Conservative: a reused import must mean the import at every use.
fn alias_shadowed(state: &mut TransformState<'_>, alias: P<Node>) -> bool {
    let Some(alias_symbol) = alias.symbol() else { return true };
    let Some(name) = alias_name(alias) else { return true };
    fn walk(node: P<Node>, name: &str, out: &mut Vec<P<Node>>) {
        if node.kind() == Kind::Identifier && node.text() == name {
            out.push(node);
        }
        node.for_each_child(&mut |child| {
            walk(child, name, out);
            false
        });
    }
    let mut identifiers = Vec::new();
    walk(state.source_file.as_node(), name, &mut identifiers);
    identifiers.into_iter().any(|id| state.checker.get_symbol_at_location_exported(id) != Some(alias_symbol))
}

/// A user import of `export` from `module_file` in this file: the alias declaration and the value to use.
fn find_user_import(
    state: &mut TransformState<'_>,
    module_file: P<SourceFile>,
    export: &str,
) -> Option<(P<Node>, Expr)> {
    for &statement in state.source_file.get().statements.nodes() {
        if statement.kind() != Kind::ImportDeclaration {
            continue;
        }
        let declaration = statement.as_import_declaration();
        let Some(clause_node) = declaration.import_clause else { continue };
        let clause = clause_node.as_import_clause();
        if clause.phase_modifier.get() == Kind::TypeKeyword {
            continue;
        }
        if get_source_file_from_module_specifier(state, declaration.module_specifier) != Some(module_file) {
            continue;
        }
        if export == "default"
            && let Some(name) = clause.name
        {
            return Some((clause_node, create::id(name.text())));
        }
        let Some(bindings) = clause.named_bindings else { continue };
        if bindings.kind() == Kind::NamespaceImport {
            let name = bindings.as_namespace_import().name.text();
            return Some((bindings, create::property(create::id(name), export)));
        }
        for &element in bindings.elements() {
            if element.is_type_only() {
                continue;
            }
            let name = element.name().expect("an import specifier has a name");
            let exported = element.property_name().unwrap_or(name);
            if exported.text() == export {
                return Some((element, create::id(name.text())));
            }
        }
    }
    None
}

/// `import_named`: the value of `export` from `specifier`, imported once per file. A user import of the same
/// export is reused (and kept alive); otherwise a `local <export> = TS.import(…).<export>` goes with the plugin
/// imports at the top of the file, named after the export unless a name in the file could shadow it.
pub(crate) fn import_named(
    state: &mut TransformState<'_>,
    host: &PluginHost,
    anchor: P<Node>,
    specifier: &str,
    export: &str,
) -> Result<Expr, String> {
    let key = (specifier.to_owned(), export.to_owned());
    if let Some(value) = state.plugin_file.import_cache.get(&key) {
        return Ok(value.clone());
    }
    let file = state.source_file;
    let resolved = host
        .resolve_module(specifier, file.get().file_name())
        .ok_or_else(|| format!("import_named: `{specifier}` cannot be resolved from {}", file.get().file_name()))?;
    let module_file = state.program.get_source_file(&resolved.file_name).ok_or_else(|| {
        format!("import_named: `{specifier}` resolves to {}, which is not in the program", resolved.file_name)
    })?;

    let reusable = find_user_import(state, module_file, export).filter(|&(alias, _)| !alias_shadowed(state, alias));
    let value = if let Some((alias, value)) = reusable {
        state.plugin_file.forced_imports.insert(alias);
        value
    } else {
        // TypeScript's optimistic unique name for the TS transformer's import: `export`, else `export_1`, …
        let id: AnyIdentifier = if rbxts_luau::is_valid_identifier(export) {
            let name = state.plugin_file.unique_plain_name(file, export);
            state.plugin_file.import_names.insert(name.clone());
            create::any_id(name)
        } else {
            temp_id(state.plugin_file.seam_hint(file, export)).into()
        };
        state.plugin_file.import_names.insert(export.to_owned());
        let import = create_import_expression_of_file(state, file, module_file, anchor);
        // the import's local, and the module's when a second import from its path shares the declaration
        let same_path = state.plugin_file.imports.iter().filter(|i| i.specifier == specifier).count();
        state.plugin_file.main_chunk_locals += if same_path == 1 { 2 } else { 1 };
        let value: Expr = id.clone().into();
        let position = if anchor.kind() == Kind::SourceFile { 0 } else { crate::util::traversal::get_start(anchor) };
        let order = (position, state.plugin_file.imports.len());
        state.plugin_file.imports.push(PluginImport {
            specifier: specifier.to_owned(),
            export: export.to_owned(),
            local: id,
            import: Some(import),
            order,
        });
        value
    };
    state.plugin_file.import_cache.insert(key, value.clone());
    Ok(value)
}
