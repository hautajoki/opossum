//! Checker types to Luau types (see the module docs of [`super`]).

use luau::{FunctionType, Indexer, NamedType, TableType, TableTypeField, Type, TypePack};
use tsrs_ast::{Kind, Node, Symbol, SymbolFlags};
use tsrs_checker::{ElementFlags, LiteralValue, ObjectFlags, Signature, SignatureKind, Type as TsType, TypeFlags};
use tsrs_core::P;

use super::{Alias, is_alias_symbol, is_exported_declaration};
use crate::classes::transform_state::TransformState;
use crate::util::types::{is_array_type, is_in_rbxts_types, is_lua_tuple_type, is_map_type, is_set_type};

/// Nesting past this is `any`; aliases start over, so this bounds one anonymous structure.
const MAX_DEPTH: u32 = 24;
/// Type nodes one alias body (or one annotation) may take before it is `any`.
const BUDGET: i32 = 4000;

/// Luau's own type names, which a file's aliases never take.
const LUAU_TYPES: &[&str] = &[
    "any",
    "nil",
    "number",
    "string",
    "boolean",
    "thread",
    "buffer",
    "vector",
    "unknown",
    "never",
    "userdata",
    "table",
    "typeof",
    "Enum",
    "EnumItem",
    "RBXScriptSignal",
    "RBXScriptConnection",
    "RaycastResult",
];

/// The type of an annotation: `t` with a fresh depth and budget.
pub(super) fn ty(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let g = &mut state.types;
    let saved = (g.depth, g.budget, std::mem::take(&mut g.expanding));
    g.depth = 0;
    g.budget = BUDGET;
    let result = emit(state, t);
    let g = &mut state.types;
    let over = g.budget < 0;
    (g.depth, g.budget, g.expanding) = saved;
    if over { Type::any() } else { result }
}

/// Puts type parameters in scope under Luau names unique among the ones already there; returns the names.
pub(super) fn bind(state: &mut TransformState<'_>, params: &[P<TsType>]) -> Vec<String> {
    let mut names = Vec::with_capacity(params.len());
    for &param in params {
        let base = sanitize(param.symbol().map_or("T", |s| s.name()), "T");
        let mut name = base.clone();
        let mut i = 0;
        while state.types.scope.iter().any(|(p, n)| *n == name && *p != param) || names.contains(&name) {
            i += 1;
            name = format!("{base}_{i}");
        }
        state.types.scope.push((param, name.clone()));
        names.push(name);
    }
    names
}

