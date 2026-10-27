//! `util/getConstantValueLiteral.ts`

use luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_checker::LiteralValue;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;

/// The literal of a const enum member access, or of a member of a regular enum whose members are all literals
/// ([`crate::util::inline_constants::inline_enum_member`]).
pub fn get_constant_value_literal(state: &mut TransformState<'_>, node: P<Node>) -> Option<Expr> {
    match state.checker.get_constant_value(node) {
        // rbxts-rs: fixes roblox-ts passing the decoded value to `luau.string`, which renders it as source text
        Some(LiteralValue::String(s)) => Some(crate::util::create_string_from_literal::create_string_from_value(s)),
        Some(LiteralValue::Number(n)) => Some(create::number(n.0)),
        Some(_) => None,
        None => crate::util::inline_constants::inline_enum_member(state, node),
    }
}
