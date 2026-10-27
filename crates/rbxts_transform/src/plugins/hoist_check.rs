//! What a hoisted value may refer to (`hoist`, `hoist_spillable`, `hoist_spillable_named`). A hoist is declared before
//! the top-level statement around the hook's node, so its value must not use anything that statement declares: a
//! temporary its lowering made so far (a prereq's, roblox-ts's or the plugin's own `push_to_var`), or a local of the
//! code around the node (a parameter, a local of a function or block, `self`, the class's own name), or a name a later
//! statement of the file declares (roblox-ts declares it as a local ahead of the statement that first uses it, after
//! the hoist). Used there, it would read a global instead (`nil`), silently. The compiler refuses such a value: an internal error naming the
//! plugin.

use rustc_hash::FxHashSet;

use rbxts_luau::{AnyIdentifier, ElseBody, Expr, InterpolatedStringPart, Stmt, TableField};
use tsrs_ast::{Kind, Node, SymbolFlags};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;

/// The temporaries and names an expression uses that it does not declare itself, scope by scope: a local of a
/// function or block the value holds counts only inside it, after its declaration (a `local function` from its own
/// name on), as Luau resolves names.
#[derive(Default)]
struct Free<'t> {
    used_temporaries: Vec<u32>,
    used_names: Vec<&'t str>,
    /// The scopes open at the walk's position, innermost last: the temporaries and names declared in each so far.
    scopes: Vec<(FxHashSet<u32>, FxHashSet<&'t str>)>,
}

impl<'t> Free<'t> {
    fn declare(&mut self, ids: &'t [AnyIdentifier]) {
        for id in ids {
            match id {
                AnyIdentifier::Temporary(t) => self.declare_temporary(t.id),
                AnyIdentifier::Identifier(id) => self.declare_name(&id.name),
            }
        }
    }

    fn declare_temporary(&mut self, id: u32) {
        if let Some((temporaries, _)) = self.scopes.last_mut() {
            temporaries.insert(id);
        }
    }

    fn declare_name(&mut self, name: &'t str) {
        if let Some((_, names)) = self.scopes.last_mut() {
            names.insert(name);
        }
    }

    fn use_temporary(&mut self, id: u32) {
        if !self.scopes.iter().any(|(temporaries, _)| temporaries.contains(&id)) {
            self.used_temporaries.push(id);
        }
    }

    fn use_name(&mut self, name: &'t str) {
        if !self.scopes.iter().any(|(_, names)| names.contains(name)) {
            self.used_names.push(name);
        }
    }

    /// `f` in a scope of its own, which starts with `declared` (parameters, loop variables).
    fn scoped(&mut self, declared: &'t [AnyIdentifier], f: impl FnOnce(&mut Self)) {
        self.scopes.push(Default::default());
        self.declare(declared);
        f(self);
        self.scopes.pop();
    }

    /// A block: its statements in a scope of their own.
    fn block(&mut self, statements: &'t [Stmt]) {
        self.scoped(&[], |free| free.statements(statements));
    }

    fn statements(&mut self, statements: &'t [Stmt]) {
        for s in statements {
            self.statement(s);
        }
    }

    fn statement(&mut self, s: &'t Stmt) {
        match s {
            Stmt::Assignment(node) => {
                self.exprs(&node.left);
                self.exprs(&node.right);
            }
            Stmt::Break | Stmt::Continue | Stmt::Comment(_) => {}
            Stmt::Call(e) => self.expr(e),
            Stmt::Do(node) => self.block(&node.statements),
            Stmt::While(node) => {
                self.expr(&node.condition);
                self.block(&node.statements);
            }
            Stmt::Repeat(node) => {
                // the condition sees the body's locals
                self.scoped(&[], |free| {
                    free.statements(&node.statements);
                    free.expr(&node.condition);
                });
            }
            Stmt::If(node) => {
                let mut node = &**node;
                loop {
                    self.expr(&node.condition);
                    self.block(&node.statements);
                    match &node.else_body {
                        ElseBody::If(elseif) => node = elseif,
                        ElseBody::Block(statements) => break self.block(statements),
                    }
                }
            }
            Stmt::NumericFor(node) => {
                self.expr(&node.start);
                self.expr(&node.end);
                if let Some(step) = &node.step {
                    self.expr(step);
                }
                self.scoped(std::slice::from_ref(&node.id), |free| free.statements(&node.statements));
            }
            Stmt::For(node) => {
                self.expr(&node.expression);
                self.scoped(&node.ids, |free| free.statements(&node.statements));
            }
            Stmt::FunctionDeclaration(node) => {
                match (&node.name, node.localize) {
                    // `local function f`: `f` is declared before its body
                    (Expr::TemporaryIdentifier(t), true) => self.declare_temporary(t.id),
                    (Expr::Identifier(id), true) => self.declare_name(&id.name),
                    (name, _) => self.expr(name),
                }
                self.scoped(&node.parameters, |free| free.statements(&node.statements));
            }
            Stmt::MethodDeclaration(node) => {
                self.expr(&node.expression);
                self.scoped(&node.parameters, |free| free.statements(&node.statements));
            }
            Stmt::VariableDeclaration(node) => {
                // `local a = a` reads the outer `a`
                self.exprs(&node.right);
                self.declare(&node.left);
            }
            Stmt::Return(node) => self.exprs(&node.expressions),
        }
    }

