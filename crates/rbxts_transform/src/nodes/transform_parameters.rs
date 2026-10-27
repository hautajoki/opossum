//! `nodes/transformParameters.ts`

use rbxts_luau::{AnyIdentifier, Expr, Stmt, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_array_binding_pattern::{
    is_omitted_binding_element, transform_array_binding_pattern,
};
use crate::nodes::binding::transform_object_binding_pattern::transform_object_binding_pattern;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::transform_initializer::transform_initializer;
use crate::util::array_binding_pattern_contains_hoists::array_like_expression_contains_spread;
use crate::util::is_method::is_method;
use crate::util::symbols::validate_identifier;

/// The parameters of a lowered function.
pub struct TransformedParameters {
    /// The Luau parameters (temporaries for destructured ones).
    pub parameters: Vec<AnyIdentifier>,
    /// The statements the body starts with (defaults, destructuring).
    pub statements: Vec<Stmt>,
    /// The function takes `...` (a rest parameter).
    pub has_dot_dot_dot: bool,
}

/// `optimizeArraySpreadParameter`: parameters in the form `...[a, b, c]: [A, B, C]` become just `(a, b, c)`.
fn optimize_array_spread_parameter(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    parameters: &mut Vec<AnyIdentifier>,
    binding_pattern: P<Node>,
) {
    for &element in binding_pattern.elements() {
        if is_omitted_binding_element(element) {
            parameters.push(AnyIdentifier::Temporary(temp_id("")));
            continue;
        }
        let binding_element = element.as_binding_element();
        let name = binding_element.name().expect("a binding element has a name");
        if name.kind() == Kind::Identifier {
            let param_id = transform_identifier_defined(state, name);
            validate_identifier(state, name);
            parameters.push(as_any_identifier(param_id.clone()));
            if let Some(initializer) = binding_element.initializer() {
                let statement = transform_initializer(state, param_id, initializer);
                prereqs.push(statement);
            }
        } else {
            let param_id = temp_id("param");
            parameters.push(AnyIdentifier::Temporary(param_id.clone()));
            let param_id = Expr::TemporaryIdentifier(param_id);
            if let Some(initializer) = binding_element.initializer() {
                let statement = transform_initializer(state, param_id.clone(), initializer);
                prereqs.push(statement);
            }
            if name.kind() == Kind::ArrayBindingPattern {
                transform_array_binding_pattern(state, prereqs, name, param_id);
            } else {
                transform_object_binding_pattern(state, prereqs, name, param_id);
            }
        }
    }
}

/// `transformParameters`: the Luau parameter list (`self` first for methods), with defaults, rest parameters and
/// destructuring as statements.
pub fn transform_parameters(state: &mut TransformState<'_>, node: P<Node>) -> TransformedParameters {
    let mut parameters: Vec<AnyIdentifier> = Vec::new();
    let mut statements: Vec<Stmt> = Vec::new();
    let mut has_dot_dot_dot = false;

    if is_method(state, node) {
        parameters.push(as_any_identifier(globals::self_()));
    }

    for &parameter in node.parameters() {
        let name = parameter.name().expect("a parameter has a name");
        if tsrs_ast::is_this_identifier(name) {
            continue;
        }
        let declaration = parameter.as_parameter_declaration();
        let is_rest = declaration.dot_dot_dot_token().is_some();

        if is_rest && name.kind() == Kind::ArrayBindingPattern && !array_like_expression_contains_spread(name) {
            let mut parameter_prereqs = Prereqs::new();
            optimize_array_spread_parameter(state, &mut parameter_prereqs, &mut parameters, name);
            statements.extend(parameter_prereqs.statements);
            continue;
        }

        let param_id: Expr = if name.kind() == Kind::Identifier {
            let id = transform_identifier_defined(state, name);
            validate_identifier(state, name);
            id
        } else {
            Expr::TemporaryIdentifier(temp_id("param"))
        };

        if is_rest {
            has_dot_dot_dot = true;
            statements.push(create::local(
                as_any_identifier(param_id.clone()),
                Some(create::array(vec![create::var_args()])),
            ));
        } else {
            parameters.push(as_any_identifier(param_id.clone()));
        }

        if let Some(initializer) = declaration.initializer() {
            statements.push(transform_initializer(state, param_id.clone(), initializer));
        }

        // destructuring
        if name.kind() != Kind::Identifier {
            let mut binding_prereqs = Prereqs::new();
            if name.kind() == Kind::ArrayBindingPattern {
                transform_array_binding_pattern(state, &mut binding_prereqs, name, param_id);
            } else {
                transform_object_binding_pattern(state, &mut binding_prereqs, name, param_id);
            }
            statements.extend(binding_prereqs.statements);
        }
    }

    TransformedParameters { parameters, statements, has_dot_dot_dot }
}
