//! `nodes/statements/transformWhileStatement.ts`

use rbxts_luau::{ElseBody, Stmt, UnaryOperator, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_statement_list::transform_body;
use crate::util::create_truthiness_checks::create_truthiness_checks;

/// `transformWhileStatement`: a condition with prerequisites moves into the loop as `if not condition then break end`.
pub fn transform_while_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let while_statement = node.as_while_statement();
    let (expression, statement) = (while_statement.expression, while_statement.statement());
    let mut while_statements = Vec::new();

    let mut condition_prereqs = Prereqs::new();
    let condition = transform_expression(state, &mut condition_prereqs, expression);
    let mut condition_exp = create_truthiness_checks(state, &mut condition_prereqs, condition, expression);

    if !condition_prereqs.statements.is_empty() {
        while_statements.extend(condition_prereqs.statements);
        while_statements.push(create::if_stmt(
            create::unary(UnaryOperator::Not, condition_exp),
            vec![create::break_stmt()],
            ElseBody::default(),
        ));
        condition_exp = create::bool(true);
    }

    while_statements.extend(transform_body(state, statement));

    vec![create::while_stmt(condition_exp, while_statements)]
}
