//! A borrowed view of any Luau node, for the evaluation walks that roblox-ts writes over `luau.Node` with
//! `forEachChild`: every child node in field order (lists in list order), declared identifiers included.

use rbxts_luau::{AnyIdentifier, ElseBody, Expr, IfStatement, InterpolatedStringPart, MapField, Stmt, TableField};

/// Any Luau node the evaluation code visits.
#[derive(Clone, Copy)]
pub enum LuauNode<'a> {
    /// An expression.
    Expr(&'a Expr),
    /// A statement.
    Stmt(&'a Stmt),
    /// A declared identifier: a local, a loop variable or a parameter.
    Id(&'a AnyIdentifier),
    /// An `elseif`: an `IfStatement` that is not an element of a statement list.
    ElseIf(&'a IfStatement),
    /// A field of a map literal.
    MapField(&'a MapField),
    /// A literal part of an interpolated string (`InterpolatedStringPart`).
    StringPart,
}

fn exprs<'a>(list: &'a [Expr], f: &mut dyn FnMut(LuauNode<'a>)) {
    for e in list {
        f(LuauNode::Expr(e));
    }
}

fn stmts<'a>(list: &'a [Stmt], f: &mut dyn FnMut(LuauNode<'a>)) {
    for s in list {
        f(LuauNode::Stmt(s));
    }
}

fn ids<'a>(list: &'a [AnyIdentifier], f: &mut dyn FnMut(LuauNode<'a>)) {
    for id in list {
        f(LuauNode::Id(id));
    }
}

fn if_children<'a>(node: &'a IfStatement, f: &mut dyn FnMut(LuauNode<'a>)) {
    f(LuauNode::Expr(&node.condition));
    stmts(&node.statements, f);
    match &node.else_body {
        ElseBody::If(else_if) => f(LuauNode::ElseIf(else_if)),
        ElseBody::Block(list) => stmts(list, f),
    }
}

/// `forEachChild` (effects.ts).
pub fn for_each_child<'a>(node: LuauNode<'a>, f: &mut dyn FnMut(LuauNode<'a>)) {
    match node {
        LuauNode::Expr(e) => match e {
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
                f(LuauNode::Expr(&n.expression));
                f(LuauNode::Expr(&n.index));
            }
            Expr::PropertyAccess(n) => f(LuauNode::Expr(&n.expression)),
            Expr::Call(n) => {
                f(LuauNode::Expr(&n.expression));
                exprs(&n.args, f);
            }
            Expr::MethodCall(n) => {
                f(LuauNode::Expr(&n.expression));
                exprs(&n.args, f);
            }
            Expr::Parenthesized(n) => f(LuauNode::Expr(&n.expression)),
            Expr::Function(n) => {
                ids(&n.parameters, f);
                stmts(&n.statements, f);
            }
            Expr::Binary(n) => {
                f(LuauNode::Expr(&n.left));
                f(LuauNode::Expr(&n.right));
            }
            Expr::Unary(n) => f(LuauNode::Expr(&n.expression)),
            Expr::If(n) => {
                f(LuauNode::Expr(&n.condition));
                f(LuauNode::Expr(&n.expression));
                f(LuauNode::Expr(&n.alternative));
            }
            Expr::InterpolatedString(n) => {
                for part in &n.parts {
                    match part {
                        InterpolatedStringPart::Text(_) => f(LuauNode::StringPart),
                        InterpolatedStringPart::Expression(e) => f(LuauNode::Expr(e)),
                    }
                }
            }
            Expr::Array(members) | Expr::Set(members) => exprs(members, f),
            Expr::Map(fields) => {
                for field in fields {
                    f(LuauNode::MapField(field));
                }
            }
            Expr::MixedTable(fields) => {
                for field in fields {
                    match field {
                        TableField::Value(e) => f(LuauNode::Expr(e)),
                        TableField::Field(m) => f(LuauNode::MapField(m)),
                    }
                }
            }
        },
        LuauNode::Id(_) | LuauNode::StringPart => {}
        LuauNode::MapField(m) => {
            f(LuauNode::Expr(&m.index));
            f(LuauNode::Expr(&m.value));
        }
        LuauNode::ElseIf(n) => if_children(n, f),
        LuauNode::Stmt(s) => match s {
            Stmt::Assignment(n) => {
                exprs(&n.left, f);
                exprs(&n.right, f);
            }
            Stmt::Break | Stmt::Continue | Stmt::Comment(_) => {}
            Stmt::Call(e) => f(LuauNode::Expr(e)),
            Stmt::Do(n) => stmts(&n.statements, f),
            Stmt::While(n) => {
                f(LuauNode::Expr(&n.condition));
                stmts(&n.statements, f);
            }
            Stmt::Repeat(n) => {
                stmts(&n.statements, f);
                f(LuauNode::Expr(&n.condition));
            }
            Stmt::If(n) => if_children(n, f),
            Stmt::NumericFor(n) => {
                f(LuauNode::Id(&n.id));
                f(LuauNode::Expr(&n.start));
                f(LuauNode::Expr(&n.end));
                if let Some(step) = &n.step {
                    f(LuauNode::Expr(step));
                }
                stmts(&n.statements, f);
            }
            Stmt::For(n) => {
                ids(&n.ids, f);
                f(LuauNode::Expr(&n.expression));
                stmts(&n.statements, f);
            }
            Stmt::FunctionDeclaration(n) => {
                f(LuauNode::Expr(&n.name));
                ids(&n.parameters, f);
                stmts(&n.statements, f);
            }
            Stmt::MethodDeclaration(n) => {
                f(LuauNode::Expr(&n.expression));
                ids(&n.parameters, f);
                stmts(&n.statements, f);
            }
            Stmt::VariableDeclaration(n) => {
                ids(&n.left, f);
                exprs(&n.right, f);
            }
            Stmt::Return(n) => exprs(&n.expressions, f),
        },
    }
}
