//! `util/wrapStatementsAsGenerator.ts`

use luau::{Stmt, create};

use crate::classes::transform_state::TransformState;

/// `return TS.generator(function() statements end)`
pub fn wrap_statements_as_generator(state: &mut TransformState<'_>, statements: Vec<Stmt>) -> Vec<Stmt> {
    let callee = state.ts("generator");
    vec![create::return_stmt(create::call(callee, vec![create::function(Vec::new(), false, statements)]))]
}
