//! `nodes/expressions/transformNoSubstitutionTemplateLiteral.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::nodes::transform_interpolated_string_part::transform_interpolated_string_part;

/// A backtick string without substitutions stays one (valid in Luau).
pub fn transform_no_substitution_template_literal(node: P<Node>) -> Expr {
    create::interpolated_string(vec![transform_interpolated_string_part(node)])
}
