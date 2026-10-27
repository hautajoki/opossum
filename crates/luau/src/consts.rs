//! `const` locals: a declaration renders `const` when none of its names is assigned after it.
//!
//! Decided on the finished tree, after lowering has added its temporaries and hoists. Named identifiers are resolved
//! by Luau's scoping (a `local` is visible after its statement, a `local function` inside its own body, a `repeat`
//! body inside its `until`); temporary identifiers are one local per id. Writes are `=` and compound assignments and
//! global-style `function name()` declarations, wherever they sit, closures included. Luau requires a value for every
//! `const` name, so a declaration with fewer values than names stays `local` unless its last value is a call or `...`.

use std::collections::HashSet;

use crate::ast::*;
use crate::walk::expr_children_mut;

/// Marks every [`VariableDeclaration`] of `block` whose names are never assigned as [`VariableDeclaration::constant`];
/// returns how many were marked.
pub fn mark_const_locals(block: &mut [Stmt]) -> usize {
    let mut pass = Pass::default();
    pass.block(block);
    pass.apply = true;
    pass.next = 0;
    pass.scopes.clear();
    pass.block(block);
    pass.marked
}

#[derive(Default)]
struct Pass {
    /// Second walk: set `constant` instead of collecting writes.
    apply: bool,
    /// Per declared named identifier, in walk order: assigned somewhere.
    written: Vec<bool>,
    /// The next named declaration's index into `written`.
    next: usize,
    /// Visible named locals, innermost scope last: the name and its index into `written` (`None` for parameters and
    /// loop variables, which are never `const`).
    scopes: Vec<Vec<(String, Option<usize>)>>,
    /// Temporary identifier ids that are assigned somewhere.
    written_temps: HashSet<u32>,
    marked: usize,
}

impl Pass {
    fn block(&mut self, block: &mut [Stmt]) {
        self.scopes.push(Vec::new());
        self.statements(block);
        self.scopes.pop();
    }

    fn statements(&mut self, block: &mut [Stmt]) {
        for statement in block {
            self.statement(statement);
        }
    }

    fn declare(&mut self, name: &str, index: Option<usize>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_owned(), index));
        }
    }

    /// Parameters and loop variables: they shadow, but are never `const`.
    fn declare_plain(&mut self, ids: &[AnyIdentifier]) {
        for id in ids {
            if let AnyIdentifier::Identifier(id) = id {
                self.declare(&id.name, None);
            }
        }
    }

    fn resolve(&self, name: &str) -> Option<Option<usize>> {
        self.scopes.iter().rev().find_map(|scope| scope.iter().rev().find(|(n, _)| n == name).map(|&(_, i)| i))
    }

    fn write(&mut self, target: &Expr) {
        match target {
            Expr::Identifier(id) => {
                if let Some(Some(index)) = self.resolve(&id.name) {
                    self.written[index] = true;
                }
            }
            Expr::TemporaryIdentifier(id) => {
                self.written_temps.insert(id.id);
            }
            _ => {}
        }
    }

    fn function(&mut self, parameters: &[AnyIdentifier], statements: &mut [Stmt]) {
        self.scopes.push(Vec::new());
        self.declare_plain(parameters);
        self.statements(statements);
        self.scopes.pop();
    }

    fn statement(&mut self, statement: &mut Stmt) {
        match statement {
            Stmt::Assignment(node) => {
                for target in &mut node.left {
                    self.expr(target);
                }
                self.exprs(&mut node.right);
                if !self.apply {
                    for target in &node.left {
                        self.write(target);
                    }
                }
            }
            Stmt::Break | Stmt::Continue | Stmt::Comment(_) | Stmt::TypeAlias(_) => {}
            Stmt::Call(expression) => self.expr(expression),
            Stmt::Do(node) => self.block(&mut node.statements),
            Stmt::While(node) => {
                self.expr(&mut node.condition);
                self.block(&mut node.statements);
            }
            Stmt::Repeat(node) => {
                self.scopes.push(Vec::new());
                self.statements(&mut node.statements);
                self.expr(&mut node.condition);
                self.scopes.pop();
            }
            Stmt::If(node) => self.if_statement(node),
            Stmt::NumericFor(node) => {
                self.expr(&mut node.start);
                self.expr(&mut node.end);
                if let Some(step) = &mut node.step {
                    self.expr(step);
                }
                self.scopes.push(Vec::new());
                self.declare_plain(std::slice::from_ref(&node.id));
                self.statements(&mut node.statements);
                self.scopes.pop();
            }
            Stmt::For(node) => {
                self.expr(&mut node.expression);
                self.scopes.push(Vec::new());
                self.declare_plain(&node.ids);
                self.statements(&mut node.statements);
                self.scopes.pop();
            }
            Stmt::FunctionDeclaration(node) => {
                if node.localize {
                    // visible inside its own body; a `local function` is never `const` here
                    if let Expr::Identifier(id) = &node.name {
                        self.declare(&id.name, None);
                    }
                } else if !self.apply {
                    self.write(&node.name);
                }
                if !node.name.is_any_identifier() {
                    self.expr(&mut node.name);
                }
                self.function(&node.parameters, &mut node.statements);
            }
            Stmt::MethodDeclaration(node) => {
                self.expr(&mut node.expression);
                self.function(&node.parameters, &mut node.statements);
            }
            Stmt::VariableDeclaration(node) => {
                self.exprs(&mut node.right);
                let first = self.next;
                for id in &node.left {
                    if let AnyIdentifier::Identifier(id) = id {
                        if !self.apply {
                            self.written.push(false);
                        }
                        self.declare(&id.name, Some(self.next));
                        self.next += 1;
                    }
                }
                if self.apply && enough_values(node) {
                    let mut named = first..self.next;
                    let constant = node.left.iter().all(|id| match id {
                        AnyIdentifier::Identifier(_) => named.next().is_some_and(|i| !self.written[i]),
                        AnyIdentifier::Temporary(t) => !self.written_temps.contains(&t.id),
                    });
                    if constant {
                        node.constant = true;
                        self.marked += 1;
                    }
                }
            }
            Stmt::Return(node) => self.exprs(&mut node.expressions),
        }
    }

    fn if_statement(&mut self, node: &mut IfStatement) {
        self.expr(&mut node.condition);
        self.block(&mut node.statements);
        match &mut node.else_body {
            ElseBody::If(elseif) => self.if_statement(elseif),
            ElseBody::Block(statements) => self.block(statements),
        }
    }

    fn exprs(&mut self, exprs: &mut [Expr]) {
        for e in exprs {
            self.expr(e);
        }
    }

    /// Walks into expressions only for the function bodies inside them.
    fn expr(&mut self, e: &mut Expr) {
        match e {
            Expr::Function(node) => self.function(&node.parameters, &mut node.statements),
            _ => expr_children_mut(e, &mut |child| self.expr(child)),
        }
    }
}

/// Luau's own rule for `const`: a value for every name, or a last value that may return several.
fn enough_values(node: &VariableDeclaration) -> bool {
    match node.right.last() {
        None => false,
        Some(Expr::Call(_) | Expr::MethodCall(_) | Expr::Require(_) | Expr::VarArgs) => true,
        Some(_) => node.right.len() == node.left.len(),
    }
}