/// A TypeScript name as a Luau one: kept when valid, else with `_` for what Luau does not take.
fn sanitize(name: &str, fallback: &str) -> String {
    if luau::is_valid_identifier(name) {
        return name.to_owned();
    }
    if luau::LUAU_RESERVED_KEYWORDS.contains(&name) {
        return format!("{name}_");
    }
    let mut out: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.is_empty() {
        out = fallback.to_owned();
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    if luau::is_valid_identifier(&out) { out } else { fallback.to_owned() }
}

/// Whether `name` is a type every Luau file sees: Luau's own, or a Roblox one (`@rbxts/types`). Looked up in the
/// global scope, so a file's own `Part` does not hide Roblox's and every file renames it the same.
fn is_global_type_name(state: &mut TransformState<'_>, name: &str) -> bool {
    if LUAU_TYPES.contains(&name) {
        return true;
    }
    if let Some(&known) = state.types.globals.get(name) {
        return known;
    }
    let global = state
        .checker
        .get_global_symbol_exported(name, SymbolFlags::Type, None)
        .is_some_and(|s| s.declarations().iter().any(|&d| is_in_rbxts_types(state, d)));
    state.types.globals.insert(name.to_owned(), global);
    global
}

/// The name an exported type has in its owner's file: deterministic, so every file that requires the owner
/// spells it the same.
fn export_name(state: &mut TransformState<'_>, symbol: P<Symbol>) -> String {
    let base = sanitize(symbol.name(), "Type");
    if is_global_type_name(state, &base) { format!("{base}_") } else { base }
}

/// The file's alias for `symbol`, registered (body still to come) on first use; `None` when it gets none.
pub(super) fn alias_of(state: &mut TransformState<'_>, symbol: P<Symbol>) -> Option<usize> {
    if let Some(&index) = state.types.by_symbol.get(&symbol) {
        return Some(index);
    }
    if !is_alias_symbol(state, symbol) {
        return None;
    }
    let declaration = symbol.declarations().first().copied()?;
    let file = tsrs_ast::get_source_file_of_node(declaration)?;
    let own = file == state.source_file;
    let exported = is_exported_declaration(symbol);
    let base = export_name(state, symbol);
    let name = if own && declaration.parent().is_some_and(|p| p.kind() == Kind::SourceFile) {
        base.clone()
    } else {
        let mut name = base.clone();
        let mut i = 0;
        while state.types.taken.contains(&name) {
            i += 1;
            name = format!("{base}_{i}");
        }
        name
    };
    let file_name = file.get().file_name();
    let owner = match (own, exported) {
        (false, Some(_)) if !file_name.ends_with(".d.ts") => {
            state.locator.get(file_name).map(|location| (location.module.clone(), base))
        }
        _ => None,
    };
    let params = if symbol.flags().intersects(SymbolFlags::Enum) {
        Vec::new()
    } else {
        state.checker.get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol)
    };
    let mut param_names: Vec<String> = Vec::with_capacity(params.len());
    for &param in &params {
        let base = sanitize(param.symbol().map_or("T", |s| s.name()), "T");
        let mut name = base.clone();
        let mut i = 0;
        while param_names.contains(&name) {
            i += 1;
            name = format!("{base}_{i}");
        }
        param_names.push(name);
    }
    state.types.taken.insert(name.clone());
    let index = state.types.aliases.len();
    state.types.aliases.push(Alias {
        symbol,
        name,
        export: own && exported.is_some(),
        params,
        param_names,
        defaults: Vec::new(),
        owner,
        body: None,
    });
    state.types.by_symbol.insert(symbol, index);
    Some(index)
}

/// Computes the structure of alias `index`, its own type parameters in scope.
pub(super) fn alias_body(state: &mut TransformState<'_>, index: usize) {
    let alias = &state.types.aliases[index];
    let symbol = alias.symbol;
    let scope: Vec<(P<TsType>, String)> = alias.params.iter().copied().zip(alias.param_names.iter().cloned()).collect();
    let saved_scope = std::mem::replace(&mut state.types.scope, scope);
    let g = &mut state.types;
    let saved = (g.depth, g.budget, std::mem::take(&mut g.expanding));
    g.depth = 0;
    g.budget = BUDGET;

    let declared = state.checker.get_declared_type_of_symbol_exported(symbol);
    let flags = symbol.flags();
    let body = if flags.intersects(SymbolFlags::Enum) {
        enum_body(declared)
    } else if flags.intersects(SymbolFlags::Class | SymbolFlags::Interface) {
        if let Some(native) = native_roblox(state, declared) { native } else { expand_object(state, declared) }
    } else {
        structure(state, declared, Some(symbol))
    };

    let g = &mut state.types;
    let over = g.budget < 0;
    (g.depth, g.budget, g.expanding) = saved;
    state.types.scope = saved_scope;
    state.types.aliases[index].body = Some(if over { Type::any() } else { body });
}

/// The defaults of alias `index`'s type parameters, which may name the parameters before them.
pub(super) fn alias_defaults(state: &mut TransformState<'_>, index: usize) {
    let alias = &state.types.aliases[index];
    let params = alias.params.clone();
    let scope: Vec<(P<TsType>, String)> = params.iter().copied().zip(alias.param_names.iter().cloned()).collect();
    let saved_scope = std::mem::replace(&mut state.types.scope, scope);
    let mut defaults = Vec::with_capacity(params.len());
    for param in params {
        let default = state.checker.get_default_from_type_parameter_exported(param);
        defaults.push(default.map(|d| ty(state, d)));
    }
    state.types.scope = saved_scope;
    state.types.aliases[index].defaults = defaults;
}

