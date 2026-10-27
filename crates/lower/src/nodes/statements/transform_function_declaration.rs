//! `nodes/statements/transformFunctionDeclaration.ts`

use std::sync::Arc;

use rbxts_luau::{AssignmentOperator, Expr, Stmt, create};
use tsrs_ast::{ModifierFlags, Node};
use tsrs_core::P;

use crate::classes::prereqs::as_any_identifier;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::transform_parameters::{TransformedParameters, transform_parameters};
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::shared::diagnostics::errors;
use crate::util::evaluation::effects::get_function_effects;
use crate::util::symbols::validate_identifier;
use crate::util::traversal::has_syntactic_modifier;
use crate::util::wrap_statements_as_generator::wrap_statements_as_generator;

/// `transformFunctionDeclaration`: `local function name(...) … end` (or an assignment when hoisted).
pub fn transform_function_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let Some(body) = node.body() else { return Vec::new() };

    let is_export_default = has_syntactic_modifier(node, ModifierFlags::ExportDefault);
    let name_node = node.name();
    assert!(name_node.is_some() || is_export_default, "a function declaration has a name");

    if let Some(name) = name_node {
        validate_identifier(state, name);
    }

    let name: Expr = match name_node {
        Some(name) => transform_identifier_defined(state, name),
        None => create::id("default"),
    };

    let TransformedParameters { parameters, mut statements, has_dot_dot_dot } = transform_parameters(state, node);
    statements.extend(transform_statement_list(state, Some(body), body.statements()));

    let mut localize = is_export_default;
    let symbol = name_node.map(|n| state.checker.get_symbol_at_location_exported(n).expect("a function has a symbol"));
    if let Some(symbol) = symbol {
        localize = state.is_hoisted.get(&symbol) != Some(&true);
    }

    let is_async = has_syntactic_modifier(node, ModifierFlags::Async);
    let is_generator = node.body_data().is_some_and(|b| b.asterisk_token.is_some());
    if let Some(symbol) = symbol
        && !is_async
        && !is_generator
    {
        let effects = get_function_effects(&parameters, &statements);
        state.multi_transform_state.function_effects.insert(symbol, Arc::new(effects));
    }

    if is_generator {
        if is_async {
            state.add_diagnostic(errors::no_async_generator_functions(node));
        }
        statements = wrap_statements_as_generator(state, node, statements);
    }

    if is_async {
        let callee = state.ts(node, "async");
        let right = create::call(callee, vec![create::function(parameters, has_dot_dot_dot, statements)]);
        if localize {
            vec![create::local(as_any_identifier(name), Some(right))]
        } else {
            vec![create::assignment(name, AssignmentOperator::Assign, right)]
        }
    } else {
        vec![create::function_declaration(localize, name, parameters, has_dot_dot_dot, statements)]
    }
}
