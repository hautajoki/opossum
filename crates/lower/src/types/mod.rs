//! Native Luau types (`Options::native_types`): annotations from the TypeScript checker on the lowered tree.
//!
//! What gets a type:
//! - functions, methods and constructors: their parameters (the written type, or for a plain identifier the
//!   type the checker gives it, so callbacks get their contextual types), `...`, generics, and the return type
//!   unless it is void, admits `nil`, or the function is async or a generator;
//! - locals whose declaration writes a type (inferred locals stay bare, as typescript-go-roblox does), and the
//!   `local a, b` that hoists them;
//! - every interface, type alias, class (its instance type, methods taking `self`) and enum declared at the top
//!   level of the file becomes a `type` alias, `export type` when exported; `Class.new` returns the class type.
//!
//! A named type from elsewhere becomes a local alias of the same name ([`TypeGen`] keeps them; [`finish`] puts
//! them in the file). When the file already requires the module that exports it, the alias is
//! `type Foo<T> = _module.Foo<T>`; otherwise it is the type's structure, copied. A copy names its own named types
//! the same way, so recursion ends at an alias name. Types never add a require: a module bound only as
//! `local x = require(m).x` gets `local _m = require(m)` split out, nothing more.
//!
//! Anything Luau cannot say (conditional, mapped over a type parameter, `keyof`, constructors, a type parameter out
//! of scope, a copy past the depth or size budget) is `any`.

mod emit;

use luau::{AnyIdentifier, Expr, Signature, Stmt, Type, TypePack};
use rustc_hash::{FxHashMap, FxHashSet};
use tsrs_ast::{Kind, ModifierFlags, Node, Symbol, SymbolFlags};
use tsrs_checker::{Type as TsType, TypeFlags};
use tsrs_core::P;

use crate::classes::prereqs::temp_id;
use crate::classes::transform_state::TransformState;
use crate::util::clean_module_name::clean_module_name;
use crate::util::types::is_lua_tuple_type;

/// A `type` alias the file declares: its own type, or a local name for one from elsewhere.
struct Alias {
    symbol: P<Symbol>,
    name: String,
    export: bool,
    /// The type parameters, as declared, and their names.
    params: Vec<P<TsType>>,
    param_names: Vec<String>,
    /// The parameters' defaults, `T = D`.
    defaults: Vec<Option<Type>>,
    /// Exported by another source file under this name: the alias may be `_module.Name`.
    owner: Option<(luau::ModulePath, String)>,
    /// `None` until [`finish`] computes it.
    body: Option<Type>,
}

/// One file's native types: the aliases it declares, and the state of the walk that turns checker types into
/// Luau types.
#[derive(Default)]
pub struct TypeGen {
    started: bool,
    aliases: Vec<Alias>,
    by_symbol: FxHashMap<P<Symbol>, usize>,
    /// Type names taken in the file (aliases), and whether a name is a global type (cached).
    taken: FxHashSet<String>,
    globals: FxHashMap<String, bool>,
    /// The type parameters that can be named where the type being emitted goes, with their names.
    scope: Vec<(P<TsType>, String)>,
    /// Anonymous object types being expanded (a cycle among them is `any`).
    expanding: Vec<P<TsType>>,
    depth: u32,
    /// Type nodes left in the current alias body's budget.
    budget: i32,
}

/// Whether native types are on for this file.
fn enabled(state: &TransformState<'_>) -> bool {
    state.options.native_types
}

