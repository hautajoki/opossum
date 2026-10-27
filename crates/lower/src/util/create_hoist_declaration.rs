//! `util/createHoistDeclaration.ts`

use rbxts_luau::{Stmt, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::as_any_identifier;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::util::symbols::validate_identifier;

/// `local a, b` for the variables used before their declaration in `statement`.
pub fn create_hoist_declaration(state: &mut TransformState<'_>, statement: P<Node>) -> Option<Stmt> {
    let hoists = state.hoists_by_statement.get(&statement)?.clone();
    if hoists.is_empty() {
        return None;
    }
    for &hoist in &hoists {
        validate_identifier(state, hoist);
    }
    let ids = hoists.iter().map(|&hoist| as_any_identifier(transform_identifier_defined(state, hoist))).collect();
    Some(create::locals(ids, Vec::new()))
}
