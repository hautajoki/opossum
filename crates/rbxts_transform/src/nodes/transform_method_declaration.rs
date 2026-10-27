//! `nodes/transformMethodDeclaration.ts`: a method of an object literal or a class, added to the table `ptr` points
//! at.

use rbxts_luau::{Expr, Stmt, create};
use tsrs_ast::{Kind, ModifierFlags, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::transform_parameters::{TransformedParameters, transform_parameters};
use crate::nodes::transform_property_name::transform_property_name;
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::shared::diagnostics::errors;
use crate::util::is_method::is_method;
use crate::util::pointer::{Pointer, assign_to_map_pointer};
use crate::util::traversal::has_syntactic_modifier;
use crate::util::wrap_statements_as_generator::wrap_statements_as_generator;

/// `transformMethodDeclaration`: `function ptr:name() … end` (or `function ptr.name()`) when `ptr` is an identifier
/// and the name is a valid identifier, else `ptr[name] = function() … end` (a map field while `ptr` is an inline
/// map).
pub fn transform_method_declaration(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    ptr: &mut Pointer,
) -> Vec<Stmt> {
    let mut result = Vec::new();

    let Some(body) = node.body() else { return Vec::new() };

    let name_node = node.name().expect("a method has a name");
    if name_node.kind() == Kind::PrivateIdentifier {
        state.add_diagnostic(errors::no_private_identifier(name_node));
        return Vec::new();
    }

    let TransformedParameters { mut parameters, mut statements, has_dot_dot_dot } = transform_parameters(state, node);
    statements.extend(transform_statement_list(state, Some(body), body.statements()));

    let mut name = transform_property_name(state, prereqs, name_node);

    if tsrs_ast::has_decorators(node) || node.parameters().iter().any(|&parameter| tsrs_ast::has_decorators(parameter))
    {
        if !name.is_simple_primitive() {
            let temp = temp_id("key");
            result.push(create::local(temp.clone(), Some(name)));
            name = Expr::TemporaryIdentifier(temp);
        }
        state.set_class_element_object_key(node, name.clone());
    }

    let is_async = has_syntactic_modifier(node, ModifierFlags::Async);

    if node.body_data().is_some_and(|b| b.asterisk_token.is_some()) {
        if is_async {
            state.add_diagnostic(errors::no_async_generator_functions(node));
        }
        statements = wrap_statements_as_generator(state, node, statements);
    }

    // can we use `function class:name() end`?
    if !is_async
        && !matches!(ptr.value, Expr::Map(_))
        && let Expr::String(literal) = &name
        && rbxts_luau::is_valid_identifier(&literal.value)
    {
        if is_method(state, node) {
            parameters.remove(0); // remove `self`
            result.push(create::method_declaration(
                ptr.value.clone(),
                literal.value.clone(),
                parameters,
                has_dot_dot_dot,
                statements,
            ));
        } else {
            result.push(create::function_declaration(
                false,
                create::property(ptr.value.clone(), literal.value.clone()),
                parameters,
                has_dot_dot_dot,
                statements,
            ));
        }
        return result;
    }

    let mut expression = create::function(parameters, has_dot_dot_dot, statements);

    if is_async {
        let callee = state.ts(node, "async");
        expression = create::call(callee, vec![expression]);
    }

    // we have to use `class[name] = function()`
    let mut assignment_prereqs = Prereqs::new();
    assign_to_map_pointer(&mut assignment_prereqs, ptr, name, expression);
    result.extend(assignment_prereqs.statements);

    result
}
