//! `nodes/statements/transformDoStatement.ts`

use rbxts_luau::{Stmt, UnaryOperator, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_statement_list::transform_body;
use crate::util::create_truthiness_checks::create_truthiness_checks;

/// `transformDoStatement`: `repeat do … end until not condition`. The body sits in a `do` block so its locals are out
/// of scope in the condition, as in JavaScript.
pub fn transform_do_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let do_statement = node.as_do_statement();
    let (mut expression, statement) = (do_statement.expression, do_statement.statement());

    let statements = transform_body(state, statement);

    let mut condition_is_inverted_in_luau = true;
    if expression.kind() == Kind::PrefixUnaryExpression {
        let unary = expression.as_prefix_unary_expression();
        if unary.operator == Kind::ExclamationToken {
            expression = unary.operand;
            condition_is_inverted_in_luau = false;
        }
    }

    let mut condition_prereqs = Prereqs::new();
    let transformed = transform_expression(state, &mut condition_prereqs, expression);
    let condition = create_truthiness_checks(state, &mut condition_prereqs, transformed, expression);

    let mut repeat_statements = vec![create::do_stmt(statements)];
    repeat_statements.extend(condition_prereqs.statements);

    let condition =
        if condition_is_inverted_in_luau { create::unary(UnaryOperator::Not, condition) } else { condition };
    vec![create::repeat_stmt(repeat_statements, condition)]
}
