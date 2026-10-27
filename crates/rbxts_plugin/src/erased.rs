//! Type-erased plugins, for the compiler's plugin list (DESIGN §11.6).
//!
//! `rbxts_cli::Cli::plugin<P: Plugin>` erases a plugin once, at registration, with [`erase`]. The compiler then
//! holds `Box<dyn DynPlugin>` and per-checker `Box<dyn DynInstance>`, with `Shared` as [`ErasedShared`], the
//! persistent state as [`ErasedState`] and file parts as [`ErasedPart`]. Each hook costs one virtual call; the
//! downcasts happen once per `instance`, `setup` and `finish`, never per node.
//!
//! Handing a plugin another plugin's shared state, state or part is a compiler bug and panics.
//!
//! **Limitation.** The check is a downcast, so it compares types (`TypeId`), not plugins: two plugins whose
//! `Shared`, `State` or `FilePart` is the same type (say `()`) cannot be told apart, and a mix-up between them
//! goes unnoticed. The compiler keeps each plugin's values in that plugin's own slot, so this is a check on the
//! compiler, not a guarantee.

use std::any::Any;
use std::sync::Arc;

use tsrs_ast::{Diagnostic, Node, Symbol};
use tsrs_core::P;

use crate::{
    AccessSite, AssignSite, CallSite, ChangeSet, CheckerInfo, Claim, ClassSite, Cx, Fatal,
    FileIndex, FinishCx, Interest, LuauExpr, LuauStmt, PResult, Plugin, PluginDiagnostic,
    PluginInfo, PluginInstance, RebuildPlan, SetupCx, SymbolRole, Tag,
};

/// A plugin's `Shared`, erased.
pub type ErasedShared = Arc<dyn Any + Send + Sync>;
/// A plugin's `FilePart`, erased.
pub type ErasedPart = Box<dyn Any + Send>;

trait StateObject: Send {
    fn clone_box(&self) -> Box<dyn StateObject>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Clone + Send + 'static> StateObject for T {
    fn clone_box(&self) -> Box<dyn StateObject> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A plugin's persistent `State`, erased. `Clone` makes the working copy of a build, which the compiler commits
/// (replacing the committed one) only when the build commits.
pub struct ErasedState(Box<dyn StateObject>);

impl Clone for ErasedState {
    fn clone(&self) -> ErasedState {
        ErasedState(self.0.clone_box())
    }
}

impl ErasedState {
    /// The state as its concrete type, if it is a `T`.
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref()
    }

    /// The state as its concrete type, if it is a `T`.
    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.0.as_any_mut().downcast_mut()
    }
}

