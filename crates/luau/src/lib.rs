//! Luau syntax tree, renderer, name solver and the O2 compile gate.
//!
//! - [`ast`]: the node types ([`Expr`], [`Stmt`] and their structs), plain owned data with no parent links, so a
//!   tree is `Send` and can be rendered on any thread.
//! - [`create`]: builders mirroring `luau.create` / `luau.id` / `luau.call` …, and [`TempIds`] (`luau.tempId`).
//! - [`globals`]: `luau.globals` and `luau.strings`.
//! - [`facts`]: the evaluation facts roblox-ts records on nodes (`TSTransformer/util/evaluation/facts.ts`).
//! - [`render()`]: Luau text from a tree, with requires spelled from the module's instance path ([`require`]).
//! - [`types`]: type syntax for native types: annotations on functions and locals, `type` aliases.
//! - [`gate`]: Luau's own compiler, the O2 gate every emitted file passes, and [`init`], the fast flag policy.
//! - [`vm`]: the Luau VM local tests, benches and exec run in.
//! - passes over a finished tree: [`mark_const_locals`] (`const` for never-assigned locals) and
//!   [`native::mark_native_functions`] (`@native` on every function).
//! - name and number rules: [`is_valid_identifier`], [`is_metamethod`], [`is_reserved_identifier`],
//!   [`is_valid_number_literal`], JavaScript's number conversions ([`js_number_to_string`]), and [`luau_string`], the
//!   string literal escaper for Luau and TypeScript text opossum generates.
//!
//! **Lists.** `luau.List` (a linked list, for O(1) push, unshift and splice) is a `Vec` here. roblox-ts mostly
//! builds lists by `push` / `pushList` (some 250 uses against 13 `unshift` / `unshiftList`), and prepending to a
//! block is rare and short, so `Vec::insert(0, …)` / `splice(0..0, …)` cost less than a linked list's pointer
//! chasing everywhere else. `list.make` is `vec![…]`, `pushList` is `extend` / `append`, `isEmpty` is `is_empty`.
//!
//! **Temporary identifiers.** A [`TemporaryIdentifier`] carries a per-file id from [`TempIds`]; copies with the same
//! id render as the same name, chosen at render time to be unique in its scope (`solveTempIds`).
#![warn(missing_docs)]

pub mod ast;
mod consts;
pub mod create;
pub mod facts;
pub mod gate;
pub mod globals;
pub mod native;
mod render;
pub mod require;
mod solve;
pub mod types;
mod util;
pub mod vm;
mod walk;

pub use ast::*;
pub use consts::mark_const_locals;
pub use create::TempIds;
pub use facts::{Binding, BindingKey, Effects, Facts};
pub use gate::init;
pub use render::{render, render_ast, render_expr, render_into, render_with_lines};
pub use require::{ModulePath, spell};
pub use types::{
    FunctionType, GenericParameter, Indexer, NamedType, Signature, TableType, TableTypeField, Type, TypeAlias, TypePack,
};
pub use util::{
    LUAU_METAMETHODS, LUAU_RESERVED_CLASS_FIELDS, LUAU_RESERVED_KEYWORDS, RESERVED_IDENTIFIERS, is_js_whitespace,
    is_metamethod, is_reserved_class_field, is_reserved_identifier, is_valid_identifier, is_valid_number_literal,
    js_number_to_string, js_string_to_number, luau_string,
};

#[cfg(test)]
mod tests;
