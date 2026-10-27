//! `nodes/statements/transformExpressionStatement.ts`

use luau::{AssignmentOperator, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_logical_or_coalescing_assignment_expression::transform_logical_or_coalescing_assignment_expression_statement;
use crate::nodes::transform_writable::{transform_writable_assignment, transform_writable_expression};
use crate::util::create_binary_from_operator::{
    create_assignment_statement, create_compound_assignment_statement, get_assignable_value,
    get_simple_assignment_operator,
};
use crate::util::traversal::skip_downwards;
use crate::util::wrap_expression_statement::wrap_expression_statement;

fn unary_parts(node: P<Node>) -> Option<(P<Node>, Kind)> {
    match node.kind() {
        Kind::PrefixUnaryExpression => {
            let unary = node.as_prefix_unary_expression();
            Some((unary.operand, unary.operator))
        }
        Kind::PostfixUnaryExpression => {
            let unary = node.as_postfix_unary_expression();
            Some((unary.operand, unary.operator))
        }
        _ => None,
    }
}

/// `transformExpressionStatementInner`: an expression whose value is discarded.
pub fn transform_expression_statement_inner(state: &mut TransformState<'_>, expression: P<Node>) -> Vec<Stmt> {
    let mut prereqs = Prereqs::new();

    if expression.kind() == Kind::BinaryExpression {
        let binary = expression.as_binary_expression();
        let operator_kind = binary.operator_token.kind();
        let (left, right) = (binary.left, binary.right());
        if tsrs_ast::is_logical_or_coalescing_assignment_expression(expression) {
            return transform_logical_or_coalescing_assignment_expression_statement(state, expression);
        } else if tsrs_ast::is_assignment_operator(operator_kind)
            && left.kind() != Kind::ArrayLiteralExpression
            && left.kind() != Kind::ObjectLiteralExpression
        {
            let writable_type = state.get_type(left);
            let value_type = state.get_type(right);
            let operator = get_simple_assignment_operator(state, writable_type, operator_kind, value_type);
            let assignment = transform_writable_assignment(
                state,
                &mut prereqs,
                left,
                right,
                operator.is_none(),
                operator != Some(AssignmentOperator::Assign),
            );

            let statement = match operator {
                Some(operator) => {
                    let value = get_assignable_value(state, operator, assignment.value, value_type);
                    create_assignment_statement(
                        assignment.writable,
                        operator,
                        value,
                        assignment.readable,
                        assignment.readable_is_writable,
                    )
                }
                None => create_compound_assignment_statement(
                    state,
                    &mut prereqs,
                    assignment.writable,
                    writable_type,
                    assignment.readable,
                    operator_kind,
                    assignment.value,
                    value_type,
                ),
            };

            let mut statements = prereqs.statements;
            statements.push(statement);
            return statements;
        }
    } else if let Some((operand, operator)) = unary_parts(expression)
        && matches!(operator, Kind::PlusPlusToken | Kind::MinusMinusToken)
    {
        let writable = transform_writable_expression(state, &mut prereqs, operand, false);
        let operator = if operator == Kind::PlusPlusToken { AssignmentOperator::Add } else { AssignmentOperator::Sub };
        let assignment = create::assignment(writable, operator, create::number(1.0));
        let mut statements = prereqs.statements;
        statements.push(assignment);
        return statements;
    }

    let transformed = transform_expression(state, &mut prereqs, expression);
    let expression_statements = wrap_expression_statement(transformed);
    let mut statements = prereqs.statements;
    statements.extend(expression_statements);
    statements
}

/// `transformExpressionStatement`
pub fn transform_expression_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let expression = skip_downwards(node.expression().expect("an expression statement has an expression"));
    transform_expression_statement_inner(state, expression)
}
