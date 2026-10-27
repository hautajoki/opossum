//! The renderer (`LuauRenderer`): Luau AST to text, byte for byte as `@roblox-ts/luau-ast` 2.1.2 renders it.
//!
//! The TypeScript renderer builds a string per node and reads `node.parent` for three decisions. Here everything is
//! written into one `String`, and the parent's part of each decision travels down as a [`Ctx`]:
//! - parentheses around binary, unary and if-expressions (`needsParentheses`);
//! - a space around a long-bracket string inside index brackets, so `[ [[a]] ]` does not read as `[[`
//!   (`needsBracketSpacing`);
//! - the `;` that keeps a statement from running into a next one that starts with `(` (`getEnding`), which needs
//!   the next statement: the renderer has the statement list at hand instead of TypeScript's list-node stack.
//!
//! Map fields are written key first, while TypeScript renders the value before the key. The order only matters for
//! the fallback names of temporary identifiers the solver never saw, which [`crate::solve`] assigns up front in
//! TypeScript's order.

use std::collections::HashMap;

use crate::ast::*;
use crate::solve::{Root, TempNames, solve_temp_ids};
use crate::util::{
    is_js_whitespace, is_valid_identifier, is_valid_number_literal, js_number_to_string, js_string_to_number,
};

/// Values of [`Expr::LateString`]s by key, filled in after the transform (DESIGN §4 step 5). A value is used as a
/// [`StringLiteral::value`]: text between the quotes, already escaped.
#[derive(Clone, Debug, Default)]
pub struct LateStrings {
    values: HashMap<String, String>,
}

impl LateStrings {
    /// No values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the value of `key`, replacing an earlier one.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    /// The value of `key`, if set.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// The number of keys set.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// No key is set.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Renders a file's statements as Luau (`renderAST`). Panics when the tree has no rendering (a compiler bug, as
/// TypeScript's asserts): an invalid identifier, a `None` expression, a statement after `return` / `break` /
/// `continue`, or a late string with no value in `late_strings`.
pub fn render(ast: &[Stmt], late_strings: &LateStrings) -> String {
    let mut out = String::new();
    render_into(ast, late_strings, &mut out);
    out
}

/// [`render`] with no late strings.
pub fn render_ast(ast: &[Stmt]) -> String {
    render(ast, &LateStrings::default())
}

/// [`render`], appending to `out`.
pub fn render_into(ast: &[Stmt], late_strings: &LateStrings, out: &mut String) {
    let temp_names = solve_temp_ids(Root::Statements(ast));
    let mut r = Renderer { out, indent: 0, temp_names: &temp_names, late_strings, starts: None };
    r.statements(ast);
}

/// [`render`], and for each of the file's own statements the line its text starts on (1-based), so a message about
/// a line of the output (Luau's) can be traced to the statement that holds it.
pub fn render_with_lines(ast: &[Stmt], late_strings: &LateStrings) -> (String, Vec<u32>) {
    let mut out = String::new();
    let temp_names = solve_temp_ids(Root::Statements(ast));
    let mut r = Renderer { out: &mut out, indent: 0, temp_names: &temp_names, late_strings, starts: Some(Vec::new()) };
    r.statements(ast);
    let starts = r.starts.take().unwrap_or_default();
    let mut lines = Vec::with_capacity(starts.len());
    let (mut line, mut counted) = (1u32, 0usize);
    for start in starts {
        line += out.as_bytes()[counted..start].iter().filter(|&&b| b == b'\n').count() as u32;
        counted = start;
        lines.push(line);
    }
    (out, lines)
}

/// Renders one expression, with its temporary identifiers named as if it were the whole file (`debugRender`; for
/// debugging and messages).
pub fn render_expr(expr: &Expr, late_strings: &LateStrings) -> String {
    let mut out = String::new();
    let temp_names = solve_temp_ids(Root::Expr(expr));
    let mut r = Renderer { out: &mut out, indent: 0, temp_names: &temp_names, late_strings, starts: None };
    r.expr(expr, Ctx::None);
    out
}

/// What a child expression needs to know about its parent.
#[derive(Clone, Copy)]
enum Ctx {
    None,
    /// The parent is a binary expression with this precedence; `right` when the child is its right operand.
    Binary {
        precedence: u8,
        right: bool,
    },
    /// The parent is a unary or if-expression with this precedence.
    Precedence(u8),
    /// The child is written inside `[ ]`: a map field's or computed index's index, or a set member.
    Brackets,
}

// https://www.lua.org/manual/5.1/manual.html#2.5.6, as roblox-ts ranks them (`//` with `*`, if-expressions lowest).
const IF_EXPRESSION_PRECEDENCE: u8 = 1;
const UNARY_PRECEDENCE: u8 = 7;

fn binary_precedence(op: BinaryOperator) -> u8 {
    use BinaryOperator::*;
    match op {
        Or => 1,
        And => 2,
        Lt | Gt | Le | Ge | Ne | Eq => 3,
        Concat => 4,
        Add | Sub => 5,
        Mul | Div | FloorDiv | Mod => 6,
        Pow => 8,
    }
}

/// `needsParentheses`: a looser child, or an equally tight right operand of a binary expression.
fn needs_parentheses(precedence: u8, ctx: Ctx) -> bool {
    match ctx {
        Ctx::Binary { precedence: parent, right } => precedence < parent || (precedence == parent && right),
        Ctx::Precedence(parent) => precedence < parent,
        Ctx::None | Ctx::Brackets => false,
    }
}

struct Renderer<'a> {
    out: &'a mut String,
    indent: usize,
    temp_names: &'a TempNames,
    late_strings: &'a LateStrings,
    /// Where each of the file's own statements starts in `out` (`render_with_lines`).
    starts: Option<Vec<usize>>,
}