/// An enum's members: `number` for numeric ones, the singleton for string ones.
fn enum_body(declared: P<TsType>) -> Type {
    let members: &[P<TsType>] =
        if declared.flags().intersects(TypeFlags::Union) { declared.types() } else { &[declared] };
    let mut out = Vec::new();
    for &member in members {
        let ty = match member.flags() {
            f if f.intersects(TypeFlags::StringLiteral) => match member.as_literal_type().value() {
                Some(LiteralValue::String(s)) => Type::String(s.to_owned()),
                _ => Type::named("string"),
            },
            f if f.intersects(TypeFlags::StringLike) => Type::named("string"),
            _ => Type::named("number"),
        };
        if !out.contains(&ty) {
            out.push(ty);
        }
    }
    union_of(out)
}

fn union_of(mut members: Vec<Type>) -> Type {
    match members.len() {
        0 => Type::nil(),
        1 => members.remove(0),
        _ => Type::Union(members),
    }
}

/// `t` itself, not the alias named `skip` that it is the declared type of.
fn structure(state: &mut TransformState<'_>, t: P<TsType>, skip: Option<P<Symbol>>) -> Type {
    let flags = t.flags();
    if flags.intersects(TypeFlags::Union) {
        return union(state, t);
    }
    if flags.intersects(TypeFlags::Intersection) {
        return intersection(state, t);
    }
    if flags.intersects(TypeFlags::Object) {
        if let Some(native) = native_roblox(state, t) {
            return native;
        }
        if let Some(collection) = collection(state, t) {
            return collection;
        }
        // `type A = B` names B
        if let Some((symbol, _)) = named(state, t)
            && Some(symbol) != skip
            && let Some(reference) = reference(state, t)
        {
            return reference;
        }
        return expand_object(state, t);
    }
    emit_unnamed(state, t)
}

/// One type, inside an annotation or alias body.
fn emit(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let g = &mut state.types;
    g.budget -= 1;
    if g.depth > MAX_DEPTH || g.budget < 0 {
        return Type::any();
    }
    g.depth += 1;
    let result = emit_inner(state, t);
    state.types.depth -= 1;
    result
}

fn emit_inner(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let flags = t.flags();
    if flags.intersects(TypeFlags::TypeParameter) {
        return type_parameter(state, t);
    }
    if flags.intersects(TypeFlags::Object | TypeFlags::Union)
        && let Some(native) = native_roblox(state, t)
    {
        return native;
    }
    if flags.intersects(TypeFlags::Object)
        && let Some(collection) = collection(state, t)
    {
        return collection;
    }
    if let Some(reference) = reference(state, t) {
        return reference;
    }
    if flags.intersects(TypeFlags::Union) {
        return union(state, t);
    }
    if flags.intersects(TypeFlags::Intersection) {
        return intersection(state, t);
    }
    if flags.intersects(TypeFlags::Object) {
        return expand_object(state, t);
    }
    emit_unnamed(state, t)
}

