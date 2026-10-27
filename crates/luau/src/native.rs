//! Luau's `@native` function attribute over a finished tree.

use crate::ast::*;
use crate::walk::{expr_children_mut, stmt_children_mut};

/// Sets the `native` flag of every function in `block`, nested ones included: Luau compiles natively only the
/// functions that carry the attribute themselves. Returns how many functions were not marked before.
pub fn mark_native_functions(block: &mut [Stmt]) -> usize {
    block.iter_mut().map(statement_native).sum()
}

fn mark(native: &mut bool) -> usize {
    let was = std::mem::replace(native, true);
    usize::from(!was)
}

fn statement_native(statement: &mut Stmt) -> usize {
    let mut marked = match statement {
        Stmt::FunctionDeclaration(node) => mark(&mut node.native),
        Stmt::MethodDeclaration(node) => mark(&mut node.native),
        _ => 0,
    };
    let mut in_blocks = 0;
    stmt_children_mut(statement, &mut |e| marked += expr_native(e), &mut |b| in_blocks += mark_native_functions(b));
    marked + in_blocks
}

fn expr_native(e: &mut Expr) -> usize {
    let mut marked = 0;
    if let Expr::Function(node) = e {
        marked += mark(&mut node.native);
        marked += mark_native_functions(&mut node.statements);
    }
    expr_children_mut(e, &mut |child| marked += expr_native(child));
    marked
}
