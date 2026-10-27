//! `nodes/binding/transformArrayBindingPattern.ts`

use rbxts_luau::Expr;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_object_binding_pattern::transform_object_binding_pattern;
use crate::nodes::statements::transform_variable_statement::transform_variable;
use crate::nodes::transform_initializer::transform_initializer;
use crate::util::binding::create_array_binding_accessor::create_array_binding_accessor;
use crate::util::validate_not_any::validate_not_any_type;

/// `ts.isOmittedExpression` on an element of an array binding pattern.
///
/// rbxts-rs: TypeScript 7 parses a hole (`[, b]`) in a binding pattern as a `BindingElement` without a name, not as an
/// `OmittedExpression` (array literals, and so assignment patterns, still have `OmittedExpression`).
pub fn is_omitted_binding_element(element: P<Node>) -> bool {
    element.kind() == Kind::OmittedExpression
        || (element.kind() == Kind::BindingElement && element.as_binding_element().name().is_none())
}

/// `transformArrayBindingPattern`: declares the names of `binding_pattern` (`[a, b = 1, ...rest]`) from the value in
/// `parent_id`.
pub fn transform_array_binding_pattern(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    binding_pattern: P<Node>,
    parent_id: Expr,
) {
    validate_not_any_type(state, binding_pattern);

    let t = state.get_type(binding_pattern);
    let mut accessor = create_array_binding_accessor(state, prereqs, binding_pattern, t, parent_id);

    for (index, &element) in binding_pattern.elements().iter().enumerate() {
        if is_omitted_binding_element(element) {
            accessor.read(prereqs, index, true);
            continue;
        }
        let binding_element = element.as_binding_element();
        let name = binding_element.name().expect("a binding element has a name");

        let is_spread_element = binding_element.dot_dot_dot_token().is_some();
        let value = if is_spread_element { accessor.rest(prereqs, index) } else { accessor.read(prereqs, index, false) };

        if name.kind() == Kind::Identifier {
            let id = transform_variable(state, prereqs, name, Some(value));
            if let Some(initializer) = binding_element.initializer() {
                let statement = transform_initializer(state, id, initializer);
                prereqs.push(statement);
            }
        } else {
            let id = prereqs.push_to_var(value, "binding");
            if let Some(initializer) = binding_element.initializer() {
                let statement = transform_initializer(state, id.clone(), initializer);
                prereqs.push(statement);
            }
            if name.kind() == Kind::ArrayBindingPattern {
                transform_array_binding_pattern(state, prereqs, name, id);
            } else {
                transform_object_binding_pattern(state, prereqs, name, id);
            }
        }
    }
}
