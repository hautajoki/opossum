//! `nodes/expressions/transformElementAccessExpression.ts`

use rbxts_luau::{AssignmentOperator, Effects, Expr, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_optional_chain::transform_optional_chain;
use crate::plugins::backend::keyed_access;
use crate::shared::diagnostics::errors;
use crate::util::add_index_diagnostics::add_index_diagnostics;
use crate::util::add_one_if_array_type::add_one_if_array_type;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::create_string_index_expression::create_string_index_expression;
use crate::util::evaluation::builtins::try_mark_builtin_member;
use crate::util::evaluation::effects::{
    effects_commute, get_effects, get_effects_of_statements, is_late_read, join_effects,
};
use crate::util::evaluation::plan::EvaluationOperand;
use crate::util::get_constant_value_literal::get_constant_value_literal;
use crate::util::get_literal_number_value::get_literal_number_value;
use crate::util::offset::offset;
use crate::util::traversal::skip_upwards;
use crate::util::types::{is_definitely_type, is_lua_tuple_type, is_mixed_string_type, is_string_type};
use crate::util::validate_not_any::validate_not_any_type;
use rbxts_plugin::AccessPosition;

/// `transformElementAccessExpressionInner`: `expression[argument]` once the receiver is transformed.
pub fn transform_element_access_expression_inner(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    mut expression: Expr,
    argument_expression: P<Node>,
) -> Expr {
    let access = node.as_element_access_expression();
    // a in a[b]
    validate_not_any_type(state, access.expression);
    // b in a[b]
    validate_not_any_type(state, access.argument_expression);

    let receiver = state.get_type(access.expression);
    let receiver_type = state.checker.get_non_optional_type(receiver);
    let node_type = state.get_type(node);
    let member_type = state.checker.get_non_optional_type(node_type);
    add_index_diagnostics(state, node, member_type, receiver_type);

    let mut index_prereqs = Prereqs::new();
    let index = transform_expression(state, &mut index_prereqs, argument_expression);

    // optional chains already guard this access against nil
    let non_nullable_type = state.checker.get_non_nullable_type(receiver_type);
    if is_definitely_type(state, non_nullable_type, &[&is_string_type]) {
        let index_type = state.get_type(argument_expression);
        return create_string_index_expression(
            state,
            prereqs,
            expression,
            EvaluationOperand { expression: index, prereqs: index_prereqs.statements },
            index_type,
        );
    }

    if is_mixed_string_type(state, non_nullable_type) {
        state.add_diagnostic(errors::no_mixed_string_index(node));
        return Expr::None;
    }

    let later = join_effects(
        get_effects_of_statements(&index_prereqs.statements),
        &(if is_late_read(&expression) { get_effects(&index) } else { Effects::none() }),
    );
    if !effects_commute(&get_effects(&expression), &later) {
        // wrapReturnIfLuaTuple does not wrap this, but now it needs to be
        if is_lua_tuple_type(state, receiver_type) {
            expression = create::array(vec![expression]);
        }
        expression = prereqs.push_to_var(expression, "exp");
    }
    prereqs.push_list(index_prereqs.statements);

    // LuaTuple<T> checks
    if expression.is_call() && is_lua_tuple_type(state, receiver_type) {
        // wrap in select() if it isn't the first value
        if get_literal_number_value(&index).is_none_or(|v| v != 0.0) || !matches!(index, Expr::Number(_)) {
            expression = create::call(globals::select(), vec![offset(index, 1.0), expression]);
        }
        // parentheses to trim off the rest of the values
        return create::paren(expression);
    }

    let delete = skip_upwards(node).parent().is_some_and(|p| p.kind() == Kind::DeleteExpression);
    // rbxts-rs: a plugin's `property_key` for a string literal key (DESIGN §11.5)
    let position = if delete { AccessPosition::Delete } else { AccessPosition::Read };
    if let Some(key) = crate::plugins::dispatch::rewritten_key(state, node, position) {
        // a computed argument is still evaluated, after the receiver
        let statements = crate::plugins::dispatch::discarded_key_statements(state, node, index);
        let expression = crate::plugins::dispatch::place_key_statements(prereqs, expression, statements);
        let target = keyed_access(expression, &key);
        if delete {
            prereqs.push(create::assignment(target, AssignmentOperator::Assign, Expr::Nil));
            return Expr::None;
        }
        return target;
    }

    if delete {
        let index = add_one_if_array_type(state, receiver_type, index);
        prereqs.push(create::assignment(
            create::computed_index(convert_to_indexable_expression(expression), index),
            AssignmentOperator::Assign,
            Expr::Nil,
        ));
        return Expr::None;
    }

    let index = add_one_if_array_type(state, receiver_type, index);
    let mut access = create::computed_index(convert_to_indexable_expression(expression), index);
    try_mark_builtin_member(state, node, &mut access);
    access
}

/// `transformElementAccessExpression`
pub fn transform_element_access_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    if let Some(constant_value) = get_constant_value_literal(state, node) {
        return constant_value;
    }
    transform_optional_chain(state, prereqs, node)
}