/// Primitives, literals, and what Luau has no word for.
fn emit_unnamed(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let flags = t.flags();
    if flags.intersects(TypeFlags::Any) {
        return Type::any();
    }
    if flags.intersects(TypeFlags::Unknown) {
        return Type::named("unknown");
    }
    if flags.intersects(TypeFlags::Never) {
        return Type::named("never");
    }
    if flags.intersects(TypeFlags::Void | TypeFlags::Undefined | TypeFlags::Null) {
        return Type::nil();
    }
    if flags.intersects(TypeFlags::StringLiteral)
        && let Some(LiteralValue::String(s)) = t.as_literal_type().value()
    {
        return Type::String(s.to_owned());
    }
    if flags.intersects(TypeFlags::BooleanLiteral) {
        return match t.as_literal_type().value() {
            Some(LiteralValue::Boolean(b)) => Type::Bool(b),
            _ => Type::named("boolean"),
        };
    }
    if flags.intersects(TypeFlags::StringLike) {
        return Type::named("string");
    }
    if flags.intersects(TypeFlags::NumberLike | TypeFlags::BigIntLike) {
        return Type::named("number");
    }
    if flags.intersects(TypeFlags::BooleanLike) {
        return Type::named("boolean");
    }
    if flags.intersects(TypeFlags::TypeParameter) {
        return type_parameter(state, t);
    }
    Type::any()
}

/// A type parameter in scope by its name; `this` as its class or interface; anything else `any`.
fn type_parameter(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    if let Some((_, name)) = state.types.scope.iter().rev().find(|(p, _)| *p == t) {
        return Type::named(name.clone());
    }
    if t.as_type_parameter().is_this_type()
        && let Some(symbol) = t.symbol()
        && let Some(index) = alias_of(state, symbol)
    {
        let alias = &state.types.aliases[index];
        let args = alias
            .params
            .iter()
            .map(|p| match state.types.scope.iter().rev().find(|(q, _)| q == p) {
                Some((_, name)) => Type::named(name.clone()),
                None => Type::any(),
            })
            .collect();
        return Type::Named(Box::new(NamedType { prefix: None, name: alias.name.clone(), args }));
    }
    Type::any()
}

/// Host types without a `_nominal_` marker or a constructor of the same name.
const UNBRANDED_ROBLOX_TYPES: &[&str] =
    &["Enum", "EnumItem", "RaycastResult", "RBXScriptConnection", "RBXScriptSignal"];

/// A Roblox engine type by its Luau name (`Part`, `Vector3`, `Enum.Material`); `@rbxts/types` also declares
/// TypeScript-only helper records, which are not.
fn native_roblox(state: &mut TransformState<'_>, t: P<TsType>) -> Option<Type> {
    let mut candidates: Vec<P<Symbol>> = Vec::new();
    for symbol in [t.symbol(), t.alias().and_then(|a| a.symbol())].into_iter().flatten() {
        if symbol.declarations().iter().any(|&d| is_in_rbxts_types(state, d)) {
            candidates.push(symbol);
        }
    }
    for &symbol in &candidates {
        // `Enum.Material` or one of its items
        let mut s = Some(symbol);
        while let Some(at) = s {
            if let Some(parent) = at.parent()
                && parent.name() == "Enum"
                && luau::is_valid_identifier(at.name())
            {
                return Some(Type::named(format!("Enum.{}", at.name())));
            }
            s = at.parent();
        }
    }
    for &symbol in &candidates {
        let name = symbol.name();
        if !luau::is_valid_identifier(name) {
            continue;
        }
        let native = UNBRANDED_ROBLOX_TYPES.contains(&name)
            || state.checker.get_property_of_type_exported(t, &format!("_nominal_{name}")).is_some()
            || {
                let file = state.source_file.as_node();
                state
                    .checker
                    .resolve_name_exported(name, file, SymbolFlags::Value, false)
                    .is_some_and(|v| v.declarations().iter().any(|&d| is_in_rbxts_types(state, d)))
            };
        if native {
            return Some(Type::named(name));
        }
    }
    None
}

