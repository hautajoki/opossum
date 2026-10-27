//! `nodes/transformLogicalOrCoalescingAssignmentExpression.ts`: `??=`, `&&=` and `||=`.

use rbxts_luau::{AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, UnaryOperator, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_writable::transform_writable_expression;
use crate::util::create_truthiness_checks::create_truthiness_checks;

/// `a ??= b`: `if a == nil then a = b end`.
fn transform_coalescing_assignment_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    left: P<Node>,
    right: P<Node>,
) -> Expr {
    let writable = transform_writable_expression(state, prereqs, left, true);
    let mut value_prereqs = Prereqs::new();
    let value = transform_expression(state, &mut value_prereqs, right);

    let mut if_statements = value_prereqs.statements;
    if_statements.push(create::assignment(writable.clone(), AssignmentOperator::Assign, value));

    prereqs.push(create::if_stmt(
        create::binary(writable.clone(), BinaryOperator::Eq, Expr::Nil),
        if_statements,
        ElseBody::default(),
    ));

    writable
}

/// Which logical assignment [`transform_logical_assignment_expression`] builds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LogicalAssignment {
    And,
    Or,
}

/// `transformLogicalAndAssignmentExpression` / `transformLogicalOrAssignmentExpression` (identical but for the
/// condition): `local condition = a; if [not] <truthy a> then condition = b end; a = condition`.
fn transform_logical_assignment_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    left: P<Node>,
    right: P<Node>,
    kind: LogicalAssignment,
) -> Expr {
    let writable = transform_writable_expression(state, prereqs, left, true);
    let mut value_prereqs = Prereqs::new();
    let value = transform_expression(state, &mut value_prereqs, right);

    let condition_id = prereqs.push_to_var(writable.clone(), "condition");

    let mut if_statements: Vec<Stmt> = value_prereqs.statements;
    if_statements.push(create::assignment(condition_id.clone(), AssignmentOperator::Assign, value));

    let mut condition = create_truthiness_checks(state, prereqs, writable.clone(), left);
    if kind == LogicalAssignment::Or {
        condition = create::unary(UnaryOperator::Not, condition);
    }
    prereqs.push(create::if_stmt(condition, if_statements, ElseBody::default()));

    prereqs.push(create::assignment(writable.clone(), AssignmentOperator::Assign, condition_id));

    writable
}

/// `transformLogicalOrCoalescingAssignmentExpression`: the assignment as prerequisites; the value is the target.
pub fn transform_logical_or_coalescing_assignment_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    let binary = node.as_binary_expression();
    let (left, right) = (binary.left, binary.right());
    match binary.operator_token.kind() {
        Kind::QuestionQuestionEqualsToken => transform_coalescing_assignment_expression(state, prereqs, left, right),
        Kind::AmpersandAmpersandEqualsToken => {
            transform_logical_assignment_expression(state, prereqs, left, right, LogicalAssignment::And)
        }
        _ => transform_logical_assignment_expression(state, prereqs, left, right, LogicalAssignment::Or),
    }
}

/// `transformLogicalOrCoalescingAssignmentExpressionStatement`
pub fn transform_logical_or_coalescing_assignment_expression_statement(
    state: &mut TransformState<'_>,
    node: P<Node>,
) -> Vec<Stmt> {
    let mut prereqs = Prereqs::new();
    transform_logical_or_coalescing_assignment_expression(state, &mut prereqs, node);
    prereqs.statements
}
