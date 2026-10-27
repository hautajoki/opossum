//! `nodes/class/transformPropertyDeclaration.ts`

use rbxts_luau::{AnyIdentifier, AssignmentOperator, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_property_name::transform_property_name;
use crate::shared::diagnostics::errors;

/// `transformPropertyDeclaration`: `Class[name] = initializer` for a static property (instance properties are set
/// in the constructor).
pub fn transform_property_declaration(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    name: &AnyIdentifier,
) -> Vec<Stmt> {
    if !tsrs_ast::has_static_modifier(node) {
        return Vec::new();
    }

    let name_node = node.name().expect("a property has a name");
    if name_node.kind() == Kind::PrivateIdentifier {
        state.add_diagnostic(errors::no_private_identifier(node));
        return Vec::new();
    }

    let Some(initializer) = node.initializer() else { return Vec::new() };

    let index = transform_property_name(state, prereqs, name_node);
    let right = transform_expression(state, prereqs, initializer);
    vec![create::assignment(create::computed_index(name.clone().into(), index), AssignmentOperator::Assign, right)]
}