    fn exprs(&mut self, exprs: &'t [Expr]) {
        for e in exprs {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &'t Expr) {
        match e {
            Expr::TemporaryIdentifier(t) => self.use_temporary(t.id),
            Expr::Identifier(id) => self.use_name(&id.name),
            Expr::ComputedIndex(node) => {
                self.expr(&node.expression);
                self.expr(&node.index);
            }
            Expr::PropertyAccess(node) => self.expr(&node.expression),
            Expr::Call(node) => {
                self.expr(&node.expression);
                self.exprs(&node.args);
            }
            Expr::MethodCall(node) => {
                self.expr(&node.expression);
                self.exprs(&node.args);
            }
            Expr::Parenthesized(node) => self.expr(&node.expression),
            Expr::Function(node) => self.scoped(&node.parameters, |free| free.statements(&node.statements)),
            Expr::Binary(node) => {
                self.expr(&node.left);
                self.expr(&node.right);
            }
            Expr::Unary(node) => self.expr(&node.expression),
            Expr::If(node) => {
                self.expr(&node.condition);
                self.expr(&node.expression);
                self.expr(&node.alternative);
            }
            Expr::InterpolatedString(node) => {
                for part in &node.parts {
                    match part {
                        InterpolatedStringPart::Expression(e) => self.expr(e),
                        InterpolatedStringPart::Text(_) => {}
                    }
                }
            }
            Expr::Array(members) | Expr::Set(members) => self.exprs(members),
            Expr::Map(fields) => {
                for field in fields {
                    self.expr(&field.index);
                    self.expr(&field.value);
                }
            }
            Expr::MixedTable(fields) => {
                for field in fields {
                    match field {
                        TableField::Value(e) => self.expr(e),
                        TableField::Field(field) => {
                            self.expr(&field.index);
                            self.expr(&field.value);
                        }
                    }
                }
            }
            Expr::None
            | Expr::Nil
            | Expr::False
            | Expr::True
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::LateString(_)
            | Expr::VarArgs => {}
        }
    }
}

/// The top-level statement around `node` (its ancestor whose parent is the source file), or the node itself.
fn root_statement(node: P<Node>) -> P<Node> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == Kind::SourceFile {
            return current;
        }
        current = parent;
    }
    current
}

/// `node` is inside a function, a class, a block or another scope of its own below the file's top level.
fn in_inner_scope(node: P<Node>) -> bool {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == Kind::SourceFile {
            return false;
        }
        if tsrs_ast::is_function_like(n)
            || tsrs_ast::is_class_like(n)
            || matches!(
                n.kind(),
                Kind::Block
                    | Kind::ModuleBlock
                    | Kind::CaseBlock
                    | Kind::CatchClause
                    | Kind::ForStatement
                    | Kind::ForInStatement
                    | Kind::ForOfStatement
                    | Kind::ClassStaticBlockDeclaration
            )
        {
            return true;
        }
        current = n.parent();
    }
    false
}

/// Why `value`, hoisted for the hook at `anchor`, would not read what it reads at the hook's node: it uses a
/// temporary declared within the current top-level statement (made at or after `temp_start`, except the seam's own
/// top-level ones in `top_level`), or a name that, at the hook's node, is a local of that statement (declared in it,
/// or a parameter or local of a function, class or block around the node) or of a later statement, or `self` /
/// `super` inside a function or class. `None` when it may be hoisted.
pub(crate) fn hoist_problem(
    state: &mut TransformState<'_>,
    anchor: P<Node>,
    value: &Expr,
    temp_start: u32,
    top_level: &FxHashSet<u32>,
) -> Option<String> {
    let mut free = Free::default();
    free.expr(value);
    for &id in &free.used_temporaries {
        if id >= temp_start && !top_level.contains(&id) {
            return Some(
                "uses a temporary declared in the current statement (a prerequisite's, or one roblox-ts made while \
                 lowering it)"
                    .to_owned(),
            );
        }
    }
    let root = root_statement(anchor);
    let file = tsrs_ast::get_source_file_of_node(anchor);
    for &name in &free.used_names {
        if matches!(name, "self" | "super") {
            if in_inner_scope(anchor) {
                return Some(format!("uses `{name}`, which is a local of the function or class around its node"));
            }
            continue;
        }
        let Some(symbol) = state.checker.resolve_name_exported(name, Some(anchor), SymbolFlags::Value, false) else {
            continue;
        };
        // the declaration roblox-ts's `checkIdentifierHoist` looks at: the value declaration (not an interface, type
        // alias or global augmentation merged with it), or an import's. A later one, ambient (`declare`) included:
        // roblox-ts declares a name used before its declaration as a local ahead of the statement that uses it
        // (`local f`), after the hoist, which would read the global instead
        let declaration = symbol
            .value_declaration()
            .or_else(|| crate::nodes::expressions::transform_identifier::get_declaration_from_import(symbol))
            .filter(|&declaration| tsrs_ast::get_source_file_of_node(declaration) == file);
        if let Some(declaration) = declaration {
            let declared_at = root_statement(declaration);
            if declared_at == root || in_inner_scope(declaration) {
                return Some(format!(
                    "uses `{name}`, which is declared in the current statement (a parameter or a local of the code \
                     around its node)"
                ));
            }
            if declared_at.pos() > root.pos() {
                return Some(format!(
                    "uses `{name}`, which a later statement of the file declares (a function, class, enum or variable \
                     declared after the statement around its node)"
                ));
            }
        }
    }
    None
}
