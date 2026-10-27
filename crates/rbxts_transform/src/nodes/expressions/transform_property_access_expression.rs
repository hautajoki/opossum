//! `nodes/expressions/transformPropertyAccessExpression.ts`

use rbxts_luau::{AssignmentOperator, Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::transform_optional_chain::transform_optional_chain;
use crate::plugins::backend::keyed_access;
use crate::util::add_index_diagnostics::add_index_diagnostics;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::evaluation::builtins::{try_mark_builtin_member, try_mark_roblox_constant};
use crate::util::get_constant_value_literal::get_constant_value_literal;
use crate::util::traversal::skip_upwards;
use crate::util::validate_not_any::validate_not_any_type;
use rbxts_plugin::AccessPosition;

/// `transformPropertyAccessExpressionInner`: `expression.name` once the receiver is transformed.
pub fn transform_property_access_expression_inner(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    name: &str,
) -> Expr {
    let receiver = node.as_property_access_expression().expression;
    // a in a.b
    validate_not_any_type(state, receiver);

    let node_type = state.get_type(node);
    let member_type = state.checker.get_non_optional_type(node_type);
    let receiver_type = state.get_type(receiver);
    add_index_diagnostics(state, node, member_type, receiver_type);

    let delete = skip_upwards(node).parent().is_some_and(|p| p.kind() == Kind::DeleteExpression);
    // rbxts-rs: a plugin's `property_key` (DESIGN §11.5)
    let position = if delete { AccessPosition::Delete } else { AccessPosition::Read };
    let rewritten = crate::plugins::dispatch::rewritten_key(state, node, position);

    if delete {
        let target = match &rewritten {
            Some(key) => keyed_access(expression, key),
            None => create::property(convert_to_indexable_expression(expression), name),
        };
        prereqs.push(create::assignment(target, AssignmentOperator::Assign, Expr::Nil));
        return Expr::None;
    }

    if let Some(key) = rewritten {
        return keyed_access(expression, &key);
    }
    let mut property = create::property(convert_to_indexable_expression(expression), name);
    try_mark_builtin_member(state, node, &mut property);
    // rbxts-rs: `Enum.AutomaticSize.Y`, `Vector2.zero`, for the JSX ordering (`try_mark_roblox_constant`)
    try_mark_roblox_constant(state, node, &mut property);
    property
}

/// `transformPropertyAccessExpression`
pub fn transform_property_access_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    if let Some(constant_value) = get_constant_value_literal(state, node) {
        return constant_value;
    }
    transform_optional_chain(state, prereqs, node)
}
