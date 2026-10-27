//! `nodes/binding/transformBindingName.ts`

use rbxts_luau::{AnyIdentifier, Expr, Stmt};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_array_binding_pattern::transform_array_binding_pattern;
use crate::nodes::binding::transform_object_binding_pattern::transform_object_binding_pattern;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;

/// `transformBindingName`: the identifier a binding name declares (a catch variable, a loop variable). A pattern
/// binds a temporary instead, and its destructuring is pushed to `initializers`.
pub fn transform_binding_name(
    state: &mut TransformState<'_>,
    name: P<Node>,
    initializers: &mut Vec<Stmt>,
) -> AnyIdentifier {
    if name.kind() == Kind::Identifier {
        as_any_identifier(transform_identifier_defined(state, name))
    } else {
        let id = temp_id("binding");
        let mut binding_prereqs = Prereqs::new();
        if name.kind() == Kind::ArrayBindingPattern {
            transform_array_binding_pattern(state, &mut binding_prereqs, name, Expr::TemporaryIdentifier(id.clone()));
        } else {
            transform_object_binding_pattern(state, &mut binding_prereqs, name, Expr::TemporaryIdentifier(id.clone()));
        }
        initializers.extend(binding_prereqs.statements);
        AnyIdentifier::Temporary(id)
    }
}
