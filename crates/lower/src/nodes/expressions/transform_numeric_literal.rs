//! `nodes/expressions/transformNumericLiteral.ts`

use luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::util::traversal::get_text;

/// A number literal, as written.
pub fn transform_numeric_literal(node: P<Node>) -> Expr {
    create::number_text(get_text(node))
}