/// [`Plugin`] with its associated types erased. See [`Plugin`] for each method's contract.
pub trait DynPlugin: Send + Sync {
    /// [`Plugin::info`].
    fn info(&self) -> PluginInfo;
    /// [`Plugin::diagnostic_source`].
    fn diagnostic_source(&self) -> Option<&'static str>;
    /// `State::default()`, for a clean build.
    fn default_state(&self) -> ErasedState;
    /// [`Plugin::setup`].
    fn setup(&self, cx: &mut SetupCx<'_>, state: &mut ErasedState) -> Result<ErasedShared, Fatal>;
    /// [`Plugin::rebuild_start`].
    fn rebuild_start(
        &self,
        shared: &ErasedShared,
        state: &ErasedState,
        changes: &ChangeSet,
    ) -> RebuildPlan;
    /// [`Plugin::instance`].
    fn instance(&self, shared: &ErasedShared, checker: CheckerInfo) -> Box<dyn DynInstance>;
    /// [`Plugin::finish`]; `parts` sorted by [`FileIndex`], each made by this plugin's instances.
    fn finish(
        &self,
        shared: &ErasedShared,
        state: &mut ErasedState,
        parts: &[(FileIndex, &ErasedPart)],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal>;
    /// [`Plugin::encode_file_part`]; `part` made by this plugin's instances.
    fn encode_file_part(&self, part: &ErasedPart) -> Option<Vec<u8>>;
    /// [`Plugin::decode_file_part`], with the part boxed.
    fn decode_file_part(&self, bytes: &[u8]) -> Option<ErasedPart>;
}

/// [`PluginInstance`] with its file part erased. See [`PluginInstance`] for each method's contract.
pub trait DynInstance {
    /// [`PluginInstance::interest`].
    fn interest(&self, interest: &mut Interest);
    /// [`PluginInstance::classify_symbol`].
    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag>;
    /// [`PluginInstance::begin_file`].
    fn begin_file(&self, cx: &mut Cx<'_>) -> PResult<()>;
    /// [`PluginInstance::call`].
    fn call(&self, cx: &mut Cx<'_>, call: &CallSite, tag: Tag) -> PResult<Claim<LuauExpr>>;
    /// [`PluginInstance::property_key`].
    fn property_key(
        &self,
        cx: &mut Cx<'_>,
        access: &AccessSite,
        tag: Tag,
    ) -> PResult<Claim<String>>;
    /// [`PluginInstance::assignment`].
    fn assignment(&self, cx: &mut Cx<'_>, site: &AssignSite, tag: Tag) -> PResult<Claim<LuauExpr>>;
    /// [`PluginInstance::class`].
    fn class(&self, cx: &mut Cx<'_>, class: &ClassSite) -> PResult<Vec<LuauStmt>>;
    /// [`PluginInstance::expression`].
    fn expression(&self, cx: &mut Cx<'_>, node: P<Node>, tag: Tag) -> PResult<Claim<LuauExpr>>;
    /// [`PluginInstance::statement`].
    fn statement(&self, cx: &mut Cx<'_>, node: P<Node>, tag: Tag) -> PResult<Claim<Vec<LuauStmt>>>;
    /// [`PluginInstance::end_file`], with the part boxed.
    fn end_file(&self, cx: &mut Cx<'_>) -> PResult<ErasedPart>;
    /// [`PluginInstance::explain_checker_diagnostic`].
    fn explain_checker_diagnostic(
        &self,
        cx: &mut Cx<'_>,
        diagnostic: P<Diagnostic>,
    ) -> Option<PluginDiagnostic>;
}

/// Erases a plugin for the compiler's plugin list.
pub fn erase<T: Plugin>(plugin: T) -> Box<dyn DynPlugin> {
    Box::new(ErasedPlugin(plugin))
}

struct ErasedPlugin<T>(T);

struct ErasedInstance<I>(I);

fn shared_of<T: Plugin>(plugin: &T, shared: &ErasedShared) -> Arc<T::Shared> {
    Arc::clone(shared)
        .downcast::<T::Shared>()
        .unwrap_or_else(|_| mismatch(plugin, "shared state"))
}

fn mismatch<T: Plugin>(plugin: &T, what: &str) -> ! {
    panic!(
        "compiler bug: plugin `{}` was handed another plugin's {what}",
        plugin.info().name
    )
}

impl<T: Plugin> DynPlugin for ErasedPlugin<T> {
    fn info(&self) -> PluginInfo {
        self.0.info()
    }

    fn diagnostic_source(&self) -> Option<&'static str> {
        self.0.diagnostic_source()
    }

    fn default_state(&self) -> ErasedState {
        ErasedState(Box::new(T::State::default()))
    }

    fn setup(&self, cx: &mut SetupCx<'_>, state: &mut ErasedState) -> Result<ErasedShared, Fatal> {
        let state = state
            .downcast_mut::<T::State>()
            .unwrap_or_else(|| mismatch(&self.0, "state"));
        let shared: ErasedShared = self.0.setup(cx, state)?;
        Ok(shared)
    }

    fn rebuild_start(
        &self,
        shared: &ErasedShared,
        state: &ErasedState,
        changes: &ChangeSet,
    ) -> RebuildPlan {
        let shared = shared_of(&self.0, shared);
        let state = state
            .downcast_ref::<T::State>()
            .unwrap_or_else(|| mismatch(&self.0, "state"));
        self.0.rebuild_start(&shared, state, changes)
    }

    fn instance(&self, shared: &ErasedShared, checker: CheckerInfo) -> Box<dyn DynInstance> {
        Box::new(ErasedInstance(
            self.0.instance(shared_of(&self.0, shared), checker),
        ))
    }

    fn finish(
        &self,
        shared: &ErasedShared,
        state: &mut ErasedState,
        parts: &[(FileIndex, &ErasedPart)],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        let shared = shared_of(&self.0, shared);
        let state = state
            .downcast_mut::<T::State>()
            .unwrap_or_else(|| mismatch(&self.0, "state"));
        let parts: Vec<(FileIndex, &T::FilePart)> = parts
            .iter()
            .map(|&(index, part)| {
                (
                    index,
                    part.downcast_ref::<T::FilePart>()
                        .unwrap_or_else(|| mismatch(&self.0, "file part")),
                )
            })
            .collect();
        self.0.finish(&shared, state, &parts, cx)
    }

    fn encode_file_part(&self, part: &ErasedPart) -> Option<Vec<u8>> {
        let part = part
            .downcast_ref::<T::FilePart>()
            .unwrap_or_else(|| mismatch(&self.0, "file part"));
        self.0.encode_file_part(part)
    }

    fn decode_file_part(&self, bytes: &[u8]) -> Option<ErasedPart> {
        let part: ErasedPart = Box::new(self.0.decode_file_part(bytes)?);
        Some(part)
    }
}

impl<I: PluginInstance> DynInstance for ErasedInstance<I> {
    fn interest(&self, interest: &mut Interest) {
        self.0.interest(interest);
    }

    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag> {
        self.0.classify_symbol(cx, symbol, role)
    }

    fn begin_file(&self, cx: &mut Cx<'_>) -> PResult<()> {
        self.0.begin_file(cx)
    }

    fn call(&self, cx: &mut Cx<'_>, call: &CallSite, tag: Tag) -> PResult<Claim<LuauExpr>> {
        self.0.call(cx, call, tag)
    }

    fn property_key(
        &self,
        cx: &mut Cx<'_>,
        access: &AccessSite,
        tag: Tag,
    ) -> PResult<Claim<String>> {
        self.0.property_key(cx, access, tag)
    }

    fn assignment(&self, cx: &mut Cx<'_>, site: &AssignSite, tag: Tag) -> PResult<Claim<LuauExpr>> {
        self.0.assignment(cx, site, tag)
    }

    fn class(&self, cx: &mut Cx<'_>, class: &ClassSite) -> PResult<Vec<LuauStmt>> {
        self.0.class(cx, class)
    }

    fn expression(&self, cx: &mut Cx<'_>, node: P<Node>, tag: Tag) -> PResult<Claim<LuauExpr>> {
        self.0.expression(cx, node, tag)
    }

    fn statement(&self, cx: &mut Cx<'_>, node: P<Node>, tag: Tag) -> PResult<Claim<Vec<LuauStmt>>> {
        self.0.statement(cx, node, tag)
    }

    fn end_file(&self, cx: &mut Cx<'_>) -> PResult<ErasedPart> {
        Ok(Box::new(self.0.end_file(cx)?))
    }

    fn explain_checker_diagnostic(
        &self,
        cx: &mut Cx<'_>,
        diagnostic: P<Diagnostic>,
    ) -> Option<PluginDiagnostic> {
        self.0.explain_checker_diagnostic(cx, diagnostic)
    }
}
