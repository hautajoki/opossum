//! The plugin API of rbxts-rs (DESIGN §11, with the build lifecycle of §14).
//!
//! A plugin is Rust compiled into the user's build. It has three parts:
//! - a **factory** ([`Plugin`]), one per build tool, `Send + Sync`. It builds the read-only [`Plugin::Shared`]
//!   state in `setup`, owns no mutable state of its own, and merges the file parts in `finish`;
//! - one **instance** ([`PluginInstance`]) per checker and build, made on that checker's thread and never `Send`.
//!   Its hooks run during the transform and see original parse-tree nodes;
//! - one **file part** ([`Plugin::FilePart`]) per transformed file, `Send`, handed back to the factory in program
//!   order. Cross-file state lives only here, so output never depends on the checker count.
//!
//! The compiler implements the services hooks call through the backend traits [`CxBackend`], [`SetupBackend`] and
//! [`FinishBackend`]. Plugins use the wrappers [`Cx`], [`SetupCx`] and [`FinishCx`], which add what does not need
//! the compiler: [`Abort`] values, Luau escaping of late strings, prereq capture.
//!
//! [`erased`] turns a typed plugin into trait objects for the compiler's plugin list (DESIGN §11.6).
//!
//! The crate depends on `rbxts_luau`, `rbxts_project` and tsrs's handle and checker types only, never on the
//! compiler, so the compiler can depend on it.
//!
//! # What the compiler offers, and when
//!
//! **The rule.** A TS transformer sees every node before roblox-ts does. A compiled-in plugin is offered every node
//! roblox-ts lowers as a node of its own, in the position it lowers it, before roblox-ts lowers it (macros included),
//! whichever path of roblox-ts's lowering reaches it: chain links, optional calls, `super` calls, block bodies,
//! `else if`, dotted JSX tags. What roblox-ts lowers only as a part of a larger form is offered with that form, which
//! has the lowering to replace (see "Parts of a form" below).
//!
//! **Offer kinds.** There are five: calls (with `new` and decorators, [`PluginInstance::call`]); property keys
//! ([`PluginInstance::property_key`]); assignments ([`PluginInstance::assignment`]); the expression and statement
//! fallbacks ([`PluginInstance::expression`], [`PluginInstance::statement`]); and the class hook. Expression hooks are
//! asked about a node first, whatever the registration order, then the call or assignment hooks; within a kind,
//! plugins are asked in registration order and the first `Take` wins. So a call can get an expression-kind offer and
//! then, after a clean decline, a call offer.
//!
//! **Calls.**
//! - A chain's first call reached through plain accesses only (`f()`, `ns.f()`, `a.b.m()`, `super.m()`) is offered
//!   before anything of it is compiled.
//! - Any other chain call is offered with its receiver or callee already compiled, and nil-checked only when it is
//!   inside an optional chain. That includes calls after a `?.` link or after another call, such as `g()()` and
//!   `f?.()`.
//! - An optional call (`o.m?.(…)`, `o[k]?.(…)`, `o?.m?.(…)`, `super.m?.(…)`) is offered inside the nil check on its
//!   callee (`o.m`): the claimed value is used only when `o.m` is not nil, and nothing of the claim (its arguments
//!   included) runs otherwise. A method call's receiver is already in a local; a non-method call's receiver
//!   (`o.f?.()` where `f` is a function property) gets one from [`Cx::compile_receiver`], declared before the callee is
//!   read from it. Either way the receiver is evaluated once.
//! - `super.m(…)` and `super[k](…)` have the `super` keyword as their [`CallSite::receiver`], but `super` is not a
//!   value: roblox-ts calls `super.m(self, …)`. [`Cx::compile_receiver`] on such a call is an internal error naming the
//!   plugin; `compile_expr` of the call and `compile_call(call, Callee::Original, …)` lower it as roblox-ts does, with
//!   `self` first. Their callee key reaches `property_key` like any method callee's.
//! - `new` is offered before constructor macros. Decorators are offered as [`CallKind::Decorator`] before their
//!   expression is compiled; the returned value replaces the decorator expression, which is then applied
//!   (`Class = value(Class) or Class`). The call inside `@D(…)` is offered as the decorator, not again as a call.
//!
//! **The expression fallback** ([`Interest::expressions`]) sees every node of its kinds that roblox-ts lowers as an
//! expression:
//! - every link of a chain, accesses and calls: the links up to the chain's first `?.` link or optional call are
//!   offered before anything of the chain is compiled, from the outermost in (the chain's first call goes to the call
//!   plugins after its own expression offer); later links are offered inside the chain's nil check, with their base
//!   compiled. A hook that takes such a link without lowering it still has its base evaluated, once, and drops the
//!   base's value;
//! - an expression statement's assignment, `++` / `--` or logical assignment, though roblox-ts lowers it as a
//!   statement;
//! - a dotted JSX tag (`<a.B />`), which roblox-ts lowers as an access (a claim's prerequisites go before the element).
//!
//! **The statement fallback** ([`Interest::statements`]) sees every statement of its kinds that roblox-ts lowers,
//! before it lowers it: every statement of a statement list, those roblox-ts emits nothing for included (types,
//! `declare`); the block body of an `if`, a loop or a `case` clause and a class static block (TypeScript's
//! `isStatement`), which roblox-ts lowers as their statement lists: a claim is the body; and an `else if`, which
//! roblox-ts lowers into the `if` around it: a claim is the `else` block.
//!
//! **Positions other offer kinds cover.** A node in these positions is part of another node's offer, which is where a
//! plugin rewrites it:
//! - a call's callee (`o.m` in `o.m()`): the call is offered (call and expression hooks), and its key to
//!   `property_key` ([`AccessPosition::Callee`]);
//! - an assignment target, a `++` / `--` operand and a `delete` operand (lowered as writes): the assignment hook,
//!   and `property_key` ([`AccessPosition::Write`], [`AccessPosition::ReadWrite`], [`AccessPosition::Delete`]);
//! - binding and assignment patterns (`const [a] = x`, `[a.b] = x`, `for (const { k } of …)`): their default values
//!   and computed keys are expressions and offered as such; their targets are written, not read;
//! - object literals a hook lowers with [`Cx::compile_object_literal`]: their values are offered, not the literal.
//!
//! **Parts of a form.** roblox-ts lowers some nodes only as parts of a larger form, never as nodes of their own, so a
//! plugin rewrites the form, which is offered: the `$tuple(…)` call of `return $tuple(…)` and the `$range(…)` of a
//! `for … of` (lowered with their statement: their arguments are offered), the head of a `for` loop roblox-ts turns
//! into a numeric `for` (`--optimizedLoops`), the inner nodes of a chain of one logical or bitwise operator
//! (`a && b && c` is one condition: the outer node and the operands are offered), the elements of an array literal it
//! spreads inline (`[...[1, 2]]`), the arguments of constructor macros (`new Map([[k, v]])`), constant enum members
//! and their parts (`E.A` is its value), the parentheses and type assertions it looks through where it looks through
//! them (`(x)`, `x as T`, `x!`: the expression inside is offered), the inner names of a dotted namespace
//! (`namespace A.B`), the statements of a namespace that holds no values (types only), and statements after a
//! `return`, `break` or `continue` in the same list (roblox-ts drops them). Offering these as nodes of their own would
//! change roblox-ts's lowering of the form for a plugin that only lowers them as roblox-ts does (an identity plugin),
//! and declining or lowering as roblox-ts does must change no output.
//!
//! **Compiling a hook's own node.** While a node's own offer runs, compiling it does not re-offer it to that kind;
//! the other kinds still see it. An expression hook that compiles its own call lets the call plugins claim that
//! call.
//! - `compile_expr`, `compile_in_order` or an `Arg::Original` slot on the hook's own node lowers it as roblox-ts
//!   does there, macros included. A call is lowered on the receiver or callee already compiled, so a single
//!   self-compile evaluates nothing twice. A decorator's own node lowers the decorator's expression. An
//!   expression-hook node offered inside a chain is lowered on its compiled base. An assignment, `++` / `--` or
//!   logical assignment whose value is unused, which roblox-ts lowers as a statement, is lowered as that statement,
//!   and the value is `nil` (return it as it is: it adds nothing).
//! - Lower your own node at most once, by exactly one of `compile_expr` / `compile_in_order` / `Arg::Original` /
//!   `compile_call`. The compiler refuses, each an internal error naming the plugin: a second self-compile,
//!   `compile_call` twice, `compile_call` after a self-compile and a self-compile after `compile_call`; a class hook
//!   compiling its own class expression; a statement hook compiling its own statement (`compile_*` takes
//!   expressions); compiling a node whose value the offer compiled already (a call's receiver or callee once
//!   compiled: use [`Cx::compile_receiver`]; the base of a chain link offered inside a chain), or a node inside or
//!   around it; an assignment hook compiling a `delete` operand (roblox-ts lowers it as the deletion).
//! - Compiling an ancestor of the hook's own node would offer and lower every other node inside it again: refused,
//!   an internal error naming the plugin.
//!
//! **Asked once.** A node every hook of a kind declined is not offered to that kind again, however often it is
//! lowered: a call hook that compiles its own call does not ask the expression hooks about it again, and neither does
//! roblox-ts lowering a node twice. `property_key` is asked once per access node; every lowering of it uses that
//! answer.
//!
//! **Declining.** A claim hook (`call`, `assignment`, `property_key`, `expression`, `statement`) cannot decline its
//! node after any of these:
//! - it lowered the node (`compile_expr` / `compile_in_order` of the node, `compile_call`);
//! - it compiled the receiver inside a `capture`;
//! - it left anything behind in the scope it writes into or the file, whether its own or a nested hook's:
//!   prereqs, a hoist, a root insertion, statements before its TS statement (`insert_before_statement`), a header
//!   statement, a plugin import, a late string, a runtime-library member (`runtime_member`), or an import kept
//!   alive (`mark_import_used`, or a user import `import_named` reused). `request_import_elision` is harmless and
//!   allowed;
//! - any plugin hook ran inside it, through its `compile_*` calls: a nested node offered (claimed or declined), a
//!   property key asked, or a class hook. That hook would run again.
//!
//! The one exception is a `compile_receiver` into the hook's own scope that is the first thing to grow that scope, or
//! the local it binds for an optional call's receiver. What it left (statements, and the hooks it ran) is kept for the
//! next plugin, or roblox-ts, to reuse. An earlier prereq or nested hook voids the exception. A decline that breaks
//! the rule is an internal error naming the plugin. The offer ends there: no other plugin of any kind, and not
//! roblox-ts, gets the node; it becomes `nil` (or an empty statement list), and the build fails.
//!
//! Plugins interact through this rule: plugin B's interest in a node nested in plugin A's node (even one B
//! declines cleanly) turns A's compile-then-decline into an error. So a plugin must decide from the AST and types
//! before compiling anything, as Flamework's TS transformer does.
//!
//! A claimed value whose node is in statement position is evaluated as a statement (`local _ = value` unless it is a
//! call). Inside parentheses roblox-ts lowers as such (a `for` head's `(x = 0)`), it is returned as it is, and the
//! statement around the parentheses evaluates it (`local _ = (x)`), as without plugins.
//!
//! **`RBXTS_PLUGIN_CLAIMS=all`** re-asks the other matching plugins of call, `new` and decorator offers, in a
//! scratch scope; it does not re-ask the other kinds. Two `Take`s are an error. A build with the switch either writes
//! the same output as without it, or fails. It fails when a re-asked plugin changed compiler state before declining
//! (any lowering, placement, import, late string, temporary, diagnostic, or an `is_usable_local_name` approval) or
//! ran a nested hook. A hook's own instance state is out of the compiler's reach, which is one more reason to decide
//! before doing anything.
//!
//! **Placement.** In each file: the `--!` directives, the `-- Compiled with` line, `local TS = require(...)`, plugin
//! imports, the user's own imports, then header statements ([`Cx::header`]).
//!
//! # Build inputs
//!
//! Whatever a plugin's output depends on besides the program is read through the compiler, which records it as a
//! build input and rebuilds what depends on it when it changes (DESIGN §14):
//! - files: [`SetupCx::read_input`];
//! - environment variables: [`SetupCx::env`] in setup, [`Cx::env`] in a file's hooks;
//! - globs: `record_glob` on [`SetupCx`], [`Cx`] and [`FinishCx`], with a [`GlobMatcher`] the compiler can call
//!   again in watch;
//! - other disk reads in `finish`: [`FinishCx::read_dir`], [`FinishCx::exists`], [`FinishCx::read_file`].
//!
//! What a change re-runs follows from where it was read: a setup input re-runs setup and transforms every file; a
//! file's input re-transforms that file (then `finish`); a finish glob re-runs `finish` only.
//!
//! **File parts across processes.** [`Plugin::encode_file_part`] / [`Plugin::decode_file_part`] let the compiler
//! keep a file's part in its incremental state on disk. A plugin that does not implement them (the default) has its
//! cached files transformed again in a new process.

