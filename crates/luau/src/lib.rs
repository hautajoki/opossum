//! Luau syntax tree and renderer: a port of `@roblox-ts/luau-ast` 2.1.2, rendering byte for byte as roblox-ts
//! does (checked on real trees by `rbxts-bench luau-parity`).
//!
//! - [`ast`]: the node types ([`Expr`], [`Stmt`] and their structs), plain owned data with no parent links, so a
//!   tree is `Send` and can be rendered on any thread.
//! - [`create`]: builders mirroring `luau.create` / `luau.id` / `luau.call` …, and [`TempIds`] (`luau.tempId`).
//! - [`globals`]: `luau.globals` and `luau.strings`.
//! - [`facts`]: the evaluation facts roblox-ts records on nodes (`TSTransformer/util/evaluation/facts.ts`).
//! - [`render()`]: `renderAST`, with [`LateStrings`] for string literals whose text is known only after the
//!   transform (DESIGN §4 step 5).
//! - name and number rules: [`is_valid_identifier`], [`is_metamethod`], [`is_reserved_identifier`],
//!   [`is_valid_number_literal`], and JavaScript's number conversions ([`js_number_to_string`]).
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
pub mod create;
pub mod facts;
pub mod globals;
mod render;
mod solve;
mod util;

pub use ast::*;
pub use create::TempIds;
pub use facts::{Binding, BindingKey, Effects, Facts};
pub use render::{LateStrings, render, render_ast, render_expr, render_into, render_with_lines};
pub use util::{
    LUAU_METAMETHODS, LUAU_RESERVED_CLASS_FIELDS, LUAU_RESERVED_KEYWORDS, RESERVED_IDENTIFIERS, is_js_whitespace,
    is_metamethod, is_reserved_class_field, is_reserved_identifier, is_valid_identifier, is_valid_number_literal,
    js_number_to_string, js_string_to_number,
};

#[cfg(test)]
mod tests;
