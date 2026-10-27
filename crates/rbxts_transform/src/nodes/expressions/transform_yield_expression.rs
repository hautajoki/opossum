//! `nodes/expressions/transformYieldExpression.ts`

use rbxts_luau::{AssignmentOperator, ElseBody, Expr, create, globals};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::symbols::is_used_as_statement;

/// `yield x`: `coroutine.yield(x)`. `yield* it` steps the delegate iterator in a loop, passing each received value
/// on, and evaluates to the delegate's return value.
pub fn transform_yield_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let yield_expression = node.as_yield_expression();
    let Some(expression_node) = yield_expression.expression else {
        return create::call(globals::coroutine::r#yield(), Vec::new());
    };

    let expression = transform_expression(state, prereqs, expression_node);
    if yield_expression.asterisk_token.is_none() {
        return create::call(globals::coroutine::r#yield(), vec![expression]);
    }

    // delegation keeps the original next function even if the iterator is later modified
    let next_id = prereqs.push_to_var(create::property(convert_to_indexable_expression(expression), "next"), "next");
    let current_input_id = prereqs.push_to_var_empty("currentInput");
    let result_id = temp_id("result");
    let result = Expr::TemporaryIdentifier(result_id.clone());

    let mut finalizer = vec![create::break_stmt()];
    let mut evaluated = Expr::None;

    if !is_used_as_statement(node) {
        let return_value = prereqs.push_to_var_empty("returnValue");
        finalizer.insert(
            0,
            create::assignment(
                return_value.clone(),
                AssignmentOperator::Assign,
                create::property(result.clone(), "value"),
            ),
        );
        evaluated = return_value;
    }

    let yield_call = create::call(globals::coroutine::r#yield(), vec![create::property(result.clone(), "value")]);

    prereqs.push(create::while_stmt(
        create::bool(true),
        vec![
            create::local(result_id, Some(create::call(next_id, vec![current_input_id.clone()]))),
            create::if_stmt(create::property(result, "done"), finalizer, ElseBody::default()),
            create::assignment(current_input_id, AssignmentOperator::Assign, yield_call),
        ],
    ));

    evaluated
}
