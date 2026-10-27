//! `nodes/expressions/transformAwaitExpression.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::traversal::skip_downwards;

/// `await x`: `TS.await(x)`.
pub fn transform_await_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let callee = state.ts(node, "await");
    let expression = node.expression().expect("await has an expression");
    let argument = transform_expression(state, prereqs, skip_downwards(expression));
    create::call(callee, vec![argument])
}