/// Arrays, tuples, maps and sets as Luau tables; `Function` as any function.
fn collection(state: &mut TransformState<'_>, t: P<TsType>) -> Option<Type> {
    if is_array_type(state, t) {
        if t.is_tuple_type() {
            // a table type has no positions: a tuple of one type is an array of it, any other is `{any}`
            let args = state.checker.get_type_arguments_exported(t);
            let element = match args.first() {
                Some(&first) if args.iter().all(|&a| a == first) => emit(state, first),
                _ => Type::any(),
            };
            return Some(Type::array(element));
        }
        let element = match state.checker.get_element_type_of_array_type_exported(t) {
            Some(element) => element,
            None => match state.checker.get_type_arguments_exported(t).first() {
                Some(&first) => first,
                None => return Some(Type::array(Type::any())),
            },
        };
        return Some(Type::array(emit(state, element)));
    }
    if is_map_type(state, t) {
        let args = state.checker.get_type_arguments_exported(t);
        let (key, value) = match args {
            [k, v, ..] => (emit(state, *k), emit(state, *v)),
            _ => (Type::any(), Type::any()),
        };
        return Some(indexer(key, value));
    }
    if is_set_type(state, t) {
        let key = match state.checker.get_type_arguments_exported(t).first() {
            Some(&k) => emit(state, k),
            None => Type::any(),
        };
        return Some(indexer(key, Type::Bool(true)));
    }
    // roblox-ts's own declarations (`Promise`, `Function`, `Object`, …) have no structure worth copying
    let symbol = t.symbol()?;
    let from_compiler_types = symbol.declarations().iter().any(|&d| {
        tsrs_ast::get_source_file_of_node(d).is_some_and(|f| f.get().file_name().contains("/@rbxts/compiler-types/"))
    });
    if !from_compiler_types || !symbol.flags().intersects(SymbolFlags::Interface) {
        return None;
    }
    Some(match symbol.name() {
        "Function" | "CallableFunction" | "NewableFunction" => Type::Function(Box::new(FunctionType {
            var_args: Some(Type::any()),
            returns: TypePack { types: Vec::new(), variadic: Some(Box::new(Type::any())) },
            ..FunctionType::default()
        })),
        "String" => Type::named("string"),
        "Number" => Type::named("number"),
        "Boolean" => Type::named("boolean"),
        _ => Type::any(),
    })
}

fn indexer(key: Type, value: Type) -> Type {
    Type::Table(Box::new(TableType { fields: Vec::new(), indexer: Some(Indexer { key, value, read: false }) }))
}

/// The alias symbol `t` is an instance of, and its type arguments.
fn named(state: &mut TransformState<'_>, t: P<TsType>) -> Option<(P<Symbol>, Vec<P<TsType>>)> {
    if let Some(alias) = t.alias()
        && let Some(symbol) = alias.symbol()
        && symbol.flags().intersects(SymbolFlags::TypeAlias)
        && is_alias_symbol(state, symbol)
    {
        return Some((symbol, alias.type_arguments().to_vec()));
    }
    let reference = t.flags().intersects(TypeFlags::Object) && t.object_flags().intersects(ObjectFlags::Reference);
    let target = if reference { t.target().unwrap_or(t) } else { t };
    let symbol = target.symbol()?;
    if !symbol.flags().intersects(SymbolFlags::Class | SymbolFlags::Interface | SymbolFlags::Enum) {
        return None;
    }
    // the enum itself, not one of its members' literal types
    if symbol.flags().intersects(SymbolFlags::Enum) && !t.flags().intersects(TypeFlags::Union) {
        return None;
    }
    if !is_alias_symbol(state, symbol) {
        return None;
    }
    let mut args = Vec::new();
    if reference {
        let all = state.checker.get_type_arguments_exported(t);
        let outer = target.try_as_interface_type().map_or(0, |i| i.outer_type_parameters().len());
        args = all.get(outer..).unwrap_or(&[]).to_vec();
    }
    Some((symbol, args))
}

