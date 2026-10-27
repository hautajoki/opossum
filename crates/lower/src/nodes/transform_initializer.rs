//! `nodes/transformInitializer.ts`

use rbxts_luau::{AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;

/// `if id == nil then id = initializer end`
pub fn transform_initializer(state: &mut TransformState<'_>, id: Expr, initializer: P<Node>) -> Stmt {
    let mut initializer_prereqs = Prereqs::new();
    let value = transform_expression(state, &mut initializer_prereqs, initializer);
    let mut statements = initializer_prereqs.statements;
    statements.push(create::assignment(id.clone(), AssignmentOperator::Assign, value));
    create::if_stmt(create::binary(id, BinaryOperator::Eq, Expr::Nil), statements, ElseBody::default())
}
