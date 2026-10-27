//! `nodes/statements/transformThrowStatement.ts`

use rbxts_luau::{Stmt, create, globals};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;

/// `transformThrowStatement`: `error(value)`.
pub fn transform_throw_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut prereqs = Prereqs::new();
    let args = vec![transform_expression(state, &mut prereqs, node.as_throw_statement().expression)];

    let mut statements = prereqs.statements;
    statements.push(create::call_statement(create::call(globals::error(), args)));
    statements
}
