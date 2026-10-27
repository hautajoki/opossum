//! `Cx`: what a hook can ask of the compiler (DESIGN §9, §11.3).
//!
//! [`CxBackend`] is the compiler's side, one implementation over its per-file transform state. [`Cx`] is what
//! plugins hold: a thin wrapper that adds the parts needing no compiler ([`Abort`] values, diagnostics builders,
//! capture scoping), and generic helpers stay off the object-safe trait.
//!
//! A plugin gets an `Abort` only from `Cx`, after an error is recorded. That is a convention, not a seal: a
//! hand-written backend that returns [`Aborted`] without recording anything can make one, which takes deliberate
//! misuse. The compiler also checks it (a hook's `Err` without a new error is an internal error).

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use rbxts_project::{PackageInfo, ProjectServices, ResolvedModuleInfo};
use tsrs_ast::{Diagnostic, Node, SourceFile, Symbol};
use tsrs_checker::{Checker, Type};
use tsrs_compiler::Program;
use tsrs_core::{CompilerOptions, P};

use crate::{
    Abort, Arg, BinOp, CallSite, Callee, FileInfo, GlobMatcher, JsDocTag, LocalCounts, LuauExpr,
    LuauIdent, LuauStmt, ObjectEdits, PResult, PluginDiagnostic, Utf16Pos,
};

/// A backend primitive failed after recording an error diagnostic (a nested node failed to compile, or a
/// phase-bound primitive was called in the wrong phase and the backend reported an internal error). [`Cx`] turns
/// it into an [`Abort`]. A backend returns it only after recording an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aborted;

/// The compiler's side of [`Cx`], implemented over the per-file transform state and the instance's checker.
///
/// One `Cx` is made per hook call; the backend is anchored at the node being dispatched (for diagnostics,
/// prereq placement and the current function's locals) and knows which plugin is calling (for late-string keys
/// and messages).
///
/// **Phases.** Which hook may call what:
///
/// | primitives | claim hooks, `class` | `begin_file`, `end_file` | `classify_symbol` | `explain_checker_diagnostic` |
/// | --- | --- | --- | --- | --- |
/// | program, file, checker, project, module and package queries, source facts (`file_identifier_texts` included) | yes | yes | yes | yes |
/// | lowering: `compile_*`, `compile_receiver`, `prereq`, `push_to_var`, capture, `insert_before_root_statement`, `insert_before_statement`, `hoist`, `hoist_spillable`, `hoist_spillable_named`, `comment` | yes | refused | refused | refused |
/// | file-level: `header`, `import_named`, `mark_import_used`, `request_import_elision`, `runtime_member`, `temp_id`, `late_string` | yes | yes | refused | refused |
/// | `report` (and `Cx::error` / `warning`) | yes | yes | refused | yes |
/// | `is_usable_local_name`, `locals` | yes | yes | yes | yes |
/// | build inputs: `env`, `record_glob` | yes | yes | refused | yes |
///
/// Build inputs are refused in `classify_symbol` because they are recorded against the current file, and which file
/// first asks for a symbol depends on the checker count. The default implementations of these two cannot see the
/// phase; a compiler that overrides them enforces the row.
///
/// A refused call records an internal error naming the plugin (so the build fails) and returns `Err(Aborted)`, or
/// a placeholder `nil` / does nothing for the primitives without a `Result`.
///
/// **Positions** are UTF-16, from TypeScript's `getStart()`: trivia skipped, JSDoc not included. Lines are counted
/// as TypeScript's `computeLineStarts` counts them: a line break is CR, LF, CRLF (one break), U+2028 or U+2029.
pub trait CxBackend {
    // --- program, file, checker ---

    /// The program. Valid for the whole build.
    fn program(&self) -> &'static Program;
    /// The program's effective compiler options.
    fn options(&self) -> P<CompilerOptions>;
    /// The file being transformed.
    fn file(&self) -> &FileInfo;
    /// The instance's own checker. The `&mut` borrow ends before any other backend call, so no checker
    /// reference survives a `compile_*` call.
    fn checker(&mut self) -> &mut Checker;
    /// The project services: directories, project type, Rojo resolver, path translator, package.json cache.
    fn project(&self) -> &ProjectServices;
    /// `checker.getPropertiesOfType(t)` in TypeScript 5's order: the same symbols as the checker's
    /// `get_properties_of_type`, in the order TypeScript 5 lists them (TypeScript 7 sorts some types' members by
    /// declaration instead: inherited members, late-bound names, spreads, homomorphic mapped types). Where TypeScript
    /// 5's order follows nothing a declaration gives (a mapped type over a union of keys, a union's own property
    /// list, tuples), it is TypeScript 7's. Flamework's guards and serializer lay out fields in this order, so it is
    /// part of their wire format.
    ///
    /// **Default:** the checker's `get_properties_of_type`, in TypeScript 7's order.
    fn ts5_properties_of_type(&mut self, t: P<Type>) -> Vec<P<Symbol>> {
        self.checker().get_properties_of_type_exported(t).to_vec()
    }
    /// Where `specifier` resolves when imported from `from` (the program's resolution, else a resolver with the
    /// program's options).
    fn resolve_module(&self, specifier: &str, from: P<SourceFile>) -> Option<ResolvedModuleInfo>;
    /// The package.json of the package that declares `symbol` (its first declaration's file), if any.
    fn declaring_package(&self, symbol: P<Symbol>) -> Option<Arc<PackageInfo>>;

