//! `macros/identifierMacros.ts`

use luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::macros::types::IdentifierMacro;

fn promise(state: &mut TransformState<'_>, _: P<Node>) -> Expr {
    state.ts("Promise")
}

/// `IDENTIFIER_MACROS`
pub const IDENTIFIER_MACROS: &[(&str, IdentifierMacro)] = &[("Promise", promise)];
