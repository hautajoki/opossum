//! `util/types.ts`: type predicates over unions, intersections, base types and constraints.
//!
//! TypeScript's `Type` methods map to checker calls: `getConstraint()` is `getBaseConstraintOfType`, `getBaseTypes()`
//! is `getBaseTypes` for class and interface types, `getProperties()` / `getCallSignatures()` / index types go
//! through the checker as `TypeObject` does.

use tsrs_ast::{Node, Symbol};
use tsrs_checker::{Checker, LiteralValue, ObjectFlags, SignatureKind, Type, TypeFlags};
use tsrs_core::P;

use crate::classes::macro_manager::{NOMINAL_LUA_TUPLE_NAME, symbol_names};
use crate::classes::transform_state::TransformState;

/// A type predicate (`TypeCheck`).
pub type TypeCheck<'c> = &'c dyn Fn(&mut TransformState<'_>, P<Type>) -> bool;

fn types_of(t: P<Type>) -> &'static [P<Type>] {
    t.types()
}

/// `type.getBaseTypes()`: the base types of a class or interface type.
fn base_types(state: &mut TransformState<'_>, t: P<Type>) -> &'static [P<Type>] {
    if t.object_flags().intersects(ObjectFlags::ClassOrInterface) {
        state.checker.get_base_types_exported(t)
    } else {
        &[]
    }
}

/// `type.getConstraint() ?? type`
fn constraint_or_self(state: &mut TransformState<'_>, t: P<Type>) -> P<Type> {
    state.checker.get_base_constraint_of_type_exported(t).unwrap_or(t)
}

fn is_definitely_type_inner(state: &mut TransformState<'_>, t: P<Type>, callbacks: &[TypeCheck<'_>]) -> bool {
    if t.flags().intersects(TypeFlags::Union) {
        types_of(t).iter().all(|&t| is_definitely_type_inner(state, t, callbacks))
    } else if t.flags().intersects(TypeFlags::Intersection) {
        types_of(t).iter().any(|&t| is_definitely_type_inner(state, t, callbacks))
    } else {
        if base_types(state, t).iter().any(|&b| is_definitely_type_inner(state, b, callbacks)) {
            return true;
        }
        callbacks.iter().any(|cb| cb(state, t))
    }
}

/// `isDefinitelyType`: every constituent passes one of the checks.
pub fn is_definitely_type(state: &mut TransformState<'_>, t: P<Type>, callbacks: &[TypeCheck<'_>]) -> bool {
    let t = constraint_or_self(state, t);
    is_definitely_type_inner(state, t, callbacks)
}

fn is_possibly_type_inner(
    state: &mut TransformState<'_>,
    t: P<Type>,
    callbacks: &[TypeCheck<'_>],
    only_undefined: bool,
) -> bool {
    if t.flags().intersects(TypeFlags::UnionOrIntersection) {
        return types_of(t).iter().any(|&t| is_possibly_type_inner(state, t, callbacks, only_undefined));
    }
    if base_types(state, t).iter().any(|&b| is_possibly_type_inner(state, b, callbacks, only_undefined)) {
        return true;
    }
    // type variable without constraint, any, or unknown
    if t.flags().intersects(TypeFlags::TypeVariable | TypeFlags::AnyOrUnknown) {
        return true;
    }
    // defined type
    if is_defined_type(state, t) {
        // if only matching undefined, then defined means not possible
        return !only_undefined;
    }
    callbacks.iter().any(|cb| cb(state, t))
}

/// `isPossiblyType`: some constituent may pass one of the checks.
pub fn is_possibly_type(state: &mut TransformState<'_>, t: P<Type>, callbacks: &[TypeCheck<'_>]) -> bool {
    let t = constraint_or_self(state, t);
    is_possibly_type_inner(state, t, callbacks, false)
}

/// `isPossiblyType(type, isUndefinedType)`: TypeScript compares the callback by identity to treat the empty object
/// type `{}` as defined, so this has its own entry point.
pub fn is_possibly_undefined(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    let t = constraint_or_self(state, t);
    is_possibly_type_inner(state, t, &[&is_undefined_type], true)
}

/// `isDefinedType`: the empty object type `{}`.
pub fn is_defined_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags() == TypeFlags::Object
        && state.checker.get_properties_of_type_exported(t).is_empty()
        && state.checker.get_signatures_of_type_exported(t, SignatureKind::Call).is_empty()
        && state.checker.get_signatures_of_type_exported(t, SignatureKind::Construct).is_empty()
        && state.checker.get_number_index_type(t).is_none()
        && state.checker.get_string_index_type(t).is_none()
}

