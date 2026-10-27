//! Luau's `@native` function attribute. Luau compiles natively only the functions that carry the attribute
//! themselves, so marking a function marks every function nested in it too.

use crate::ast::*;
use crate::walk::{expr_children_mut, stmt_children_mut};

/// Marks every function in `block`.
pub fn mark_native_functions(block: &mut [Stmt]) {
    block.iter_mut().for_each(mark_native_stmt);
}

/// Marks every function in `statement`: a function or method declaration itself, and any function its expressions
/// or blocks hold (the `TS.async(function …)` of an async declaration).
pub fn mark_native_stmt(statement: &mut Stmt) {
    match statement {
        Stmt::FunctionDeclaration(node) => node.native = true,
        Stmt::MethodDeclaration(node) => node.native = true,
        _ => {}
    }
    stmt_children_mut(statement, &mut mark_native_expr, &mut |b| mark_native_functions(b));
}

/// Marks every function in `e`: a function expression itself, or the one a wrapper call (`TS.async(…)`) takes.
pub fn mark_native_expr(e: &mut Expr) {
    if let Expr::Function(node) = e {
        node.native = true;
        mark_native_functions(&mut node.statements);
    }
    expr_children_mut(e, &mut mark_native_expr);
}
