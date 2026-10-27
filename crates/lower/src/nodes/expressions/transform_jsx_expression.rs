//! `nodes/expressions/transformJsxExpression.ts`

use rbxts_luau::{Expr, create, globals};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;

/// `transformJsxExpression`: `{expression}` as its value, `{...expression}` as `unpack(expression)`.
pub fn transform_jsx_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let jsx_expression = node.as_jsx_expression();
    if let Some(inner) = jsx_expression.expression() {
        let expression = transform_expression(state, prereqs, inner);
        if jsx_expression.dot_dot_dot_token().is_some() {
            return create::call(globals::unpack(), vec![expression]);
        }
        return expression;
    }
    create::none()
}
