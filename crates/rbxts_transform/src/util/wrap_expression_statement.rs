//! `util/wrapExpressionStatement.ts`

use rbxts_luau::{Expr, Stmt, create};

use crate::classes::prereqs::temp_id;

/// An expression evaluated for its effects: a call statement, nothing for a temporary or `None`, else
/// `local _ = expression`.
pub fn wrap_expression_statement(node: Expr) -> Vec<Stmt> {
    match node {
        Expr::TemporaryIdentifier(_) | Expr::None => Vec::new(),
        Expr::Call(_) | Expr::MethodCall(_) => vec![create::call_statement(node)],
        _ => vec![create::local(temp_id(""), Some(node))],
    }
}
