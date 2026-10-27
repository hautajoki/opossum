//! `nodes/transformPropertyName.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_string_literal::transform_string_literal;

/// `transformPropertyName`: `{ a: value }` gives the key `"a"`; computed and literal names are transformed.
pub fn transform_property_name(state: &mut TransformState<'_>, prereqs: &mut Prereqs, name: P<Node>) -> Expr {
    if name.kind() == Kind::Identifier {
        create::string(name.text())
    } else {
        let expression =
            if name.kind() == Kind::ComputedPropertyName { name.expression().expect("a computed name") } else { name };
        transform_expression(state, prereqs, expression)
    }
}

/// `transformPropertyName` for the names of export and import specifiers (an identifier or a string literal), which
/// need no transform state.
pub fn property_name_key(name: P<Node>) -> Expr {
    if name.kind() == Kind::StringLiteral { transform_string_literal(name) } else { create::string(name.text()) }
}
