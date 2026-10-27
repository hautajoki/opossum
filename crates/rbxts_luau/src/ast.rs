//! The Luau syntax tree (`LuauAST/types` of `@roblox-ts/luau-ast`).
//!
//! Differences from the TypeScript tree, none of which changes what is rendered:
//! - **No `parent`.** The renderer passes what it needs from the parent down while it walks (parentheses,
//!   long-bracket spacing, block edges for temporary identifiers).
//! - **`luau.List` is a `Vec`.** A field typed `X | List<X>` in TypeScript is a `Vec` here: one element renders the
//!   same as the single node.
//! - **Node kinds are enum variants.** `luau.isX` guards become `match`; the semantic guards (`is_simple`,
//!   `is_indexable`, …) are methods.
//! - **Evaluation facts** ([`Facts`]) ride along on the expression kinds roblox-ts marks; the renderer ignores them.
//! - **Late strings** ([`Expr::LateString`]) are new: string literals whose text is known only at render time.

use crate::facts::Facts;

/// An expression. The first seven variants are TypeScript's `IndexableExpression`s: the ones that can be called,
/// indexed or have a method called on them without parentheses (see [`Expr::is_indexable`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    // indexable expressions
    /// A named local or global.
    Identifier(Identifier),
    /// A local named at render time.
    TemporaryIdentifier(TemporaryIdentifier),
    /// `expression[index]`
    ComputedIndex(Box<ComputedIndexExpression>),
    /// `expression.name`
    PropertyAccess(Box<PropertyAccessExpression>),
    /// `expression(args)`
    Call(Box<CallExpression>),
    /// `expression:name(args)`
    MethodCall(Box<MethodCallExpression>),
    /// `(expression)`
    Parenthesized(Box<ParenthesizedExpression>),

    // expressions
    /// A placeholder for "no expression"; rendering it is a compiler bug.
    None,
    /// `nil`
    Nil,
    /// `false`
    False,
    /// `true`
    True,
    /// A number literal.
    Number(NumberLiteral),
    /// A string literal.
    String(StringLiteral),
    /// A string literal whose value is looked up by key in the [`crate::LateStrings`] given to the renderer
    /// (DESIGN §4 step 5). It renders and behaves exactly like a [`StringLiteral`] with that value and no quote.
    LateString(LateString),
    /// `...`
    VarArgs,
    /// `function(parameters) statements end`
    Function(Box<FunctionExpression>),
    /// `left operator right`
    Binary(Box<BinaryExpression>),
    /// `operator expression`
    Unary(Box<UnaryExpression>),
    /// `if condition then expression else alternative`
    If(Box<IfExpression>),
    /// `` `text{expression}text` ``
    InterpolatedString(InterpolatedString),
    /// `{ a, b }`
    Array(Vec<Expr>),
    /// `{ k = v, [k] = v }`, one field per line.
    Map(Vec<MapField>),
    /// `{ k = true, [k] = true }`, one member per line.
    Set(Vec<Expr>),
    /// Positional and keyed fields, one per line.
    MixedTable(Vec<TableField>),
}

/// A Luau identifier. Rendering asserts that `name` is a valid identifier ([`crate::is_valid_identifier`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Identifier {
    /// The name, as written.
    pub name: String,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// A local whose final name is chosen at render time, unique within its scope: `_name`, `_name_1`, … (or `_`,
