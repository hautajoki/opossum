//! `nodes/statements/transformBlock.ts`

use rbxts_luau::{Stmt, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::nodes::transform_statement_list::transform_statement_list;

/// `{ … }` as `do … end`.
pub fn transform_block(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    vec![create::do_stmt(transform_statement_list(state, Some(node), node.statements()))]
}