/// The types of the function `node` lowered with these parameters: its generics, parameter types (after an
/// implicit `self`), `...` and, unless `body_returns` is false (async, generator), its return.
fn signature(
    state: &mut TransformState<'_>,
    node: P<Node>,
    parameters: &[AnyIdentifier],
    has_dot_dot_dot: bool,
    body_returns: bool,
) -> Box<Signature> {
    start(state);
    let saved = state.types.scope.len();
    // the type parameters of this function and of the functions around it are in scope in Luau too
    let mut binders = Vec::new();
    let mut at = Some(node);
    while let Some(n) = at {
        if tsrs_ast::is_function_like(n) {
            binders.push(n);
        }
        at = n.parent();
    }
    for &binder in binders.iter().rev() {
        let params = type_parameters_of(state, binder);
        let names = emit::bind(state, &params);
        if binder == node {
            let mut sig = Box::new(Signature { generics: names, ..Signature::default() });
            fill_signature(state, node, parameters, has_dot_dot_dot, body_returns, &mut sig);
            state.types.scope.truncate(saved);
            return sig;
        }
    }
    state.types.scope.truncate(saved);
    Box::default()
}

fn type_parameters_of(state: &mut TransformState<'_>, node: P<Node>) -> Vec<P<TsType>> {
    let declarations = match node.kind() {
        Kind::Constructor | Kind::GetAccessor | Kind::SetAccessor | Kind::ClassStaticBlockDeclaration => &[][..],
        _ => node.type_parameters(),
    };
    declarations
        .iter()
        .filter_map(|d| d.symbol())
        .map(|s| state.checker.get_declared_type_of_symbol_exported(s))
        .collect()
}

fn fill_signature(
    state: &mut TransformState<'_>,
    node: P<Node>,
    parameters: &[AnyIdentifier],
    has_dot_dot_dot: bool,
    body_returns: bool,
    sig: &mut Signature,
) {
    let mut types: Vec<Option<Type>> = Vec::new();
    for &parameter in node.parameters() {
        let Some(name) = parameter.name() else { continue };
        if tsrs_ast::is_this_identifier(name) {
            continue;
        }
        let declaration = parameter.as_parameter_declaration();
        let written = parameter.type_node();
        if declaration.dot_dot_dot_token().is_some() {
            if name.kind() == Kind::ArrayBindingPattern {
                // `...[a, b]: [A, B]` is lowered to `(a, b)` when it holds no spread, `...` otherwise
                if !has_dot_dot_dot {
                    for &element in name.elements() {
                        let element_name = element.name().filter(|n| n.kind() == Kind::Identifier);
                        types.push(element_name.map(|n| {
                            let t = state.get_type(n);
                            emit::ty(state, t)
                        }));
                    }
                }
                continue;
            }
            let rest = match written {
                Some(written) => state.checker.get_type_from_type_node_exported(written),
                None => state.get_type(name),
            };
            sig.var_args = Some(match state.checker.get_element_type_of_array_type_exported(rest) {
                Some(element) if !rest.is_tuple_type() => emit::ty(state, element),
                _ => Type::any(),
            });
            continue;
        }
        let t = match written {
            Some(written) => Some(state.checker.get_type_from_type_node_exported(written)),
            None if name.kind() == Kind::Identifier => Some(state.get_type(name)),
            None => None,
        };
        let mut annotation = t.map(|t| emit::ty(state, t));
        if declaration.question_token().is_some() || declaration.initializer().is_some() {
            annotation = annotation.map(Type::optional);
        }
        types.push(annotation);
    }
    // an implicit `self` comes first
    let skip = parameters.len().wrapping_sub(types.len());
    sig.parameters = match skip {
        0 => types,
        1 => std::iter::once(None).chain(types).collect(),
        _ => Vec::new(),
    };
    if body_returns {
        let declared = state.checker.get_signature_from_declaration_exported(node);
        let returns = state.checker.get_return_type_of_signature_exported(declared);
        sig.returns = return_annotation(state, returns);
    }
}

