//! `nodes/binding/transformArrayAssignmentPattern.ts`

use rbxts_luau::Expr;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_assignment_pattern_element::transform_assignment_pattern_element;
use crate::util::binding::create_array_binding_accessor::create_array_binding_accessor;
use crate::util::binding::get_assignment_pattern_type::get_assignment_pattern_type;

/// `transformArrayAssignmentPattern`: assigns the elements of the value in `parent_id` to the targets of
/// `assignment_pattern` (`[a, b.c, ...d] = …`).
pub fn transform_array_assignment_pattern(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    assignment_pattern: P<Node>,
    parent_id: Expr,
) {
    let t = get_assignment_pattern_type(state, assignment_pattern);
    let mut accessor = create_array_binding_accessor(state, prereqs, assignment_pattern, t, parent_id);
    for (index, &element) in assignment_pattern.elements().iter().enumerate() {
        if element.kind() == Kind::OmittedExpression {
            accessor.read(prereqs, index, true);
            continue;
        }

        let mut value_prereqs = Prereqs::new();
        let spread = element.kind() == Kind::SpreadElement;
        let value =
            if spread { accessor.rest(&mut value_prereqs, index) } else { accessor.read(&mut value_prereqs, index, false) };
        let target = if spread { element.as_spread_element().expression } else { element };
        transform_assignment_pattern_element(state, prereqs, target, value_prereqs, value, None);
    }
}