/// `_1`, … for an empty name).
///
/// Identity is `id`: every copy of a temporary identifier with the same id renders as the same name, as every clone
/// of the TypeScript node shares its id. Ids are unique per file (see [`crate::TempIds`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TemporaryIdentifier {
    /// The base of the rendered name (`_name`).
    pub name: String,
    /// Unique per file: copies with the same id are the same local.
    pub id: u32,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `expression[index]`, or `expression.name` when the index is a string literal that is a valid identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedIndexExpression {
    /// Should be indexable ([`Expr::is_indexable`]).
    pub expression: Expr,
    /// The index; a string literal that is a valid identifier renders as `.name`.
    pub index: Expr,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `expression.name`, or `expression["name"]` when `name` is not a valid identifier (the name is not escaped).
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyAccessExpression {
    /// Should be indexable ([`Expr::is_indexable`]).
    pub expression: Expr,
    /// The property name, unescaped.
    pub name: String,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `expression(args)`
#[derive(Clone, Debug, PartialEq)]
pub struct CallExpression {
    /// Should be indexable ([`Expr::is_indexable`]).
    pub expression: Expr,
    /// The arguments.
    pub args: Vec<Expr>,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `expression:name(args)`. Rendering asserts that `name` is a valid identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct MethodCallExpression {
    /// The method name.
    pub name: String,
    /// Should be indexable ([`Expr::is_indexable`]).
    pub expression: Expr,
    /// The arguments, after the receiver.
    pub args: Vec<Expr>,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// A number literal, as source text. Text that is not a valid Luau number literal (`0o17`, `Infinity`) is
/// rendered as JavaScript's `String(Number(text))`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NumberLiteral {
    /// The literal's source text.
    pub value: String,
}

/// A string literal. `value` is the text between the quotes, already escaped: the renderer only picks the
/// delimiter (`"`, `'`, then a long bracket `[[…]]` when the text has both quotes or a newline).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StringLiteral {
    /// The text between the quotes, already escaped.
    pub value: String,
    /// An explicit delimiter; `value` must then be escaped for it.
    pub quote: Option<Quote>,
}

/// The delimiter of a [`StringLiteral`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quote {
    /// `"`
    Double,
    /// `'`
    Single,
}

impl Quote {
    /// The delimiter as a character.
    pub fn as_char(self) -> char {
        match self {
            Quote::Double => '"',
            Quote::Single => '\'',
        }
    }
}

/// See [`Expr::LateString`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LateString {
    /// The key of the value in [`crate::LateStrings`].
    pub key: String,
}

