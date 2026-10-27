//! `nodes/expressions/transformClassExpression.ts`

use rbxts_luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::class::transform_class_like_declaration::{TransformedClass, transform_class_like_declaration};

/// `transformClassExpression`: the class is built in prerequisites; the expression is its name.
pub fn transform_class_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let TransformedClass { statements, name } = transform_class_like_declaration(state, node);
    prereqs.push_list(statements);
    name.into()
}
