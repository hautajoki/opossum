//! The two traits a plugin implements (DESIGN §11.1, §11.2, §14).

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;

use tsrs_ast::{Diagnostic, Node, Symbol};
use tsrs_core::P;

use crate::{
    AccessSite, AssignSite, CallSite, ChangeSet, CheckerInfo, Claim, ClassSite, Cx, Fatal,
    FileIndex, FinishCx, Interest, LuauExpr, LuauStmt, PResult, PluginDiagnostic, PluginInfo,
    RebuildPlan, SetupCx, SymbolRole, Tag,
};

/// The factory: one per build tool, shared by every thread. It holds options set in the user's `main`, never
/// mutable build state: that is [`Plugin::Shared`] (read-only, per build), [`Plugin::State`] (persistent, owned
/// by the compiler) and [`Plugin::FilePart`] (per file).
///
/// **Lifecycle** (DESIGN §14). Per build: [`Plugin::rebuild_start`] (watch rebuilds that kept a `Shared`) or
/// [`Plugin::setup`] → [`Plugin::instance`] per checker → the instances' hooks → [`Plugin::finish`]. Then the
/// compiler renders and, only when the build has no errors, commits outputs, artifacts and state in one write
/// phase. A plugin never writes files itself.
///
/// **`Shared` and `State` travel together.** The compiler keeps a `Shared` only together with the state it was
/// made from. A `Shared` made by a build that does not commit (errors or `Fatal`) is dropped, and the next build
/// runs `setup` again on the committed state. (A `Shared` kept from an earlier committed build stays kept when a
/// later build fails: it still matches the committed state.)
///
/// **No program handles between builds (the compiler's contract).** `Shared`, `State` and `FilePart` outlive the
/// program they were made with: a watch session frees each build's program once the next build has replaced it,
/// with its checkers and everything allocated while one was held (the types, symbols, signatures and diagnostics
/// they made, and the nodes and diagnostics a transform made). So none of them may hold a program handle: no
/// `P<Node>`, `P<Symbol>`, `P<Type>`, `P<Signature>`, `P<Diagnostic>`, `P<SourceFile>`, `&'static Program` or
/// `&'static str` borrowed from source text or a checker; and no [`FileIndex`], which is renumbered every build.
/// Keep owned data: strings, numbers, paths (a file is keyed by `cx.file().path`). Handles belong in the
/// [`PluginInstance`], which lives for one build. The types enforce most of it: the three types must be
/// `UnwindSafe + RefUnwindSafe` (the compiler catches a panic in a hook and goes on with them), which the handles to
/// tsrs's objects are not, as those objects have interior mutability: a `P<…>` field, or a collection of them, does
/// not compile. Borrowed source text (`&'static str`) passes the bound and is the plugin's to avoid. Wrapping a value
/// in [`std::panic::AssertUnwindSafe`] silences the bound: only for a value that holds no handle (an `Arc<dyn Fn>`).
///
/// ```compile_fail
/// use std::sync::Arc;
/// use rbxts_plugin::ts::{Node, P};
/// use rbxts_plugin::*;
///
/// struct Keeper;
/// struct KeeperInstance;
/// // a node kept in a file part: it would outlive its program
/// struct KeptPart(P<Node>);
///
/// impl Plugin for Keeper {
///     type Shared = ();
///     type State = ();
///     type FilePart = KeptPart;
///     type Instance = KeeperInstance;
///     fn info(&self) -> PluginInfo {
///         PluginInfo { name: "keeper", version: "0", tsconfig_transform: None }
///     }
///     fn setup(&self, _: &mut SetupCx<'_>, _: &mut ()) -> Result<Arc<()>, Fatal> {
///         Ok(Arc::new(()))
///     }
///     fn instance(&self, _: Arc<()>, _: CheckerInfo) -> KeeperInstance {
///         KeeperInstance
///     }
///     fn finish(&self, _: &(), _: &mut (), _: &[(FileIndex, &KeptPart)], _: &mut FinishCx<'_>) -> Result<(), Fatal> {
///         Ok(())
///     }
/// }
///
/// impl PluginInstance for KeeperInstance {
///     type FilePart = KeptPart;
///     fn interest(&self, _: &mut Interest) {}
///     fn end_file(&self, _: &mut Cx<'_>) -> PResult<KeptPart> {
///         unimplemented!()
///     }
/// }
/// ```
pub trait Plugin: Send + Sync + 'static {
    /// Read-only state for the transform: config, env, ids decided up front. Made by `setup`; kept across watch
    /// rebuilds until a build input changes or `rebuild_start` asks for a new one, and only together with the
    /// committed state it was made from: a `Shared` whose build does not commit is dropped (see above).
    ///
    /// It never depends on the contents of the plugin's own state file ([`FinishCx::write_state_file`]): what the
    /// plugin persists belongs in [`Plugin::State`], reloaded with [`SetupCx::read_persisted`] when `setup` starts
    /// from [`crate::StateOrigin::Fresh`]. `setup` may read that file through [`SetupCx::read_input`] (Flamework's
    /// candidate scan stops at its own `flamework.build`), but then only whether it exists may matter.
    ///
    /// A kept `Shared` outlives the program `setup` saw, so it holds **no handles** either: see "No program handles
    /// between builds" above.
    type Shared: Send + Sync + UnwindSafe + RefUnwindSafe + 'static;
    /// Persistent state across builds (Flamework: the id table, salt, seed, mode), owned by the compiler.
    ///
    /// The compiler hands a working copy of the last committed state to `setup` and `finish` (a clone, so a
    /// failing build leaves the committed state untouched), and commits the working copy with the build's write
    /// phase. A clean build starts from `Default`. To survive across processes, the plugin persists it itself with
    /// [`FinishCx::write_state_file`] and reloads it in `setup` ([`SetupCx::read_persisted`]) when the state is
    /// [`crate::StateOrigin::Fresh`]. In watch mode the committed state is handed on in memory and never re-read
    /// from disk between rebuilds (DESIGN §14).
    ///
    /// It outlives the program it was made with, so it holds **no handles** (`P<…>`, `&'static` program or
    /// source text) **and no [`FileIndex`]**: key per-file data by the file's path (`cx.file().path`). See "No
    /// program handles between builds" above.
    type State: Clone + Default + Send + UnwindSafe + RefUnwindSafe + 'static;
    /// What one file contributes to cross-file results. The compiler keeps each file's part from its last
    /// successful transform (by normalised path) and hands the parts of all current files to `finish`.
    ///
    /// Cached parts outlive the program that made them (watch rebuilds free replaced programs, DESIGN §14), so a
    /// part holds **no handles** (`P<Node>`, `P<Symbol>`, `P<Type>`, `&'static` program or source text) **and no
    /// [`FileIndex`]**, which is renumbered every build. Owned strings, numbers and paths only. See "No program
    /// handles between builds" above.
    type FilePart: Send + UnwindSafe + RefUnwindSafe + 'static;
    /// The per-checker instance.
    type Instance: PluginInstance<FilePart = Self::FilePart>;

    /// Name, version and activation. Called at registration and when reporting; must be cheap and constant.
    fn info(&self) -> PluginInfo;

    /// The source this plugin's diagnostics name, unless a diagnostic names its own
    /// ([`crate::PluginDiagnostic::source`]). Flamework's is `@flamework-experimental/transformer`, as the TS
    /// transformer names its own. Must be cheap and constant.
    ///
    /// **Rendering (the compiler's contract).** A diagnostic from a hook (`Cx::report`, `error`, `warning`) or a
    /// replacement from [`PluginInstance::explain_checker_diagnostic`] whose source is `Some(source)` is rendered with
    /// the source in TypeScript's code slot, as the diagnostic code `" " + source` (a leading space), exactly as
    /// roblox-ts renders its own as `" roblox-ts"`: the pretty reporter then prints
    /// `error TS @flamework-experimental/transformer: message`, and a plain one the same after the location. With
    /// `None`, the output is what it is today. The source is text only: it does not change the category, the
    /// location or whether the build fails. `SetupCx::warn`, `FinishCx::warn_at` and `info` are not diagnostics and
    /// do not take it.
    ///
    /// Default: `None`.
    fn diagnostic_source(&self) -> Option<&'static str> {
        None
    }

    /// Builds the build's `Shared` state. Runs once per build that has no `Shared` to keep, single-threaded,
    /// after the check and before any transform. Read files and env only through `cx` (they become build inputs,
    /// watched and hashed). `state` is the working copy of the persistent state (see [`SetupCx::state_origin`]);
    /// setup may fill it (Flamework creates salt and seed here, in a fixed order).
    fn setup(
        &self,
        cx: &mut SetupCx<'_>,
        state: &mut Self::State,
    ) -> Result<Arc<Self::Shared>, Fatal>;

    /// Called at the start of a watch rebuild, and only when a kept `Shared` exists: the last build committed,
    /// and no build input this plugin read changed (the compiler re-runs `setup` on its own when one did). Never
    /// called for [`crate::BuildKind::Clean`] or [`crate::BuildKind::Incremental`], which always run `setup`.
    /// The plugin may still ask for a new `Shared` ([`RebuildPlan::RerunSetup`], which re-transforms every file).
    /// Default: keep it.
    fn rebuild_start(
        &self,
        shared: &Self::Shared,
        state: &Self::State,
        changes: &ChangeSet,
    ) -> RebuildPlan {
        let _ = (shared, state, changes);
        RebuildPlan::KeepShared
    }

    /// Makes the instance of one checker. Called on that checker's thread, once per checker group and build
    /// (instances never outlive a build, so their caches can key on nodes).
    fn instance(&self, shared: Arc<Self::Shared>, checker: CheckerInfo) -> Self::Instance;

    /// Merges the file parts of every current source file, in transform order (`parts` is sorted by this build's
    /// [`FileIndex`]; files that were not transformed this build bring their cached part). Resolves the late
    /// strings of every current file, cached files included ([`FinishCx::resolve_late`]), and queues artifacts.
    /// Runs once per build, after every checker group, on one thread. Not called when the transform reported an
    /// error.
    fn finish(
        &self,
        shared: &Self::Shared,
        state: &mut Self::State,
        parts: &[(FileIndex, &Self::FilePart)],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal>;

    /// The bytes of a file part, for the compiler's incremental state on disk, or `None` when this part (or this
    /// plugin's parts in general) cannot be persisted.
    ///
    /// **Contract.** [`Plugin::decode_file_part`] of the bytes gives a part that `finish` treats exactly as the
    /// original: same artifacts, late values and warnings. The format is the plugin's, and must be stable for a
    /// given [`PluginInfo::version`] at least: carry a format version and reject other versions in `decode`. The
    /// compiler stores the bytes with the plugin's name and version, and starts a clean build itself when either
    /// changes, so a plugin never sees another version's bytes in a normal build; the format version guards the
    /// rest (a hand-copied state, a version not bumped).
    ///
    /// **What the compiler does with it.** When the incremental state is written (commit step 4), each current
    /// file's part is encoded; a file whose part gives `None` is stored without one. In a build in a new process,
    /// a file that is not transformed brings its decoded part to `finish`; a file without stored bytes, or whose
    /// bytes `decode` rejects, is transformed again instead.
    ///
    /// Default: `None`, not persistable (every cached file is transformed again in a new process).
    fn encode_file_part(&self, part: &Self::FilePart) -> Option<Vec<u8>> {
        let _ = part;
        None
    }

    /// The part [`Plugin::encode_file_part`] encoded, or `None` when the bytes are not a part this plugin version
    /// can read (another format version, truncated or extra bytes). Never panics on any input: a `None` makes the
    /// compiler transform the file again.
    ///
    /// Default: `None`.
    fn decode_file_part(&self, bytes: &[u8]) -> Option<Self::FilePart> {
        let _ = bytes;
        None
    }
}

