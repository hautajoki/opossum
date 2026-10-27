//! A walk over compiled Luau that may replace expressions: the spill of `hoist_spillable` locals into a table
//! ([`super::spill`]) and the renaming of plugin imports into TypeScript's print order ([`super::dispatch`]).

use rbxts_luau::{AnyIdentifier, ElseBody, Expr, IfStatement, InterpolatedStringPart, Stmt, TableField};

/// What [`walk_statements`] shows its visitor.
pub(crate) enum Visit<'a> {
    /// An expression, before its parts: the visitor may replace it (the replacement's parts are walked).
    Expr(&'a mut Expr),
    /// Names a statement or a function declares: locals, parameters, loop variables.
    Declared(&'a [AnyIdentifier]),
    /// The name of a `local function`, before it is walked as an expression.
    LocalFunction(&'a Expr),
}

/// Walks `statements` in order, nested blocks and function bodies included.
pub(crate) fn walk_statements(statements: &mut [Stmt], f: &mut dyn FnMut(Visit<'_>)) {
    for statement in statements {
        walk_statement(statement, f);
    }
}

fn walk_statement(statement: &mut Stmt, f: &mut dyn FnMut(Visit<'_>)) {
    match statement {
        Stmt::Assignment(assignment) => {
            for e in assignment.left.iter_mut().chain(assignment.right.iter_mut()) {
                walk_expr(e, f);
            }
        }
        Stmt::Break | Stmt::Continue | Stmt::Comment(_) => {}
        Stmt::Call(e) => walk_expr(e, f),
        Stmt::Do(block) => walk_statements(&mut block.statements, f),
        Stmt::While(w) => {
            walk_expr(&mut w.condition, f);
            walk_statements(&mut w.statements, f);
        }
        Stmt::Repeat(r) => {
            walk_statements(&mut r.statements, f);
            walk_expr(&mut r.condition, f);
        }
        Stmt::If(i) => walk_if(i, f),
        Stmt::NumericFor(n) => {
            f(Visit::Declared(std::slice::from_ref(&n.id)));
            walk_expr(&mut n.start, f);
            walk_expr(&mut n.end, f);
            if let Some(step) = &mut n.step {
                walk_expr(step, f);
            }
            walk_statements(&mut n.statements, f);
        }
        Stmt::For(r) => {
            f(Visit::Declared(&r.ids));
            walk_expr(&mut r.expression, f);
            walk_statements(&mut r.statements, f);
        }
        Stmt::FunctionDeclaration(d) => {
            if d.localize {
                f(Visit::LocalFunction(&d.name));
            }
            walk_expr(&mut d.name, f);
            f(Visit::Declared(&d.parameters));
            walk_statements(&mut d.statements, f);
        }
        Stmt::MethodDeclaration(m) => {
            walk_expr(&mut m.expression, f);
            f(Visit::Declared(&m.parameters));
            walk_statements(&mut m.statements, f);
        }
        Stmt::VariableDeclaration(v) => {
            f(Visit::Declared(&v.left));
            for e in &mut v.right {
                walk_expr(e, f);
            }
        }
        Stmt::Return(r) => {
            for e in &mut r.expressions {
                walk_expr(e, f);
            }
        }
    }
}

fn walk_if(statement: &mut IfStatement, f: &mut dyn FnMut(Visit<'_>)) {
    walk_expr(&mut statement.condition, f);
    walk_statements(&mut statement.statements, f);
    match &mut statement.else_body {
        ElseBody::If(next) => walk_if(next, f),
        ElseBody::Block(statements) => walk_statements(statements, f),
    }
}

fn walk_expr(expr: &mut Expr, f: &mut dyn FnMut(Visit<'_>)) {
    f(Visit::Expr(expr));
    match expr {
        Expr::TemporaryIdentifier(_)
        | Expr::Identifier(_)
        | Expr::None
        | Expr::Nil
        | Expr::False
        | Expr::True
        | Expr::Number(_)
        | Expr::String(_)
        | Expr::LateString(_)
        | Expr::VarArgs => {}
        Expr::ComputedIndex(c) => {
            walk_expr(&mut c.expression, f);
            walk_expr(&mut c.index, f);
        }
        Expr::PropertyAccess(p) => walk_expr(&mut p.expression, f),
        Expr::Call(c) => {
            walk_expr(&mut c.expression, f);
            for e in &mut c.args {
                walk_expr(e, f);
            }
        }
        Expr::MethodCall(m) => {
            walk_expr(&mut m.expression, f);
            for e in &mut m.args {
                walk_expr(e, f);
            }
        }
        Expr::Parenthesized(p) => walk_expr(&mut p.expression, f),
        Expr::Function(function) => {
            f(Visit::Declared(&function.parameters));
            walk_statements(&mut function.statements, f);
        }
        Expr::Binary(b) => {
            walk_expr(&mut b.left, f);
            walk_expr(&mut b.right, f);
        }
        Expr::Unary(u) => walk_expr(&mut u.expression, f),
        Expr::If(i) => {
            walk_expr(&mut i.condition, f);
            walk_expr(&mut i.expression, f);
            walk_expr(&mut i.alternative, f);
        }
        Expr::InterpolatedString(s) => {
            for part in &mut s.parts {
                match part {
                    InterpolatedStringPart::Text(_) => {}
                    InterpolatedStringPart::Expression(e) => walk_expr(e, f),
                }
            }
        }
        Expr::Array(items) | Expr::Set(items) => {
            for e in items {
                walk_expr(e, f);
            }
        }
        Expr::Map(fields) => {
            for field in fields {
                walk_expr(&mut field.index, f);
                walk_expr(&mut field.value, f);
            }
        }
        Expr::MixedTable(fields) => {
            for field in fields {
                match field {
                    TableField::Value(e) => walk_expr(e, f),
                    TableField::Field(field) => {
                        walk_expr(&mut field.index, f);
                        walk_expr(&mut field.value, f);
                    }
                }
            }
        }
    }
}
