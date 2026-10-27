//! `nodes/expressions/transformStringLiteral.ts`

use rbxts_luau::{Expr, Quote, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::util::create_string_from_literal::create_string_from_literal;
use crate::util::traversal::get_text;

/// A string literal, keeping its quote character.
pub fn transform_string_literal(node: P<Node>) -> Expr {
    let quote = if get_text(node).starts_with('\'') { Quote::Single } else { Quote::Double };
    create::string_quoted(create_string_from_literal(node), quote)
}