    // --- lowering original nodes the roblox-ts way ---

    /// Compiles an original expression node in the current prereq scope; nested nodes are dispatched to plugins
    /// as usual. On the hook's own node it lowers the node as roblox-ts does, macros included, without offering
    /// it to that offer kind again (see "Compiling a hook's own node" in the crate docs); do that at most once.
    fn compile_expr(&mut self, node: P<Node>) -> Result<LuauExpr, Aborted>;
    /// Compiles expressions keeping their evaluation order (`ensureTransformOrder`): earlier values are bound to
    /// locals when a later one has prereqs.
    fn compile_in_order(&mut self, nodes: &[P<Node>]) -> Result<Vec<LuauExpr>, Aborted>;
    /// The receiver of a property or element call ([`CallSite::receiver`]): already compiled (and nil-checked
    /// inside an optional chain, by the chain lowering: roblox-ts's property-call macro shape), or compiled now,
    /// once. The value may be an inline expression: bind it with `push_to_var` to use it more than once. On a
    /// non-method optional call (`o.f?.()`) it binds the receiver to a local, declared before the callee is read
    /// from it. On a call without a receiver (a plain call, `f?.()`, `new`, a decorator, `super(…)`), and on
    /// `super.m()` (`super` is not a value), it is an internal error naming the plugin.
    fn compile_receiver(&mut self, call: &CallSite) -> Result<LuauExpr, Aborted>;
    /// Compiles the call with another callee and/or argument slots; `Original` slots are compiled in order.
    ///
    /// With [`Callee::Original`] the call is lowered with roblox-ts's call lowering; roblox-ts macros are refused
    /// here. For a chain-start call the callee or receiver is compiled lazily, at `compile_call` time (unless
    /// `compile_receiver` already compiled it): only the prereqs of arguments the plugin compiled earlier (into
    /// `Arg::Luau` values) run before it, and a Luau value itself is evaluated at the call, after the lazily
    /// compiled receiver or callee. For an eagerly offered call (after `?.` or after another call) the callee or
    /// receiver is already compiled. For an optional call it is the call inside the nil check, `callee(self?,
    /// args)`. `super.m()` is `super.m(self, args)` when `m` is a method.
    fn compile_call(
        &mut self,
        call: &CallSite,
        callee: Callee,
        args: Vec<Arg>,
    ) -> Result<LuauExpr, Aborted>;
    /// Compiles an object literal node with edits; spreads, methods and computed keys are kept.
    fn compile_object_literal(
        &mut self,
        node: P<Node>,
        edits: ObjectEdits,
    ) -> Result<LuauExpr, Aborted>;
    /// `left op right` on original operands, with roblox-ts's operator lowering (string `+` stays `..`).
    fn compile_binary(
        &mut self,
        op: BinOp,
        left: P<Node>,
        right: P<Node>,
    ) -> Result<LuauExpr, Aborted>;

    // --- prereqs and placement ---

