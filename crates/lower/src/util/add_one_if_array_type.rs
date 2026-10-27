//! `util/addOneIfArrayType.ts`

use rbxts_luau::Expr;
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::offset::offset;
use crate::util::types::{is_array_type, is_definitely_type, is_undefined_type};

/// Arrays index from 1 in Luau.
pub fn add_one_if_array_type(state: &mut TransformState<'_>, t: P<Type>, expression: Expr) -> Expr {
    if is_definitely_type(state, t, &[&is_array_type, &is_undefined_type]) {
        offset(expression, 1.0)
    } else {
        expression
    }
}