/// The annotation of a function returning `t`: none for void, `null` or anything admitting `nil` (a written `T?`
/// would make Luau ask every path to return), `(...nil)` for `undefined`, a pack for a `LuaTuple`.
fn return_annotation(state: &mut TransformState<'_>, t: P<TsType>) -> Option<TypePack> {
    let flags = t.flags();
    if flags.intersects(TypeFlags::Undefined) {
        return Some(TypePack { types: Vec::new(), variadic: Some(Box::new(Type::nil())) });
    }
    if flags.intersects(TypeFlags::Void | TypeFlags::Null) {
        return None;
    }
    if let Some(pack) = emit::lua_tuple_pack(state, t) {
        return Some(pack);
    }
    if t.flags().intersects(TypeFlags::Union) && t.types().iter().any(|&m| is_lua_tuple_type(state, m)) {
        return Some(TypePack { types: Vec::new(), variadic: Some(Box::new(Type::any())) });
    }
    let ty = emit::ty(state, t);
    (!ty.is_nilable()).then(|| TypePack::one(ty))
}

/// Annotates a lowered function (`Expr::Function`, a function or method declaration) from its TypeScript `node`.
pub fn annotate_function_expr(state: &mut TransformState<'_>, node: P<Node>, function: &mut Expr, body_returns: bool) {
    if !enabled(state) {
        return;
    }
    if let Expr::Function(f) = function {
        f.signature = Some(signature(state, node, &f.parameters, f.has_dot_dot_dot, body_returns));
    }
}

/// [`annotate_function_expr`] for a function or method declaration statement.
pub fn annotate_function_stmt(state: &mut TransformState<'_>, node: P<Node>, function: &mut Stmt, body_returns: bool) {
    if !enabled(state) {
        return;
    }
    match function {
        Stmt::FunctionDeclaration(f) => {
            f.signature = Some(signature(state, node, &f.parameters, f.has_dot_dot_dot, body_returns));
        }
        Stmt::MethodDeclaration(f) => {
            f.signature = Some(signature(state, node, &f.parameters, f.has_dot_dot_dot, body_returns));
        }
        _ => {}
    }
}

/// The written type of the variable `identifier` names, when its declaration writes one.
fn written_variable_type(state: &mut TransformState<'_>, identifier: P<Node>) -> Option<Type> {
    let mut declaration = identifier.parent()?;
    if declaration.kind() != Kind::VariableDeclaration {
        // a hoist is recorded at a use; the declaration is the symbol's
        let symbol = state.checker.get_symbol_at_location_exported(identifier)?;
        declaration = symbol.value_declaration()?;
    }
    if declaration.kind() != Kind::VariableDeclaration || declaration.name()?.kind() != Kind::Identifier {
        return None;
    }
    let written = declaration.type_node()?;
    let t = state.checker.get_type_from_type_node_exported(written);
    // types in the declaration's scope are the ones of the functions around it
    let saved = state.types.scope.len();
    let mut binders = Vec::new();
    let mut at = declaration.parent();
    while let Some(n) = at {
        if tsrs_ast::is_function_like(n) {
            binders.push(n);
        }
        at = n.parent();
    }
    for &binder in binders.iter().rev() {
        let params = type_parameters_of(state, binder);
        emit::bind(state, &params);
    }
    let ty = emit::ty(state, t);
    state.types.scope.truncate(saved);
    Some(ty)
}

/// Annotates `local name = …` (the statement `transform_variable` made for `identifier`) with the written type.
pub fn annotate_local(state: &mut TransformState<'_>, identifier: P<Node>, statement: &mut Stmt) {
    if !enabled(state) {
        return;
    }
    let Stmt::VariableDeclaration(declaration) = statement else { return };
    if declaration.left.len() != 1 {
        return;
    }
    start(state);
    if let Some(ty) = written_variable_type(state, identifier) {
        declaration.types = vec![Some(ty)];
    }
}