    /// Appends a statement to the current prereq scope: it runs before the expression being built.
    fn prereq(&mut self, stmt: LuauStmt);
    /// Binds `value` to a new temporary local in the prereqs and returns that local.
    fn push_to_var(&mut self, value: LuauExpr, hint: &str) -> LuauExpr;
    /// Opens a nested prereq scope. Paired with [`CxBackend::end_capture`]; scopes nest.
    fn begin_capture(&mut self);
    /// Closes the innermost scope opened by [`CxBackend::begin_capture`] and returns its statements.
    fn end_capture(&mut self) -> Vec<LuauStmt>;
    /// Statements placed before the top-level statement that contains the current node.
    fn insert_before_root_statement(&mut self, stmts: Vec<LuauStmt>);
    /// Statements placed before the Luau of the nearest TS statement that contains the current node, as Flamework's
    /// TS transformer places `prereqList` statements: ahead of that statement's leading comments and of everything
    /// roblox-ts emits for it (its own prerequisites included), after the file's root insertions when it is a
    /// top-level statement.
    ///
    /// **Which statement.** The node itself when it is a statement (a statement hook), else its nearest ancestor that
    /// is a statement (TypeScript's `isStatement`; a block counts only through the statements in it). So:
    /// - a class field initializer, a parameter initializer, a decorator: the class statement (or the statement a
    ///   class expression is in);
    /// - an arrow function's expression body, a `for` head, a loop or `if` condition, a `switch` expression: the
    ///   statement around it (the statements run once, before it, not per call or per iteration);
    /// - a statement in a block, a loop body, an `if` branch (a block or a single statement), a `case` clause, a
    ///   function body: that statement, in its own statement list;
    /// - the condition of an `else if`: before that inner `if`, which then becomes `else <statements> if … end end`
    ///   (as roblox-ts lowers the `else { …; if … }` block Flamework's transform makes there).
    ///
    /// **Order.** Insertions before one statement go in TypeScript's visit order, as a TS transform's `prereq`s are
    /// made, not in roblox-ts's lowering order: by the source position of the outermost running hook's node inside
    /// the statement, so a `for (init; condition; incrementor)` head's go init, condition, incrementor (roblox-ts
    /// lowers the incrementor first), and a class's go member by member as written (roblox-ts lowers the
    /// constructor's field initializers, then the methods, then the static fields). Insertions made under one such
    /// hook (its own, and those of hooks its `compile_*` calls ran) keep the order they were made in: the plugin
    /// chose it, as its TS transform's code does.
    ///
    /// **Unplaced.** When the chosen statement is not lowered in a statement list (no such case is known in
    /// roblox-ts's lowerings, but one would drop the statements), the file gets an internal error naming that
    /// statement at the end of the file, and the build fails: the insertion is never lost silently.
    ///
    /// It is an effect: a claim hook cannot decline its node after it (see "Declining" in the crate docs). Locals it
    /// declares before a top-level statement count toward the main chunk's ([`CxBackend::locals`]).
    ///
    /// **Default:** [`CxBackend::insert_before_root_statement`].
    fn insert_before_statement(&mut self, stmts: Vec<LuauStmt>) {
        self.insert_before_root_statement(stmts);
    }
    /// Statements placed once per file, after the file's leading run of imports (plugin imports, then the
    /// user's), in call order.
    fn header(&mut self, stmts: Vec<LuauStmt>);
    /// A local holding `value`, declared before the current root statement; returns the local.
    ///
    /// The value is declared before the root statement (the top-level statement around the hook's node), so it must
    /// not use anything that statement declares: a temporary its lowering made so far, the prereqs of the hook and of
    /// `compile_*` calls included (`hoist(compile_expr(x ?? y))` uses a temporary declared later), a parameter or local
    /// of the code around the node (`hoist(compile_expr(p))` in a function with parameter `p`), `self` / `super`
    /// inside a function or class, or anything a later statement of the file declares (a function, class, enum or
    /// variable, `declare`d ones included: roblox-ts declares a name used before its declaration as a local ahead of
    /// the statement that uses it, after the hoist). There it would read a global instead. The compiler refuses such
    /// a value (for `hoist_spillable` and `hoist_spillable_named` too): an internal error naming the plugin, and
    /// `nil`. What is declared before the root statement is fine: earlier top-level locals, other hoists, plugin
    /// imports, header and root-insertion locals, and a function's own locals inside a function value (in that
    /// function only: a name its body declares does not cover the same name used outside it).
    fn hoist(&mut self, value: LuauExpr, hint: &str) -> LuauExpr;
    /// As `hoist`, but the compiler may turn the local into a field of one spill table when the main chunk would
    /// pass Luau's 200-local limit; returns the expression to use either way.
    fn hoist_spillable(&mut self, value: LuauExpr, hint: &str) -> LuauExpr;
    /// As [`CxBackend::hoist_spillable`], with a plain local name (`constant_22`) instead of a temporary's hint:
    /// the local is `name` itself when the name is free, else the first free `name_1`, `name_2`, … (`name1`, … when
    /// `name` ends in `_`): TypeScript's printer rule for an optimistic unique name (`createUniqueName(name,
    /// Optimistic)`), which Flamework's TS transformer uses. Free means what
    /// [`CxBackend::is_usable_local_name`] checks (no identifier text of the file, JSDoc included, no plugin import,
    /// no seam temporary, no approved name, no `luau.globals` name), so the local stays capture-safe; the name
    /// chosen is recorded as taken. A spill makes it the field `_spill.<chosen name>`.
    ///
    /// A `name` that is not a valid Luau identifier is an internal error naming the plugin, and the value is `nil`.
    ///
    /// **Default:** [`CxBackend::hoist_spillable`] with `name` as the hint.
    fn hoist_spillable_named(&mut self, value: LuauExpr, name: &str) -> LuauExpr {
        self.hoist_spillable(value, name)
    }
    /// The value of `export` from module `specifier`, imported once per file and placed with roblox-ts's imports
    /// (reusing a user import of the same export).
    ///
    /// **Placement.** Plugin imports go after the `--!` directives, the `-- Compiled with` line and
    /// `local TS = require(...)`, and after a header comment the file starts with (a run of comments with a blank
    /// line after it, which TypeScript's printer keeps at the top of the file), ahead of the user's own imports.
    /// They are placed as Flamework's TS transformer adds its import declarations while it walks the file: one per
    /// path, the paths in the order their first import was asked for, each listing its imports in the order they
    /// were asked for. That order is the source position of the node whose hook asked (the file for
    /// `begin_file` / `end_file`), then the request order. A path with several imports binds its module to a
    /// local first (`local _core = TS.import(…)`, `local a = _core.a`), as roblox-ts compiles such a declaration.
    ///
    /// **Name.** The local is named after the export; when that name is taken (see
    /// [`CxBackend::hoist_spillable_named`]), the first free `export_1`, `export_2`, …, as TypeScript's printer
    /// names the TS transformer's unique import name. The printer names them in the placement order above, so when
    /// two paths both give an export `t`, the one placed first is `t` whichever was asked for first. An export name
    /// that is not a valid Luau identifier gets a temporary instead.
    ///
    /// A reused user import is kept alive like a [`CxBackend::mark_import_used`] one, so it counts as left behind
    /// for declining.
    fn import_named(&mut self, specifier: &str, export: &str) -> Result<LuauExpr, Aborted>;
    /// `TS.<object>.<member>`: a member of the roblox-ts runtime library, as roblox-ts compiles a reference to a
    /// global it provides (`Promise.is` becomes `TS.Promise.is`). The file gets `local TS = require(...)` as for the
    /// compiler's own runtime-library uses, and the use counts for import elision like theirs. Both names must be
    /// valid Luau identifiers (else an internal error naming the plugin, and `nil`). In a project, a use in
    /// ReplicatedFirst gets roblox-ts's warning.
    ///
    /// File-level, like `import_named`: refused in `classify_symbol` and `explain_checker_diagnostic`. It is an
    /// effect for declining (the file would keep its `local TS`).
    ///
    /// **Default:** the expression `TS.<object>.<member>`, with no `local TS` made for it.
    fn runtime_member(&mut self, object: &str, member: &str) -> LuauExpr {
        rbxts_luau::create::property(
            rbxts_luau::create::property(rbxts_luau::create::id("TS"), object),
            member,
        )
    }
    /// Keeps an import alias declaration alive through import elision, though no remaining value use names it.
    fn mark_import_used(&mut self, alias_declaration: P<Node>);
    /// The calling plugin removed value uses of this import alias declaration (an ImportSpecifier, ImportClause,
    /// NamespaceImport or ImportEqualsDeclaration): it claimed the nodes that used it and did not compile them.
    /// The checker still counts those uses, so without this request the import would stay (a runtime `require` of
    /// a types-only module). The compiler then elides the alias unless the Luau it emits for this file still
    /// needs it: a reference to it that the compiler compiled itself (its own lowering or a plugin's `compile_*`),
    /// or a [`CxBackend::mark_import_used`] on it, or an `import_named` of any plugin that reused it. When every
    /// alias of a declaration is elided, the whole import goes, as roblox-ts elides imports.
    fn request_import_elision(&mut self, alias_declaration: P<Node>);
    /// A new temporary local name (`luau.tempId`), unique in its scope at render time.
    ///
    /// Temporaries a plugin asks for (`temp_id`, `hoist`, `hoist_spillable`, `push_to_var`, and the local
    /// `import_named` picks when the export's name is taken) are named so that nothing can capture them: no
    /// identifier of the file, no name `is_usable_local_name` approved, and no plugin import. Their rendered name
    /// may differ from the hint (`_hint2`, …).
    fn temp_id(&mut self, hint: &str) -> LuauIdent;
    /// `name` can be declared as a local here: a valid Luau identifier that is not a `luau.globals` name, not any
    /// identifier text in the file's source, not a plugin import's name, not a seam temporary's name, and not
    /// already approved.
    ///
    /// It is not idempotent: a true answer records the name, so a second call for the same name is false.
    /// Temporaries made earlier keep a name from being approved, and later ones avoid approved names.
    fn is_usable_local_name(&self, name: &str) -> bool;
    /// Locals declared so far ([`LocalCounts`]): `main_chunk`, the locals of the main chunk's top level (statements
    /// done, hoists, imports, header); `function`, the locals the body of the innermost function around the hook's
    /// node declared at its top level, not counting its parameters or nested blocks (in the main chunk, the same as
    /// `main_chunk`). It counts the Luau built so far, so it is exact only for code already emitted: statements after
    /// the current node, and the rest of the current statement, are not in the count.
    fn locals(&self) -> LocalCounts;
    /// A string literal whose text the calling plugin provides in `finish` under the same key
    /// ([`crate::FinishCx::resolve_late`]).
    ///
    /// **Contract.** Keys are per plugin. The compiler records, per file, the late keys its Luau uses and their
    /// committed values. Every `finish` must resolve the keys of every current file, cached files included; a
    /// missing one fails the build, naming the plugin and the key. A key's value may change between builds: after
    /// finish the compiler re-renders (or re-transforms) every cached file whose values changed. Plugins should
    /// still keep the values of committed keys stable where they can (Flamework appends new ids from its
    /// persistent table), so this rarely fires.
    fn late_string(&mut self, key: &str) -> LuauExpr;
    /// A Luau comment before the next statement emitted.
    ///
    /// When it is the first thing the nearest TS statement around the node emits (a statement, see
    /// [`CxBackend::insert_before_statement`]), it goes above that statement's leading comments, where TypeScript's
    /// printer puts a TS transformer's prereq comment. Elsewhere (after other prerequisites of the statement, or
    /// in a scope the plugin captured) it stays where it is.
    fn comment(&mut self, text: &str);