/// `Name<args>` for a type the file has (or gets) an alias for.
fn reference(state: &mut TransformState<'_>, t: P<TsType>) -> Option<Type> {
    let (symbol, mut args) = named(state, t)?;
    let index = alias_of(state, symbol)?;
    let params = &state.types.aliases[index].params;
    // `Name` alone leaves the defaulted parameters to the alias's own defaults
    if args.len() < params.len()
        && params[args.len()..].iter().any(|&p| state.checker.get_default_from_type_parameter_exported(p).is_none())
    {
        return None;
    }
    args.truncate(params.len());
    let args = args.into_iter().map(|a| emit(state, a)).collect();
    Some(Type::Named(Box::new(NamedType { prefix: None, name: state.types.aliases[index].name.clone(), args })))
}

/// `A | B`, with `nil` members as `?`, `true | false` as `boolean`, and duplicates dropped.
fn union(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let mut members: Vec<Type> = Vec::new();
    let (mut nil, mut yes, mut no) = (false, false, false);
    for &member in t.types() {
        let flags = member.flags();
        if flags.intersects(TypeFlags::Void | TypeFlags::Undefined | TypeFlags::Null) {
            nil = true;
            continue;
        }
        if flags.intersects(TypeFlags::BooleanLiteral) {
            match member.as_literal_type().value() {
                Some(LiteralValue::Boolean(true)) => yes = true,
                _ => no = true,
            }
            continue;
        }
        let ty = emit(state, member);
        match ty.simple_name() {
            Some("any") => return Type::any(),
            Some("unknown") => return Type::named("unknown"),
            _ => {}
        }
        let parts = match ty {
            Type::Union(parts) => parts,
            Type::Optional(inner) => {
                nil = true;
                vec![*inner]
            }
            ty => vec![ty],
        };
        for part in parts {
            if !members.contains(&part) {
                members.push(part);
            }
        }
    }
    match (yes, no) {
        (true, true) => members.push(Type::named("boolean")),
        (true, false) => members.push(Type::Bool(true)),
        (false, true) => members.push(Type::Bool(false)),
        _ => {}
    }
    if members.is_empty() {
        return Type::nil();
    }
    let ty = union_of(members);
    if nil { ty.optional() } else { ty }
}

/// `A & B`. `{}` drops out, and a primitive intersected with tables is a branded primitive: just the primitive,
/// unless a package declares the brand. A package's brand stands for a Luau type of its own (jecs's `Entity` is
/// `{ __T: T }` on the Luau side), which no primitive matches, so it is `any`.
fn intersection(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let primitive_flags = TypeFlags::NumberLike | TypeFlags::StringLike | TypeFlags::BooleanLike;
    let branded = t.types().iter().any(|m| m.flags().intersects(primitive_flags));
    if branded && t.types().iter().any(|&m| declared_in_package(m)) {
        return Type::any();
    }
    let mut members: Vec<Type> = Vec::new();
    for &member in t.types() {
        if is_empty_object(state, member) {
            continue;
        }
        let ty = emit(state, member);
        if ty.simple_name() == Some("any") {
            return Type::any();
        }
        if !members.contains(&ty) {
            members.push(ty);
        }
    }
    if branded {
        members.retain(|m| !matches!(m, Type::Table(_)));
    }
    match members.len() {
        0 => Type::any(),
        1 => members.remove(0),
        _ => Type::Intersection(members),
    }
}

fn declared_in_package(t: P<TsType>) -> bool {
    t.flags().intersects(TypeFlags::Object)
        && t.symbol()
            .and_then(|s| s.declarations().first().copied())
            .and_then(tsrs_ast::get_source_file_of_node)
            .is_some_and(|file| file.get().file_name().contains("/node_modules/"))
}

/// TypeScript's `{}`, anything but `nil`, or a table of brand fields only, which Luau drops.
fn is_empty_object(state: &mut TransformState<'_>, t: P<TsType>) -> bool {
    t.flags().intersects(TypeFlags::Object)
        && state.checker.get_signatures_of_type_exported(t, SignatureKind::Call).is_empty()
        && state.checker.get_signatures_of_type_exported(t, SignatureKind::Construct).is_empty()
        && state.checker.get_index_infos_of_type_exported(t).is_empty()
        && state.checker.get_properties_of_type_exported(t).iter().all(|p| is_hidden_property(p.name()))
}

