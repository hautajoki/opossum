//! The compiler side of the plugin seam (DESIGN §9, §11, §14): `rbxts_plugin`'s backend traits implemented over the
//! transform, the dispatch tables built from each instance's `Interest`, and the build lifecycle around them.
//!
//! - [`host`]: [`PluginHost`], one per checker: the instances, the dispatch tables, the `(symbol, role)`
//!   classification memo, the source-fact caches (line maps, JSDoc) and the hook timings.
//! - [`file_state`]: what one file's transform records for plugins (imports, header, root insertions, late keys,
//!   import-elision requests, file parts, offer marks, spillable hoists).
//! - [`backend`]: `CxBackend` over [`TransformState`](crate::classes::transform_state::TransformState), with the
//!   phase table, and the lowering of `compile_call` / `compile_receiver`.
//! - [`dispatch`]: the claim points the transform calls (calls, `new`, decorators, property keys, assignments,
//!   by-kind fallbacks, classes, files, root statements, imports).
//! - [`lifecycle`]: activation from the tsconfig, `SetupBackend`, `FinishBackend`, the late-string key check, the
//!   commit with atomic writes, and `explain_checker_diagnostic`.
//! - `lowering`: `compile_binary` and `compile_object_literal`; `spill`: Luau's limits (spill, load check). `hoist_check`:
//!   what a hoisted value may use.
//!
//! **Offer kinds.** There are five: calls (with `new` and decorators), property keys, assignments, the expression
//! and statement fallbacks, and the class hook. Within a kind, plugins are asked in registration order and the first
//! `Take` wins; all of them come before roblox-ts's own macros. Across kinds, the expression hooks are asked about
//! a node first, whatever the registration order, then the call or assignment hooks (`RBXTS_PLUGIN_CLAIMS=all` /
//! [`PluginOptions::claims_all`] re-asks the call plugins of call, `new` and decorator offers and reports a double
//! claim).
//!
//! **Calls.**
//! - A chain's first call reached through plain accesses only (`f()`, `ns.f()`, `a.b.m()`, `super.m()`) is offered
//!   before anything of the chain is compiled, so a plugin that takes it leaves the callee uncompiled.
//! - Any other chain call (after a `?.` link or another call) is offered with its receiver or callee already
//!   compiled, and nil-checked inside an optional chain; `compile_receiver` returns that receiver. `f?.()` is
//!   offered once the chain lowering has nil-checked `f` (it has no receiver).
//! - An optional call (`o.m?.()`, `o[k]?.()`) is offered inside the nil check on its callee (`o.m`): the claimed value
//!   is used only when the callee is not nil. A method call's receiver is already in a local; a non-method call's
//!   receiver is bound to one by `compile_receiver`, declared before the callee is read from it.
//! - `super.m()` / `super[k]()` have `super` as their receiver node, but `super` is not a value: `compile_receiver`
//!   on them is an internal error. `compile_expr` and `compile_call(Callee::Original)` lower them as roblox-ts does
//!   (`super.m(self, …)`).
//! - `new` expressions before constructor macros; decorators (`@D`, `@D(…)`) before the decorator expression is
//!   compiled.
//!
//! **Lowering a hook's own node.** `compile_expr` / `compile_in_order` / an `Arg::Original` slot of the node a hook
//! was offered lowers it as roblox-ts does there (its macros included), without offering it to that kind again: a
//! call on the receiver or callee already compiled (so nothing is evaluated twice); an expression-hook node offered
//! after its chain base was compiled, on that base; an assignment, `++` / `--` or logical assignment that roblox-ts
//! lowers as a statement (its value unused), as that statement, with `nil` for its value (returned as it is, it adds
//! nothing). Other kinds still see the node: an expression hook compiling its own call lets the call plugins claim
//! it. The compiler refuses, as internal errors naming the plugin: a second self-compile, `compile_call` twice or
//! after a self-compile, a class hook compiling its own class, a statement hook compiling its own statement, any hook
//! compiling an ancestor of its own node or a node whose value its offer compiled already (a call's receiver once
//! compiled, a chain link's base), and an assignment hook compiling a `delete` operand (roblox-ts lowers it as the
//! deletion).
//!
//! **Asked once.** A node every hook of a kind declined is not offered to that kind again, however often it is
//! lowered (a call hook compiling its own call does not re-ask the expression hooks); `property_key` answers are
//! kept per access node.
//!
//! **The expression fallback** sees every node of its kinds that roblox-ts lowers as an expression, including chain
//! links: the links up to the first `?.` link or optional call are offered before anything of the chain is
//! compiled, from the outermost in (the chain's first call to the call plugins after its own expression offer);
//! later links inside the chain's nil check, their base compiled (a hook that takes such a link without lowering it
//! drops the base's value, its base still evaluated once). An expression statement's assignment or update (which
//! roblox-ts lowers as a statement) is offered too, and so is a dotted JSX tag (`<a.B />`, `transform_jsx_tag_name`:
//! to `property_key` too, a claim's prereqs before the element). Positions other kinds cover: callees (the call, and
//! `property_key`), assignment targets and `delete` operands (lowered as writes: the assignment hook and
//! `property_key`), binding patterns' targets, and object literals a hook lowers with `compile_object_literal`. Nodes
//! roblox-ts lowers only as parts of a larger form (`return $tuple(…)`, `$range`, numeric-`for` heads, flattened
//! logical and bitwise chains, inline array spreads, constructor-macro arguments, constant enum members, parentheses
//! and type assertions it looks through) are offered with the form: offering them alone would change how an identity
//! plugin's lowering of them lowers the form (`rbxts_plugin` crate docs, "Parts of a form").
//!
//! **The statement fallback** sees every statement roblox-ts lowers (`transform_statement`, before its `declare`
//! check), block bodies of `if`, loops and `case` clauses (`transform_statement_list::transform_body`) and class
//! static blocks (a claim is the body), and `else if` (`transform_if_statement`: a claim is the `else` block, after
//! the `insert_before_statement` statements anchored there).
//!
//! **Property keys** (`accesses_by_marker`). Every property and element access whose receiver's apparent
//! non-nullable type has a marker, when roblox-ts lowers it: reads, `delete`, writes,
//! compound assignments, `++` / `--`, destructuring targets, `for … of` heads, callees (method calls, optional calls,
//! `super.m()`) and every link of optional chains. The hook lowers nothing (the lowering primitives are refused, as
//! in `begin_file`); its answer is the key: `base.key` when it is a valid Luau identifier, else
//! `base[text_literal(key)]`, and a method callee `base:key(…)` / `base[text_literal(key)](base, …)`. A key equal to
//! the written one leaves the access as written. The answer is kept per access node, so `AccessSite.position` is the
//! position of the first lowering that asked. `AccessSite.key` is the property name, or the value of the element
//! argument's type when that is a single string literal (`obj["k"]`, `obj[k as "k"]`, a literal-typed `k`); else
//! `None` (numeric keys too: roblox-ts offsets numeric indices, and no string stands for them), and such a site can
//! only be declined or failed (a `Take` is an internal error naming the plugin). A rewritten key that replaces a
//! computed argument still evaluates the argument for its effects, after the receiver (one that only reads, such as an
//! enum member, is dropped with it). Not asked: destructuring sources
//! (`const { a } = obj`), object literal keys, `"a" in obj`, and callees of roblox-ts macros (no access is emitted).
//!
//! **Assignments** (`assignments_by_receiver_symbol`). An assignment, compound or logical assignment, `++` / `--` or
//! `delete` whose target is `receiver.key` (or `receiver[e]`) and whose receiver's symbol the plugin classifies
//! (`SymbolRole::AssignReceiver`), offered before roblox-ts lowers the node and after the expression hooks.
//! Destructuring targets and `for … of` heads are not offered, and neither are targets whose receiver has no symbol
//! (`f().x = 1`, `a[i].x = 1`). A computed key is offered with `AssignSite.key` `None` (the rule of property keys);
//! the hook can compile `key_node` itself.
//! `compile_binary(op, site.target, value)` reads the target, receiver included: a hook that also writes back
//! through the receiver (`receiver:set(key, value)`) evaluates the receiver twice.
//!
//! **Declining.** Offer state lives in a stack of hook frames ([`file_state::HookFrame`]), one per running hook,
//! so a hook nested in another (through its `compile_*` calls) never resets the outer one's. A claim hook cannot
//! decline its node after it lowered it (`compile_expr` of the node, `compile_call`), compiled the receiver inside
//! a capture, or left anything behind in the scope it writes into or the file: prereqs, a hoist, a root insertion,
//! statements before its TS statement, a header statement, a plugin import, a late string, a runtime-library member,
//! or an import kept alive (`mark_import_used`), its own or a
//! nested hook's (`request_import_elision` is harmless and allowed). Nor after any hook ran inside it (a nested node
//! offered or claimed, a class hook): that hook would run again. The one exception is a `compile_receiver` into
//! the offer's own scope that is the first thing to grow it (or the binding of an optional call's receiver): what
//! it left is kept for the next plugin, or roblox-ts, to reuse. A decline that breaks the rule is an internal error,
//! and the offer ends there: no other plugin, and not roblox-ts, gets the node. The rule makes plugins interact:
//! plugin B's interest in a node nested in A's (even one B declines cleanly) makes A's compile-then-decline an
//! error, so a plugin must decide from the AST and types before compiling anything.
//!
//! A claimed value whose node is in statement position is evaluated as a statement (`local _ = value` unless it is a
//! call), so no lowering drops its effects; inside parentheses roblox-ts lowers (a `for` head's `(x = 0)`) it is
//! returned as it is, and the statement around the parentheses evaluates it (`local _ = (x)`), as without plugins.
//! A literal that is indexed or called, a claimed value or one roblox-ts lowers there itself (a cast, an inlined const
//! enum member), is rendered in parentheses (`("x"):m()`), as Luau needs.
//!
//! `RBXTS_PLUGIN_CLAIMS=all` asks the other matching plugins of call, `new` and decorator offers too (not the other
//! kinds), in a scratch scope. A build with it either writes the same output as without it or fails: one where such
//! a plugin changed compiler state before declining (an approval by `is_usable_local_name` included) fails.
//!
//! **Hoists** (`hoist_check`): a hoisted value must not use what the top-level statement around the hook's node
//! declares, which it would not read before that statement: a temporary made since the statement started
//! (`PluginFileState::root_temp_start`; the seam's own top-level temporaries excepted), a name that resolves at the
//! hook's node to a declaration in that statement, in a function, class or block, or in a later statement of the file
//! (roblox-ts declares a name used before its declaration ahead of the statement using it, after the hoist), or
//! `self` / `super` inside a function or class. The value's own functions and blocks are scopes: a local they
//! declare covers its uses inside them only. The declaration looked at is the name's value declaration (or an
//! import's), as roblox-ts's `checkIdentifierHoist` does: an interface, type alias or global augmentation merged with
//! it does not count. Such a value is an internal error naming the plugin.
//!
//! **`classify_symbol`** (`backend::run_classify`): what it reports is anchored at the symbol's declaration and
//! taken out of the file's diagnostics; the build reports each such diagnostic once, sorted
//! (`compile::classify_diagnostics`), whatever the checker count.
//!
//! **Plugin diagnostics** (`compile::drop_plugin_duplicates`): each file's list keeps everything its hooks reported;
//! the build then keeps one of each plugin diagnostic and the diagnostics equal to it (same file, range, code or
//! source, message text), the copy TypeScript's `sortAndDeduplicateDiagnostics` keeps (more related information
//! first, then a plugin's, then transform order and report order). In watch and incremental builds a transformed file
//! commits its list as its transform reported it, before the drop, and the drop runs over the transformed files'
//! lists and the cached lists of the files not transformed again together: the copy kept is a clean build's, also
//! when the file holding it stops reporting it or is deleted. (A new process of an incremental build reports only
//! the files it transforms, as roblox-ts's does; their duplicates are dropped among them.)
//!
//! **Names.** Temporary identifiers plugins ask for (`temp_id`, `hoist`, `hoist_spillable`, `push_to_var`) are
//! named so that no identifier of the file, no name `is_usable_local_name` approved, and no plugin import can capture
//! them (a hoist or import is declared at the top of the file and may be used in any nested function); approved
//! names and import names avoid the seam temporaries made before them in turn. Plain names (`hoist_spillable_named`,
//! an `import_named` local) follow TypeScript's printer for an optimistic unique name: the name, else `name_1`, …
//! "The file's identifiers" are TypeScript 5's `sourceFile.identifiers` (`rbxts_plugin::ts5_file_identifiers`:
//! JSDoc identifiers and the literals its parser interns included). roblox-ts's own temporaries keep its naming.
//!
//! **Placement, as TypeScript prints a TS transformer's output.** Before a statement of a statement list
//! (`dispatch::before_statement`): the file's header comment (detached comments) for the first statement, then the
//! root insertions of a top-level statement, the `insert_before_statement` statements (in TypeScript's visit order),
//! a plugin comment that leads the statement's Luau, then the statement's own leading comments. Plugin imports go
//! after the header comment, one declaration per path as Flamework's transform adds them, ordered by the position of
//! the node that asked for them, and named in that order. In a build with plugins a numeric literal with
//! separators is its value's text, as the reprint of stock roblox-ts gives (`transform_numeric_literal`).
//!
//! **Luau's limits** (`spill`). Every file a plugin emitted into is compiled by Luau 0.740 (`rbxts_luau_check`,
//! pinned, built from source) once rendered, files in parallel, its `--!optimize` directive honoured: refused at
//! optimisation level 1 or 2 with debug level 1 (Studio's default, live games) is an error quoting Luau's message
//! (local registers, registers, upvalues, nesting depth), at the TS statement holding the line Luau names (a
//! function's: the main chunk's statement containing it), and the file is not written; refused only at
//! `--!optimize 0` or debug level 2 is a warning (it loads only with Luau's constant folding). Before that, a cheap
//! pre-count of the main chunk (every local name, `local exports` and `local TS` included, and the registers an
//! expression holds) spills the `hoist_spillable` locals into one table local (`_spill`) past 200 locals or 255
//! registers. Other files, and builds without plugins, keep roblox-ts's behaviour.