    // --- source facts ---

    /// The node's start (`getStart()`) in UTF-16: offset, and line and character as TypeScript's
    /// `getLineAndCharacterOfPosition` gives them (line breaks CR, LF, CRLF, U+2028, U+2029).
    fn utf16_pos(&self, node: P<Node>) -> Utf16Pos;
    /// The node's width from `getStart()` to its end, in UTF-16 code units.
    fn utf16_width(&self, node: P<Node>) -> u32;
    /// The node's source text from `getStart()` to its end (safe for every kind, unlike `Node::text`).
    fn source_text(&self, node: P<Node>) -> &'static str;
    /// The JSDoc tags of a declaration, with `{@link}` names resolved on this checker. Cached per checker.
    fn jsdoc_tags(&mut self, declaration: P<Node>) -> Rc<[JsDocTag]>;
    /// The checker's diagnostics of this file (read-only; nothing is checked again). During the transform it holds
    /// only non-error diagnostics: a file with checker errors is not transformed (it reaches
    /// [`crate::PluginInstance::explain_checker_diagnostic`] instead).
    fn file_diagnostics(&self) -> &[P<Diagnostic>];
    /// Every identifier text of the file, sorted and without duplicates: TypeScript 5's `sourceFile.identifiers`,
    /// which its printer checks a unique name against ([`crate::ts5_file_identifiers`]). That is every identifier
    /// and private identifier, those in JSDoc included (tag names, `@param` names, `{@link}` names, names in JSDoc
    /// types), and the string and numeric literals TypeScript's parser interns: property names (`{ "a": 1 }`,
    /// `"a": T` in a type, enum members, `{ "a": x }` in a binding), element access arguments (`o["a"]`, `o[0]`),
    /// module specifiers and ambient module names. The compiler's own capture-safe naming
    /// ([`CxBackend::is_usable_local_name`], seam temporaries, `import_named`) counts the same set.
    ///
    /// **Default:** [`crate::ts5_file_identifiers`] of [`CxBackend::file`]'s source, computed on every call.
    fn file_identifier_texts(&self) -> Rc<[&'static str]> {
        crate::identifiers::ts5_file_identifiers(self.file().source).into()
    }

