//! Names for temporary identifiers (`solveTempIds`), chosen before rendering.
//!
//! The walk follows TypeScript's `visit` exactly, since the order in which temporary identifiers are met decides
//! who gets `_x` and who gets `_x_1`. Scopes as in TypeScript:
//! - every block (the statements of a function, loop, `do`, `if` / `elseif` / `else` branch) is a scope, but not the
//!   file's top-level list;
//! - functions and `for` loops also get a scope of their own around their parameters (or loop variables) and block.
//!
//! TypeScript finds block edges through `node.parent` (the first and last statement of a block push and pop the
//! block's scope); here the walk knows the block it is in. One quirk is kept: TypeScript pushes a function's own
//! scope before the block scope when a function declaration is the first statement of a block, which gives the
//! same scope tree as pushing the block first.
//!
//! Declared [`Identifier`]s (locals and parameters) and earlier temporary names count as taken. The `in` expression
//! of a generic `for` is not walked for temporaries (as in TypeScript); a temporary identifier met only there gets a
//! fallback name `_0`, `_1`, … in render order.
//!
//! rbxts-rs: fixes roblox-ts (luau-ast's `solveTempIds` and `RenderState.getTempName`) choosing names that capture
//! or shadow the program's own identifiers. For a temporary met first in scope S, a name is also taken when:
//! - an identifier of that name is used in or under S (`for … in` expressions included) and refers to a local,
//!   parameter or loop variable declared outside S, or to a global: the temporary would capture it;
//! - a local, parameter or loop variable of that name is declared in a scope between one of the temporary's uses and
//!   S, before that use: it would hide the temporary from it.
//!
//! A fallback name skips every identifier and temporary name of the tree. A name that collides with nothing is the
//! name roblox-ts gives.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::ast::*;

/// The chosen name of each temporary identifier, by id.
#[derive(Default)]
pub(crate) struct TempNames {
    names: FxHashMap<u32, String>,
}

impl TempNames {
    pub(crate) fn get(&self, id: u32) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }
}

