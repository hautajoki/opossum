//! `util/evaluation/events.ts`: the order in which a macro expansion evaluates its operands and other effects.

use rustc_hash::FxHashMap;

use rbxts_luau::{AssignmentOperator, BinaryOperator, Effects, ElseBody, Expr, Stmt};

use super::effects::{get_intrinsic_effects, is_late_read};
use super::facts::is_constant_reference;
use super::node::{LuauNode, for_each_child};

/// `EvaluationReference`: an opaque operand standing in for `expression`.
pub struct EvaluationReference {
    /// The operand it stands for.
    pub operand_index: usize,
    /// The operand's expression.
    pub expression: Expr,
    /// What evaluating it does.
    pub effects: Effects,
}

/// `EvaluationEvent`
pub struct EvaluationEvent {
    /// What the event does.
    pub effects: Effects,
    /// The operand it evaluates, if it is one.
    pub operand_index: Option<usize>,
    /// It may not happen (inside a branch).
    pub conditional: bool,
}

struct Walker<'r> {
    references: &'r FxHashMap<u32, EvaluationReference>,
    events: Vec<EvaluationEvent>,
    may_exit: bool,
}

impl<'r> Walker<'r> {
    fn reference(&self, node: LuauNode<'_>) -> Option<&'r EvaluationReference> {
        match node {
            LuauNode::Expr(Expr::TemporaryIdentifier(t)) => self.references.get(&t.id),
            _ => None,
        }
    }

    fn is_late_reference(&self, node: &Expr) -> bool {
        match self.reference(LuauNode::Expr(node)) {
            Some(r) => is_late_read(&r.expression),
            None => is_late_read(node),
        }
    }

    fn expr(&mut self, e: &Expr, conditional: bool) {
        self.visit(LuauNode::Expr(e), conditional);
    }

    fn stmts(&mut self, list: &[Stmt]) {
        for s in list {
            self.visit(LuauNode::Stmt(s), true);
        }
    }

    // field order differs from execution order when Luau reads a local's register directly; conditional uses must
    // still preserve eager argument evaluation
    fn visit(&mut self, node: LuauNode<'_>, conditional: bool) {
        if let Some(reference) = self.reference(node) {
            self.events.push(EvaluationEvent {
                effects: reference.effects.clone(),
                operand_index: Some(reference.operand_index),
                conditional: conditional || self.may_exit,
            });
            return;
        }
        if let LuauNode::Expr(e) = node {
            if is_constant_reference(e) {
                return;
            }
            // source callbacks are opaque operands
            assert!(!matches!(e, Expr::Function(_)), "a macro expansion must not contain a function expression");
        }
        if let LuauNode::Id(id) = node
            && id.facts().is_constant()
        {
            return;
        }
        if let LuauNode::Stmt(s) = node {
            // new macro-owned control flow needs an ordering rule here before it can contain operand references
            assert!(
                matches!(
                    s,
                    Stmt::VariableDeclaration(_)
                        | Stmt::Assignment(_)
                        | Stmt::If(_)
                        | Stmt::For(_)
                        | Stmt::NumericFor(_)
                        | Stmt::Call(_)
                        | Stmt::Comment(_)
                        | Stmt::Break
                ),
                "unexpected statement in a macro expansion"
            );
        }
        match node {
            LuauNode::Expr(Expr::Binary(n)) => {
                let short_circuit = matches!(n.operator, BinaryOperator::And | BinaryOperator::Or);
                if !short_circuit && n.operator != BinaryOperator::Concat && self.is_late_reference(&n.left) {
                    self.expr(&n.right, conditional);
                    self.expr(&n.left, conditional);
                } else {
                    self.expr(&n.left, conditional);
                    self.expr(&n.right, conditional || short_circuit);
                }
            }
            LuauNode::Expr(Expr::ComputedIndex(n)) => {
                if self.is_late_reference(&n.expression) {
                    self.expr(&n.index, conditional);
                    self.expr(&n.expression, conditional);
                } else {
                    self.expr(&n.expression, conditional);
                    self.expr(&n.index, conditional);
                }
            }
            LuauNode::Expr(Expr::Call(n)) => {
                self.expr(&n.expression, conditional);
                for a in &n.args {
                    self.expr(a, conditional);
                }
            }
            LuauNode::Expr(Expr::MethodCall(n)) => {
                self.expr(&n.expression, conditional);
                for a in &n.args {
                    self.expr(a, conditional);
                }
            }
            LuauNode::Stmt(Stmt::VariableDeclaration(n)) => {
                assert!(n.left.len() == 1 && n.right.len() <= 1, "a macro declares one local at a time");
                if let Some(right) = n.right.first() {
                    self.expr(right, conditional);
                }
            }
            LuauNode::Stmt(Stmt::Assignment(n)) => {
                assert!(n.left.len() == 1 && n.right.len() == 1, "a macro assigns one target at a time");
                let target = &n.left[0];
                assert!(
                    self.reference(LuauNode::Expr(target)).is_none(),
                    "A macro must not assign to an operand reference"
                );
                let mut delayed: Vec<&Expr> = Vec::new();
                if !target.is_any_identifier() {
                    let Expr::ComputedIndex(t) = target else { panic!("a macro assigns to identifiers or indexes") };
                    if self.is_late_reference(&t.expression) {
                        delayed.push(&t.expression);
                    } else {
                        self.expr(&t.expression, conditional);
                    }
                    if self.is_late_reference(&t.index) {
                        delayed.push(&t.index);
                    } else {
                        self.expr(&t.index, conditional);
                    }
                }
                if n.operator != AssignmentOperator::Assign {
                    self.expr(target, conditional);
                }
                self.expr(&n.right[0], conditional);
                for e in delayed {
                    self.expr(e, conditional);
                }
            }
            LuauNode::Expr(Expr::If(n)) => {
                self.expr(&n.condition, conditional);
                self.expr(&n.expression, true);
                self.expr(&n.alternative, true);
            }
            LuauNode::Stmt(Stmt::If(n)) => {
                let ElseBody::Block(else_body) = &n.else_body else { panic!("a macro's if has a plain else block") };
                self.expr(&n.condition, conditional);
                self.stmts(&n.statements);
                self.stmts(else_body);
            }
            LuauNode::Stmt(Stmt::For(n)) => {
                self.expr(&n.expression, conditional);
                self.stmts(&n.statements);
            }
            LuauNode::Stmt(Stmt::NumericFor(n)) => {
                assert!(n.step.is_none(), "a macro's numeric for has no step");
                self.expr(&n.start, conditional);
                self.expr(&n.end, conditional);
                self.stmts(&n.statements);
            }
            _ => for_each_child(node, &mut |child| self.visit(child, conditional)),
        }
        if matches!(node, LuauNode::Stmt(Stmt::Break)) {
            self.may_exit = true;
        }
        self.events.push(EvaluationEvent {
            effects: get_intrinsic_effects(node),
            operand_index: None,
            conditional: false,
        });
    }
}

/// `getEvaluationEvents`
pub fn get_evaluation_events(
    statements: &[Stmt],
    result: &Expr,
    references: &FxHashMap<u32, EvaluationReference>,
) -> Vec<EvaluationEvent> {
    let mut walker = Walker { references, events: Vec::new(), may_exit: false };
    for s in statements {
        walker.visit(LuauNode::Stmt(s), false);
    }
    walker.visit(LuauNode::Expr(result), false);
    walker.events
}
