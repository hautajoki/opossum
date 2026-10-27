//! `nodes/expressions/transformParenthesizedExpression.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::traversal::skip_downwards;

/// `(x)`: parentheses are kept unless the inner expression is simple.
pub fn transform_parenthesized_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    let inner = node.expression().expect("parentheses have an expression");
    let expression = transform_expression(state, prereqs, skip_downwards(inner));
    // rbxts-rs: no value (an unused optional chain, `for ((a?.f()); ;)`) stays no value, which a statement drops;
    // roblox-ts wraps it, then fails rendering it ("Cannot render None")
    if expression.is_simple() || expression.is_none() { expression } else { create::paren(expression) }
}