impl Renderer<'_> {
    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push('\t');
        }
    }

    /// `renderStatements`
    fn statements(&mut self, statements: &[Stmt]) {
        let mut has_final = false;
        for (i, statement) in statements.iter().enumerate() {
            if self.indent == 0
                && let Some(starts) = &mut self.starts
            {
                starts.push(self.out.len());
            }
            assert!(!has_final || statement.is_comment(), "Cannot render statement after break, continue, or return!");
            has_final |= statement.is_final();
            self.statement(statement, &statements[i + 1..]);
        }
    }

    fn block(&mut self, statements: &[Stmt]) {
        self.indent += 1;
        self.statements(statements);
        self.indent -= 1;
    }

    fn line(&mut self, text: &str) {
        self.write_indent();
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// Ends a line that may need a `;` (`state.line(text, endNode)`).
    fn end_line(&mut self, statement: &Stmt, rest: &[Stmt]) {
        if needs_semicolon(statement, rest) {
            self.out.push(';');
        }
        self.out.push('\n');
    }

    /// One statement; `rest` is what follows it in its list.
    fn statement(&mut self, statement: &Stmt, rest: &[Stmt]) {
        match statement {
            Stmt::Assignment(node) => {
                assert!(!node.left.is_empty() && !node.right.is_empty(), "assignment with an empty side");
                self.write_indent();
                self.list(&node.left);
                self.out.push(' ');
                self.out.push_str(node.operator.as_str());
                self.out.push(' ');
                self.list(&node.right);
                self.end_line(statement, rest);
            }
            Stmt::Break => self.line("break"),
            Stmt::Continue => self.line("continue"),
            Stmt::Call(expression) => {
                self.write_indent();
                self.expr(expression, Ctx::None);
                self.end_line(statement, rest);
            }
            Stmt::Do(node) => {
                self.line("do");
                self.block(&node.statements);
                self.line("end");
            }
            Stmt::While(node) => {
                self.write_indent();
                self.out.push_str("while ");
                self.expr(&node.condition, Ctx::None);
                self.out.push_str(" do\n");
                self.block(&node.statements);
                self.line("end");
            }
            Stmt::Repeat(node) => {
                self.line("repeat");
                self.block(&node.statements);
                self.write_indent();
                self.out.push_str("until ");
                self.expr(&node.condition, Ctx::None);
                self.out.push('\n');
            }
            Stmt::If(node) => {
                self.write_indent();
                self.out.push_str("if ");
                self.expr(&node.condition, Ctx::None);
                self.out.push_str(" then\n");
                self.block(&node.statements);
                let mut else_body = &node.else_body;
                while let ElseBody::If(elseif) = else_body {
                    self.write_indent();
                    self.out.push_str("elseif ");
                    self.expr(&elseif.condition, Ctx::None);
                    self.out.push_str(" then\n");
                    self.block(&elseif.statements);
                    else_body = &elseif.else_body;
                }
                if let ElseBody::Block(statements) = else_body
                    && !statements.is_empty()
                {
                    self.line("else");
                    self.block(statements);
                }
                self.line("end");
            }
            Stmt::NumericFor(node) => {
                self.write_indent();
                self.out.push_str("for ");
                self.any_identifier(&node.id);
                self.out.push_str(" = ");
                self.expr(&node.start, Ctx::None);
                self.out.push_str(", ");
                self.expr(&node.end, Ctx::None);
                // a step of 1 is the default
                if let Some(step) = &node.step
                    && !matches!(step, Expr::Number(n) if js_string_to_number(&n.value) == 1.0)
                {
                    self.out.push_str(", ");
                    self.expr(step, Ctx::None);
                }
                self.out.push_str(" do\n");
                self.block(&node.statements);
                self.line("end");
            }
            Stmt::For(node) => {
                self.write_indent();
                self.out.push_str("for ");
                if node.ids.is_empty() {
                    self.out.push('_');
                }
                for (i, id) in node.ids.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    self.any_identifier(id);
                }
                self.out.push_str(" in ");
                self.expr(&node.expression, Ctx::None);
                self.out.push_str(" do\n");
                self.block(&node.statements);
                self.line("end");
            }
            Stmt::FunctionDeclaration(node) => {
                if node.localize {
                    assert!(node.name.is_any_identifier(), "local function cannot be a property");
                }
                self.write_indent();
                if node.localize {
                    self.out.push_str("local ");
                }
                self.out.push_str("function ");
                self.expr(&node.name, Ctx::None);
                self.parameters(&node.parameters, node.has_dot_dot_dot);
                self.out.push('\n');
                self.block(&node.statements);
                self.line("end");
            }
            Stmt::MethodDeclaration(node) => {
                self.write_indent();
                self.out.push_str("function ");
                self.expr(&node.expression, Ctx::None);
                self.out.push(':');
                self.out.push_str(&node.name);
                self.parameters(&node.parameters, node.has_dot_dot_dot);
                self.out.push('\n');
                self.block(&node.statements);
                self.line("end");
            }
            Stmt::VariableDeclaration(node) => {
                assert!(!node.left.is_empty(), "variable declaration without names");
                self.write_indent();
                self.out.push_str("local ");
                for (i, id) in node.left.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    self.any_identifier(id);
                }
                if !node.right.is_empty() {
                    self.out.push_str(" = ");
                    self.list(&node.right);
                }
                self.end_line(statement, rest);
            }
            Stmt::Return(node) => {
                self.write_indent();
                self.out.push_str("return ");
                self.list(&node.expressions);
                self.out.push('\n');
            }
            Stmt::Comment(text) => self.comment(text),
        }
    }

    /// `renderComment`: `--text`, or a long-bracket comment block for several lines.
    fn comment(&mut self, text: &str) {
        if !text.contains('\n') {
            self.write_indent();
            self.out.push_str("--");
            self.out.push_str(text);
            self.out.push('\n');
            return;
        }
        let equals = safe_bracket_equals(text);
        self.write_indent();
        self.out.push_str("--[");
        self.push_equals(equals);
        self.out.push_str("[\n");
        self.indent += 1;
        for line in text.split('\n') {
            // indenting a blank line would only add trailing whitespace
            if line.chars().all(is_js_whitespace) {
                self.out.push('\n');
            } else {
                self.line(line);
            }
        }
        self.indent -= 1;
        self.write_indent();
        self.out.push(']');
        self.push_equals(equals);
        self.out.push_str("]\n");
    }

    fn push_equals(&mut self, n: usize) {
        for _ in 0..n {
            self.out.push('=');
        }
    }

    /// Comma-separated expressions (`renderArguments`).
    fn list(&mut self, expressions: &[Expr]) {
        for (i, e) in expressions.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.expr(e, Ctx::None);
        }
    }

    /// `(a, b, ...)` (`renderParameters`).
    fn parameters(&mut self, parameters: &[AnyIdentifier], has_dot_dot_dot: bool) {
        self.out.push('(');
        for (i, p) in parameters.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.any_identifier(p);
        }
        if has_dot_dot_dot {
            if !parameters.is_empty() {
                self.out.push_str(", ");
            }
            self.out.push_str("...");
        }
        self.out.push(')');
    }

    fn any_identifier(&mut self, id: &AnyIdentifier) {
        match id {
            AnyIdentifier::Identifier(id) => self.identifier(&id.name),
            AnyIdentifier::Temporary(id) => self.temporary_identifier(id),
        }
    }

    fn identifier(&mut self, name: &str) {
        assert!(is_valid_identifier(name), "Invalid Luau Identifier: \"{name}\"");
        self.out.push_str(name);
    }

    fn temporary_identifier(&mut self, id: &TemporaryIdentifier) {
        let name = self.temp_names.get(id.id).expect("every temporary identifier is named before rendering");
        assert!(is_valid_identifier(name), "Invalid Temporary Identifier: \"{name}\"");
        self.out.push_str(name);
    }

    /// What is indexed or called. rbxts-rs: every literal (`nil`, a boolean, a number, a string) keeps its
    /// parentheses there, which roblox-ts's renderer drops around a simple value (`"x":rep(2)`, `(5).x` as `5.x`,
    /// neither of which parses). roblox-ts lowers literals into that position itself (a cast such as
    /// `(true as unknown as T).m()`, an inlined const enum member, `void 0`), and so can a plugin's claimed value.
    fn base(&mut self, e: &Expr) {
        let mut innermost = e;
        while let Expr::Parenthesized(p) = innermost {
            innermost = &p.expression;
        }
        if innermost.is_simple_primitive() {
            self.out.push('(');
            self.expr(innermost, Ctx::None);
            self.out.push(')');
        } else {
            self.expr(e, Ctx::None);
        }
    }

    fn expr(&mut self, e: &Expr, ctx: Ctx) {
        let late = self.late_strings;
        match e {
            Expr::Identifier(id) => self.identifier(&id.name),
            Expr::TemporaryIdentifier(id) => self.temporary_identifier(id),
            Expr::ComputedIndex(node) => {
                self.base(&node.expression);
                match string_value(late, &node.index) {
                    Some(name) if is_valid_identifier(name) => {
                        self.out.push('.');
                        self.out.push_str(name);
                    }
                    _ => {
                        self.out.push('[');
                        self.expr(&node.index, Ctx::Brackets);
                        self.out.push(']');
                    }
                }
            }
            Expr::PropertyAccess(node) => {
                self.base(&node.expression);
                if is_valid_identifier(&node.name) {
                    self.out.push('.');
                    self.out.push_str(&node.name);
                } else {
                    self.out.push_str("[\"");
                    self.out.push_str(&node.name);
                    self.out.push_str("\"]");
                }
            }
            Expr::Call(node) => {
                self.base(&node.expression);
                self.out.push('(');
                self.list(&node.args);
                self.out.push(')');
            }
            Expr::MethodCall(node) => {
                assert!(is_valid_identifier(&node.name), "Invalid method name: \"{}\"", node.name);
                self.base(&node.expression);
                self.out.push(':');
                self.out.push_str(&node.name);
                self.out.push('(');
                self.list(&node.args);
                self.out.push(')');
            }
            Expr::Parenthesized(node) => {
                // nested parentheses collapse, and simple expressions need none
                let inner = &node.expression;
                let mut innermost = inner;
                while let Expr::Parenthesized(e) = innermost {
                    innermost = &e.expression;
                }
                if innermost.is_simple() {
                    self.expr(inner, Ctx::None);
                } else {
                    self.out.push('(');
                    self.expr(inner, Ctx::None);
                    self.out.push(')');
                }
            }
            Expr::None => panic!("Cannot render None"),
            Expr::Nil => self.out.push_str("nil"),
            Expr::False => self.out.push_str("false"),
            Expr::True => self.out.push_str("true"),
            Expr::Number(n) => {
                if is_valid_number_literal(&n.value) {
                    self.out.push_str(&n.value);
                } else {
                    let value = js_string_to_number(&n.value.replace('_', ""));
                    self.out.push_str(&js_number_to_string(value));
                }
            }
            Expr::String(s) => string_literal(self.out, &s.value, s.quote, matches!(ctx, Ctx::Brackets)),
            Expr::LateString(s) => string_literal(self.out, late_value(late, s), None, matches!(ctx, Ctx::Brackets)),
            Expr::VarArgs => self.out.push_str("..."),
            Expr::Function(node) => {
                self.out.push_str("function");
                self.parameters(&node.parameters, node.has_dot_dot_dot);
                if node.statements.is_empty() {
                    self.out.push_str(" end");
                } else {
                    self.out.push('\n');
                    self.block(&node.statements);
                    self.write_indent();
                    self.out.push_str("end");
                }
            }
            Expr::Binary(node) => {
                let precedence = binary_precedence(node.operator);
                let parens = needs_parentheses(precedence, ctx);
                if parens {
                    self.out.push('(');
                }
                self.expr(&node.left, Ctx::Binary { precedence, right: false });
                self.out.push(' ');
                self.out.push_str(node.operator.as_str());
                self.out.push(' ');
                self.expr(&node.right, Ctx::Binary { precedence, right: true });
                if parens {
                    self.out.push(')');
                }
            }
            Expr::Unary(node) => {
                let parens = needs_parentheses(UNARY_PRECEDENCE, ctx);
                if parens {
                    self.out.push('(');
                }
                self.out.push_str(node.operator.as_str());
                // `not` needs a space, and `- -a` must not become a `--` comment
                if node.operator == UnaryOperator::Not
                    || matches!(&node.expression, Expr::Unary(inner) if inner.operator == UnaryOperator::Neg)
                {
                    self.out.push(' ');
                }
                self.expr(&node.expression, Ctx::Precedence(UNARY_PRECEDENCE));
                if parens {
                    self.out.push(')');
                }
            }
            Expr::If(node) => {
                // rbxts-rs: fixes roblox-ts (luau-ast's `needsParentheses`) leaving an if-expression bare as the left
                // operand of `or`, its equal: `if a then b else c or d` parses as `if a then b else (c or d)`. The
                // `else` branch takes everything after it, so an if-expression is parenthesized as any binary operand.
                let parens = matches!(ctx, Ctx::Binary { .. }) || needs_parentheses(IF_EXPRESSION_PRECEDENCE, ctx);
                if parens {
                    self.out.push('(');
                }
                let child = Ctx::Precedence(IF_EXPRESSION_PRECEDENCE);
                self.out.push_str("if ");
                self.expr(&node.condition, child);
                self.out.push_str(" then ");
                self.expr(&node.expression, child);
                self.out.push(' ');
                let mut alternative = &node.alternative;
                while let Expr::If(elseif) = alternative {
                    self.out.push_str("elseif ");
                    self.expr(&elseif.condition, child);
                    self.out.push_str(" then ");
                    self.expr(&elseif.expression, child);
                    self.out.push(' ');
                    alternative = &elseif.alternative;
                }
                self.out.push_str("else ");
                self.expr(alternative, child);
                if parens {
                    self.out.push(')');
                }
            }
            Expr::InterpolatedString(node) => {
                self.out.push('`');
                for part in &node.parts {
                    match part {
                        InterpolatedStringPart::Text(text) => interpolated_text(self.out, text),
                        InterpolatedStringPart::Expression(e) => {
                            self.out.push('{');
                            // `{{}}` is invalid, so tables are parenthesized
                            if e.is_table() {
                                self.out.push('(');
                                self.expr(e, Ctx::None);
                                self.out.push(')');
                            } else {
                                self.expr(e, Ctx::None);
                            }
                            self.out.push('}');
                        }
                    }
                }
                self.out.push('`');
            }
            Expr::Array(members) => {
                if members.is_empty() {
                    self.out.push_str("{}");
                } else {
                    self.out.push_str("{ ");
                    self.list(members);
                    self.out.push_str(" }");
                }
            }
            Expr::Map(fields) => {
                if fields.is_empty() {
                    self.out.push_str("{}");
                    return;
                }
                self.out.push_str("{\n");
                self.indent += 1;
                for field in fields {
                    self.write_indent();
                    self.map_field(field);
                    self.out.push_str(",\n");
                }
                self.indent -= 1;
                self.write_indent();
                self.out.push('}');
            }
            Expr::Set(members) => {
                if members.is_empty() {
                    self.out.push_str("{}");
                    return;
                }
                self.out.push_str("{\n");
                self.indent += 1;
                for member in members {
                    self.write_indent();
                    match string_value(late, member) {
                        Some(name) if is_valid_identifier(name) => self.out.push_str(name),
                        _ => {
                            self.out.push('[');
                            self.expr(member, Ctx::Brackets);
                            self.out.push(']');
                        }
                    }
                    self.out.push_str(" = true,\n");
                }
                self.indent -= 1;
                self.write_indent();
                self.out.push('}');
            }
            Expr::MixedTable(fields) => {
                if fields.is_empty() {
                    self.out.push_str("{}");
                    return;
                }
                self.out.push_str("{\n");
                self.indent += 1;
                for field in fields {
                    self.write_indent();
                    match field {
                        TableField::Value(e) => self.expr(e, Ctx::None),
                        TableField::Field(f) => self.map_field(f),
                    }
                    self.out.push_str(",\n");
                }
                self.indent -= 1;
                self.write_indent();
                self.out.push('}');
            }
        }
    }

    /// `renderMapField`: `name = value`, or `[index] = value`.
    fn map_field(&mut self, field: &MapField) {
        match string_value(self.late_strings, &field.index) {
            Some(name) if is_valid_identifier(name) => self.out.push_str(name),
            _ => {
                self.out.push('[');
                self.expr(&field.index, Ctx::Brackets);
                self.out.push(']');
            }
        }
        self.out.push_str(" = ");
        self.expr(&field.value, Ctx::None);
    }
}

