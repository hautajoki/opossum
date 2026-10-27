//! `util/getConstantValueLiteral.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_checker::LiteralValue;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;

/// The literal of a constant enum member access.
pub fn get_constant_value_literal(state: &mut TransformState<'_>, node: P<Node>) -> Option<Expr> {
    match state.checker.get_constant_value(node)? {
        // rbxts-rs: fixes roblox-ts passing the decoded value to `luau.string`, which renders it as source text
        LiteralValue::String(s) => Some(crate::util::create_string_from_literal::create_string_from_value(s)),
        LiteralValue::Number(n) => Some(create::number(n.0)),
        _ => None,
    }
}
