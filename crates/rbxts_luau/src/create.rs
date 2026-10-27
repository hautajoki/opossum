//! Node builders, mirroring `luau.create` and its helpers (`luau.id`, `luau.string`, `luau.call`, …).
//!
//! The struct literals in [`crate::ast`] work as well; these keep transformer code short. Unlike `luau.create`,
//! nothing is cloned behind the caller's back: a node used twice is `.clone()`d by the caller (a temporary
//! identifier's copies keep its id, so they still render as one name).

use crate::ast::*;
use crate::facts::Facts;
use crate::util::js_number_to_string;

/// Hands out the ids of [`TemporaryIdentifier`]s (`luau.tempId`). One per file: ids only need to be unique within
/// the tree being rendered.
#[derive(Debug, Default)]
pub struct TempIds {
    next: u32,
}

impl TempIds {
    /// An allocator whose first id is 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// A new temporary identifier, rendered as `_name` or `_name_<n>` (`_`, `_<n>` for an empty name).
    pub fn temp_id(&mut self, name: impl Into<String>) -> TemporaryIdentifier {
        let id = self.next;
        self.next += 1;
        TemporaryIdentifier { name: name.into(), id, facts: Facts::default() }
    }

    /// The id the next temporary gets: every temporary made before has a smaller one.
    pub fn next_id(&self) -> u32 {
        self.next
    }

    /// [`TempIds::temp_id`] as an expression.
    pub fn expr(&mut self, name: impl Into<String>) -> Expr {
        Expr::TemporaryIdentifier(self.temp_id(name))
    }
}

/// `luau.id`: an identifier.
pub fn id(name: impl Into<String>) -> Expr {
    Expr::Identifier(Identifier { name: name.into(), facts: Facts::default() })
}

/// An identifier as an [`AnyIdentifier`] (parameters, locals, loop variables).
pub fn any_id(name: impl Into<String>) -> AnyIdentifier {
    AnyIdentifier::Identifier(Identifier { name: name.into(), facts: Facts::default() })
}

/// `luau.none`
pub fn none() -> Expr {
    Expr::None
}

/// `luau.nil`
pub fn nil() -> Expr {
    Expr::Nil
}

/// `luau.bool`
pub fn bool(value: bool) -> Expr {
    if value { Expr::True } else { Expr::False }
}

/// `luau.number`: a literal for non-negative values (as JavaScript's `String(value)`), a negation of one otherwise.
///
/// rbxts-rs: values that have no Luau literal are written as expressions that evaluate to them: NaN as `0/0`,
/// infinity as `math.huge` (and `-math.huge`). TypeScript's `luau.number(NaN)` recurses forever, and its
/// `luau.number(Infinity)` renders `Infinity`, which is an undefined global in Luau.
pub fn number(value: f64) -> Expr {
    if value.is_nan() {
        binary(number(0.0), BinaryOperator::Div, number(0.0))
    } else if value == f64::INFINITY {
        property(id("math"), "huge")
    } else if value >= 0.0 {
        Expr::Number(NumberLiteral { value: js_number_to_string(value) })
    } else {
        unary(UnaryOperator::Neg, number(-value))
    }
}

/// A number literal from source text (kept as written, e.g. `0x10`, `1_000`).
pub fn number_text(text: impl Into<String>) -> Expr {
    Expr::Number(NumberLiteral { value: text.into() })
}

/// `luau.string`: a string literal; `value` is the escaped text between the quotes.
pub fn string(value: impl Into<String>) -> Expr {
    Expr::String(StringLiteral { value: value.into(), quote: None })
}

/// `luau.string(value, quote)`: a string literal with an explicit delimiter; `value` must be escaped for it.
pub fn string_quoted(value: impl Into<String>, quote: Quote) -> Expr {
    Expr::String(StringLiteral { value: value.into(), quote: Some(quote) })
}

/// A string literal whose value is filled in at render time from [`crate::LateStrings`].
pub fn late_string(key: impl Into<String>) -> Expr {
    Expr::LateString(LateString { key: key.into() })
}

/// `luau.comment`
pub fn comment(text: impl Into<String>) -> Stmt {
    Stmt::Comment(text.into())
}

/// `luau.array`
pub fn array(members: Vec<Expr>) -> Expr {
    Expr::Array(members)
}

/// `luau.set`
pub fn set(members: Vec<Expr>) -> Expr {
    Expr::Set(members)
}

/// `luau.map`: keyed fields, in order.
pub fn map(fields: impl IntoIterator<Item = (Expr, Expr)>) -> Expr {
    Expr::Map(fields.into_iter().map(|(index, value)| MapField { index, value }).collect())
}

/// `luau.mixedTable`
pub fn mixed_table(fields: Vec<TableField>) -> Expr {
    Expr::MixedTable(fields)
}

/// `luau.binary`
pub fn binary(left: Expr, operator: BinaryOperator, right: Expr) -> Expr {
    Expr::Binary(Box::new(BinaryExpression { left, operator, right, facts: Facts::default() }))
}

/// `luau.unary`
pub fn unary(operator: UnaryOperator, expression: Expr) -> Expr {
    Expr::Unary(Box::new(UnaryExpression { operator, expression, facts: Facts::default() }))
}

/// `luau.property`: `expression.name`.
pub fn property(expression: Expr, name: impl Into<String>) -> Expr {
    debug_assert!(expression.is_indexable(), "property access on a non-indexable expression");
    Expr::PropertyAccess(Box::new(PropertyAccessExpression { expression, name: name.into(), facts: Facts::default() }))
}