/// The per-checker part of a plugin. It holds this checker's types and symbols (caches in `RefCell` / `Cell`), so
/// it is not `Send`: it is made on its checker's thread and stays there.
///
/// **Re-entrancy.** Hooks take `&self`, because `cx.compile_*` dispatches nested nodes, which can reach the same
/// instance again. Never hold a `RefCell` borrow across a `cx.compile_*` call.
///
/// **Ordering** (DESIGN §11.5). Claim hooks (`call`, `property_key`, `assignment`, `expression`, `statement`)
/// ask matching plugins in registration order; the first `Take` wins, and when all decline roblox-ts lowers the
/// node, its own macros included. Broadcast hooks (`begin_file`, `class`, `end_file`) run for every active plugin
/// in registration order.
///
/// **Errors.** `Err(Abort)`: the compiler emits `nil` for the expression or drops the statement and goes on. A
/// hook that returns `Err` without having recorded a new error diagnostic during that call is a plugin bug: the
/// compiler reports an internal error naming the plugin and the file. A panic is caught per file and reported the
/// same way.
///
/// **Phases.** What each hook may call on `Cx` is listed on [`crate::CxBackend`]: the lowering primitives
/// (`compile_*`, prereqs, capture, hoists, root insertions) only in claim hooks and `class`.
pub trait PluginInstance: 'static {
    /// The plugin's [`Plugin::FilePart`].
    type FilePart: Send + UnwindSafe + RefUnwindSafe + 'static;

    /// Registers the nodes this instance wants. Called once, right after the instance is made.
    fn interest(&self, interest: &mut Interest);

    /// Classifies a symbol for the symbol-keyed interests. The compiler memoises the answer per checker and per
    /// `(symbol, role)`: it is asked at most once for each, from inside the first hook dispatch that needs it
    /// (`cx.file()` is that site's file; use `cx` for symbol facts only).
    ///
    /// **No diagnostics here.** How often, and on which file, this runs depends on the checker count, so a
    /// warning or error from it would make the diagnostics depend on it too. The compiler reports any diagnostic
    /// from `classify_symbol` (and any refused primitive) as an internal error, anchored at the symbol's declaration
    /// rather than at the site that first asked, and reported by the build once, apart from any file's list: the same
    /// for any checker count. Return a tag for the odd case and report it from the hook, per site. The lowering,
    /// file-level and build-input primitives are refused as well (the phase table on [`crate::CxBackend`]).
    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag> {
        let _ = (cx, symbol, role);
        None
    }

    /// Starts a file, before any other hook of the file. Reset per-file caches here. The lowering primitives
    /// are refused; `header`, `import_named` and `late_string` work.
    fn begin_file(&self, cx: &mut Cx<'_>) -> PResult<()> {
        let _ = cx;
        Ok(())
    }

    /// A call, `new` or decorator call that matched a call interest with `tag`. Return the call's Luau value; for
    /// a decorator it replaces the decorator expression, which is then applied (`Class = value(Class) or Class`).
    /// Decide whether to take the call before compiling anything: see "Declining" in the crate docs.
    fn call(&self, cx: &mut Cx<'_>, call: &CallSite, tag: Tag) -> PResult<Claim<LuauExpr>> {
        let _ = (cx, call, tag);
        Ok(Claim::Decline)
    }

    /// A property access that matched an access interest. Return the key to use instead of the written one, as
    /// plain text (not escaped). roblox-ts lowers the access with it in every position (read, write, compound,
    /// `++`, chains): `obj.key` when the key is a valid Luau identifier, else
    /// `obj[text_literal(key)]` ([`crate::text_literal`]). As a method callee, a valid identifier gives
    /// `obj:key(...)`, anything else `obj[text_literal(key)](obj, ...)`.
    ///
    /// **Computed keys.** Element accesses are offered whatever their key. An answer equal to `access.key` leaves
    /// the access as written (`obj[k]` stays `obj[k]`). When a rewritten key replaces a computed argument
    /// (`obj[k]` with `k: "a"`), an argument that only reads (an identifier, `this`, a literal, an enum member
    /// such as `E.A`) is dropped with it; any other is still evaluated once for its effects, after the receiver, and
    /// its value dropped. A site whose `key` is `None` ([`AccessSite::key`]) can only be declined or failed (`Err`, after
    /// reporting why, as Flamework's "must be accessed directly"): a `Take` there is an internal error naming the
    /// plugin, and the access is lowered as written.
    fn property_key(
        &self,
        cx: &mut Cx<'_>,
        access: &AccessSite,
        tag: Tag,
    ) -> PResult<Claim<String>> {
        let _ = (cx, access, tag);
        Ok(Claim::Decline)
    }

    /// An assignment, update or `delete` whose target's property symbol was classified with `tag`. Return the
    /// whole expression's Luau value (when `site.value_used`) or an expression to run as a statement.
    ///
    /// Targets with a computed key are offered too, with `site.key` `None` ([`AssignSite::key`]): unlike a
    /// `property_key` answer, a claimed assignment can use a key it does not know, by compiling `site.key_node`
    /// (once: `compile_binary(op, site.target, value)` reads the target, its key included).
    fn assignment(&self, cx: &mut Cx<'_>, site: &AssignSite, tag: Tag) -> PResult<Claim<LuauExpr>> {
        let _ = (cx, site, tag);
        Ok(Claim::Decline)
    }

    /// A class the filter admits. The statements go where roblox-ts emits static blocks: after the class is
    /// created, before its decorators are applied. Plugins' statements are concatenated in registration order.
    fn class(&self, cx: &mut Cx<'_>, class: &ClassSite) -> PResult<Vec<LuauStmt>> {
        let _ = (cx, class);
        Ok(Vec::new())
    }

    /// Fallback by syntax kind ([`Interest::expressions`]): an expression of a registered kind, before roblox-ts lowers
    /// it. Return its Luau value.
    fn expression(&self, cx: &mut Cx<'_>, node: P<Node>, tag: Tag) -> PResult<Claim<LuauExpr>> {
        let _ = (cx, node, tag);
        Ok(Claim::Decline)
    }

    /// Fallback by syntax kind ([`Interest::statements`]): a statement of a registered kind, before roblox-ts lowers
    /// it. Return the statements that replace it (for a block body, the body; for an `else if`, the `else` block).
    /// A statement hook cannot compile its own statement: lower its parts.
    fn statement(&self, cx: &mut Cx<'_>, node: P<Node>, tag: Tag) -> PResult<Claim<Vec<LuauStmt>>> {
        let _ = (cx, node, tag);
        Ok(Claim::Decline)
    }

    /// Ends a file, after every other hook of the file, and returns its part. The lowering primitives are
    /// refused; `header`, `import_named` and `late_string` work.
    fn end_file(&self, cx: &mut Cx<'_>) -> PResult<Self::FilePart>;

    /// Only for files that failed checking (and so are not transformed): may replace a checker diagnostic with a
    /// clearer one. The program, file, checker and source facts, `report` (and `Cx::error` / `warning`),
    /// `is_usable_local_name` and `locals` are available here; the lowering and file-level primitives are refused
    /// (the phase table on [`crate::CxBackend`]).
    fn explain_checker_diagnostic(
        &self,
        cx: &mut Cx<'_>,
        diagnostic: P<Diagnostic>,
    ) -> Option<PluginDiagnostic> {
        let _ = (cx, diagnostic);
        None
    }
}
