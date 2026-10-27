//! Luau's `@native` function attribute over a finished tree.

use crate::ast::*;
use crate::walk::{expr_children_mut, stmt_children_mut};

/// Sets the `native` flag of every function in `block`, nested ones included: Luau compiles natively only the
/// functions that carry the attribute themselves.
pub fn mark_native_functions(block: &mut [Stmt]) {
    for statement in block {
        match statement {
            Stmt::FunctionDeclaration(node) => node.native = true,
            Stmt::MethodDeclaration(node) => node.native = true,
            _ => {}
        }
        stmt_children_mut(statement, &mut mark_expr, &mut |b| mark_native_functions(b));
    }
}

fn mark_expr(e: &mut Expr) {
    if let Expr::Function(node) = e {
        node.native = true;
        mark_native_functions(&mut node.statements);
    }
    expr_children_mut(e, &mut mark_expr);
}
