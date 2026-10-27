//! `nodes/expressions/transformDeleteExpression.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::symbols::is_used_as_statement;

/// `delete x.y`: the property and element access transforms do the deleting (they see the `delete` parent); here
/// only their prerequisites are kept, and the value is `true`.
pub fn transform_delete_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    // rbxts-rs: a `delete receiver.key` a plugin claims (DESIGN §11.5)
    if let Some(claimed) = crate::plugins::dispatch::claim_assignment(state, prereqs, node) {
        return claimed;
    }
    let expression = node.expression().expect("delete has an expression");
    transform_expression(state, prereqs, expression);
    if !is_used_as_statement(node) { create::bool(true) } else { Expr::None }
}
