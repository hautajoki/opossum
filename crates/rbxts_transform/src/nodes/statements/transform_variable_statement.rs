//! `nodes/statements/transformVariableStatement.ts`

use rbxts_luau::{AnyIdentifier, AssignmentOperator, Expr, Stmt, create};
use tsrs_ast::{Kind, Node, NodeFlags};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::binding::transform_array_binding_pattern::{
    is_omitted_binding_element, transform_array_binding_pattern,
};
use crate::nodes::binding::transform_object_binding_pattern::transform_object_binding_pattern;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::transform_initializer::transform_initializer;
use crate::shared::diagnostics::errors;
use crate::util::array_binding_pattern_contains_hoists::{
    array_binding_pattern_contains_hoists, array_like_expression_contains_spread,
};
use crate::util::binding::object_accessor::object_accessor;
use crate::util::binding::transform_binding_pattern::transform_binding_pattern;
use crate::util::check_variable_hoist::check_variable_hoist;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::evaluation::facts::{copy_value_facts, get_call_effects, is_constant_reference};
use crate::util::symbols::{is_symbol_mutable, validate_identifier};
use crate::util::types::is_lua_tuple_type;
use crate::util::validate_not_any::validate_not_any_type;
use crate::util::wrap_expression_statement::wrap_expression_statement;

/// `transformVariable`: declares (or, for `export let`, assigns) one variable. Returns the expression it is now
/// reachable as.
pub fn transform_variable(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    identifier: P<Node>,
    right: Option<Expr>,
) -> Expr {
    validate_identifier(state, identifier);

    let symbol = state.checker.get_symbol_at_location_exported(identifier).expect("a variable has a symbol");

    // export let
    if is_symbol_mutable(state, symbol)
        && let Some(export_access) = state.get_module_id_property_access(symbol)
    {
        if let Some(right) = right {
            prereqs.push(create::assignment(export_access.clone(), AssignmentOperator::Assign, right));
        }
        return export_access;
    }

    let mut left = transform_identifier_defined(state, identifier);
    if let Some(right) = &right
        && is_constant_reference(&left)
    {
        if let Some(effects) = get_call_effects(right) {
            state.multi_transform_state.function_effects.insert(symbol, effects);
        }
        copy_value_facts(right, &mut left);
    }

    check_variable_hoist(state, identifier, symbol);
    if state.is_hoisted.get(&symbol) == Some(&true) {
        // no need to do `x = nil` if the variable is already created
        if let Some(right) = right {
            prereqs.push(create::assignment(left.clone(), AssignmentOperator::Assign, right));
        }
    } else {
        prereqs.push(create::local(as_any_identifier(left.clone()), right));
    }

    left
}

/// `transformOptimizedArrayBindingPattern`: `local a, b = rhs`, for a call returning a LuaTuple (`rhs` is one
/// expression) or an array literal (`rhs` is its members).
fn transform_optimized_array_binding_pattern(
    state: &mut TransformState<'_>,
    binding_pattern: P<Node>,
    rhs: Vec<Expr>,
) -> Vec<Stmt> {
    let mut ids: Vec<AnyIdentifier> = Vec::new();
    let mut binding_statements = Vec::new();
    for &element in binding_pattern.elements() {
        if is_omitted_binding_element(element) {
            ids.push(AnyIdentifier::Temporary(temp_id("")));
            continue;
        }
        let binding_element = element.as_binding_element();
        let name = binding_element.name().expect("a binding element has a name");
        if name.kind() == Kind::Identifier {
            validate_identifier(state, name);
            let id = transform_identifier_defined(state, name);
            ids.push(as_any_identifier(id.clone()));
            if let Some(initializer) = binding_element.initializer() {
                binding_statements.push(transform_initializer(state, id, initializer));
            }
        } else {
            let id = temp_id("binding");
            ids.push(AnyIdentifier::Temporary(id.clone()));
            let id = Expr::TemporaryIdentifier(id);
            if let Some(initializer) = binding_element.initializer() {
                binding_statements.push(transform_initializer(state, id.clone(), initializer));
            }

            let mut binding_prereqs = Prereqs::new();
            if name.kind() == Kind::ArrayBindingPattern {
                transform_array_binding_pattern(state, &mut binding_prereqs, name, id);
            } else {
                transform_object_binding_pattern(state, &mut binding_prereqs, name, id);
            }
            binding_statements.extend(binding_prereqs.statements);
        }
    }

    assert!(!ids.is_empty(), "an optimized binding pattern declares something");
    let mut statements = vec![create::locals(ids, rhs)];
    statements.extend(binding_statements);
    statements
}