/// Annotates the `local a, b` that hoists `identifiers` with their written types.
pub fn annotate_hoist(state: &mut TransformState<'_>, identifiers: &[P<Node>], statement: &mut Stmt) {
    if !enabled(state) {
        return;
    }
    let Stmt::VariableDeclaration(declaration) = statement else { return };
    if declaration.left.len() != identifiers.len() {
        return;
    }
    start(state);
    let types: Vec<Option<Type>> = identifiers.iter().map(|&id| written_variable_type(state, id)).collect();
    if types.iter().any(Option::is_some) {
        declaration.types = types;
    }
}

/// Types a class's `new`: it returns the class type, and its `self` is `any` while the constructor fills it.
pub fn annotate_class_new(
    state: &mut TransformState<'_>,
    node: P<Node>,
    class: &AnyIdentifier,
    statements: &mut [Stmt],
) {
    if !enabled(state) {
        return;
    }
    let Some(symbol) = node.symbol() else { return };
    start(state);
    let Some(index) = emit::alias_of(state, symbol) else { return };
    let alias = &state.types.aliases[index];
    let returns = Type::Named(Box::new(luau::NamedType {
        prefix: None,
        name: alias.name.clone(),
        args: alias.params.iter().map(|_| Type::any()).collect(),
    }));
    let class_expr: Expr = class.clone().into();
    for statement in statements {
        let Stmt::FunctionDeclaration(f) = statement else { continue };
        let Expr::PropertyAccess(name) = &f.name else { continue };
        if name.name != "new" || name.expression != class_expr {
            continue;
        }
        f.signature = Some(Box::new(Signature { returns: Some(TypePack::one(returns)), ..Signature::default() }));
        if let Some(Stmt::VariableDeclaration(local)) = f.statements.first_mut() {
            local.types = vec![Some(Type::any())];
        }
        return;
    }
}

/// Registers every type the file declares at the top level, so their names are reserved before anything else is
/// named, and they are emitted whether used or not.
fn start(state: &mut TransformState<'_>) {
    if state.types.started {
        return;
    }
    state.types.started = true;
    for &statement in state.source_file.get().statements.nodes() {
        if !matches!(
            statement.kind(),
            Kind::InterfaceDeclaration | Kind::TypeAliasDeclaration | Kind::ClassDeclaration | Kind::EnumDeclaration
        ) {
            continue;
        }
        if let Some(symbol) = statement.symbol() {
            emit::alias_of(state, symbol);
        }
    }
}

/// Whether `symbol` is declared by `export` at the top level of a source file, which then declares its alias as
/// `export type`.
fn is_exported_declaration(symbol: P<Symbol>) -> Option<P<Node>> {
    let [declaration] = symbol.declarations() else { return None };
    let declaration = *declaration;
    let flags = tsrs_ast::get_combined_modifier_flags(declaration);
    let top_level = declaration.parent().is_some_and(|p| p.kind() == Kind::SourceFile);
    (top_level
        && flags.intersects(ModifierFlags::Export)
        && !flags.intersects(ModifierFlags::Default | ModifierFlags::Ambient))
    .then_some(declaration)
}

/// The top-level locals bound to a module: `local m = require(path)`, by path.
fn module_locals(statements: &[Stmt]) -> FxHashMap<luau::ModulePath, (usize, AnyIdentifier)> {
    let mut locals = FxHashMap::default();
    for (i, statement) in statements.iter().enumerate() {
        if let Stmt::VariableDeclaration(d) = statement
            && let ([left], [Expr::Require(path)]) = (d.left.as_slice(), d.right.as_slice())
        {
            locals.entry((**path).clone()).or_insert((i, left.clone()));
        }
    }
    locals
}

