//! `nodes/binding/transformObjectBindingPattern.ts`

use rbxts_luau::Expr;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_array_binding_pattern::transform_array_binding_pattern;
use crate::nodes::statements::transform_variable_statement::transform_variable;
use crate::nodes::transform_initializer::transform_initializer;
use crate::util::binding::create_object_rest::create_object_rest;
use crate::util::binding::object_accessor::{ObjectAccess, object_accessor};
use crate::util::validate_not_any::validate_not_any_type;

/// `transformObjectBindingPattern`: declares the names of `binding_pattern` (`{ a, b: c = 1, ...rest }`) from the
/// value in `parent_id`.
pub fn transform_object_binding_pattern(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    binding_pattern: P<Node>,
    parent_id: Expr,
) {
    validate_not_any_type(state, binding_pattern);
    let mut keys: Vec<Expr> = Vec::new();
    let t = state.get_type(binding_pattern);
    for &element in binding_pattern.elements() {
        let binding_element = element.as_binding_element();
        let name = binding_element.name().expect("a binding element has a name");
        let prop = binding_element.property_name();
        let is_spread = binding_element.dot_dot_dot_token().is_some();

        if name.kind() == Kind::Identifier {
            let value = if is_spread {
                create_object_rest(state, prereqs, element, t, parent_id.clone(), &keys)
            } else {
                let ObjectAccess { key, value } = object_accessor(state, prereqs, parent_id.clone(), t, prop.unwrap_or(name));
                keys.push(key);
                value
            };

            let id = transform_variable(state, prereqs, name, Some(value));
            if let Some(initializer) = binding_element.initializer() {
                let statement = transform_initializer(state, id, initializer);
                prereqs.push(statement);
            }
        } else {
            // if name is not identifier, it must be a binding pattern; in that case, prop is guaranteed to exist
            let prop = prop.expect("a nested object binding pattern has a property name");
            assert!(!is_spread, "a rest element binds an identifier");
            let ObjectAccess { key, value } = object_accessor(state, prereqs, parent_id.clone(), t, prop);
            keys.push(key);

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
