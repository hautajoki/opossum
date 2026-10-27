//! `nodes/expressions/transformConditionalExpression.ts`

use rbxts_luau::{AssignmentOperator, ElseBody, Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::create_truthiness_checks::create_truthiness_checks;
use crate::util::symbols::is_used_as_statement;
use crate::util::wrap_expression_statement::wrap_expression_statement;

/// `a ? b : c`: a Luau if-expression when neither branch has prerequisites, else an if statement (assigning a
/// temporary when the value is used).
pub fn transform_conditional_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let conditional = node.as_conditional_expression();
    let condition = transform_expression(state, prereqs, conditional.condition);
    let mut when_true_prereqs = Prereqs::new();
    let when_true = transform_expression(state, &mut when_true_prereqs, conditional.when_true);
    let mut when_false_prereqs = Prereqs::new();
    let when_false = transform_expression(state, &mut when_false_prereqs, conditional.when_false);

    if is_used_as_statement(node) {
        when_true_prereqs.push_list(wrap_expression_statement(when_true));
        when_false_prereqs.push_list(wrap_expression_statement(when_false));
        let condition = create_truthiness_checks(state, prereqs, condition, conditional.condition);
        prereqs.push(create::if_stmt(
            condition,
            when_true_prereqs.statements,
            ElseBody::Block(when_false_prereqs.statements),
        ));
        return Expr::None;
    }

    if when_true_prereqs.statements.is_empty() && when_false_prereqs.statements.is_empty() {
        let condition = create_truthiness_checks(state, prereqs, condition, conditional.condition);
        return create::if_expr(condition, when_true, when_false);
    }

    let temp = temp_id("result");
    prereqs.push(create::local(temp.clone(), None));
    let temp = Expr::TemporaryIdentifier(temp);

    when_true_prereqs.push(create::assignment(temp.clone(), AssignmentOperator::Assign, when_true));
    when_false_prereqs.push(create::assignment(temp.clone(), AssignmentOperator::Assign, when_false));

    let condition = create_truthiness_checks(state, prereqs, condition, conditional.condition);
    prereqs.push(create::if_stmt(
        condition,
        when_true_prereqs.statements,
        ElseBody::Block(when_false_prereqs.statements),
    ));

    temp
}
