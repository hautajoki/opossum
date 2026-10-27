//! Child iteration for the passes over a finished tree.

use crate::ast::*;

/// Calls `f` on each direct subexpression of `e`. A function expression's body is statements, not subexpressions:
/// callers handle [`Expr::Function`] themselves.
pub(crate) fn expr_children_mut(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match e {
        Expr::ComputedIndex(node) => {
            f(&mut node.expression);
            f(&mut node.index);
        }
        Expr::PropertyAccess(node) => f(&mut node.expression),
        Expr::Call(node) => {
            f(&mut node.expression);
            node.args.iter_mut().for_each(f);
        }
        Expr::MethodCall(node) => {
            f(&mut node.expression);
            node.args.iter_mut().for_each(f);
        }
        Expr::Parenthesized(node) => f(&mut node.expression),
        Expr::Binary(node) => {
            f(&mut node.left);
            f(&mut node.right);
        }
        Expr::Unary(node) => f(&mut node.expression),
        Expr::If(node) => {
            f(&mut node.condition);
            f(&mut node.expression);
            f(&mut node.alternative);
        }
        Expr::InterpolatedString(node) => {
            for part in &mut node.parts {
                if let InterpolatedStringPart::Expression(e) = part {
                    f(e);
                }
            }
        }
        Expr::Array(items) | Expr::Set(items) => items.iter_mut().for_each(f),
        Expr::Map(fields) => {
            for field in fields {
                f(&mut field.index);
                f(&mut field.value);
            }
        }
        Expr::MixedTable(fields) => {
            for field in fields {
                match field {
                    TableField::Value(value) => f(value),
                    TableField::Field(field) => {
                        f(&mut field.index);
                        f(&mut field.value);
                    }
                }
            }
        }
        Expr::Function(_)
        | Expr::Identifier(_)
        | Expr::TemporaryIdentifier(_)
        | Expr::None
        | Expr::Nil
        | Expr::False
        | Expr::True
        | Expr::Number(_)
        | Expr::String(_)
        | Expr::Require(_)
        | Expr::VarArgs => {}
    }
}

/// Calls `expr` on each expression a statement holds directly and `block` on each block it holds directly (an `if`
/// statement's `elseif` blocks included).
pub(crate) fn stmt_children_mut(s: &mut Stmt, expr: &mut dyn FnMut(&mut Expr), block: &mut dyn FnMut(&mut Vec<Stmt>)) {
    match s {
        Stmt::Assignment(node) => {
            node.left.iter_mut().for_each(&mut *expr);
            node.right.iter_mut().for_each(expr);
        }
        Stmt::Break | Stmt::Continue | Stmt::Comment(_) | Stmt::TypeAlias(_) => {}
        Stmt::Call(e) => expr(e),
        Stmt::Do(node) => block(&mut node.statements),
        Stmt::While(node) => {
            expr(&mut node.condition);
            block(&mut node.statements);
        }
        Stmt::Repeat(node) => {
            block(&mut node.statements);
            expr(&mut node.condition);
        }
        Stmt::If(node) => {
            let mut node = &mut **node;
            loop {
                expr(&mut node.condition);
                block(&mut node.statements);
                match &mut node.else_body {
                    ElseBody::If(elseif) => node = elseif,
                    ElseBody::Block(statements) => {
                        block(statements);
                        break;
                    }
                }
            }
        }
        Stmt::NumericFor(node) => {
            expr(&mut node.start);
            expr(&mut node.end);
            if let Some(step) = &mut node.step {
                expr(step);
            }
            block(&mut node.statements);
        }
        Stmt::For(node) => {
            expr(&mut node.expression);
            block(&mut node.statements);
        }
        Stmt::FunctionDeclaration(node) => {
            expr(&mut node.name);
            block(&mut node.statements);
        }
        Stmt::MethodDeclaration(node) => {
            expr(&mut node.expression);
            block(&mut node.statements);
        }
        Stmt::VariableDeclaration(node) => node.right.iter_mut().for_each(expr),
        Stmt::Return(node) => node.expressions.iter_mut().for_each(expr),
    }
}
