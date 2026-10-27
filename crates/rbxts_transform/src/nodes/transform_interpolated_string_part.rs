//! `nodes/transformInterpolatedStringPart.ts`

use rbxts_luau::InterpolatedStringPart;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::util::create_string_from_literal::create_string_from_literal;

/// A template literal token (or string literal) as literal text of an interpolated string.
pub fn transform_interpolated_string_part(node: P<Node>) -> InterpolatedStringPart {
    InterpolatedStringPart::Text(create_string_from_literal(node))
}
