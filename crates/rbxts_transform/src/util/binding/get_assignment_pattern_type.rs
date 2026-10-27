//! `util/binding/getAssignmentPatternType.ts`: the source type of a destructuring assignment pattern.
//!
//! TypeScript's assignment-pattern query cannot traverse a rest element's parent, so that part of the source path is
//! resolved here, keeping tuple positions through nested rest elements.

use rbxts_luau::js_number_to_string;
use tsrs_ast::{Kind, Node};
use tsrs_checker::{ElementFlags, LiteralValue, Type, TypeFlags};
use tsrs_core::P;
use tsrs_core::jsnum::Number;

use crate::classes::macro_manager::symbol_names;
use crate::classes::transform_state::TransformState;
use crate::util::types::{is_array_type, is_definitely_type};

fn constituents(t: P<Type>) -> Vec<P<Type>> {
    if t.flags().intersects(TypeFlags::Union) { t.types().to_vec() } else { vec![t] }
}

fn index_of(nodes: &[P<Node>], node: P<Node>) -> usize {
    nodes.iter().position(|&n| n == node).expect("a pattern element is in its parent")
}

/// `getIterableElementType`: what iterating `t` yields, through `[Symbol.iterator]().next()`.
fn get_iterable_element_type(state: &mut TransformState<'_>, t: P<Type>, node: P<Node>) -> P<Type> {
    if is_definitely_type(state, t, &[&is_array_type]) {
        return state.checker.get_number_index_type(t).expect("an array type has a number index");
    }

    let iterable_symbol = state.macro_manager.get_symbol_or_throw(symbol_names::ITERABLE);
    let iterable = state.checker.get_declared_type_of_symbol_exported(iterable_symbol);
    let iterator_name = state.checker.get_properties_of_type_exported(iterable)[0].name();
    let iterator = *state
        .checker
        .get_properties_of_type_exported(t)
        .iter()
        .find(|property| property.name() == iterator_name)
        .expect("an iterable type has an iterator method");
    let iterator_function =
        state.checker.get_type_of_symbol_at_location(iterator, Some(node)).expect("the iterator method has a type");
    let signature = state.checker.get_call_signatures(iterator_function)[0];
    let iterator_type = state.checker.get_return_type_of_signature_exported(signature);
    let next = state.checker.get_type_of_property_of_type_exported(iterator_type, "next").expect("an iterator has next");
    let next_signature = state.checker.get_call_signatures(next)[0];
    let result = state.checker.get_return_type_of_signature_exported(next_signature);
    let mut values = Vec::new();
    // `getTrueType()` without `fresh` is the regular `true`, the type of `done: true`
    let true_type = state.checker.regular_true_type;
    for part in constituents(result) {
        if state.checker.get_type_of_property_of_type_exported(part, "done") != Some(true_type) {
            let value =
                state.checker.get_type_of_property_of_type_exported(part, "value").expect("an iterator result has a value");
            values.push(value);
        }
    }
    state.checker.get_union_type(&values)
}

/// The array literal around a `SpreadElement` pattern parent, and the spread's index in it.
fn spread_container(spread: P<Node>) -> (P<Node>, usize) {
    let container = spread.parent().expect("a spread has a parent");
    assert!(container.kind() == Kind::ArrayLiteralExpression, "a pattern spread is in an array literal");
    (container, index_of(container.elements(), spread))
}

/// `getElementType`: the type of element `index` of the array assignment pattern `pattern`.
fn get_element_type(state: &mut TransformState<'_>, pattern: P<Node>, index: f64) -> P<Type> {
    let parent = pattern.parent().expect("a pattern has a parent");
    if parent.kind() == Kind::SpreadElement {
        let (container, spread_index) = spread_container(parent);
        return get_element_type(state, container, spread_index as f64 + index);
    }
    let t = get_assignment_pattern_type(state, pattern);
    // contextual property lookup includes tuple positions beyond the fixed prefix
    let apparent = state.checker.get_apparent_type_exported(t);
    match state.checker.get_type_of_property_of_contextual_type_exported(apparent, &js_number_to_string(index)) {
        Some(element) => element,
        None => get_iterable_element_type(state, t, pattern),
    }
}

/// `getRestElementType`: the element type of the rest of the array assignment pattern `pattern` from `index` on.
fn get_rest_element_type(state: &mut TransformState<'_>, pattern: P<Node>, index: usize) -> P<Type> {
    let parent = pattern.parent().expect("a pattern has a parent");
    if parent.kind() == Kind::SpreadElement {
        let (container, spread_index) = spread_container(parent);
        return get_rest_element_type(state, container, spread_index + index);
    }

    let pattern_type = get_assignment_pattern_type(state, pattern);
    let t = state.checker.get_apparent_type_exported(pattern_type);
    let mut values = Vec::new();
    for part in constituents(t) {
        if part.is_tuple_type() {
            let elements = state.checker.get_type_arguments_exported(part);
            let target = part.target().expect("a tuple reference has a target");
            let tuple = target.as_tuple_type();
            let element_flags = tuple.element_flags();
            // a variable tail can still contribute elements after its first position was consumed
            for i in index.min(tuple.fixed_length())..elements.len() {
                let element = elements[i];
                values.push(if element_flags[i].intersects(ElementFlags::Variadic) {
                    get_iterable_element_type(state, element, pattern)
                } else {
                    element
                });
            }
        } else {
            values.push(get_iterable_element_type(state, part, pattern));
        }
    }
    state.checker.get_union_type(&values)
}

