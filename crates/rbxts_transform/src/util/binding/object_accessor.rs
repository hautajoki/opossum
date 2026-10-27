//! `util/binding/objectAccessor.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::shared::diagnostics::errors;
use crate::util::add_index_diagnostics::add_index_diagnostics;
use crate::util::add_one_if_array_type::add_one_if_array_type;
use crate::util::create_string_index_expression::create_string_index_expression;
use crate::util::evaluation::plan::EvaluationOperand;
use crate::util::types::{is_definitely_type, is_mixed_string_type, is_string_type};

/// What [`object_accessor`] returns: the key (for object rest's exclusions) and the read of the member.
pub struct ObjectAccess {
    /// The member's key.
    pub key: Expr,
    /// The read of the member.
    pub value: Expr,
}

/// `objectAccessor`: reads the property `name` of `parent_id`, whose type is `receiver_type`.
pub fn object_accessor(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    parent_id: Expr,
    receiver_type: P<Type>,
    name: P<Node>,
) -> ObjectAccess {
    let member_type = state.get_type(name);
    add_index_diagnostics(state, name, member_type, receiver_type);

    // NoSubstitutionTemplateLiteral is part of ts.PropertyName but TS rejects it as a binding key (TS1180/TS1136), so it
    // can never reach here
    assert!(name.kind() != Kind::NoSubstitutionTemplateLiteral, "template literal as a binding key");

    if is_mixed_string_type(state, receiver_type) {
        state.add_diagnostic(errors::no_mixed_string_index(name));
        return ObjectAccess { key: Expr::None, value: Expr::None };
    }

    if name.kind() == Kind::PrivateIdentifier {
        state.add_diagnostic(errors::no_private_identifier(name));
        return ObjectAccess { key: Expr::None, value: Expr::None };
    }

    if name.kind() == Kind::Identifier {
        let text = name.text();
        return ObjectAccess { key: create::string(text), value: create::property(parent_id, text) };
    }

    let is_computed = name.kind() == Kind::ComputedPropertyName;
    let key_node = if is_computed { name.as_computed_property_name().expression } else { name };
    let mut key = transform_expression(state, prereqs, key_node);
    if is_computed && !key.is_simple_primitive() {
        // computed keys are evaluated before assignment targets and reused by object rest
        key = prereqs.push_to_var(key, "key");
    }

    if is_definitely_type(state, receiver_type, &[&is_string_type]) {
        let index_type = if key_node.kind() == Kind::StringLiteral {
            state.checker.get_string_literal_type(key_node.text())
        } else {
            state.get_type(key_node)
        };
        let value = create_string_index_expression(
            state,
            prereqs,
            parent_id,
            EvaluationOperand { expression: key.clone(), prereqs: Vec::new() },
            index_type,
        );
        return ObjectAccess { key, value };
    }

    let key = add_one_if_array_type(state, receiver_type, key);
    ObjectAccess { value: create::computed_index(parent_id, key.clone()), key }
}
