//! `decline-all`: a plugin that is offered everything it can be and declines all of it. Its output must equal a build
//! without plugins (`rbxts-bench build --plugin decline-all --parity …`): offering a node and getting a clean decline
//! changes nothing.
//!
//! What it registers: calls by symbol (every callee symbol classified), `property_key` for every property name the
//! program's accesses use (`identity-key`'s markers), assignments by receiver symbol (every symbol), the class hook
//! for every class, and the expression and statement fallbacks for every expression and statement kind the program's
//! files contain. Every hook declines without touching the compiler (the class hook adds no statements).
//!
//! **`inert`** is active but registers nothing: the baseline a plugin build is compared with (a build with plugins
//! prints a numeric literal with separators as its value, as stock roblox-ts's reprint does).
//!
//! **`decline-audit`** is the same plugin, recording what it was offered: at the end of each file it lists the
//! file's expression nodes the expression hooks were not offered, its statements the statement hooks were not
//! offered, and (for calls, accesses and assignments) the nodes of those shapes the call, `property_key` and
//! assignment hooks were not offered, each as a warning `audit <hook>: <kind> in <parent kind> in <grandparent kind> @
//! <file>:<line>` (aggregate them by the part before `@`). Many are nodes roblox-ts never lowers as values (names,
//! declarations, types); the audit is for finding the others.

use std::cell::RefCell;
use std::sync::Arc;