/// `Number(key.value)` of a string or number literal type.
fn literal_number(t: P<Type>) -> Option<f64> {
    match t.as_literal_type().value.get() {
        Some(LiteralValue::Number(n)) => Some(n.0),
        Some(LiteralValue::String(s)) => Some(rbxts_luau::js_string_to_number(s)),
        _ => None,
    }
}

/// `String(key.value)` of a string or number literal type.
fn literal_string(t: P<Type>) -> String {
    match t.as_literal_type().value.get() {
        Some(LiteralValue::Number(n)) => js_number_to_string(n.0),
        Some(LiteralValue::String(s)) => s.to_owned(),
        _ => unreachable!("a string or number literal type"),
    }
}

/// `getPropertyType`: the type of the property `name` of the object assignment pattern `pattern`.
fn get_property_type(state: &mut TransformState<'_>, pattern: P<Node>, name: P<Node>) -> P<Type> {
    let t = get_assignment_pattern_type(state, pattern);
    let key_type = match name.kind() {
        Kind::ComputedPropertyName => state.get_type(name.as_computed_property_name().expression),
        Kind::NumericLiteral => {
            state.checker.get_number_literal_type(Number(rbxts_luau::js_string_to_number(name.text())))
        }
        _ => state.checker.get_string_literal_type(name.text()),
    };
    let mut values = Vec::new();
    for key in constituents(key_type) {
        let mut value = None;
        if key.flags().intersects(TypeFlags::StringLiteral | TypeFlags::NumberLiteral) {
            let parent = pattern.parent().expect("a pattern has a parent");
            if parent.kind() == Kind::SpreadElement {
                let (container, spread_index) = spread_container(parent);
                let offset = literal_number(key).expect("a string or number literal");
                value = Some(get_element_type(state, container, spread_index as f64 + offset));
            } else {
                let apparent = state.checker.get_apparent_type_exported(t);
                value = state.checker.get_type_of_property_of_contextual_type_exported(apparent, &literal_string(key));
            }
        }
        // valid array keys include numeric-string types, which use the number index signature
        let value = match value {
            Some(value) => value,
            None => {
                let index_kind = if is_definitely_type(state, t, &[&is_array_type])
                    || key.flags().intersects(TypeFlags::NumberLike)
                {
                    state.checker.get_number_type()
                } else {
                    state.checker.get_string_type()
                };
                state.checker.get_index_type_of_type_exported(t, index_kind).expect("an index signature")
            }
        };
        values.push(value);
    }
    state.checker.get_union_type(&values)
}

/// `hasRestAssignmentAncestor`
fn has_rest_assignment_ancestor(pattern: P<Node>) -> bool {
    let mut node = pattern;
    while tsrs_ast::is_assignment_target(node) {
        let parent = node.parent().expect("an assignment target has a parent");
        if parent.kind() == Kind::SpreadElement {
            return true;
        }
        // property wrappers belong to the pattern, but independent assignments end the search
        node = if parent.kind() == Kind::PropertyAssignment {
            parent.parent().expect("a property assignment has a parent")
        } else {
            parent
        };
    }
    false
}

/// `getAssignmentPatternType`: the type flowing into the assignment pattern `pattern` (an array or object literal on
/// the left of a destructuring assignment, possibly nested).
pub fn get_assignment_pattern_type(state: &mut TransformState<'_>, pattern: P<Node>) -> P<Type> {
    if !has_rest_assignment_ancestor(pattern) {
        return type_of_assignment_pattern(state, pattern);
    }
    let parent = pattern.parent().expect("a pattern has a parent");
    if parent.kind() == Kind::SpreadElement {
        let (container, spread_index) = spread_container(parent);
        let element = get_rest_element_type(state, container, spread_index);
        return state.checker.create_array_type(element);
    }

    let element = if parent.kind() == Kind::BinaryExpression { parent } else { pattern };
    let container = element.parent().expect("a pattern element has a parent");
    let source = if container.kind() == Kind::ArrayLiteralExpression {
        get_element_type(state, container, index_of(container.elements(), element) as f64)
    } else {
        assert!(container.kind() == Kind::PropertyAssignment, "a nested object pattern is a property's value");
        let object = container.parent().expect("a property assignment has a parent");
        get_property_type(state, object, container.as_property_assignment().name())
    };
    if element.kind() == Kind::BinaryExpression {
        let non_nullable = state.checker.get_non_nullable_type(source);
        let default = state.get_type(element.as_binary_expression().right());
        state.checker.get_union_type(&[non_nullable, default])
    } else {
        source
    }
}

/// `checker.getTypeOfAssignmentPattern`
pub fn type_of_assignment_pattern(state: &mut TransformState<'_>, pattern: P<Node>) -> P<Type> {
    state.checker.get_type_of_assignment_pattern(pattern).unwrap_or_else(|| state.checker.get_error_type())
}