pub mod backend;
pub mod dispatch;
pub(crate) mod file_state;
pub(crate) mod host;
pub mod lifecycle;

mod facts;
mod hoist_check;
mod lowering;
mod luau_walk;
pub(crate) mod spill;

/// The source a plugin diagnostic names (`PluginDiagnostic::source`, else its plugin's `diagnostic_source()`), for
/// the reporter's code slot: `error TS <source>: …`, also for one a new process made again from the incremental state
/// ([`crate::build::replay`]). `None` for every other diagnostic.
pub fn diagnostic_source(diagnostic: tsrs_core::P<tsrs_ast::Diagnostic>) -> Option<&'static str> {
    backend::diagnostic_source(diagnostic).or_else(|| crate::build::replay::source(diagnostic))
}

pub use host::PluginHost;
pub use lifecycle::{BuildInput, PluginOptions, PluginTimings};

/// The hooks a plugin has, for timings and messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookKind {
    /// `setup`.
    Setup,
    /// `classify_symbol`.
    Classify,
    /// `begin_file`.
    BeginFile,
    /// `call`.
    Call,
    /// `property_key`.
    PropertyKey,
    /// `assignment`.
    Assignment,
    /// `class`.
    Class,
    /// `expression`.
    Expression,
    /// `statement`.
    Statement,
    /// `end_file`.
    EndFile,
    /// `explain`.
    Explain,
    /// `finish`.
    Finish,
}

impl HookKind {
    /// Every hook kind, in report order.
    pub const ALL: [HookKind; 12] = [
        HookKind::Setup,
        HookKind::Classify,
        HookKind::BeginFile,
        HookKind::Call,
        HookKind::PropertyKey,
        HookKind::Assignment,
        HookKind::Class,
        HookKind::Expression,
        HookKind::Statement,
        HookKind::EndFile,
        HookKind::Explain,
        HookKind::Finish,
    ];

    /// The hook's name as plugins know it.
    pub fn label(self) -> &'static str {
        match self {
            HookKind::Setup => "setup",
            HookKind::Classify => "classify_symbol",
            HookKind::BeginFile => "begin_file",
            HookKind::Call => "call",
            HookKind::PropertyKey => "property_key",
            HookKind::Assignment => "assignment",
            HookKind::Class => "class",
            HookKind::Expression => "expression",
            HookKind::Statement => "statement",
            HookKind::EndFile => "end_file",
            HookKind::Explain => "explain_checker_diagnostic",
            HookKind::Finish => "finish",
        }
    }

    pub(crate) fn index(self) -> usize {
        HookKind::ALL.iter().position(|&k| k == self).expect("a hook kind")
    }
}