    // --- diagnostics ---

    /// Records a diagnostic for this file. Any error fails the build: nothing is written.
    ///
    /// The build keeps one of each, as TypeScript's `sortAndDeduplicateDiagnostics` does. Equal means the same
    /// node range, the same source (the plugin's [`crate::Plugin::diagnostic_source`] standing in for a `None`
    /// source; it renders as the code) and the same message text, the hint included as the line under the message;
    /// the category and the related information are not compared. Of a plugin diagnostic and the diagnostics equal
    /// to it, reported by this file or any other (another plugin, another file's hook at the same node of a third
    /// file, roblox-ts itself), the one first in TypeScript 5.9's `compareDiagnostics` order is kept: the one with
    /// more related information; then a plugin's before a compiler diagnostic (stock adds transformer diagnostics
    /// first and sorts stably), then by roblox-ts's transform order and report order.
    /// Watch and incremental builds keep the same copy as a clean build: the choice is made over every file's list,
    /// the cached lists of files not transformed again included, as each file's own transform reported it.
    fn report(&mut self, diagnostic: PluginDiagnostic);

    // --- build inputs (DESIGN §14) ---

    /// An environment variable of the process, read while this file is transformed, recorded as an input of this
    /// file.
    ///
    /// **Value.** What [`crate::SetupBackend::env`] returns for the same name in the same build: one snapshot of the
    /// process environment per build (in watch it does not change). Only the process environment: files such as
    /// `.env` are the plugin's to read in setup ([`crate::SetupBackend::read_input`]) and to layer (Flamework puts
    /// the process environment over `.env.local` over `.env`).
    ///
    /// **Contract for an override.** Records `(plugin, name, value)` against the current file, kept with the file's
    /// cached part and replaced when the file is transformed again. A build in a new process (incremental) that
    /// reads a different value for a recorded name transforms the file again, then runs `finish`. Refused in
    /// `classify_symbol` (the phase table): an internal error naming the plugin, and `None`.
    ///
    /// **Default:** the process environment through `std::env::var` (a name the environment cannot hold is unset),
    /// recording nothing.
    fn env(&mut self, name: &str) -> Option<String> {
        crate::glob::process_env(name)
    }