/// The value of a string literal or late string, for the "is it a valid identifier" key shortcuts.
fn string_value<'e>(late_strings: &'e LateStrings, e: &'e Expr) -> Option<&'e str> {
    match e {
        Expr::String(s) => Some(&s.value),
        Expr::LateString(s) => Some(late_value(late_strings, s)),
        _ => None,
    }
}

fn late_value<'e>(late_strings: &'e LateStrings, s: &LateString) -> &'e str {
    late_strings.get(&s.key).unwrap_or_else(|| panic!("late string `{}` has no value", s.key))
}

/// `renderStringLiteral`:`"…"`, else `'…'`, else a long bracket `[=[…]=]` (with spaces inside `[ ]`).
fn string_literal(out: &mut String, value: &str, quote: Option<Quote>, in_brackets: bool) {
    if let Some(quote) = quote {
        out.push(quote.as_char());
        out.push_str(value);
        out.push(quote.as_char());
        return;
    }
    let multiline = value.contains('\n');
    if !multiline && !value.contains('"') {
        out.push('"');
        out.push_str(value);
        out.push('"');
    } else if !multiline && !value.contains('\'') {
        out.push('\'');
        out.push_str(value);
        out.push('\'');
    } else {
        let equals = safe_bracket_equals(value);
        if in_brackets {
            out.push(' ');
        }
        out.push('[');
        out.extend(std::iter::repeat_n('=', equals));
        out.push('[');
        out.push_str(value);
        out.push(']');
        out.extend(std::iter::repeat_n('=', equals));
        out.push(']');
        if in_brackets {
            out.push(' ');
        }
    }
}