/// `function(parameters) statements end`
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionExpression {
    /// The parameters, before any `...`.
    pub parameters: Vec<AnyIdentifier>,
    /// Adds `...` after the parameters.
    pub has_dot_dot_dot: bool,
    /// The body.
    pub statements: Vec<Stmt>,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `(expression)`; rendered without parentheses when the innermost expression is simple.
#[derive(Clone, Debug, PartialEq)]
pub struct ParenthesizedExpression {
    /// The parenthesized expression.
    pub expression: Expr,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `left operator right`, parenthesized when the parent binds tighter.
#[derive(Clone, Debug, PartialEq)]
pub struct BinaryExpression {
    /// The left operand.
    pub left: Expr,
    /// The operator.
    pub operator: BinaryOperator,
    /// The right operand.
    pub right: Expr,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `operator expression`, parenthesized when the parent binds tighter.
#[derive(Clone, Debug, PartialEq)]
pub struct UnaryExpression {
    /// The operator.
    pub operator: UnaryOperator,
    /// The operand.
    pub expression: Expr,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `if condition then expression else alternative`; an [`IfExpression`] alternative renders as `elseif`.
#[derive(Clone, Debug, PartialEq)]
pub struct IfExpression {
    /// The condition.
    pub condition: Expr,
    /// The value when the condition is truthy.
    pub expression: Expr,
    /// The value otherwise.
    pub alternative: Expr,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// `` `text{expression}text` ``
#[derive(Clone, Debug, PartialEq)]
pub struct InterpolatedString {
    /// Text and expressions, in order.
    pub parts: Vec<InterpolatedStringPart>,
    /// Evaluation facts ([`crate::facts`]); not rendered.
    pub facts: Facts,
}

/// A part of an [`InterpolatedString`].
#[derive(Clone, Debug, PartialEq)]
pub enum InterpolatedStringPart {
    /// Literal text, escaped as for a string literal; the renderer escapes `{`, `}` and line breaks.
    Text(String),
    /// An expression, rendered inside `{}`.
    Expression(Expr),
}

/// A keyed field of a [`Expr::Map`] or [`Expr::MixedTable`]: `index = value`, or `[index] = value`.
#[derive(Clone, Debug, PartialEq)]
pub struct MapField {
    /// The key; a string literal that is a valid identifier renders as `name =`, anything else as `[index] =`.
    pub index: Expr,
    /// The value.
    pub value: Expr,
}

/// A field of a [`Expr::MixedTable`].
#[derive(Clone, Debug, PartialEq)]
pub enum TableField {
    /// A positional value.
    Value(Expr),
    /// A keyed field.
    Field(MapField),
}

/// A statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// `left operator right`
    Assignment(Box<Assignment>),
    /// `break`
    Break,
    /// A call statement; the expression is an [`Expr::Call`] or an [`Expr::MethodCall`].
    Call(Expr),
    /// `continue`
    Continue,
    /// `do statements end`
    Do(DoStatement),
    /// `while condition do statements end`
    While(Box<WhileStatement>),
    /// `repeat statements until condition`
    Repeat(Box<RepeatStatement>),
    /// `if condition then statements … end`
    If(Box<IfStatement>),
    /// `for id = start, end, step do statements end`
    NumericFor(Box<NumericForStatement>),
    /// `for ids in expression do statements end`
    For(Box<ForStatement>),
    /// `[local ]function name(parameters) statements end`
    FunctionDeclaration(Box<FunctionDeclaration>),
    /// `function expression:name(parameters) statements end`
    MethodDeclaration(Box<MethodDeclaration>),
    /// `local left = right`
    VariableDeclaration(Box<VariableDeclaration>),
    /// `return expressions`
    Return(ReturnStatement),
    /// `--text`, or a `--[[ ]]` block when the text has several lines.
    Comment(String),
}

/// `left operator right`, with comma-separated lists on both sides.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    /// Writable expressions (identifiers, property accesses, computed indexes); not empty.
    pub left: Vec<Expr>,
    /// `=` or a compound assignment operator.
    pub operator: AssignmentOperator,
    /// Not empty.
    pub right: Vec<Expr>,
}

/// `do statements end`
#[derive(Clone, Debug, PartialEq)]
pub struct DoStatement {
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `while condition do statements end`
#[derive(Clone, Debug, PartialEq)]
pub struct WhileStatement {
    /// The loop condition, checked before each pass.
    pub condition: Expr,
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `repeat statements until condition`
#[derive(Clone, Debug, PartialEq)]
pub struct RepeatStatement {
    /// The condition that ends the loop, checked after each pass.
    pub condition: Expr,
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `if condition then statements [elseif … | else …] end`
#[derive(Clone, Debug, PartialEq)]
pub struct IfStatement {
    /// The condition.
    pub condition: Expr,
    /// The `then` block.
    pub statements: Vec<Stmt>,
    /// What follows the `then` block.
    pub else_body: ElseBody,
}

/// What follows the `then` block of an [`IfStatement`].
#[derive(Clone, Debug, PartialEq)]
pub enum ElseBody {
    /// `elseif …`
    If(Box<IfStatement>),
    /// `else statements`; nothing when empty.
    Block(Vec<Stmt>),
}

impl Default for ElseBody {
    fn default() -> Self {
        ElseBody::Block(Vec::new())
    }
}

/// `for id = start, end[, step] do statements end`; a literal step of 1 is left out.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericForStatement {
    /// The loop variable.
    pub id: AnyIdentifier,
    /// The first value.
    pub start: Expr,
    /// The last value (inclusive).
    pub end: Expr,
    /// The step; `None` leaves it out (a step of 1).
    pub step: Option<Expr>,
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `for ids in expression do statements end`; no ids render as `_`.
#[derive(Clone, Debug, PartialEq)]
pub struct ForStatement {
    /// The loop variables.
    pub ids: Vec<AnyIdentifier>,
    /// The iteration expression.
    pub expression: Expr,
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `[local ]function name(parameters) statements end`
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDeclaration {
    /// `local function`; `name` must then be an identifier.
    pub localize: bool,
    /// An identifier, temporary identifier or property access.
    pub name: Expr,
    /// The parameters, before any `...`.
    pub parameters: Vec<AnyIdentifier>,
    /// Adds `...` after the parameters.
    pub has_dot_dot_dot: bool,
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `function expression:name(parameters) statements end`
#[derive(Clone, Debug, PartialEq)]
pub struct MethodDeclaration {
    /// Should be indexable ([`Expr::is_indexable`]).
    pub expression: Expr,
    /// The method name.
    pub name: String,
    /// The parameters after the implicit `self`, before any `...`.
    pub parameters: Vec<AnyIdentifier>,
    /// Adds `...` after the parameters.
    pub has_dot_dot_dot: bool,
    /// The body.
    pub statements: Vec<Stmt>,
}

/// `local left[ = right]`
#[derive(Clone, Debug, PartialEq)]
pub struct VariableDeclaration {
    /// Not empty.
    pub left: Vec<AnyIdentifier>,
    /// Empty for `local a` (no initializer).
    pub right: Vec<Expr>,
}

/// `return expressions` (comma-separated).
#[derive(Clone, Debug, PartialEq)]
pub struct ReturnStatement {
    /// The returned values, comma-separated.
    pub expressions: Vec<Expr>,
}

/// An identifier or a temporary identifier: what can be declared (parameters, locals, loop variables).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnyIdentifier {
    /// A named identifier.
    Identifier(Identifier),
    /// A temporary identifier.
    Temporary(TemporaryIdentifier),
}

impl From<AnyIdentifier> for Expr {
    fn from(id: AnyIdentifier) -> Expr {
        match id {
            AnyIdentifier::Identifier(id) => Expr::Identifier(id),
            AnyIdentifier::Temporary(id) => Expr::TemporaryIdentifier(id),
        }
    }
}

impl From<Identifier> for Expr {
    fn from(id: Identifier) -> Expr {
        Expr::Identifier(id)
    }
}

impl From<TemporaryIdentifier> for Expr {
    fn from(id: TemporaryIdentifier) -> Expr {
        Expr::TemporaryIdentifier(id)
    }
}

impl From<Identifier> for AnyIdentifier {
    fn from(id: Identifier) -> AnyIdentifier {
        AnyIdentifier::Identifier(id)
    }
}

impl From<TemporaryIdentifier> for AnyIdentifier {
    fn from(id: TemporaryIdentifier) -> AnyIdentifier {
        AnyIdentifier::Temporary(id)
    }
}

impl TryFrom<Expr> for AnyIdentifier {
    type Error = Expr;

    fn try_from(expr: Expr) -> Result<AnyIdentifier, Expr> {
        match expr {
            Expr::Identifier(id) => Ok(AnyIdentifier::Identifier(id)),
            Expr::TemporaryIdentifier(id) => Ok(AnyIdentifier::Temporary(id)),
            other => Err(other),
        }
    }
}

macro_rules! operators {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $(#[doc = concat!("`", $text, "`")] $variant),* }

        impl $name {
            /// The operator as written in Luau.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),* }
            }

            /// The operator written `text`, if any.
            pub fn parse(text: &str) -> Option<Self> {
                match text { $($text => Some($name::$variant),)* _ => None }
            }
        }
    };
}

operators!(
    /// A binary operator.
    BinaryOperator {
        Add = "+", Sub = "-", Mul = "*", Div = "/", FloorDiv = "//", Pow = "^", Mod = "%", Concat = "..",
        Lt = "<", Le = "<=", Gt = ">", Ge = ">=", Eq = "==", Ne = "~=", And = "and", Or = "or",
    }
);

operators!(
    /// A unary operator.
    UnaryOperator { Neg = "-", Not = "not", Len = "#" }
);

operators!(
    /// An assignment operator (Luau's compound assignments included).
    AssignmentOperator {
        Assign = "=", Add = "+=", Sub = "-=", Mul = "*=", Div = "/=", FloorDiv = "//=", Mod = "%=", Pow = "^=",
        Concat = "..=",
    }
);

impl Expr {
    /// TypeScript's `isIndexableExpression`: identifiers, indexing, calls and parenthesized expressions, which can
    /// be called or indexed as they are.
    pub fn is_indexable(&self) -> bool {
        matches!(
            self,
            Expr::Identifier(_)
                | Expr::TemporaryIdentifier(_)
                | Expr::ComputedIndex(_)
                | Expr::PropertyAccess(_)
                | Expr::Call(_)
                | Expr::MethodCall(_)
                | Expr::Parenthesized(_)
        )
    }

    /// `isAnyIdentifier`
    pub fn is_any_identifier(&self) -> bool {
        matches!(self, Expr::Identifier(_) | Expr::TemporaryIdentifier(_))
    }

    /// `isSimple`: identifiers and primitive literals. Cheap to repeat and never needs parentheses.
    pub fn is_simple(&self) -> bool {
        self.is_any_identifier() || self.is_simple_primitive()
    }

    /// `isSimplePrimitive`: `nil`, booleans, numbers and strings.
    pub fn is_simple_primitive(&self) -> bool {
        matches!(self, Expr::Nil | Expr::True | Expr::False | Expr::Number(_) | Expr::String(_) | Expr::LateString(_))
    }

    /// `isTable`: array, set, map and mixed table constructors.
    pub fn is_table(&self) -> bool {
        matches!(self, Expr::Array(_) | Expr::Set(_) | Expr::Map(_) | Expr::MixedTable(_))
    }

    /// `isCall`: a call or a method call.
    pub fn is_call(&self) -> bool {
        matches!(self, Expr::Call(_) | Expr::MethodCall(_))
    }

    /// `isWritableExpression`: what can be assigned to.
    pub fn is_writable(&self) -> bool {
        matches!(
            self,
            Expr::Identifier(_) | Expr::TemporaryIdentifier(_) | Expr::PropertyAccess(_) | Expr::ComputedIndex(_)
        )
    }

    /// `isExpressionWithPrecedence`: binary, unary and if-expressions, which may need parentheses.
    pub fn has_precedence(&self) -> bool {
        matches!(self, Expr::Binary(_) | Expr::Unary(_) | Expr::If(_))
    }

    /// `isNone`
    pub fn is_none(&self) -> bool {
        matches!(self, Expr::None)
    }

    /// The node's evaluation facts, for the kinds that carry them.
    pub fn facts(&self) -> Option<&Facts> {
        Some(match self {
            Expr::Identifier(n) => &n.facts,
            Expr::TemporaryIdentifier(n) => &n.facts,
            Expr::ComputedIndex(n) => &n.facts,
            Expr::PropertyAccess(n) => &n.facts,
            Expr::Call(n) => &n.facts,
            Expr::MethodCall(n) => &n.facts,
            Expr::Parenthesized(n) => &n.facts,
            Expr::Function(n) => &n.facts,
            Expr::Binary(n) => &n.facts,
            Expr::Unary(n) => &n.facts,
            Expr::If(n) => &n.facts,
            Expr::InterpolatedString(n) => &n.facts,
            _ => return None,
        })
    }

    /// Mutable [`Expr::facts`]; `None` for kinds that carry no facts (a mark on them is dropped, see
    /// [`crate::facts`]).
    pub fn facts_mut(&mut self) -> Option<&mut Facts> {
        Some(match self {
            Expr::Identifier(n) => &mut n.facts,
            Expr::TemporaryIdentifier(n) => &mut n.facts,
            Expr::ComputedIndex(n) => &mut n.facts,
            Expr::PropertyAccess(n) => &mut n.facts,
            Expr::Call(n) => &mut n.facts,
            Expr::MethodCall(n) => &mut n.facts,
            Expr::Parenthesized(n) => &mut n.facts,
            Expr::Function(n) => &mut n.facts,
            Expr::Binary(n) => &mut n.facts,
            Expr::Unary(n) => &mut n.facts,
            Expr::If(n) => &mut n.facts,
            Expr::InterpolatedString(n) => &mut n.facts,
            _ => return None,
        })
    }
}

impl AnyIdentifier {
    /// The identifier's evaluation facts.
    pub fn facts(&self) -> &Facts {
        match self {
            AnyIdentifier::Identifier(n) => &n.facts,
            AnyIdentifier::Temporary(n) => &n.facts,
        }
    }

    /// Mutable [`AnyIdentifier::facts`].
    pub fn facts_mut(&mut self) -> &mut Facts {
        match self {
            AnyIdentifier::Identifier(n) => &mut n.facts,
            AnyIdentifier::Temporary(n) => &mut n.facts,
        }
    }
}

impl Stmt {
    /// `isFinalStatement`: `break`, `continue` and `return`, after which only comments may follow in a block.
    pub fn is_final(&self) -> bool {
        matches!(self, Stmt::Break | Stmt::Continue | Stmt::Return(_))
    }

    /// `isFunctionLike` for statements: function and method declarations.
    pub fn is_function_like(&self) -> bool {
        matches!(self, Stmt::FunctionDeclaration(_) | Stmt::MethodDeclaration(_))
    }

    /// `isComment`
    pub fn is_comment(&self) -> bool {
        matches!(self, Stmt::Comment(_))
    }

    /// The statement's own block, for the kinds that have one (`hasStatements`; an if statement's `then` block).
    pub fn statements(&self) -> Option<&Vec<Stmt>> {
        match self {
            Stmt::Do(s) => Some(&s.statements),
            Stmt::While(s) => Some(&s.statements),
            Stmt::Repeat(s) => Some(&s.statements),
            Stmt::If(s) => Some(&s.statements),
            Stmt::NumericFor(s) => Some(&s.statements),
            Stmt::For(s) => Some(&s.statements),
            Stmt::FunctionDeclaration(s) => Some(&s.statements),
            Stmt::MethodDeclaration(s) => Some(&s.statements),
            _ => None,
        }
    }

    /// Mutable [`Stmt::statements`].
    pub fn statements_mut(&mut self) -> Option<&mut Vec<Stmt>> {
        match self {
            Stmt::Do(s) => Some(&mut s.statements),
            Stmt::While(s) => Some(&mut s.statements),
            Stmt::Repeat(s) => Some(&mut s.statements),
            Stmt::If(s) => Some(&mut s.statements),
            Stmt::NumericFor(s) => Some(&mut s.statements),
            Stmt::For(s) => Some(&mut s.statements),
            Stmt::FunctionDeclaration(s) => Some(&mut s.statements),
            Stmt::MethodDeclaration(s) => Some(&mut s.statements),
            _ => None,
        }
    }
}