/// `transformOptimizedObjectBindingPattern`: `{ a }` or `{ a: b }` reads one named property of the receiver once,
/// without computed key prerequisites.
fn transform_optimized_object_binding_pattern(
    state: &mut TransformState<'_>,
    binding_pattern: P<Node>,
    rhs: Expr,
) -> Vec<Stmt> {
    validate_not_any_type(state, binding_pattern);
    let element = binding_pattern.elements()[0].as_binding_element();
    let name = element.name().expect("a binding element has a name");

    let mut prereqs = Prereqs::new();
    let receiver_type = state.get_type(binding_pattern);
    let access = object_accessor(
        state,
        &mut prereqs,
        convert_to_indexable_expression(rhs),
        receiver_type,
        element.property_name().unwrap_or(name),
    )
    .value;
    let id = transform_variable(state, &mut prereqs, name, Some(access));
    if let Some(initializer) = element.initializer() {
        let statement = transform_initializer(state, id, initializer);
        prereqs.push(statement);
    }

    prereqs.statements
}

/// `transformVariableDeclaration`
pub fn transform_variable_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut statements = Vec::new();
    let declaration = node.as_variable_declaration();
    let initializer = declaration.initializer();
    let mut value = None;
    if let Some(initializer) = initializer {
        // the value is transformed before checking isHoisted, so references inside it can be hoisted
        let mut initializer_prereqs = Prereqs::new();
        value = Some(transform_expression(state, &mut initializer_prereqs, initializer));
        statements.extend(initializer_prereqs.statements);
    }

    let name = declaration.name;
    if name.kind() == Kind::Identifier {
        let mut variable_prereqs = Prereqs::new();
        transform_variable(state, &mut variable_prereqs, name, value);
        statements.extend(variable_prereqs.statements);
    } else {
        let value = value.expect("a destructuring declaration has an initializer");
        // optimize empty destructure
        if name.elements().is_empty() {
            if !matches!(&value, Expr::Array(members) if members.is_empty()) {
                statements.extend(wrap_expression_statement(value));
            }
            return statements;
        }

        let initializer = initializer.expect("a destructuring declaration has an initializer");
        if name.kind() == Kind::ArrayBindingPattern {
            if value.is_call()
                && {
                    let t = state.get_type(initializer);
                    is_lua_tuple_type(state, t)
                }
                && !array_binding_pattern_contains_hoists(state, name)
                && !array_like_expression_contains_spread(name)
            {
                statements.extend(transform_optimized_array_binding_pattern(state, name, vec![value]));
            } else if matches!(&value, Expr::Array(members) if !members.is_empty())
                // we can't localize multiple variables at the same time if any of them are hoisted
                && !array_binding_pattern_contains_hoists(state, name)
                && !array_like_expression_contains_spread(name)
            {
                let Expr::Array(members) = value else { unreachable!() };
                statements.extend(transform_optimized_array_binding_pattern(state, name, members));
            } else {
                let mut binding_prereqs = Prereqs::new();
                transform_binding_pattern(state, &mut binding_prereqs, name, value);
                statements.extend(binding_prereqs.statements);
            }
        } else {
            let element = name.elements()[0].as_binding_element();
            if name.elements().len() == 1
                && element.name().is_some_and(|name| name.kind() == Kind::Identifier)
                && element.dot_dot_dot_token().is_none()
                && element.property_name().is_none_or(|property| property.kind() == Kind::Identifier)
            {
                statements.extend(transform_optimized_object_binding_pattern(state, name, value));
            } else {
                let mut binding_prereqs = Prereqs::new();
                transform_binding_pattern(state, &mut binding_prereqs, name, value);
                statements.extend(binding_prereqs.statements);
            }
        }
    }

    statements
}

/// `isVarDeclaration`
pub fn is_var_declaration(node: P<Node>) -> bool {
    !node.flags().intersects(NodeFlags::Const) && !node.flags().intersects(NodeFlags::Let)
}

/// `transformVariableDeclarationList`
pub fn transform_variable_declaration_list(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    if is_var_declaration(node) {
        state.add_diagnostic(errors::no_var(node));
    }

    let mut statements = Vec::new();
    for &declaration in node.as_variable_declaration_list().declarations.nodes() {
        statements.extend(transform_variable_declaration(state, declaration));
    }
    statements
}

/// `transformVariableStatement`
pub fn transform_variable_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    transform_variable_declaration_list(state, node.as_variable_statement().declaration_list)
}