/// `getSafeBracketEquals`: the fewest `=` for which neither `]=…=]` occurs in `text` nor `text` ends with `]=…=`.
fn safe_bracket_equals(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut equals = 0;
    loop {
        // `]`, `equals` times `=`, then `]` somewhere, or the same without the last `]` at the end
        let closes_at = |i: usize| {
            bytes[i] == b']' && bytes.len() > i + equals && bytes[i + 1..i + 1 + equals].iter().all(|&b| b == b'=')
        };
        let inside = (0..bytes.len()).any(|i| closes_at(i) && bytes.get(i + 1 + equals) == Some(&b']'));
        let at_end = bytes.len() > equals && closes_at(bytes.len() - 1 - equals);
        if !inside && !at_end {
            return equals;
        }
        equals += 1;
    }
}

/// `renderInterpolatedStringPart`: complete escapes are kept as they are (so an escaped backslash cannot start a
/// `\u{…}` escape), and `{`, `}` and line breaks get a backslash.
fn interpolated_text(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                // \u{hex}, \r\n, or a backslash and any one character
                let rest = &bytes[i + 1..];
                let len = if let Some(hex) = rest.strip_prefix(b"u{")
                    && let Some(close) = hex.iter().position(|&b| b == b'}')
                    && close > 0
                    && hex[..close].iter().all(u8::is_ascii_hexdigit)
                {
                    3 + close + 1
                } else if rest.starts_with(b"\r\n") {
                    3
                } else {
                    1 + text[i + 1..].chars().next().map_or(0, char::len_utf8)
                };
                i += len;
            }
            b'{' | b'}' | b'\n' | b'\r' => {
                out.push_str(&text[copied..i]);
                out.push('\\');
                let len = if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                out.push_str(&text[i..i + len]);
                i += len;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
}

