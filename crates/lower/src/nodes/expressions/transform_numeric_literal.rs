//! `nodes/expressions/transformNumericLiteral.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_ast::tokenflags::TokenFlags;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::traversal::get_text;

/// A number literal, as written.
///
/// rbxts-rs: in a build with plugins, a literal with numeric separators (`1_000_000`) is its value's text
/// (`1000000`, `0xFF_FF` as `65535`). Stock roblox-ts reprints every file once any TS transformer runs, and
/// TypeScript's printer (`createPrinter()`, no `target`) cannot keep separators: it prints the scanner's value
/// (`canUseOriginalText`, `getLiteralText`), which tsrs keeps the same way. Without transformers stock keeps the source
/// text, and so does this.
pub fn transform_numeric_literal(state: &TransformState<'_>, node: P<Node>) -> Expr {
    if state.plugins.is_some() && node.as_numeric_literal().token_flags().intersects(TokenFlags::ContainsSeparator) {
        return create::number_text(node.text());
    }
    create::number_text(get_text(node))
}
