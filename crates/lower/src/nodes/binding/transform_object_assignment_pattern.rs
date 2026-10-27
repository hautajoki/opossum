//! `nodes/binding/transformObjectAssignmentPattern.ts`

use rbxts_luau::Expr;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_assignment_pattern_element::transform_assignment_pattern_element;
use crate::nodes::expressions::transform_expression::kind_name;
use crate::util::binding::create_object_rest::create_object_rest;
use crate::util::binding::get_assignment_pattern_type::get_assignment_pattern_type;
use crate::util::binding::object_accessor::{ObjectAccess, object_accessor};

/// `transformObjectAssignmentPattern`: assigns the members of the value in `parent_id` to the targets of
/// `assignment_pattern` (`({ a, b: c.d, ...e } = …)`).
pub fn transform_object_assignment_pattern(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    assignment_pattern: P<Node>,
    parent_id: Expr,
) {
    let t = get_assignment_pattern_type(state, assignment_pattern);
    let mut keys: Vec<Expr> = Vec::new();
    for &property in assignment_pattern.properties() {
        let mut value_prereqs = Prereqs::new();
        match property.kind() {
            Kind::SpreadAssignment => {
                let value = create_object_rest(state, &mut value_prereqs, property, t, parent_id.clone(), &keys);
                let target = property.as_spread_assignment().expression;
                transform_assignment_pattern_element(state, prereqs, target, value_prereqs, value, None);
            }
            Kind::ShorthandPropertyAssignment | Kind::PropertyAssignment => {
                let shorthand = property.kind() == Kind::ShorthandPropertyAssignment;
                let name = if shorthand {
                    property.as_shorthand_property_assignment().name()
                } else {
                    property.as_property_assignment().name()
                };
                // evaluate the property key before the destination, then read the value after it
                let ObjectAccess { key, value } = object_accessor(state, prereqs, parent_id.clone(), t, name);
                keys.push(key);
                let (target, initializer) = if shorthand {
                    (name, property.as_shorthand_property_assignment().object_assignment_initializer())
                } else {
                    (property.as_property_assignment().initializer(), None)
                };
                transform_assignment_pattern_element(state, prereqs, target, value_prereqs, value, initializer);
            }
            kind => {
                // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
                state.internal_error(property, &format!("Invalid object assignment property: {}", kind_name(kind)));
            }
        }
    }
}
