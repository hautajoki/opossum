//! `nodes/binding/transformAssignmentPatternElement.ts`

use rbxts_luau::{AssignmentOperator, Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_array_assignment_pattern::transform_array_assignment_pattern;
use crate::nodes::binding::transform_object_assignment_pattern::transform_object_assignment_pattern;
use crate::nodes::expressions::transform_expression::kind_name;
use crate::nodes::transform_initializer::transform_initializer;
use crate::nodes::transform_writable::{capture_writable_assignment_target, transform_writable_expression};
use crate::util::evaluation::effects::{get_effects, get_effects_of_statements};
use crate::util::traversal::skip_downwards;

/// `transformAssignmentPatternElement`: assigns `value` (with the statements that compute it, `value_prereqs`) to
/// one target of an assignment pattern: a writable expression or a nested pattern, with an optional default.
pub fn transform_assignment_pattern_element(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    target: P<Node>,
    mut value_prereqs: Prereqs,
    mut value: Expr,
    mut initializer: Option<P<Node>>,
) {
    let mut target = skip_downwards(target);
    if target.kind() == Kind::BinaryExpression {
        let binary = target.as_binary_expression();
        initializer = Some(skip_downwards(binary.right()));
        target = skip_downwards(binary.left);
    }

    let nested = matches!(target.kind(), Kind::ArrayLiteralExpression | Kind::ObjectLiteralExpression);
    if nested || initializer.is_some() {
        let id = value_prereqs.push_to_var(value, "binding");
        if let Some(initializer) = initializer {
            let statement = transform_initializer(state, id.clone(), initializer);
            value_prereqs.push(statement);
        }
        value = id;
    }

    if nested {
        assert!(value.is_any_identifier(), "a nested pattern reads an identifier");
        prereqs.push_list(value_prereqs.statements);
        if target.kind() == Kind::ArrayLiteralExpression {
            transform_array_assignment_pattern(state, prereqs, target, value);
        } else {
            transform_object_assignment_pattern(state, prereqs, target, value);
        }
    } else {
        if !matches!(
            target.kind(),
            Kind::Identifier | Kind::PropertyAccessExpression | Kind::ElementAccessExpression
        ) {
            // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
            state.internal_error(
                target,
                &format!("Invalid destructuring assignment target: {}", kind_name(target.kind())),
            );
            return;
        }
        let id = transform_writable_expression(state, prereqs, target, false);
        let id = capture_writable_assignment_target(
            prereqs,
            id,
            &get_effects_of_statements(&value_prereqs.statements),
            &get_effects(&value),
        );
        prereqs.push_list(value_prereqs.statements);
        prereqs.push(create::assignment(id, AssignmentOperator::Assign, value));
    }
}
