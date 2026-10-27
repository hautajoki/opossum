//! `util/wrapStatementsAsGenerator.ts`

use rbxts_luau::{Stmt, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;

/// `return TS.generator(function() statements end)`
pub fn wrap_statements_as_generator(state: &mut TransformState<'_>, node: P<Node>, statements: Vec<Stmt>) -> Vec<Stmt> {
    let callee = state.ts(node, "generator");
    vec![create::return_stmt(create::call(callee, vec![create::function(Vec::new(), false, statements)]))]
}