use rbxts_plugin::ts::{Kind, Node, P, Symbol};
use rbxts_plugin::{
    AccessSite, AssignSite, CallSite, CheckerInfo, Claim, ClassFilter, ClassSite, Cx, Fatal, FileIndex, FinishCx,
    Interest, LuauExpr, LuauStmt, PResult, Plugin, PluginInfo, PluginInstance, SetupCx, SymbolRole, Tag,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// `decline-all` (`audit` false) or `decline-audit`; `inert` registers nothing at all (a plugin build's baseline).
pub struct DeclineAll {
    /// Record what it was offered (`decline-audit`).
    pub audit: bool,
    /// Register nothing (`inert`).
    pub inert: bool,
}

/// What setup decided: the mode, the property names and the expression and statement kinds to register.
pub struct DeclineShared {
    audit: bool,
    inert: bool,
    markers: Vec<&'static str>,
    expressions: Vec<Kind>,
    statements: Vec<Kind>,
}

/// The hooks an offer can reach, for the audit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
enum Offer {
    Call,
    Key,
    Assign,
    Expression,
    Statement,
    Class,
}

/// The per-checker instance (with what it was offered, for the audit).
pub struct DeclineInstance {
    shared: Arc<DeclineShared>,
    offered: RefCell<FxHashMap<P<Node>, Vec<Offer>>>,
}

/// Every expression and statement kind in the program's source files (not declaration files).
fn kinds(cx: &SetupCx<'_>) -> (Vec<Kind>, Vec<Kind>) {
    fn walk(node: P<Node>, expressions: &mut FxHashSet<Kind>, statements: &mut FxHashSet<Kind>) {
        if tsrs_ast::is_expression(node) {
            expressions.insert(node.kind());
        }
        if tsrs_ast::is_statement(node) {
            statements.insert(node.kind());
        }
        node.for_each_child(&mut |child| {
            walk(child, expressions, statements);
            false
        });
    }
    let (mut expressions, mut statements) = (FxHashSet::default(), FxHashSet::default());
    for &file in cx.program().source_files() {
        if !file.get().is_declaration_file() {
            walk(file.as_node(), &mut expressions, &mut statements);
        }
    }
    let mut expressions: Vec<Kind> = expressions.into_iter().collect();
    let mut statements: Vec<Kind> = statements.into_iter().collect();
    expressions.sort_unstable_by_key(|k| *k as u32);
    statements.sort_unstable_by_key(|k| *k as u32);
    (expressions, statements)
}

impl Plugin for DeclineAll {
    type Shared = DeclineShared;
    type State = ();
    type FilePart = ();
    type Instance = DeclineInstance;

    fn info(&self) -> PluginInfo {
        let name = match (self.inert, self.audit) {
            (true, _) => "inert",
            (false, true) => "decline-audit",
            (false, false) => "decline-all",
        };
        PluginInfo { name, version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, cx: &mut SetupCx<'_>, _: &mut ()) -> Result<Arc<DeclineShared>, Fatal> {
        let markers = crate::identity::accessed_names(cx);
        let (expressions, statements) = kinds(cx);
        Ok(Arc::new(DeclineShared { audit: self.audit, inert: self.inert, markers, expressions, statements }))
    }

    fn instance(&self, shared: Arc<DeclineShared>, _: CheckerInfo) -> DeclineInstance {
        DeclineInstance { shared, offered: RefCell::new(FxHashMap::default()) }
    }

    fn finish(&self, _: &DeclineShared, _: &mut (), _: &[(FileIndex, &())], _: &mut FinishCx<'_>) -> Result<(), Fatal> {
        Ok(())
    }
}

impl DeclineInstance {
    fn note(&self, node: P<Node>, offer: Offer) {
        if self.shared.audit {
            self.offered.borrow_mut().entry(node).or_default().push(offer);
        }
    }

    fn was(&self, node: P<Node>, offer: Offer) -> bool {
        self.offered.borrow().get(&node).is_some_and(|o| o.contains(&offer))
    }

    /// The audit of one file: a warning per node and hook it was not offered to.
    fn audit(&self, cx: &mut Cx<'_>) {
        let mut out = Vec::new();
        let file = cx.file().source;
        let name = cx.file().relative_path.clone();
        let mut stack = vec![file.as_node()];
        while let Some(node) = stack.pop() {
            node.for_each_child(&mut |child| {
                stack.push(child);
                false
            });
            let mut missed = Vec::new();
            if is_value_position(node) && !self.was(node, Offer::Expression) {
                missed.push("expression");
            }
            if tsrs_ast::is_statement(node) && !self.was(node, Offer::Statement) {
                missed.push("statement");
            }
            if matches!(node.kind(), Kind::CallExpression | Kind::NewExpression | Kind::Decorator)
                && !self.was(node, Offer::Call)
            {
                missed.push("call");
            }
            if matches!(node.kind(), Kind::PropertyAccessExpression | Kind::ElementAccessExpression)
                && !self.was(node, Offer::Key)
            {
                missed.push("key");
            }
            if is_assignment_like(node) && !self.was(node, Offer::Assign) {
                missed.push("assign");
            }
            if tsrs_ast::is_class_like(node) && !self.was(node, Offer::Class) {
                missed.push("class");
            }
            if missed.is_empty() {
                continue;
            }
            let parent = node.parent().map_or(Kind::Unknown, |p| p.kind());
            let grandparent = node.parent().and_then(|p| p.parent()).map_or(Kind::Unknown, |p| p.kind());
            let line = cx.utf16_pos(node).line + 1;
            for hook in missed {
                out.push((node, format!("audit {hook}: {:?} in {parent:?} in {grandparent:?} @ {name}:{line}", node.kind())));
            }
        }
        for (node, message) in out {
            cx.warning(node, message);
        }
    }
}

/// An expression node in a value position (TypeScript's `isExpressionNode`), but for a property access's name and
/// what is inside a type query (`typeof x` in a type).
fn is_value_position(node: P<Node>) -> bool {
    if !tsrs_ast::is_expression_node(node) {
        return false;
    }
    let parent = node.parent();
    if parent.is_some_and(|p| p.kind() == Kind::PropertyAccessExpression && p.as_property_access_expression().name == node) {
        return false;
    }
    let mut current = parent;
    while let Some(n) = current {
        if n.kind() == Kind::TypeQuery {
            return false;
        }
        current = n.parent();
    }
    true
}

/// An assignment, compound or logical assignment, `++` / `--` or `delete` of a property access (the assignment
/// hook's shapes, before its receiver-symbol filter).
fn is_assignment_like(node: P<Node>) -> bool {
    let target = match node.kind() {
        Kind::BinaryExpression => {
            let binary = node.as_binary_expression();
            if !tsrs_ast::is_assignment_operator(binary.operator_token.kind()) {
                return false;
            }
            binary.left
        }
        Kind::PrefixUnaryExpression => {
            let unary = node.as_prefix_unary_expression();
            if !matches!(unary.operator, Kind::PlusPlusToken | Kind::MinusMinusToken) {
                return false;
            }
            unary.operand
        }
        Kind::PostfixUnaryExpression => {
            let unary = node.as_postfix_unary_expression();
            if !matches!(unary.operator, Kind::PlusPlusToken | Kind::MinusMinusToken) {
                return false;
            }
            unary.operand
        }
        Kind::DeleteExpression => match node.expression() {
            Some(e) => e,
            None => return false,
        },
        _ => return false,
    };
    let mut target = target;
    while target.kind() == Kind::ParenthesizedExpression {
        let Some(inner) = target.expression() else { break };
        target = inner;
    }
    matches!(target.kind(), Kind::PropertyAccessExpression | Kind::ElementAccessExpression)
}

impl PluginInstance for DeclineInstance {
    type FilePart = ();

    fn interest(&self, interest: &mut Interest) {
        if self.shared.inert {
            return;
        }
        interest.calls_by_symbol();
        for &marker in &self.shared.markers {
            interest.accesses_by_marker(marker, Tag(0));
        }
        interest.assignments_by_receiver_symbol();
        interest.classes(ClassFilter::All);
        for &kind in &self.shared.expressions {
            interest.expressions(kind, Tag(0));
        }
        for &kind in &self.shared.statements {
            interest.statements(kind, Tag(0));
        }
    }

    fn classify_symbol(&self, _: &mut Cx<'_>, _: P<Symbol>, _: SymbolRole) -> Option<Tag> {
        Some(Tag(0))
    }

    fn begin_file(&self, _: &mut Cx<'_>) -> PResult<()> {
        self.offered.borrow_mut().clear();
        Ok(())
    }

    fn call(&self, _: &mut Cx<'_>, call: &CallSite, _: Tag) -> PResult<Claim<LuauExpr>> {
        self.note(call.node, Offer::Call);
        Ok(Claim::Decline)
    }

    fn property_key(&self, _: &mut Cx<'_>, access: &AccessSite, _: Tag) -> PResult<Claim<String>> {
        self.note(access.node, Offer::Key);
        Ok(Claim::Decline)
    }

    fn assignment(&self, _: &mut Cx<'_>, site: &AssignSite, _: Tag) -> PResult<Claim<LuauExpr>> {
        self.note(site.node, Offer::Assign);
        Ok(Claim::Decline)
    }

    fn class(&self, _: &mut Cx<'_>, class: &ClassSite) -> PResult<Vec<LuauStmt>> {
        self.note(class.node, Offer::Class);
        Ok(Vec::new())
    }

    fn expression(&self, _: &mut Cx<'_>, node: P<Node>, _: Tag) -> PResult<Claim<LuauExpr>> {
        self.note(node, Offer::Expression);
        Ok(Claim::Decline)
    }

    fn statement(&self, _: &mut Cx<'_>, node: P<Node>, _: Tag) -> PResult<Claim<Vec<LuauStmt>>> {
        self.note(node, Offer::Statement);
        Ok(Claim::Decline)
    }

    fn end_file(&self, cx: &mut Cx<'_>) -> PResult<()> {
        if self.shared.audit {
            self.audit(cx);
        }
        Ok(())
    }
}
