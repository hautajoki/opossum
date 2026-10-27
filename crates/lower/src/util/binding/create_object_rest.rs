//! `util/binding/createObjectRest.ts`

use rbxts_luau::{AnyIdentifier, AssignmentOperator, BinaryOperator, ElseBody, Expr, UnaryOperator, create};
use tsrs_ast::Node;
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::types::{is_definitely_type, is_roblox_type, is_shared_table_type, walk_types};

/// `createObjectRest`: `{ ...rest }` of `source`, a table of the members whose keys are not in `keys` (the keys the
/// pattern has already extracted). Returns the rest table.
pub fn create_object_rest(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    t: P<Type>,
    source: Expr,
    keys: &[Expr],
) -> Expr {
    let is_unsupported =
        |state: &mut TransformState<'_>, t: P<Type>| is_roblox_type(state, t) && !is_shared_table_type(state, t);
    let mut unsupported = false;
    walk_types(state, t, &mut |state, t| {
        unsupported = unsupported || is_definitely_type(state, t, &[&is_unsupported]);
    });
    if unsupported {
        state.add_diagnostic(errors::no_rest_spreading_of_roblox_types(node));
        return Expr::None;
    }

    let rest = prereqs.push_to_var(create::map(Vec::new()), "rest");
    let key_id = temp_id("key");
    let value_id = temp_id("value");
    let key = Expr::TemporaryIdentifier(key_id.clone());
    let mut statements = vec![create::assignment(
        create::computed_index(rest.clone(), key.clone()),
        AssignmentOperator::Assign,
        Expr::TemporaryIdentifier(value_id.clone()),
    )];
    if !keys.is_empty() {
        let condition = if keys.len() == 1 {
            create::binary(key.clone(), BinaryOperator::Ne, keys[0].clone())
        } else {
            let extracted = prereqs.push_to_var(create::set(keys.to_vec()), "extracted");
            create::unary(UnaryOperator::Not, create::computed_index(extracted, key.clone()))
        };
        statements = vec![create::if_stmt(condition, statements, ElseBody::default())];
    }
    prereqs.push(create::for_in(
        vec![AnyIdentifier::Temporary(key_id), AnyIdentifier::Temporary(value_id)],
        source,
        statements,
    ));
    rest
}