    /// Matches a glob under `root` (an absolute directory) and records it as an input of this file: the file's output
    /// may depend on the matches. Returns `matcher.expand(root)` as it is.
    ///
    /// **Contract for an override.**
    /// - Returns `matcher.expand(root)` unchanged, and records `(plugin, root, pattern, matcher, answer)` against
    ///   the current file, kept with its cached part and replaced when the file is transformed again. One plugin's
    ///   records with an equal `(root, pattern)` in one file are one record: the plugin guarantees that an equal
    ///   pattern means an equal matcher.
    /// - Watch: the compiler watches `root` recursively, files and directories (its nearest existing ancestor
    ///   while it does not exist). After an add, delete or rename under it that [`GlobMatcher::affected_by`]
    ///   accepts, it calls `expand` again, before the rebuild's transform. If the answer differs from the recorded
    ///   one, the file is transformed again (and `finish` runs). A rename is a delete plus an add.
    /// - A build in a new process has the recorded answer but no matcher: it transforms every cached file with a
    ///   recorded glob again (the file records it anew).
    /// - Refused in `classify_symbol` (the phase table): an internal error naming the plugin, and no matches.
    ///
    /// **Default:** `matcher.expand(root)`, recording nothing.
    fn record_glob(
        &mut self,
        root: &Path,
        pattern: &str,
        matcher: Arc<dyn GlobMatcher>,
    ) -> Vec<String> {
        let _ = pattern;
        crate::glob::expand_unrecorded(root, &*matcher)
    }
}

/// The context of one hook call. See [`CxBackend`] for what each primitive guarantees; the methods here forward
/// to it, turning [`Aborted`] into [`Abort`].
pub struct Cx<'a> {
    backend: &'a mut dyn CxBackend,
}

impl<'a> Cx<'a> {
    /// Wraps the compiler's backend for one hook call.
    pub fn new(backend: &'a mut dyn CxBackend) -> Cx<'a> {
        Cx { backend }
    }

    // --- program, file, checker ---

    /// The program ([`CxBackend::program`]).
    pub fn program(&self) -> &'static Program {
        self.backend.program()
    }

    /// The compiler options ([`CxBackend::options`]).
    pub fn options(&self) -> P<CompilerOptions> {
        self.backend.options()
    }

    /// The file being transformed: source, index, paths, Rojo facts.
    pub fn file(&self) -> &FileInfo {
        self.backend.file()
    }

    /// This instance's checker, for type queries. Raw tsrs: names follow tsrs's on every rebase.
    pub fn checker(&mut self) -> &mut Checker {
        self.backend.checker()
    }

    /// The project services ([`CxBackend::project`]).
    pub fn project(&self) -> &ProjectServices {
        self.backend.project()
    }

    /// A type's properties in TypeScript 5's order ([`CxBackend::ts5_properties_of_type`]).
    pub fn ts5_properties_of_type(&mut self, t: P<Type>) -> Vec<P<Symbol>> {
        self.backend.ts5_properties_of_type(t)
    }

    /// Module resolution from a file ([`CxBackend::resolve_module`]).
    pub fn resolve_module(
        &self,
        specifier: &str,
        from: P<SourceFile>,
    ) -> Option<ResolvedModuleInfo> {
        self.backend.resolve_module(specifier, from)
    }

    /// The package declaring a symbol ([`CxBackend::declaring_package`]).
    pub fn declaring_package(&self, symbol: P<Symbol>) -> Option<Arc<PackageInfo>> {
        self.backend.declaring_package(symbol)
    }

    // --- lowering ---

    /// Compiles an original expression ([`CxBackend::compile_expr`]).
    pub fn compile_expr(&mut self, node: P<Node>) -> PResult<LuauExpr> {
        self.backend.compile_expr(node).map_err(abort)
    }

    /// Compiles expressions in evaluation order ([`CxBackend::compile_in_order`]).
    pub fn compile_in_order(&mut self, nodes: &[P<Node>]) -> PResult<Vec<LuauExpr>> {
        self.backend.compile_in_order(nodes).map_err(abort)
    }

    /// The compiled, nil-checked receiver of a method call ([`CxBackend::compile_receiver`]).
    pub fn compile_receiver(&mut self, call: &CallSite) -> PResult<LuauExpr> {
        self.backend.compile_receiver(call).map_err(abort)
    }

