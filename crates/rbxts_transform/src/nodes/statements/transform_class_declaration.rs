//! `nodes/statements/transformClassDeclaration.ts`

use rbxts_luau::Stmt;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::nodes::class::transform_class_like_declaration::transform_class_like_declaration;

/// `transformClassDeclaration`
pub fn transform_class_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    transform_class_like_declaration(state, node).statements
}