/// `expression[index]`
pub fn computed_index(expression: Expr, index: Expr) -> Expr {
    debug_assert!(expression.is_indexable(), "index into a non-indexable expression");
    Expr::ComputedIndex(Box::new(ComputedIndexExpression { expression, index, facts: Facts::default() }))
}

/// `luau.call`: `expression(args)`.
pub fn call(expression: Expr, args: Vec<Expr>) -> Expr {
    debug_assert!(expression.is_indexable(), "call of a non-indexable expression");
    Expr::Call(Box::new(CallExpression { expression, args, facts: Facts::default() }))
}

/// `expression:name(args)`
pub fn method_call(expression: Expr, name: impl Into<String>, args: Vec<Expr>) -> Expr {
    debug_assert!(expression.is_indexable(), "method call on a non-indexable expression");
    Expr::MethodCall(Box::new(MethodCallExpression { name: name.into(), expression, args, facts: Facts::default() }))
}

/// `(expression)`; rendered without parentheses when the expression is simple.
pub fn paren(expression: Expr) -> Expr {
    Expr::Parenthesized(Box::new(ParenthesizedExpression { expression, facts: Facts::default() }))
}

/// `if condition then expression else alternative`
pub fn if_expr(condition: Expr, expression: Expr, alternative: Expr) -> Expr {
    Expr::If(Box::new(IfExpression { condition, expression, alternative, facts: Facts::default() }))
}

/// `function(parameters[, ...]) statements end`
pub fn function(parameters: Vec<AnyIdentifier>, has_dot_dot_dot: bool, statements: Vec<Stmt>) -> Expr {
    Expr::Function(Box::new(FunctionExpression { parameters, has_dot_dot_dot, statements, facts: Facts::default() }))
}

/// `...`
pub fn var_args() -> Expr {
    Expr::VarArgs
}

/// An interpolated string.
pub fn interpolated_string(parts: Vec<InterpolatedStringPart>) -> Expr {
    Expr::InterpolatedString(InterpolatedString { parts, facts: Facts::default() })
}

/// A call statement; `call` must be a call or a method call.
pub fn call_statement(call: Expr) -> Stmt {
    debug_assert!(call.is_call(), "call statement of something that is not a call");
    Stmt::Call(call)
}

/// `left = right` (or a compound assignment).
pub fn assignment(left: Expr, operator: AssignmentOperator, right: Expr) -> Stmt {
    Stmt::Assignment(Box::new(Assignment { left: vec![left], operator, right: vec![right] }))
}

/// `local left = right`, or `local left` when `right` is `None`.
pub fn local(left: impl Into<AnyIdentifier>, right: Option<Expr>) -> Stmt {
    Stmt::VariableDeclaration(Box::new(VariableDeclaration {
        left: vec![left.into()],
        right: right.into_iter().collect(),
    }))
}

/// `local a, b = x, y`
pub fn locals(left: Vec<AnyIdentifier>, right: Vec<Expr>) -> Stmt {
    Stmt::VariableDeclaration(Box::new(VariableDeclaration { left, right }))
}

/// `return expression`
pub fn return_stmt(expression: Expr) -> Stmt {
    Stmt::Return(ReturnStatement { expressions: vec![expression] })
}

/// `do statements end`
pub fn do_stmt(statements: Vec<Stmt>) -> Stmt {
    Stmt::Do(DoStatement { statements })
}

/// `while condition do statements end`
pub fn while_stmt(condition: Expr, statements: Vec<Stmt>) -> Stmt {
    Stmt::While(Box::new(WhileStatement { condition, statements }))
}

/// `repeat statements until condition`
pub fn repeat_stmt(statements: Vec<Stmt>, condition: Expr) -> Stmt {
    Stmt::Repeat(Box::new(RepeatStatement { condition, statements }))
}

/// `if condition then statements [else …] end`
pub fn if_stmt(condition: Expr, statements: Vec<Stmt>, else_body: ElseBody) -> Stmt {
    Stmt::If(Box::new(IfStatement { condition, statements, else_body }))
}

/// `for id = start, end[, step] do statements end`
pub fn numeric_for(id: AnyIdentifier, start: Expr, end: Expr, step: Option<Expr>, statements: Vec<Stmt>) -> Stmt {
    Stmt::NumericFor(Box::new(NumericForStatement { id, start, end, step, statements }))
}

/// `for ids in expression do statements end`
pub fn for_in(ids: Vec<AnyIdentifier>, expression: Expr, statements: Vec<Stmt>) -> Stmt {
    Stmt::For(Box::new(ForStatement { ids, expression, statements }))
}

/// `[local ]function name(parameters) statements end`
pub fn function_declaration(
    localize: bool,
    name: Expr,
    parameters: Vec<AnyIdentifier>,
    has_dot_dot_dot: bool,
    statements: Vec<Stmt>,
) -> Stmt {
    Stmt::FunctionDeclaration(Box::new(FunctionDeclaration { localize, name, parameters, has_dot_dot_dot, statements }))
}

/// `function expression:name(parameters) statements end`
pub fn method_declaration(
    expression: Expr,
    name: impl Into<String>,
    parameters: Vec<AnyIdentifier>,
    has_dot_dot_dot: bool,
    statements: Vec<Stmt>,
) -> Stmt {
    Stmt::MethodDeclaration(Box::new(MethodDeclaration {
        expression,
        name: name.into(),
        parameters,
        has_dot_dot_dot,
        statements,
    }))
}

/// `break`
pub fn break_stmt() -> Stmt {
    Stmt::Break
}

/// `continue`
pub fn continue_stmt() -> Stmt {
    Stmt::Continue
}
