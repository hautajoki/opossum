//! `nodes/expressions/transformTypeExpression.ts`

use rbxts_luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;

/// `x as T`, `x!`, `x satisfies T`, `<T>x`, `x<T>`: the expression alone.
pub fn transform_type_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let expression = node.expression().expect("a type expression wraps an expression");
    transform_expression(state, prereqs, expression)
}
