//! `nodes/expressions/transformUnaryExpression.ts`

use rbxts_luau::{AssignmentOperator, Expr, UnaryOperator, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_writable::transform_writable_expression;
use crate::shared::diagnostics::errors;
use crate::util::create_truthiness_checks::create_truthiness_checks;
use crate::util::types::{is_definitely_type, is_number_type};
use crate::util::validate_not_any::validate_not_any_type;

fn update_operator(kind: Kind) -> AssignmentOperator {
    match kind {
        Kind::PlusPlusToken => AssignmentOperator::Add,
        Kind::MinusMinusToken => AssignmentOperator::Sub,
        other => panic!("Unexpected update operator {other:?}"),
    }
}

/// `x++` / `x--` used as a value: the original value.
pub fn transform_postfix_unary_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    let unary = node.as_postfix_unary_expression();
    validate_not_any_type(state, unary.operand);

    // rbxts-rs: an update of `receiver.key` a plugin claims (DESIGN §11.5)
    if let Some(claimed) = crate::plugins::dispatch::claim_assignment(state, prereqs, node) {
        return claimed;
    }

    let writable = transform_writable_expression(state, prereqs, unary.operand, true);
    let orig_value = temp_id("original");
    let operator = update_operator(unary.operator);

    prereqs.push(create::local(orig_value.clone(), Some(writable.clone())));
    prereqs.push(create::assignment(writable, operator, create::number(1.0)));

    Expr::TemporaryIdentifier(orig_value)
}

/// `++x`, `--x`, `+x`, `-x`, `!x`, `~x`
pub fn transform_prefix_unary_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let unary = node.as_prefix_unary_expression();
    validate_not_any_type(state, unary.operand);

    match unary.operator {
        Kind::PlusPlusToken | Kind::MinusMinusToken => {
            // rbxts-rs: an update of `receiver.key` a plugin claims (DESIGN §11.5)
            if let Some(claimed) = crate::plugins::dispatch::claim_assignment(state, prereqs, node) {
                return claimed;
            }
            let writable = transform_writable_expression(state, prereqs, unary.operand, true);
            prereqs.push(create::assignment(writable.clone(), update_operator(unary.operator), create::number(1.0)));
            writable
        }
        Kind::PlusToken => {
            state.add_diagnostic(errors::no_unary_plus(node));
            transform_expression(state, prereqs, unary.operand)
        }
        Kind::MinusToken => {
            let operand_type = state.get_type(unary.operand);
            if !is_definitely_type(state, operand_type, &[&is_number_type]) {
                state.add_diagnostic(errors::no_non_number_unary_minus(node));
            }
            create::unary(UnaryOperator::Neg, transform_expression(state, prereqs, unary.operand))
        }
        Kind::ExclamationToken => {
            let operand = transform_expression(state, prereqs, unary.operand);
            let checks = create_truthiness_checks(state, prereqs, operand, unary.operand);
            create::unary(UnaryOperator::Not, checks)
        }
        Kind::TildeToken => {
            let operand = transform_expression(state, prereqs, unary.operand);
            create::call(create::property(globals::bit32(), "bnot"), vec![operand])
        }
        other => panic!("Unexpected prefix unary operator {other:?}"),
    }
}