/// `local x = require(path).y` at the top level, as `local _m = require(path)` and `local x = _m.y`; returns the
/// module local and where it is.
fn split_require(statements: &mut Vec<Stmt>, path: &luau::ModulePath) -> Option<(usize, AnyIdentifier)> {
    let index = statements.iter().position(|s| {
        let Stmt::VariableDeclaration(d) = s else { return false };
        let ([_], [right]) = (d.left.as_slice(), d.right.as_slice()) else { return false };
        let base = match right {
            Expr::ComputedIndex(e) => &e.expression,
            Expr::PropertyAccess(e) => &e.expression,
            _ => return false,
        };
        matches!(base, Expr::Require(p) if **p == *path)
    })?;
    let module =
        AnyIdentifier::Temporary(temp_id(clean_module_name(path.names.last().map_or("module", String::as_str))));
    let Stmt::VariableDeclaration(d) = &mut statements[index] else { return None };
    let base = match &mut d.right[0] {
        Expr::ComputedIndex(e) => &mut e.expression,
        Expr::PropertyAccess(e) => &mut e.expression,
        _ => return None,
    };
    let require = std::mem::replace(base, module.clone().into());
    statements.insert(index, luau::create::local(module.clone(), Some(require)));
    Some((index, module))
}

/// Puts the file's aliases in `statements` (the file's lowered statements, before its header): copies are
/// computed now, a type exported by a module the file requires becomes `_module.Name`.
pub fn finish(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    if !enabled(state) {
        return;
    }
    start(state);
    let mut locals = module_locals(statements);
    // after the last module local an alias names, since `m.T` only resolves below `local m = require(…)`
    let mut after = 0;
    let mut i = 0;
    while i < state.types.aliases.len() {
        if let Some((path, name)) = state.types.aliases[i].owner.clone() {
            let local = match locals.get(&path) {
                Some(local) => Some(local.clone()),
                None => {
                    let split = split_require(statements, &path);
                    if let Some((at, _)) = &split {
                        // the split shifted everything after it by one
                        for (index, _) in locals.values_mut() {
                            if *index >= *at {
                                *index += 1;
                            }
                        }
                        if after > *at {
                            after += 1;
                        }
                    }
                    if let Some(split) = &split {
                        locals.insert(path.clone(), split.clone());
                    }
                    split
                }
            };
            if let Some((index, module)) = local {
                let alias = &mut state.types.aliases[i];
                let args = alias.param_names.iter().map(|n| Type::named(n.clone())).collect();
                alias.body = Some(Type::Named(Box::new(luau::NamedType { prefix: Some(module), name, args })));
                after = after.max(index + 1);
            }
        }
        if state.types.aliases[i].body.is_none() {
            emit::alias_body(state, i);
        }
        emit::alias_defaults(state, i);
        i += 1;
    }
    let aliases: Vec<Stmt> = std::mem::take(&mut state.types.aliases)
        .into_iter()
        .map(|a| {
            let generics = a
                .param_names
                .into_iter()
                .zip(a.defaults)
                .map(|(name, default)| luau::GenericParameter { name, default })
                .collect();
            luau::create::type_alias(a.export, a.name, generics, a.body.unwrap_or_else(Type::any))
        })
        .collect();
    statements.splice(after..after, aliases);
}

/// Whether the symbol is a type Luau gets an alias for: an interface, a type alias with a structure, a named class
/// or an enum.
fn is_alias_symbol(state: &mut TransformState<'_>, symbol: P<Symbol>) -> bool {
    let flags = symbol.flags();
    if flags.intersects(SymbolFlags::Interface | SymbolFlags::Enum) {
        return true;
    }
    if flags.intersects(SymbolFlags::Class) {
        return symbol.declarations().iter().any(|d| d.kind() == Kind::ClassDeclaration && d.name().is_some());
    }
    if flags.intersects(SymbolFlags::TypeAlias) {
        let declared = state.checker.get_declared_type_of_symbol_exported(symbol);
        let structural = TypeFlags::Object
            | TypeFlags::Union
            | TypeFlags::Intersection
            | TypeFlags::Primitive
            | TypeFlags::Any
            | TypeFlags::Unknown
            | TypeFlags::Never;
        return declared.flags().intersects(structural)
            && !declared.object_flags().intersects(tsrs_checker::ObjectFlags::Mapped);
    }
    false
}