fn is_hidden_property(name: &str) -> bool {
    name.starts_with(tsrs_ast::INTERNAL_SYMBOL_NAME_PREFIX)
        || name.starts_with("_nominal_")
        || name.starts_with("__nominal_")
}

/// An object type's structure: a table, a function, or both; a constructor is `any`.
fn expand_object(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    if state.types.expanding.contains(&t) {
        return Type::any();
    }
    state.types.expanding.push(t);
    let result = expand_object_inner(state, t);
    state.types.expanding.pop();
    result
}

fn expand_object_inner(state: &mut TransformState<'_>, t: P<TsType>) -> Type {
    let calls = state.checker.get_signatures_of_type_exported(t, SignatureKind::Call);
    let constructs = state.checker.get_signatures_of_type_exported(t, SignatureKind::Construct);
    if !constructs.is_empty() {
        return Type::any();
    }
    let properties = state.checker.get_properties_of_type_exported(t);
    let infos = state.checker.get_index_infos_of_type_exported(t);
    if !calls.is_empty() && properties.is_empty() && infos.is_empty() {
        return callable(state, calls);
    }

    // Luau tables take one indexer: several merge into `[K1 | K2]: V1 | V2`
    let (mut keys, mut values, mut read) = (Vec::new(), Vec::new(), true);
    for &info in infos {
        let key = emit(state, info.key_type());
        let value = emit(state, info.value_type());
        read &= info.is_readonly();
        keys.push(key);
        values.push(value);
    }
    let mut fields = Vec::new();
    let mut numeric = Vec::new();
    for &property in properties {
        let name = property.name();
        if is_hidden_property(name) {
            continue;
        }
        let property_type = state.checker.get_type_of_symbol_exported(property);
        let mut ty = emit(state, property_type);
        if property.flags().intersects(SymbolFlags::Optional) {
            ty = ty.optional();
        }
        // `{ 0: T }` has no Luau spelling; it joins the number indexer
        if name.bytes().all(|b| b.is_ascii_digit()) {
            numeric.push(ty);
            continue;
        }
        let read = state.checker.is_readonly_symbol_exported(property);
        fields.push(TableTypeField { name: name.to_owned(), ty, read });
    }
    if !numeric.is_empty() && !keys.iter().any(|k: &Type| k.simple_name() == Some("number")) {
        keys.push(Type::named("number"));
        values.push(union_of(numeric));
        read = false;
    }
    let indexer = match keys.len() {
        0 => None,
        1 => Some(Indexer { key: keys.remove(0), value: values.remove(0), read }),
        _ => Some(Indexer { key: Type::Union(keys), value: Type::Union(values), read }),
    };
    if fields.is_empty() && indexer.is_none() {
        // `{}` admits every value but nil, not just tables
        return if calls.is_empty() { Type::any() } else { callable(state, calls) };
    }
    let table = Type::Table(Box::new(TableType { fields, indexer }));
    if calls.is_empty() { table } else { Type::Intersection(vec![callable(state, calls), table]) }
}

/// One function type, or the intersection of the overloads.
fn callable(state: &mut TransformState<'_>, signatures: &[P<Signature>]) -> Type {
    if let [one] = signatures {
        return signature(state, *one);
    }
    Type::Intersection(signatures.iter().map(|&s| signature(state, s)).collect())
}

