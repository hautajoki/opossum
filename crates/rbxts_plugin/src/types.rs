//! The plain types of the API: results, sites, call shapes, diagnostics, positions (DESIGN §11.2, §11.3).

use rbxts_project::FileRojoInfo;
use tsrs_ast::{Kind, Node, SourceFile, Symbol};
use tsrs_core::P;
use tsrs_diagnostics::Category;

use crate::{LuauExpr, LuauKey};

/// A plugin-defined label. A hook's interest registration ([`crate::Interest`]) or
/// [`crate::PluginInstance::classify_symbol`] gives it; the compiler passes it back to the hook that the node
/// reaches, so the hook knows why it was called without asking again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(pub u32);

/// A claim hook's answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Claim<T> {
    /// Not this plugin's node: the next plugin is asked, then roblox-ts lowers it itself.
    Decline,
    /// The plugin lowered the node. No later plugin is asked.
    Take(T),
}

/// A hook failed at a node and the failure is already recorded as an error diagnostic. Plugins get one only from
/// [`crate::Cx::error`], [`crate::Cx::report`] and the `Cx` primitives (when a nested node failed), so an `Err`
/// normally has a diagnostic. Making one otherwise takes deliberate misuse (a hand-written backend), and the
/// compiler reports a hook's `Err` without a new error as an internal error. The compiler emits `nil` for the expression or drops the statement, keeps transforming to collect
/// more diagnostics, and writes nothing (DESIGN §11.5).
#[derive(Debug, PartialEq, Eq)]
pub struct Abort(pub(crate) ());

/// The result of a hook or a `Cx` primitive.
pub type PResult<T> = Result<T, Abort>;

/// Stops the build from `setup` or `finish` with a message, as `process.exit` did in TS plugins. In watch mode the
/// compiler reports it and keeps watching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fatal {
    pub message: String,
    pub hint: Option<String>,
}

impl Fatal {
    /// A fatal error without a hint.
    pub fn new(message: impl Into<String>) -> Fatal {
        Fatal {
            message: message.into(),
            hint: None,
        }
    }

    /// Adds a hint line.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Fatal {
        self.hint = Some(hint.into());
        self
    }
}

impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n{hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Fatal {}

/// A source file's position in **this build's** transform order: roblox-ts's order (`getChangedFilePaths` of a clean
/// build: each program file followed by the files that import it, depth first), over every program file, declaration
/// files included. It is the order files are transformed and reported in and file parts reach
/// [`crate::Plugin::finish`] in (TS5 Flamework allocates its ids in it), a function of the program, the same for any
/// checker count and for watch and incremental builds.
///
/// Never persist it, and never key anything across builds by it (file parts, state, late-string keys): use the
/// file's path (`cx.file().path`). The compiler caches parts by normalised path and renumbers them at every
/// finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileIndex(pub u32);

/// Which checker an instance belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckerInfo {
    /// 0-based.
    pub index: usize,
    /// The number of checkers in this build.
    pub count: usize,
}

/// What a plugin is called and when it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    /// Used in diagnostics, timings and error messages ("flamework").
    pub name: &'static str,
    pub version: &'static str,
    /// The plugin is on for a project whose tsconfig `compilerOptions.plugins` has an entry with this `transform`,
    /// and off otherwise. `None` means always on. A `plugins` entry no registered plugin claims is an error ("not
    /// compiled into this toolchain").
    pub tsconfig_transform: Option<&'static str>,
}

/// Why [`crate::PluginInstance::classify_symbol`] is asked about a symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolRole {
    /// The resolved callee of a call, `new` or decorator call ([`crate::Interest::calls_by_symbol`]).
    Callee,
    /// The symbol of the receiver expression of an assignment, update or `delete` target `receiver.key` (or
    /// `receiver[key]`), aliases skipped: for `this.attributes.x`, the `attributes` property
    /// ([`crate::Interest::assignments_by_receiver_symbol`]).
    AssignReceiver,
}