/// `isAnyType`
pub fn is_any_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    t == state.checker.get_any_type()
}

/// `isBooleanType`
pub fn is_boolean_type(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags().intersects(TypeFlags::Boolean | TypeFlags::BooleanLiteral)
}

/// The regular `true` or `false` literal type (`getTrueType()` without `fresh`): `boolean` is the union of the two.
pub fn regular_boolean_literal(checker: &Checker, value: bool) -> P<Type> {
    let name = if value { "true" } else { "false" };
    let parts = checker.get_boolean_type().types();
    *parts.iter().find(|t| t.as_intrinsic_type().intrinsic_name() == name).expect("boolean is false | true")
}

/// `isBooleanLiteralType(state, value)`
pub fn is_boolean_literal_type(value: bool) -> impl Fn(&mut TransformState<'_>, P<Type>) -> bool {
    move |state, t| {
        if t.flags().intersects(TypeFlags::BooleanLiteral) {
            return t == regular_boolean_literal(state.checker, value);
        }
        is_boolean_type(state, t)
    }
}

/// `isNumberType`
pub fn is_number_type(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags().intersects(TypeFlags::Number | TypeFlags::NumberLike | TypeFlags::NumberLiteral)
}

fn number_literal_value(t: P<Type>) -> Option<f64> {
    if !t.flags().intersects(TypeFlags::NumberLiteral) {
        return None;
    }
    match t.as_literal_type().value() {
        Some(LiteralValue::Number(n)) => Some(n.0),
        _ => None,
    }
}

/// The value of a string literal type (`type.isStringLiteral() && type.value`).
pub fn string_literal_value(t: P<Type>) -> Option<&'static str> {
    if !t.flags().intersects(TypeFlags::StringLiteral) {
        return None;
    }
    match t.as_literal_type().value() {
        Some(LiteralValue::String(s)) => Some(s),
        _ => None,
    }
}

/// `isNumberLiteralType(value)`
pub fn is_number_literal_type(value: f64) -> impl Fn(&mut TransformState<'_>, P<Type>) -> bool {
    move |state, t| {
        if t.flags().intersects(TypeFlags::NumberLiteral) {
            return number_literal_value(t) == Some(value);
        }
        is_number_type(state, t)
    }
}

/// `isNaNType`
pub fn is_nan_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    is_number_type(state, t) && !t.flags().intersects(TypeFlags::NumberLiteral)
}

/// `isStringType`
pub fn is_string_type(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags().intersects(TypeFlags::String | TypeFlags::StringLike | TypeFlags::StringLiteral)
}

/// `isMixedStringType`: a union of strings and other types.
pub fn is_mixed_string_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    let t = constraint_or_self(state, t);
    t.flags().intersects(TypeFlags::Union)
        && !is_definitely_type(state, t, &[&is_string_type])
        && types_of(t).iter().any(|&t| is_definitely_type(state, t, &[&is_string_type]))
}

/// `isArrayType(state)`
pub fn is_array_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    // isArrayLikeType() is true for `any`, so rule it out here
    if t.flags().intersects(TypeFlags::Any) {
        return false;
    }
    if t.is_tuple_type() || state.checker.is_array_like_type_exported(t) {
        return true;
    }
    let symbol = t.symbol();
    let mm = state.macro_manager;
    [
        symbol_names::READONLY_ARRAY,
        symbol_names::ARRAY,
        symbol_names::READ_VOXELS_ARRAY,
        symbol_names::TEMPLATE_STRINGS_ARRAY,
    ]
    .iter()
    .any(|name| symbol.is_some_and(|s| mm.is(s, name)))
}

fn symbol_is_one_of(state: &TransformState<'_>, t: P<Type>, names: &[&str]) -> bool {
    let symbol = t.symbol();
    names.iter().any(|name| symbol.is_some_and(|s| state.macro_manager.is(s, name)))
}

/// `isSetType(state)`
pub fn is_set_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    symbol_is_one_of(state, t, &[symbol_names::SET, symbol_names::READONLY_SET, symbol_names::WEAK_SET])
}

/// `isMapType(state)`
pub fn is_map_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    symbol_is_one_of(state, t, &[symbol_names::MAP, symbol_names::READONLY_MAP, symbol_names::WEAK_MAP])
}

/// `isSharedTableType(state)`
pub fn is_shared_table_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    symbol_is_one_of(state, t, &[symbol_names::SHARED_TABLE])
}

/// `isGeneratorType(state)`
pub fn is_generator_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    symbol_is_one_of(state, t, &[symbol_names::GENERATOR])
}