    /// The call with another callee or argument slots ([`CxBackend::compile_call`]).
    pub fn compile_call(
        &mut self,
        call: &CallSite,
        callee: Callee,
        args: Vec<Arg>,
    ) -> PResult<LuauExpr> {
        self.backend.compile_call(call, callee, args).map_err(abort)
    }

    /// An object literal with edits ([`CxBackend::compile_object_literal`]).
    pub fn compile_object_literal(
        &mut self,
        node: P<Node>,
        edits: ObjectEdits,
    ) -> PResult<LuauExpr> {
        self.backend
            .compile_object_literal(node, edits)
            .map_err(abort)
    }

    /// A binary operation on original operands ([`CxBackend::compile_binary`]).
    pub fn compile_binary(
        &mut self,
        op: BinOp,
        left: P<Node>,
        right: P<Node>,
    ) -> PResult<LuauExpr> {
        self.backend.compile_binary(op, left, right).map_err(abort)
    }

    // --- prereqs and placement ---

    /// A statement that runs before the expression being built ([`CxBackend::prereq`]).
    pub fn prereq(&mut self, stmt: LuauStmt) {
        self.backend.prereq(stmt);
    }

    /// Binds a value to a temporary local in the prereqs ([`CxBackend::push_to_var`]).
    pub fn push_to_var(&mut self, value: LuauExpr, hint: &str) -> LuauExpr {
        self.backend.push_to_var(value, hint)
    }