/// The shape of a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallKind {
    /// `f(...)`, `a.b.f(...)` when `f` is not a method (roblox-ts's `isMethod` is false), `f?.(...)`.
    Call,
    /// `obj.m(...)` / `obj:m(...)` in Luau: a method call; [`CallSite::receiver`] is `obj`.
    Method,
    /// `new C(...)`.
    New,
    /// `@D` or `@D(...)` on a class or member. `node` is the Decorator; `args` is empty for `@D`. A claim's value
    /// replaces the decorator expression, which is then applied (`Class = value(Class) or Class`).
    Decorator,
    /// `super(...)` in a constructor.
    SuperCall,
}

/// A call that matched a plugin's call interest.
#[derive(Clone, Copy, Debug)]
pub struct CallSite {
    /// The CallExpression, NewExpression or Decorator.
    pub node: P<Node>,
    pub kind: CallKind,
    /// The callee expression (for a decorator, the decorator's expression without the call).
    pub callee: P<Node>,
    /// The argument nodes; empty for a decorator without a call.
    pub args: &'static [P<Node>],
    /// The resolved callee symbol, aliases skipped, as this instance's checker sees it.
    pub symbol: Option<P<Symbol>>,
    /// `Some(object)` for every property or element call (`obj.m(…)`, `obj[k](…)`, `obj.m?.(…)`), whether or not
    /// `kind` is [`CallKind::Method`]. For `super.m(…)` / `super[k](…)` it is the `super` keyword, which is not a
    /// value: roblox-ts calls `super.m(self, …)` ([`crate::Cx::compile_receiver`] refuses it). `None` for other
    /// calls, `new`, decorators, `super(…)`, and `f?.()`.
    pub receiver: Option<P<Node>>,
    /// The call is part of an optional chain (`a?.b()`, `a?.()`).
    pub in_optional_chain: bool,
    /// False in statement position, where the call's value is dropped.
    pub value_used: bool,
}

/// How a claimed property access is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessPosition {
    Read,
    /// The target of `=`.
    Write,
    /// The target of a compound assignment or `++` / `--`.
    ReadWrite,
    /// The operand of `delete`.
    Delete,
    /// The callee of a call: a method call's, a non-method property call's, an optional call's (`o.m?.()`) and
    /// `super.m()`'s.
    Callee,
}

/// A property access that matched [`crate::Interest::accesses_by_marker`].
#[derive(Clone, Copy, Debug)]
pub struct AccessSite {
    /// The PropertyAccessExpression or ElementAccessExpression.
    pub node: P<Node>,
    pub receiver: P<Node>,
    /// The key, when it is known at compile time: the property name of `obj.k`, or for `obj[e]` the text of a
    /// string literal `e`, else the value of `e`'s type when that type is a single string literal. The type is
    /// read through casts and identifiers: `obj[k as "k"]`, `obj[k]` with `k: "k"`, a string enum member, a
    /// call returning `"k"` all have the key `"k"`.
    ///
    /// This is wider than Flamework's TS transformer, which knows a key only for `obj.k` and a string-literal
    /// argument `obj["k"]`: it leaves `obj[x as "k"]` untouched and reports an error for `obj[k]` with `k: "k"`.
    /// A plugin that needs that stricter rule inspects [`AccessSite::key_node`] itself.
    ///
    /// `None` for any other key (`obj[k]` with `k: string`, a union of literals, a template). A numeric key
    /// (`obj[0]`, a number literal type) is `None` too: Luau tables tell `0` from `"0"`, and roblox-ts offsets
    /// array indices, so no string key stands for it. A `None` site can only be declined or failed
    /// ([`crate::PluginInstance::property_key`]): there is no key to rewrite.
    pub key: Option<&'static str>,
    /// The key's node: the name Identifier of `obj.k`, the argument expression of `obj[e]` (as written,
    /// parentheses and casts included).
    pub key_node: P<Node>,
    pub position: AccessPosition,
}

/// A binary operator, as TypeScript's operator token kind (`Kind::PlusToken`, `Kind::AsteriskAsteriskToken`, …).
/// Compound assignments carry the operator without `=` (`+=` is `PlusToken`).
pub type BinOp = Kind;