mod cx;
pub mod erased;
mod escape;
mod finish_cx;
mod glob;
mod identifiers;
mod interest;
mod lifecycle;
mod plugin;
mod setup_cx;
mod types;

pub use cx::{Aborted, Cx, CxBackend};
pub use escape::{escape_luau_string, synthesized_string, text_literal};
pub use finish_cx::{DirEntry, EntryKind, FinishBackend, FinishCx};
pub use glob::GlobMatcher;
pub use identifiers::ts5_file_identifiers;
pub use interest::{ClassFilter, Interest, ReceiverInterest};
pub use lifecycle::{BuildKind, ChangeSet, RebuildPlan, StateOrigin};
pub use plugin::{Plugin, PluginInstance};
pub use setup_cx::{SetupBackend, SetupCx};
pub use types::*;

/// A Luau expression (`rbxts_luau::Expr`).
pub type LuauExpr = rbxts_luau::Expr;
/// A Luau statement (`rbxts_luau::Stmt`).
pub type LuauStmt = rbxts_luau::Stmt;
/// A unique local name chosen at render time (`luau.tempId`).
pub type LuauIdent = rbxts_luau::TemporaryIdentifier;
/// A table key in [`ObjectEdits`]: usually a string literal ([`text_literal`]) or a late string.
pub type LuauKey = rbxts_luau::Expr;

/// The handle and checker types hooks receive, re-exported so a plugin needs no direct tsrs dependency for them.
pub mod ts {
    pub use tsrs_ast::{Diagnostic, Kind, Node, SourceFile, Symbol};
    pub use tsrs_checker::{Checker, Signature, Type};
    pub use tsrs_compiler::Program;
    pub use tsrs_core::{CompilerOptions, P};
    pub use tsrs_diagnostics::Category;
}