    /// Runs `f` with a fresh prereq scope and returns its result with the statements it emitted, instead of
    /// letting them reach the enclosing scope (for wrapping them in a `do` block, a function, or a branch).
    ///
    /// `end_capture` runs on every exit: when `f` returns `Err` the scope is closed and its statements are
    /// dropped, and when `f` panics the scope is closed before the panic continues, so the backend's scope stack
    /// stays balanced for the per-file panic handler.
    pub fn capture<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<(T, Vec<LuauStmt>)> {
        self.backend.begin_capture();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *self)));
        let statements = self.backend.end_capture();
        match result {
            Ok(result) => result.map(|value| (value, statements)),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Statements before the current root statement ([`CxBackend::insert_before_root_statement`]).
    pub fn insert_before_root_statement(&mut self, stmts: impl IntoIterator<Item = LuauStmt>) {
        self.backend
            .insert_before_root_statement(stmts.into_iter().collect());
    }

    /// Statements before the nearest TS statement around the current node, as Flamework's `prereqList` places them
    /// ([`CxBackend::insert_before_statement`]).
    pub fn insert_before_statement(&mut self, stmts: impl IntoIterator<Item = LuauStmt>) {
        self.backend
            .insert_before_statement(stmts.into_iter().collect());
    }

    /// Statements after the file's imports ([`CxBackend::header`]).
    pub fn header(&mut self, stmts: impl IntoIterator<Item = LuauStmt>) {
        self.backend.header(stmts.into_iter().collect());
    }

    /// A hoisted local ([`CxBackend::hoist`]).
    pub fn hoist(&mut self, value: LuauExpr, hint: &str) -> LuauExpr {
        self.backend.hoist(value, hint)
    }

    /// A hoisted local that may spill into a table ([`CxBackend::hoist_spillable`]).
    pub fn hoist_spillable(&mut self, value: LuauExpr, hint: &str) -> LuauExpr {
        self.backend.hoist_spillable(value, hint)
    }

    /// A hoisted local with a plain name (`name`, else `name_1`, …) that may spill into a table
    /// ([`CxBackend::hoist_spillable_named`]).
    pub fn hoist_spillable_named(&mut self, value: LuauExpr, name: &str) -> LuauExpr {
        self.backend.hoist_spillable_named(value, name)
    }

    /// An imported value ([`CxBackend::import_named`]).
    pub fn import_named(&mut self, specifier: &str, export: &str) -> PResult<LuauExpr> {
        self.backend.import_named(specifier, export).map_err(abort)
    }

    /// `TS.<object>.<member>`, a member of the roblox-ts runtime library, with the file's `local TS`
    /// ([`CxBackend::runtime_member`]).
    pub fn runtime_member(&mut self, object: &str, member: &str) -> LuauExpr {
        self.backend.runtime_member(object, member)
    }

    /// The Luau string roblox-ts emits for a string literal a TS transformer made with
    /// `factory.createStringLiteral(text)` ([`crate::synthesized_string`]). Needs no compiler state.
    pub fn synthesized_string(&self, text: &str) -> LuauExpr {
        crate::synthesized_string(text)
    }

    /// Keeps an import alive ([`CxBackend::mark_import_used`]).
    pub fn mark_import_used(&mut self, alias_declaration: P<Node>) {
        self.backend.mark_import_used(alias_declaration);
    }

    /// Asks the compiler to elide an import whose value uses this plugin removed, if nothing it emits still
    /// needs it ([`CxBackend::request_import_elision`]).
    pub fn request_import_elision(&mut self, alias_declaration: P<Node>) {
        self.backend.request_import_elision(alias_declaration);
    }

    /// A unique temporary local name ([`CxBackend::temp_id`]).
    pub fn temp_id(&mut self, hint: &str) -> LuauIdent {
        self.backend.temp_id(hint)
    }

    /// Whether a local can take this name ([`CxBackend::is_usable_local_name`]).
    pub fn is_usable_local_name(&self, name: &str) -> bool {
        self.backend.is_usable_local_name(name)
    }

    /// Local counts of the Luau built so far, exact only for code already emitted ([`CxBackend::locals`]).
    pub fn locals(&self) -> LocalCounts {
        self.backend.locals()
    }

    /// A string literal filled in by this plugin's `finish` ([`crate::FinishCx::resolve_late`]) under the same
    /// `key`. Keys are per plugin; the same key may be handed out any number of times. A value may change between
    /// builds (cached files using it are re-rendered); see the contract on [`CxBackend::late_string`].
    pub fn late_string(&mut self, key: &str) -> LuauExpr {
        self.backend.late_string(key)
    }

    /// A Luau comment before the next statement ([`CxBackend::comment`]).
    pub fn comment(&mut self, text: &str) {
        self.backend.comment(text);
    }

    // --- source facts ---

    /// The node's UTF-16 start, with TypeScript's line counting ([`CxBackend::utf16_pos`]).
    pub fn utf16_pos(&self, node: P<Node>) -> Utf16Pos {
        self.backend.utf16_pos(node)
    }

    /// The node's UTF-16 width ([`CxBackend::utf16_width`]).
    pub fn utf16_width(&self, node: P<Node>) -> u32 {
        self.backend.utf16_width(node)
    }

    /// The node's source text ([`CxBackend::source_text`]).
    pub fn source_text(&self, node: P<Node>) -> &'static str {
        self.backend.source_text(node)
    }

    /// A declaration's JSDoc tags ([`CxBackend::jsdoc_tags`]).
    pub fn jsdoc_tags(&mut self, declaration: P<Node>) -> Rc<[JsDocTag]> {
        self.backend.jsdoc_tags(declaration)
    }

    /// The JSDoc tags of every declaration of `symbol`, in declaration order.
    pub fn symbol_jsdoc_tags(&mut self, symbol: P<Symbol>) -> Vec<JsDocTag> {
        let mut tags = Vec::new();
        for &declaration in symbol.declarations() {
            tags.extend(self.backend.jsdoc_tags(declaration).iter().cloned());
        }
        tags
    }

    /// The file's checker diagnostics ([`CxBackend::file_diagnostics`]).
    pub fn file_diagnostics(&self) -> &[P<Diagnostic>] {
        self.backend.file_diagnostics()
    }

    /// The file's identifier texts as TypeScript 5's printer sees them, sorted ([`CxBackend::file_identifier_texts`]).
    pub fn file_identifier_texts(&self) -> Rc<[&'static str]> {
        self.backend.file_identifier_texts()
    }

    // --- diagnostics ---

    /// Records an error at `node` and returns the [`Abort`] to return: `return Err(cx.error(node, "…"))`.
    pub fn error(&mut self, node: P<Node>, message: impl Into<String>) -> Abort {
        self.backend.report(PluginDiagnostic::error(node, message));
        Abort(())
    }

    /// Records a warning at `node`.
    pub fn warning(&mut self, node: P<Node>, message: impl Into<String>) {
        self.backend
            .report(PluginDiagnostic::warning(node, message));
    }

    /// Records a diagnostic. Returns `Some(Abort)` when it is an error, for `if let Some(a) = cx.report(d)
    /// { return Err(a) }`.
    pub fn report(&mut self, diagnostic: PluginDiagnostic) -> Option<Abort> {
        let is_error = diagnostic.category == tsrs_diagnostics::Category::Error;
        self.backend.report(diagnostic);
        is_error.then_some(Abort(()))
    }

    // --- build inputs ---

    /// An environment variable of the process, recorded as an input of this file ([`CxBackend::env`]). The same
    /// value [`crate::SetupCx::env`] gives in this build.
    pub fn env(&mut self, name: &str) -> Option<String> {
        self.backend.env(name)
    }

    /// Matches a glob under the absolute directory `root`, recorded as an input of this file: in watch, a change in
    /// the matches transforms the file again ([`CxBackend::record_glob`]). `pattern` names the glob: one pattern,
    /// one matcher.
    pub fn record_glob(
        &mut self,
        root: impl AsRef<Path>,
        pattern: &str,
        matcher: impl GlobMatcher,
    ) -> Vec<String> {
        self.backend
            .record_glob(root.as_ref(), pattern, Arc::new(matcher))
    }
}

fn abort(_: Aborted) -> Abort {
    Abort(())
}
