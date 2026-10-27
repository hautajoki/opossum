//! `util/evaluation/substitute.ts`: replace operand references (temporary identifiers) in a macro expansion.

use rustc_hash::FxHashMap;

use rbxts_luau::{BinaryOperator, ElseBody, Expr, InterpolatedStringPart, Stmt, TableField};

use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::offset::offset;

fn take(e: &mut Expr) -> Expr {
    std::mem::replace(e, Expr::None)
}

/// `substitute` on an expression.
pub fn substitute_expr(node: &mut Expr, replacements: &FxHashMap<u32, Expr>) {
    if let Expr::TemporaryIdentifier(t) = node
        && let Some(replacement) = replacements.get(&t.id)
    {
        *node = replacement.clone();
        return;
    }
    match node {
        Expr::Identifier(_)
        | Expr::TemporaryIdentifier(_)
        | Expr::None
        | Expr::Nil
        | Expr::False
        | Expr::True
        | Expr::Number(_)
        | Expr::String(_)
        | Expr::LateString(_)
        | Expr::VarArgs => {}
        Expr::ComputedIndex(n) => {
            substitute_expr(&mut n.expression, replacements);
            substitute_expr(&mut n.index, replacements);
            // substitution can turn an indexable reference into a call, literal or conditional
            n.expression = convert_to_indexable_expression(take(&mut n.expression));
        }
        Expr::PropertyAccess(n) => {
            substitute_expr(&mut n.expression, replacements);
            n.expression = convert_to_indexable_expression(take(&mut n.expression));
        }
        Expr::Call(n) => {
            substitute_expr(&mut n.expression, replacements);
            for a in &mut n.args {
                substitute_expr(a, replacements);
            }
            n.expression = convert_to_indexable_expression(take(&mut n.expression));
        }
        Expr::MethodCall(n) => {
            substitute_expr(&mut n.expression, replacements);
            for a in &mut n.args {
                substitute_expr(a, replacements);
            }
            n.expression = convert_to_indexable_expression(take(&mut n.expression));
        }
        Expr::Parenthesized(n) => substitute_expr(&mut n.expression, replacements),
        Expr::Function(n) => substitute_stmts(&mut n.statements, replacements),
        Expr::Binary(n) => {
            substitute_expr(&mut n.left, replacements);
            substitute_expr(&mut n.right, replacements);
            // recover arithmetic folding which was intentionally hidden behind references
            if matches!(n.operator, BinaryOperator::Add | BinaryOperator::Sub)
                && let Expr::Number(right) = &n.right
            {
                let sign = if n.operator == BinaryOperator::Sub { -1.0 } else { 1.0 };
                let value = rbxts_luau::js_string_to_number(&right.value) * sign;
                let left = take(&mut n.left);
                *node = offset(left, value);
            }
        }
        Expr::Unary(n) => substitute_expr(&mut n.expression, replacements),
        Expr::If(n) => {
            substitute_expr(&mut n.condition, replacements);
            substitute_expr(&mut n.expression, replacements);
            substitute_expr(&mut n.alternative, replacements);
        }
        Expr::InterpolatedString(n) => {
            for part in &mut n.parts {
                if let InterpolatedStringPart::Expression(e) = part {
                    substitute_expr(e, replacements);
                }
            }
        }
        Expr::Array(members) | Expr::Set(members) => {
            for m in members {
                substitute_expr(m, replacements);
            }
        }
        Expr::Map(fields) => {
            for f in fields {
                substitute_expr(&mut f.index, replacements);
                substitute_expr(&mut f.value, replacements);
            }
        }
        Expr::MixedTable(fields) => {
            for f in fields {
                match f {
                    TableField::Value(e) => substitute_expr(e, replacements),
                    TableField::Field(m) => {
                        substitute_expr(&mut m.index, replacements);
                        substitute_expr(&mut m.value, replacements);
                    }
                }
            }
        }
    }
}

/// `substitute` on a statement list.
pub fn substitute_stmts(statements: &mut [Stmt], replacements: &FxHashMap<u32, Expr>) {
    for s in statements {
        substitute_stmt(s, replacements);
    }
}

fn substitute_if(n: &mut rbxts_luau::IfStatement, replacements: &FxHashMap<u32, Expr>) {
    substitute_expr(&mut n.condition, replacements);
    substitute_stmts(&mut n.statements, replacements);
    match &mut n.else_body {
        ElseBody::If(e) => substitute_if(e, replacements),
        ElseBody::Block(list) => substitute_stmts(list, replacements),
    }
}

/// `substitute` on a statement.
pub fn substitute_stmt(node: &mut Stmt, replacements: &FxHashMap<u32, Expr>) {
    match node {
        Stmt::Assignment(n) => {
            for e in n.left.iter_mut().chain(n.right.iter_mut()) {
                substitute_expr(e, replacements);
            }
        }
        Stmt::Break | Stmt::Continue | Stmt::Comment(_) => {}
        Stmt::Call(e) => substitute_expr(e, replacements),
        Stmt::Do(n) => substitute_stmts(&mut n.statements, replacements),
        Stmt::While(n) => {
            substitute_expr(&mut n.condition, replacements);
            substitute_stmts(&mut n.statements, replacements);
        }
        Stmt::Repeat(n) => {
            substitute_stmts(&mut n.statements, replacements);
            substitute_expr(&mut n.condition, replacements);
        }
        Stmt::If(n) => substitute_if(n, replacements),
        Stmt::NumericFor(n) => {
            substitute_expr(&mut n.start, replacements);
            substitute_expr(&mut n.end, replacements);
            if let Some(step) = &mut n.step {
                substitute_expr(step, replacements);
            }
            substitute_stmts(&mut n.statements, replacements);
        }
        Stmt::For(n) => {
            substitute_expr(&mut n.expression, replacements);
            substitute_stmts(&mut n.statements, replacements);
        }
        Stmt::FunctionDeclaration(n) => {
            substitute_expr(&mut n.name, replacements);
            substitute_stmts(&mut n.statements, replacements);
        }
        Stmt::MethodDeclaration(n) => {
            substitute_expr(&mut n.expression, replacements);
            substitute_stmts(&mut n.statements, replacements);
        }
        Stmt::VariableDeclaration(n) => {
            for e in &mut n.right {
                substitute_expr(e, replacements);
            }
        }
        Stmt::Return(n) => {
            for e in &mut n.expressions {
                substitute_expr(e, replacements);
            }
        }
    }
}