/// The operation of an [`AssignSite`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    /// `target = value`
    Assign,
    /// `target op= value`
    Compound(BinOp),
    /// `++target`
    PreInc,
    /// `target++`
    PostInc,
    /// `--target`
    PreDec,
    /// `target--`
    PostDec,
    /// `delete target`
    Delete,
}

/// An assignment, update or `delete` whose target matched
/// [`crate::Interest::assignments_by_receiver_symbol`].
#[derive(Clone, Copy, Debug)]
pub struct AssignSite {
    /// The BinaryExpression, PrefixUnaryExpression, PostfixUnaryExpression or DeleteExpression.
    pub node: P<Node>,
    /// The access being assigned: `receiver.key` or `receiver[key]`.
    pub target: P<Node>,
    pub receiver: P<Node>,
    /// The key, when it is known at compile time, by the rule of [`AccessSite::key`]: the property name, or the
    /// value of a single string literal type of the element access's argument. `None` for a computed key
    /// (numeric keys included); the hook then reads the key from `key_node` (`cx.compile_expr(site.key_node)`), or
    /// declines.
    pub key: Option<&'static str>,
    /// The key's node: the name Identifier of `receiver.key`, the argument expression of `receiver[e]`.
    pub key_node: P<Node>,
    pub op: AssignOp,
    /// The right-hand side; `None` for `++`, `--` and `delete`.
    pub value: Option<P<Node>>,
    /// False in statement position.
    pub value_used: bool,
}

/// A class declaration the plugin's class filter admits.
#[derive(Clone, Debug)]
pub struct ClassSite {
    /// The ClassDeclaration or ClassExpression.
    pub node: P<Node>,
    /// The class's Luau identifier, as the class's own statements use it.
    pub luau_name: LuauExpr,
    /// The class or one of its members has a decorator.
    pub decorated: bool,
}

/// An argument slot of [`crate::Cx::compile_call`].
#[derive(Clone, Debug)]
pub enum Arg {
    /// An original argument node, compiled by roblox-ts. All `Original` slots of one call are compiled together in
    /// order (`ensureTransformOrder`), so their evaluation order is kept.
    Original(P<Node>),
    /// A ready Luau value.
    Luau(LuauExpr),
    /// `nil`, to pad a missing argument.
    Nil,
}

/// The callee of [`crate::Cx::compile_call`].
#[derive(Clone, Debug)]
pub enum Callee {
    /// The call's own callee, lowered as roblox-ts would (a method call stays `obj:m(...)`). On a chain-start
    /// call it is compiled lazily, after any arguments already compiled into `Arg::Luau` (see
    /// [`crate::CxBackend::compile_call`]).
    Original,
    /// Call this value instead: `f(args)`.
    Function(LuauExpr),
    /// Call a method instead: `object:name(args)`. `name` must be a valid Luau identifier
    /// ([`rbxts_luau::is_valid_identifier`]); the renderer asserts it. For any other key, use
    /// `Callee::Function(index)` with `object` as the first argument, after binding `object` to a local with
    /// [`crate::Cx::push_to_var`] so that it is evaluated once (the index and the argument both name it).
    Method { object: LuauExpr, name: String },
}

/// The order of the fields of an object literal compiled with [`ObjectEdits`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FieldOrder {
    /// As written, appended fields last.
    #[default]
    Source,
    /// Emitted field `i` is field `order[i]` of the written-then-appended list. Values are still evaluated in
    /// source order (the compiler binds them to locals first when the order differs and a value has effects).
    Permuted(Vec<u32>),
}

/// Edits applied while roblox-ts compiles an object literal ([`crate::Cx::compile_object_literal`]). Spreads,
/// methods and computed keys are kept and lowered as usual; edits name fields by their written key.
#[derive(Clone, Debug, Default)]
pub struct ObjectEdits {
    /// Written key → emitted key.
    pub rename: Vec<(String, LuauKey)>,
    /// Written key → value. The original value is not evaluated.
    pub replace: Vec<(String, LuauExpr)>,
    /// Extra fields after the written ones.
    pub append: Vec<(LuauKey, LuauExpr)>,
    pub order: FieldOrder,
}