/// `isIterableFunctionType(state)`
pub fn is_iterable_function_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    symbol_is_one_of(state, t, &[symbol_names::ITERABLE_FUNCTION])
}

/// `isLuaTupleType(state)`
pub fn is_lua_tuple_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    let nominal = state.macro_manager.symbol(NOMINAL_LUA_TUPLE_NAME);
    let property = state.checker.get_property_of_type_exported(t, NOMINAL_LUA_TUPLE_NAME);
    // `type.getProperty(…) === getSymbolOrThrow(…)`: both undefined counts as equal in TypeScript, but the macro
    // manager's lookup throws then, so a missing nominal symbol never compares
    nominal.is_some() && property == nominal
}

/// `isIterableFunctionLuaTupleType(state)`
pub fn is_iterable_function_lua_tuple_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    if is_iterable_function_type(state, t) {
        let first = get_type_arguments(state, t).first().copied();
        return first.is_some_and(|f| is_lua_tuple_type(state, f));
    }
    false
}

/// `isIterableType(state)`
pub fn is_iterable_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    symbol_is_one_of(state, t, &[symbol_names::ITERABLE])
}

/// `isObjectType`
pub fn is_object_type(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags().intersects(TypeFlags::Object)
}

/// `isUndefinedType`
pub fn is_undefined_type(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags().intersects(TypeFlags::Undefined | TypeFlags::Void)
}

/// `isEmptyStringType`
pub fn is_empty_string_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    if let Some(value) = string_literal_value(t) {
        return value.is_empty();
    }
    if t.flags().intersects(TypeFlags::TemplateLiteral) {
        let texts = t.as_template_literal_type().texts();
        return texts.is_empty() || texts.iter().all(|s| s.is_empty());
    }
    is_string_type(state, t)
}

/// `isRobloxType(state)`: declared in `@rbxts/types`.
pub fn is_roblox_type(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    let Some(symbol) = t.symbol() else { return false };
    symbol.declarations().iter().any(|&d| is_in_rbxts_types(state, d))
}

/// Whether `declaration` is in `node_modules/@rbxts/types` (cached per file).
pub fn is_in_rbxts_types(state: &mut TransformState<'_>, declaration: P<Node>) -> bool {
    let Some(file) = tsrs_ast::get_source_file_of_node(declaration) else { return false };
    if let Some(&cached) = state.multi_transform_state.is_rbxts_types_file_cache.get(&file) {
        return cached;
    }
    let result = file.get().file_name().contains("/node_modules/@rbxts/types/");
    state.multi_transform_state.is_rbxts_types_file_cache.insert(file, result);
    result
}

// type utilities

/// `walkTypes`: every constituent of unions and intersections, through constraints.
pub fn walk_types(
    state: &mut TransformState<'_>,
    t: P<Type>,
    callback: &mut dyn FnMut(&mut TransformState<'_>, P<Type>),
) {
    if t.flags().intersects(TypeFlags::UnionOrIntersection) {
        for &t in types_of(t) {
            walk_types(state, t, callback);
        }
    } else {
        // in template literal types, constraint === type and this causes infinite recursion
        match state.checker.get_base_constraint_of_type_exported(t) {
            Some(constraint) if constraint != t => walk_types(state, constraint, callback),
            _ => callback(state, t),
        }
    }
}

/// `getFirstConstructSymbol`: the first construct signature of an interface the expression's type is declared by.
pub fn get_first_construct_symbol(state: &mut TransformState<'_>, expression: P<Node>) -> Option<P<Symbol>> {
    let t = state.get_type(expression);
    let symbol = t.symbol()?;
    for &declaration in symbol.declarations() {
        if declaration.kind() == tsrs_ast::Kind::InterfaceDeclaration {
            for &member in declaration.members() {
                if member.kind() == tsrs_ast::Kind::ConstructSignature {
                    return member.symbol();
                }
            }
        }
    }
    None
}

/// `getFirstDefinedSymbol`
pub fn get_first_defined_symbol(state: &mut TransformState<'_>, t: P<Type>) -> Option<P<Symbol>> {
    if t.flags().intersects(TypeFlags::UnionOrIntersection) {
        for &t in types_of(t) {
            if let Some(symbol) = t.symbol()
                && !state.checker.is_undefined_symbol(symbol)
            {
                return Some(symbol);
            }
        }
        None
    } else {
        t.symbol()
    }
}

/// `getTypeArguments`
pub fn get_type_arguments(state: &mut TransformState<'_>, t: P<Type>) -> &'static [P<Type>] {
    state.checker.get_type_arguments_exported(t)
}