/// `getEnding`: a `;` after a statement that ends in an indexable expression when the next statement (comments
/// skipped) starts with a parenthesis, which Luau would otherwise read as a call.
fn needs_semicolon(statement: &Stmt, rest: &[Stmt]) -> bool {
    let Some(next) = rest.iter().find(|s| !s.is_comment()) else { return false };
    ends_with_indexable(statement) && starts_with_parenthesis(next)
}

fn ends_with_indexable(statement: &Stmt) -> bool {
    fn inner(e: &Expr) -> bool {
        match e {
            // `a`, `(a)`, `a.b`, `a[b]`, `a()`
            _ if e.is_indexable() => true,
            Expr::Binary(node) => inner(&node.right),
            Expr::Unary(node) => inner(&node.expression),
            Expr::If(node) => inner(&node.alternative),
            _ => false,
        }
    }
    match statement {
        Stmt::Call(_) => true,
        Stmt::VariableDeclaration(node) => match node.right.last() {
            Some(e) => inner(e),
            // `local a`: ends with the (indexable) name
            None => !node.left.is_empty(),
        },
        Stmt::Assignment(node) => node.right.last().is_some_and(inner),
        _ => false,
    }
}

fn starts_with_parenthesis(statement: &Stmt) -> bool {
    fn inner(e: &Expr) -> bool {
        match e {
            // a literal base is rendered in parentheses (`base`)
            Expr::Parenthesized(_) => true,
            e if e.is_simple_primitive() => true,
            Expr::Call(node) => inner(&node.expression),
            Expr::MethodCall(node) => inner(&node.expression),
            Expr::PropertyAccess(node) => inner(&node.expression),
            Expr::ComputedIndex(node) => inner(&node.expression),
            _ => false,
        }
    }
    match statement {
        // the callee: `(a)()`, `(a):b()`
        Stmt::Call(Expr::Call(node)) => inner(&node.expression),
        Stmt::Call(Expr::MethodCall(node)) => inner(&node.expression),
        Stmt::Assignment(node) => node.left.first().is_some_and(inner),
        _ => false,
    }
}