/// What is being rendered: a file's statements, or one expression (for debugging).
#[derive(Clone, Copy)]
pub(crate) enum Root<'a> {
    Statements(&'a [Stmt]),
    Expr(&'a Expr),
}

pub(crate) fn solve_temp_ids(root: Root) -> TempNames {
    let mut solver = Solver {
        parents: vec![None],
        stack: vec![0],
        declared: FxHashMap::default(),
        temps: Vec::new(),
        unwalked: Vec::new(),
        uses: Vec::new(),
        user_declarations: FxHashMap::default(),
        seq: 0,
    };
    match root {
        Root::Statements(ast) => solver.statements(ast, false),
        Root::Expr(e) => solver.expr(e),
    }
    // rbxts-rs: where each name would capture a use (see the module doc), and every scope a temporary appears in
    let captures = solver.captures();
    let mut appearances: FxHashMap<u32, Vec<(usize, usize)>> = FxHashMap::default();
    for &(temp, scope, seq) in &solver.temps {
        appearances.entry(temp.id).or_default().push((scope, seq));
    }

    let mut names = TempNames::default();
    let mut last_try: FxHashMap<(usize, String), u32> = FxHashMap::default();
    for (temp, scope, _) in std::mem::take(&mut solver.temps) {
        if names.names.contains_key(&temp.id) {
            continue;
        }
        let separator = if temp.name.is_empty() { "" } else { "_" };
        let mut input = format!("_{}", temp.name);
        // TypeScript records the last try under the name it settled on, so this only helps a name that was free
        let mut i = last_try.get(&(scope, input.clone())).copied().unwrap_or(1);
        while solver.has_id(scope, &input)
            || captures.get(input.as_str()).is_some_and(|scopes| scopes.contains(scope))
            || appearances[&temp.id].iter().any(|&(used_in, seq)| solver.hides(used_in, seq, scope, &input))
        {
            input = format!("_{}{separator}{i}", temp.name);
            i += 1;
        }
        last_try.insert((scope, input.clone()), i);
        solver.declare(scope, Cow::Owned(input.clone()));
        names.names.insert(temp.id, input);
    }

    if solver.unwalked.iter().any(|e| {
        let mut unnamed = false;
        walk_expr(e, &mut temporaries(|t: &TemporaryIdentifier| unnamed |= !names.names.contains_key(&t.id)));
        unnamed
    }) {
        // `RenderState.getTempName`'s fallback, in the order the renderer meets them
        // rbxts-rs: skipping every name of the tree (see the module doc)
        let taken: FxHashMap<&str, ()> = (solver.declared.keys().map(|n| (n.as_ref(), ())))
            .chain(solver.uses.iter().map(|&(_, n)| (n, ())))
            .chain(solver.user_declarations.keys().map(|&n| (n, ())))
            .collect();
        let mut next = 0;
        let mut fallback = |t: &TemporaryIdentifier| {
            names.names.entry(t.id).or_insert_with(|| loop {
                next += 1;
                let name = format!("_{}", next - 1);
                if !taken.contains_key(name.as_str()) {
                    break name;
                }
            });
        };
        let mut fallback = temporaries(&mut fallback);
        match root {
            Root::Statements(ast) => walk_statements(ast, &mut fallback),
            Root::Expr(e) => walk_expr(e, &mut fallback),
        }
    }
    names
}

/// The scopes that declare a name. Most names are declared once, so the first needs no allocation.
struct Declarers {
    first: usize,
    more: Vec<usize>,
}

impl Declarers {
    fn contains(&self, scope: usize) -> bool {
        self.first == scope || self.more.contains(&scope)
    }
}

struct Solver<'a> {
    /// The parent of every scope made, by scope index; `stack` is the current chain.
    parents: Vec<Option<usize>>,
    stack: Vec<usize>,
    /// Declared names (locals, parameters, temporary names given so far) and the scopes they are declared in: one
    /// map for the file rather than a set per scope, since most scopes are blocks that are never asked about.
    declared: FxHashMap<Cow<'a, str>, Declarers>,
    /// Temporary identifiers in visit order, with the scope each was met in (and, rbxts-rs, when).
    temps: Vec<(&'a TemporaryIdentifier, usize, usize)>,
    /// `for … in` expressions, which the visit skips.
    unwalked: Vec<&'a Expr>,
    /// rbxts-rs: the uses of identifiers that start with `_` (the only ones a temporary name can equal), with the
    /// scope of each.
    uses: Vec<(usize, &'a str)>,
    /// rbxts-rs: the locals, parameters and loop variables that start with `_`, with the scope that declares each
    /// and when the declaration takes effect (after a `local`'s values).
    user_declarations: FxHashMap<&'a str, Vec<(usize, usize)>>,
    /// rbxts-rs: a counter that orders temporary appearances and declarations.
    seq: usize,
}

impl<'a> Solver<'a> {
    fn current(&self) -> usize {
        *self.stack.last().expect("the file scope is never popped")
    }

    fn push(&mut self) {
        self.parents.push(Some(self.current()));
        self.stack.push(self.parents.len() - 1);
    }

    fn pop(&mut self) {
        self.stack.pop();
    }

    fn declare(&mut self, scope: usize, name: Cow<'a, str>) {
        match self.declared.get_mut(name.as_ref()) {
            Some(d) => {
                if !d.contains(scope) {
                    d.more.push(scope);
                }
            }
            None => {
                self.declared.insert(name, Declarers { first: scope, more: Vec::new() });
            }
        }
    }

    /// `scopeHasId`: declared in `scope` or one of its ancestors.
    fn has_id(&self, mut scope: usize, id: &str) -> bool {
        let Some(declarers) = self.declared.get(id) else { return false };
        loop {
            if declarers.contains(scope) {
                return true;
            }
            match self.parents[scope] {
                Some(parent) => scope = parent,
                None => return false,
            }
        }
    }

    fn register(&mut self, ids: &'a [AnyIdentifier]) {
        let scope = self.current();
        for id in ids {
            if let AnyIdentifier::Identifier(id) = id {
                self.declare(scope, Cow::Borrowed(&id.name));
            }
        }
    }

    /// rbxts-rs: records the use of an identifier.
    fn use_name(&mut self, name: &'a str) {
        if name.starts_with('_') {
            self.uses.push((self.current(), name));
        }
    }

    /// rbxts-rs: records the declarations of a local, parameter or loop variable list, from now on.
    fn user_declare(&mut self, ids: &'a [AnyIdentifier]) {
        let scope = self.current();
        for id in ids {
            if let AnyIdentifier::Identifier(id) = id
                && id.name.starts_with('_')
            {
                self.seq += 1;
                self.user_declarations.entry(&id.name).or_default().push((scope, self.seq));
            }
        }
    }

    /// A temporary identifier met in the current scope.
    fn temp(&mut self, t: &'a TemporaryIdentifier) {
        self.seq += 1;
        self.temps.push((t, self.current(), self.seq));
    }

    /// rbxts-rs: whether `id` is declared by the program in this scope.
    fn user_declares(&self, scope: usize, id: &str) -> bool {
        self.user_declarations.get(id).is_some_and(|d| d.iter().any(|&(s, _)| s == scope))
    }

    /// rbxts-rs: for each used name, the scopes where a temporary of that name would capture a use: from the use's
    /// scope up to, not including, the scope that declares the name (all the way up for a global).
    fn captures(&self) -> FxHashMap<&'a str, Declarers> {
        let mut captures: FxHashMap<&'a str, Declarers> = FxHashMap::default();
        for &(used_in, name) in &self.uses {
            let scopes = captures.entry(name).or_insert(Declarers { first: usize::MAX, more: Vec::new() });
            let mut scope = Some(used_in);
            while let Some(s) = scope {
                if self.user_declares(s, name) || scopes.contains(s) {
                    break;
                }
                if scopes.first == usize::MAX {
                    scopes.first = s;
                } else {
                    scopes.more.push(s);
                }
                scope = self.parents[s];
            }
        }
        captures
    }

    /// rbxts-rs: whether the program declares `id` before `seq` in `scope` or a scope around it below `top`, hiding a
    /// temporary of scope `top` named `id` from its appearance at `seq`.
    fn hides(&self, mut scope: usize, seq: usize, top: usize, id: &str) -> bool {
        let Some(declarations) = self.user_declarations.get(id) else { return false };
        while scope != top {
            if declarations.iter().any(|&(s, declared_at)| s == scope && declared_at < seq) {
                return true;
            }
            match self.parents[scope] {
                Some(parent) => scope = parent,
                None => return false,
            }
        }
        false
    }

    fn statements(&mut self, statements: &'a [Stmt], is_block: bool) {
        let scoped = is_block && !statements.is_empty();
        if scoped {
            self.push();
        }
        for s in statements {
            self.statement(s);
        }
        if scoped {
            self.pop();
        }
    }

    fn any_identifier(&mut self, id: &'a AnyIdentifier) {
        if let AnyIdentifier::Temporary(t) = id {
            self.temp(t);
        }
    }

    /// A function's own scope: its parameters, then its body.
    fn function(&mut self, name: Option<&'a Expr>, parameters: &'a [AnyIdentifier], statements: &'a [Stmt]) {
        self.push();
        self.register(parameters);
        self.user_declare(parameters);
        if let Some(name) = name {
            self.expr(name);
        }
        for p in parameters {
            self.any_identifier(p);
        }
        self.statements(statements, true);
        self.pop();
    }

    fn if_statement(&mut self, node: &'a IfStatement) {
        self.expr(&node.condition);
        self.statements(&node.statements, true);
        match &node.else_body {
            ElseBody::If(elseif) => self.if_statement(elseif),
            ElseBody::Block(statements) => self.statements(statements, true),
        }
    }

    fn statement(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Assignment(node) => {
                self.exprs(&node.left);
                self.exprs(&node.right);
            }
            Stmt::Break | Stmt::Continue | Stmt::Comment(_) => {}
            Stmt::Call(e) => self.expr(e),
            Stmt::Do(node) => self.statements(&node.statements, true),
            Stmt::While(node) => {
                self.expr(&node.condition);
                self.statements(&node.statements, true);
            }
            Stmt::Repeat(node) => {
                self.statements(&node.statements, true);
                self.expr(&node.condition);
            }
            Stmt::If(node) => self.if_statement(node),
            Stmt::NumericFor(node) => {
                self.push();
                self.user_declare(std::slice::from_ref(&node.id));
                self.any_identifier(&node.id);
                self.expr(&node.start);
                self.expr(&node.end);
                if let Some(step) = &node.step {
                    self.expr(step);
                }
                self.statements(&node.statements, true);
                self.pop();
            }
            Stmt::For(node) => {
                // rbxts-rs: the names in the `in` expression, which belong to the enclosing scope
                walk_names_expr(&node.expression, &mut |name| self.use_name(name));
                self.push();
                self.user_declare(&node.ids);
                for id in &node.ids {
                    self.any_identifier(id);
                }
                self.unwalked.push(&node.expression);
                self.statements(&node.statements, true);
                self.pop();
            }
            Stmt::FunctionDeclaration(node) => self.function(Some(&node.name), &node.parameters, &node.statements),
            Stmt::MethodDeclaration(node) => self.function(Some(&node.expression), &node.parameters, &node.statements),
            Stmt::VariableDeclaration(node) => {
                self.register(&node.left);
                for id in &node.left {
                    self.any_identifier(id);
                }
                self.exprs(&node.right);
                self.user_declare(&node.left);
            }
            Stmt::Return(node) => self.exprs(&node.expressions),
        }
    }

    fn exprs(&mut self, exprs: &'a [Expr]) {
        for e in exprs {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &'a Expr) {
        match e {
            Expr::TemporaryIdentifier(t) => self.temp(t),
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
            Expr::Function(node) => self.function(None, &node.parameters, &node.statements),
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
                    if let InterpolatedStringPart::Expression(e) = part {
                        self.expr(e);
                    }
                }
            }
            Expr::Array(members) | Expr::Set(members) => self.exprs(members),
            Expr::Map(fields) => {
                for f in fields {
                    self.expr(&f.index);
                    self.expr(&f.value);
                }
            }
            Expr::MixedTable(fields) => {
                for f in fields {
                    match f {
                        TableField::Value(e) => self.expr(e),
                        TableField::Field(f) => {
                            self.expr(&f.index);
                            self.expr(&f.value);
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

// Every temporary identifier the renderer writes, in the order it writes them. TypeScript renders a map field's
// value before its key, and a numeric for's step only when it is not a literal 1 (which holds no identifiers).
// rbxts-rs: the walk also reports the named identifiers it meets.

/// What the walk meets.
enum Found<'t> {
    Temporary(&'t TemporaryIdentifier),
    /// A named identifier: declared, a parameter, a loop variable, or used.
    Name(&'t str),
}

/// rbxts-rs: every named identifier in an expression, functions in it included.
fn walk_names_expr<'t>(e: &'t Expr, f: &mut impl FnMut(&'t str)) {
    walk_expr(e, &mut |found| {
        if let Found::Name(name) = found {
            f(name);
        }
    });
}

/// The temporary identifiers only.
fn temporaries<'t>(mut f: impl FnMut(&'t TemporaryIdentifier)) -> impl FnMut(Found<'t>) {
    move |found| {
        if let Found::Temporary(t) = found {
            f(t);
        }
    }
}

fn walk_statements<'t>(statements: &'t [Stmt], f: &mut impl FnMut(Found<'t>)) {
    for s in statements {
        walk_statement(s, f);
    }
}

fn walk_any_identifiers<'t>(ids: &'t [AnyIdentifier], f: &mut impl FnMut(Found<'t>)) {
    for id in ids {
        match id {
            AnyIdentifier::Temporary(t) => f(Found::Temporary(t)),
            AnyIdentifier::Identifier(id) => f(Found::Name(&id.name)),
        }
    }
}

fn walk_statement<'t>(s: &'t Stmt, f: &mut impl FnMut(Found<'t>)) {
    match s {
        Stmt::Assignment(node) => {
            walk_exprs(&node.left, f);
            walk_exprs(&node.right, f);
        }
        Stmt::Break | Stmt::Continue | Stmt::Comment(_) => {}
        Stmt::Call(e) => walk_expr(e, f),
        Stmt::Do(node) => walk_statements(&node.statements, f),
        Stmt::While(node) => {
            walk_expr(&node.condition, f);
            walk_statements(&node.statements, f);
        }
        Stmt::Repeat(node) => {
            walk_statements(&node.statements, f);
            walk_expr(&node.condition, f);
        }
        Stmt::If(node) => {
            let mut node = &**node;
            loop {
                walk_expr(&node.condition, f);
                walk_statements(&node.statements, f);
                match &node.else_body {
                    ElseBody::If(elseif) => node = elseif,
                    ElseBody::Block(statements) => break walk_statements(statements, f),
                }
            }
        }
        Stmt::NumericFor(node) => {
            walk_any_identifiers(std::slice::from_ref(&node.id), f);
            walk_expr(&node.start, f);
            walk_expr(&node.end, f);
            if let Some(step) = &node.step {
                walk_expr(step, f);
            }
            walk_statements(&node.statements, f);
        }
        Stmt::For(node) => {
            walk_any_identifiers(&node.ids, f);
            walk_expr(&node.expression, f);
            walk_statements(&node.statements, f);
        }
        Stmt::FunctionDeclaration(node) => {
            walk_expr(&node.name, f);
            walk_any_identifiers(&node.parameters, f);
            walk_statements(&node.statements, f);
        }
        Stmt::MethodDeclaration(node) => {
            walk_expr(&node.expression, f);
            walk_any_identifiers(&node.parameters, f);
            walk_statements(&node.statements, f);
        }
        Stmt::VariableDeclaration(node) => {
            walk_any_identifiers(&node.left, f);
            walk_exprs(&node.right, f);
        }
        Stmt::Return(node) => walk_exprs(&node.expressions, f),
    }
}

fn walk_exprs<'t>(exprs: &'t [Expr], f: &mut impl FnMut(Found<'t>)) {
    for e in exprs {
        walk_expr(e, f);
    }
}

fn walk_expr<'t>(e: &'t Expr, f: &mut impl FnMut(Found<'t>)) {
    match e {
        Expr::TemporaryIdentifier(t) => f(Found::Temporary(t)),
        Expr::Identifier(id) => f(Found::Name(&id.name)),
        Expr::ComputedIndex(node) => {
            walk_expr(&node.expression, f);
            walk_expr(&node.index, f);
        }
        Expr::PropertyAccess(node) => walk_expr(&node.expression, f),
        Expr::Call(node) => {
            walk_expr(&node.expression, f);
            walk_exprs(&node.args, f);
        }
        Expr::MethodCall(node) => {
            walk_expr(&node.expression, f);
            walk_exprs(&node.args, f);
        }
        Expr::Parenthesized(node) => walk_expr(&node.expression, f),
        Expr::Function(node) => {
            walk_any_identifiers(&node.parameters, f);
            walk_statements(&node.statements, f);
        }
        Expr::Binary(node) => {
            walk_expr(&node.left, f);
            walk_expr(&node.right, f);
        }
        Expr::Unary(node) => walk_expr(&node.expression, f),
        Expr::If(node) => {
            walk_expr(&node.condition, f);
            walk_expr(&node.expression, f);
            walk_expr(&node.alternative, f);
        }
        Expr::InterpolatedString(node) => {
            for part in &node.parts {
                if let InterpolatedStringPart::Expression(e) = part {
                    walk_expr(e, f);
                }
            }
        }
        Expr::Array(members) | Expr::Set(members) => walk_exprs(members, f),
        Expr::Map(fields) => {
            for field in fields {
                walk_expr(&field.value, f);
                walk_expr(&field.index, f);
            }
        }
        Expr::MixedTable(fields) => {
            for field in fields {
                match field {
                    TableField::Value(e) => walk_expr(e, f),
                    TableField::Field(field) => {
                        walk_expr(&field.value, f);
                        walk_expr(&field.index, f);
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

// FxHash (rustc's hasher): the sets and maps here hold short names, for which SipHash is the slow part.
type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

#[derive(Default)]
struct FxHasher {
    hash: u64,
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.hash = (self.hash.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(SEED);
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write(&n.to_le_bytes());
    }

    fn finish(&self) -> u64 {
        self.hash
    }
}
