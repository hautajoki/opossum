//! `nodes/expressions/transformVoidExpression.ts`

use rbxts_luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::statements::transform_expression_statement::transform_expression_statement_inner;
use crate::util::traversal::skip_downwards;

/// `void x`: `x` as a statement, then `nil`.
pub fn transform_void_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let expression = node.expression().expect("void has an expression");
    let statements = transform_expression_statement_inner(state, skip_downwards(expression));
    prereqs.push_list(statements);
    Expr::Nil
}
