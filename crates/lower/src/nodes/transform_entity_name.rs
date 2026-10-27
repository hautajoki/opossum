//! `nodes/transformEntityName.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_identifier::transform_identifier;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::symbols::validate_identifier;

/// `transformEntityName`: `A` or `A.B.C`.
pub fn transform_entity_name(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    if node.kind() == Kind::Identifier {
        validate_identifier(state, node);
        transform_identifier(state, node)
    } else {
        let qualified = node.as_qualified_name();
        let left = transform_entity_name(state, qualified.left);
        create::property(convert_to_indexable_expression(left), qualified.right.text())
    }
}