/// A call signature as a function type. A method takes its receiver as `self`: roblox-ts calls it with `:`.
fn signature(state: &mut TransformState<'_>, sig: P<Signature>) -> Type {
    let saved = state.types.scope.len();
    let generics = bind(state, sig.type_parameters());
    let mut function = FunctionType { generics, ..FunctionType::default() };

    if let Some(this) = sig.this_parameter() {
        let this_type = state.checker.get_type_of_symbol_exported(this);
        if !this_type.flags().intersects(TypeFlags::Void) {
            let ty = emit(state, this_type);
            function.parameters.push((Some("self".to_owned()), ty));
        }
    } else if let Some(declaration) = sig.declaration()
        && matches!(declaration.kind(), Kind::MethodSignature | Kind::MethodDeclaration)
    {
        let ty = receiver(state, declaration);
        function.parameters.push((Some("self".to_owned()), ty));
    }

    let parameters = sig.parameters();
    let min = sig.min_argument_count();
    for (i, &parameter) in parameters.iter().enumerate() {
        let parameter_type = state.checker.get_type_of_symbol_exported(parameter);
        if sig.has_rest_parameter() && i == parameters.len() - 1 {
            function.var_args = Some(match state.checker.get_element_type_of_array_type_exported(parameter_type) {
                Some(element) if !parameter_type.is_tuple_type() => emit(state, element),
                _ => Type::any(),
            });
            break;
        }
        let mut ty = emit(state, parameter_type);
        if i >= min || parameter.flags().intersects(SymbolFlags::Optional) {
            ty = ty.optional();
        }
        let name = parameter.name();
        let name = (luau::is_valid_identifier(name) && name != "self").then(|| name.to_owned());
        function.parameters.push((name, ty));
    }

    let returns = state.checker.get_return_type_of_signature_exported(sig);
    function.returns = if returns.flags().intersects(TypeFlags::Void) {
        TypePack::default()
    } else if let Some(pack) = lua_tuple_pack(state, returns) {
        pack
    } else if returns.flags().intersects(TypeFlags::Union)
        && returns.types().iter().any(|&m| is_lua_tuple_type(state, m))
    {
        TypePack { types: Vec::new(), variadic: Some(Box::new(Type::any())) }
    } else {
        TypePack::one(emit(state, returns))
    };
    state.types.scope.truncate(saved);
    Type::Function(Box::new(function))
}

/// The receiver of a method declared in `declaration`'s interface or class; `any` in an object type literal.
fn receiver(state: &mut TransformState<'_>, declaration: P<Node>) -> Type {
    let Some(parent) = declaration.parent() else { return Type::any() };
    if !matches!(parent.kind(), Kind::InterfaceDeclaration | Kind::ClassDeclaration) {
        return Type::any();
    }
    let Some(symbol) = parent.symbol() else { return Type::any() };
    let declared = state.checker.get_declared_type_of_symbol_exported(symbol);
    emit(state, declared)
}

/// `LuaTuple<[A, B, ...C[]]>` as the return pack `(A, B, ...C)`.
pub(super) fn lua_tuple_pack(state: &mut TransformState<'_>, t: P<TsType>) -> Option<TypePack> {
    if !is_lua_tuple_type(state, t) {
        return None;
    }
    let tuple = *t.alias()?.type_arguments().first()?;
    if !tuple.is_tuple_type() {
        return None;
    }
    let flags = tuple.target()?.as_tuple_type().element_flags();
    let elements = state.checker.get_type_arguments_exported(tuple);
    if elements.len() != flags.len() {
        return None;
    }
    let mut pack = TypePack::default();
    for (i, (&element, &flag)) in elements.iter().zip(&flags).enumerate() {
        if flag.intersects(ElementFlags::Variable) {
            // only a trailing rest has a Luau spelling
            if i != elements.len() - 1 {
                return None;
            }
            let item = state.checker.get_element_type_of_array_type_exported(element).unwrap_or(element);
            let ty = if flag.intersects(ElementFlags::Variadic) && item.flags().intersects(TypeFlags::TypeParameter) {
                Type::any()
            } else {
                emit(state, item)
            };
            pack.variadic = Some(Box::new(ty));
            continue;
        }
        let mut ty = emit(state, element);
        if flag.intersects(ElementFlags::Optional) {
            ty = ty.optional();
        }
        pack.types.push(ty);
    }
    Some(pack)
}