/// A diagnostic made by a plugin. Its location is the node's `getStart()` to its end, as roblox-ts's own.
#[derive(Clone, Debug)]
pub struct PluginDiagnostic {
    pub node: P<Node>,
    pub category: Category,
    pub message: String,
    /// An extra line under the message.
    pub hint: Option<String>,
    /// Related information: (node, message).
    pub related: Vec<(P<Node>, String)>,
    /// Who the diagnostic says it comes from, overriding the plugin's [`crate::Plugin::diagnostic_source`] for this
    /// one diagnostic. `None` (what every constructor sets) uses the plugin's. How the compiler renders a source is
    /// on [`crate::Plugin::diagnostic_source`].
    pub source: Option<&'static str>,
}

impl PluginDiagnostic {
    /// A diagnostic of `category` at `node`.
    pub fn new(node: P<Node>, category: Category, message: impl Into<String>) -> PluginDiagnostic {
        PluginDiagnostic {
            node,
            category,
            message: message.into(),
            hint: None,
            related: Vec::new(),
            source: None,
        }
    }

    /// An error at `node`.
    pub fn error(node: P<Node>, message: impl Into<String>) -> PluginDiagnostic {
        PluginDiagnostic::new(node, Category::Error, message)
    }

    /// A warning at `node`.
    pub fn warning(node: P<Node>, message: impl Into<String>) -> PluginDiagnostic {
        PluginDiagnostic::new(node, Category::Warning, message)
    }

    /// Adds a hint line.
    pub fn with_hint(mut self, hint: impl Into<String>) -> PluginDiagnostic {
        self.hint = Some(hint.into());
        self
    }

    /// Names this diagnostic's source, instead of the plugin's ([`PluginDiagnostic::source`]).
    pub fn with_source(mut self, source: &'static str) -> PluginDiagnostic {
        self.source = Some(source);
        self
    }

    /// Adds related information at `node`.
    pub fn with_related(mut self, node: P<Node>, message: impl Into<String>) -> PluginDiagnostic {
        self.related.push((node, message.into()));
        self
    }
}

/// A position in UTF-16 code units, as TypeScript and Flamework's callsite ids count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Utf16Pos {
    /// From the start of the file.
    pub offset: u32,
    /// 0-based.
    pub line: u32,
    /// 0-based, within the line.
    pub character: u32,
}

/// Locals declared so far in the Luau being built (Luau allows 200 per function).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalCounts {
    /// In the innermost function being built.
    pub function: u32,
    /// In the file's main chunk, hoists and header statements included.
    pub main_chunk: u32,
}

/// A JSDoc tag on a declaration ([`crate::Cx::jsdoc_tags`]).
#[derive(Clone, Debug)]
pub struct JsDocTag {
    /// The JSDocTag node.
    pub node: P<Node>,
    /// The tag name without `@` (`metadata`).
    pub name: &'static str,
    /// The comment text after the name, trimmed; a `{@link X}` is written as its text `X`.
    pub comment: String,
    /// The `{@link}`s in the comment, in order.
    pub links: Vec<JsDocLink>,
}

/// A `{@link name}` inside a JSDoc tag.
#[derive(Clone, Copy, Debug)]
pub struct JsDocLink {
    /// The link's name node (an Identifier or QualifiedName; read its text with [`crate::Cx::source_text`]).
    pub name: P<Node>,
    /// What the name resolves to on this instance's checker (aliases not skipped), if it resolves.
    pub symbol: Option<P<Symbol>>,
    /// The display text after the name: `text` in `{@link Name text}` and in `{@link Name | text}`, trimmed (the
    /// `|` and the spaces around it dropped). `None` when there is none (`{@link Name}`, `{@link Name |}`).
    pub text: Option<&'static str>,
}

/// The file being transformed.
#[derive(Clone, Debug)]
pub struct FileInfo {
    pub source: P<SourceFile>,
    /// This build's index; not stable across builds (see [`FileIndex`]).
    pub index: FileIndex,
    /// The file name as the program spells it (absolute, forward slashes). The stable key of a file.
    pub path: String,
    /// Relative to the project directory, forward slashes (`src/server/main.ts`).
    pub relative_path: String,
    /// Where the output goes, and its place in the Rojo tree.
    pub rojo: FileRojoInfo,
}
