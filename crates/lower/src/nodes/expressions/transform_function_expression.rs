//! `nodes/expressions/transformFunctionExpression.ts`

use std::sync::Arc;

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, ModifierFlags, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::nodes::statements::transform_return_statement::transform_return_statement_inner;
use crate::nodes::transform_parameters::{TransformedParameters, transform_parameters};
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::shared::diagnostics::errors;
use crate::util::evaluation::effects::get_function_effects;
use crate::util::evaluation::facts::set_call_effects;
use crate::util::traversal::has_syntactic_modifier;
use crate::util::wrap_statements_as_generator::wrap_statements_as_generator;

/// `transformFunctionExpression`: function expressions and arrow functions.
pub fn transform_function_expression(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    if let Some(name) = node.name() {
        state.add_diagnostic(errors::no_function_expression_name(name));
    }

    let TransformedParameters { parameters, mut statements, has_dot_dot_dot } = transform_parameters(state, node);

    let body = node.body().expect("a function expression has a body");
    if body.kind() == Kind::Block {
        statements.extend(transform_statement_list(state, Some(body), body.statements()));
    } else {
        statements.extend(transform_return_statement_inner(state, body));
    }

    let is_async = has_syntactic_modifier(node, ModifierFlags::Async);
    let is_generator = node.body_data().is_some_and(|b| b.asterisk_token.is_some());

    if is_generator {
        if is_async {
            state.add_diagnostic(errors::no_async_generator_functions(node));
        }
        statements = wrap_statements_as_generator(state, node, statements);
    }

    let effects = (!is_async && !is_generator).then(|| get_function_effects(&parameters, &statements));
    let mut expression = create::function(parameters, has_dot_dot_dot, statements);

    if is_async {
        let callee = state.ts(node, "async");
        expression = create::call(callee, vec![expression]);
    } else if let Some(effects) = effects {
        set_call_effects(&mut expression, Arc::new(effects));
    }

    expression
}
