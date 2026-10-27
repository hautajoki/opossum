//! `nodes/class/transformClassConstructor.ts`: `function Class:constructor(…)`, with parameter properties and
//! instance property initializers.

use rbxts_luau::{AnyIdentifier, AssignmentOperator, Expr, Stmt, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::transform_parameters::{TransformedParameters, transform_parameters};
use crate::nodes::transform_property_name::transform_property_name;
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::shared::diagnostics::errors;
use crate::util::symbols::{get_extends_node, get_statements};

const CONSTRUCTOR: &str = "constructor";

/// `self[name] = initializer` for each instance property with an initializer.
fn transform_property_initializers(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut statements = Vec::new();
    for &member in node.members() {
        if member.kind() != Kind::PropertyDeclaration || tsrs_ast::has_static_modifier(member) {
            continue;
        }

        let name = member.name().expect("a property has a name");
        if name.kind() == Kind::PrivateIdentifier {
            state.add_diagnostic(errors::no_private_identifier(node));
            continue;
        }

        let Some(initializer) = member.initializer() else { continue };

        let mut index_prereqs = Prereqs::new();
        let index = transform_property_name(state, &mut index_prereqs, name);
        statements.extend(index_prereqs.statements);

        let mut right_prereqs = Prereqs::new();
        let right = transform_expression(state, &mut right_prereqs, initializer);
        statements.extend(right_prereqs.statements);

        statements.push(create::assignment(
            create::computed_index(globals::self_(), index),
            AssignmentOperator::Assign,
            right,
        ));
    }
    statements
}

/// `transformImplicitClassConstructor`: the constructor of a class that declares none. A subclass passes its
/// arguments on to `super.constructor`.
pub fn transform_implicit_class_constructor(
    state: &mut TransformState<'_>,
    node: P<Node>,
    name: &AnyIdentifier,
) -> Vec<Stmt> {
    let mut statements = Vec::new();
    let mut has_dot_dot_dot = false;

    if get_extends_node(node).is_some() {
        has_dot_dot_dot = true;
        statements.push(create::call_statement(create::call(
            create::property(globals::super_(), CONSTRUCTOR),
            vec![globals::self_(), create::var_args()],
        )));
    }

    statements.extend(transform_property_initializers(state, node));

    vec![create::method_declaration(name.clone().into(), CONSTRUCTOR, Vec::new(), has_dot_dot_dot, statements)]
}

/// `transformClassConstructor`: parameter properties and property initializers go after the first `super(…)`
/// statement.
pub fn transform_class_constructor(state: &mut TransformState<'_>, node: P<Node>, name: &AnyIdentifier) -> Vec<Stmt> {
    let TransformedParameters { parameters, mut statements, has_dot_dot_dot } = transform_parameters(state, node);
    let body = node.body().expect("a constructor implementation has a body");
    let body_statements = get_statements(body);

    // property parameters must come after the first super() call
    let super_index = body_statements.iter().position(|&statement| {
        statement.kind() == Kind::ExpressionStatement
            && statement.expression().is_some_and(|e| {
                e.kind() == Kind::CallExpression && e.expression().is_some_and(|c| c.kind() == Kind::SuperKeyword)
            })
    });
    // `findIndex` gives -1 when absent, so the first part is empty
    let split = super_index.map_or(0, |i| i + 1);

    statements.extend(transform_statement_list(state, Some(body), &body_statements[..split]));

    for &parameter in node.parameters() {
        if tsrs_ast::is_parameter_property_declaration(parameter, node) {
            let name_node = parameter.name().expect("a parameter has a name");
            let param_id = transform_identifier_defined(state, name_node);
            let property_name = match &param_id {
                Expr::Identifier(identifier) => identifier.name.clone(),
                _ => name_node.text().to_owned(),
            };
            let property = create::property(globals::self_(), property_name);
            statements.push(create::assignment(property, AssignmentOperator::Assign, param_id));
        }
    }

    statements.extend(transform_property_initializers(state, node.parent().expect("a constructor has a class")));

    statements.extend(transform_statement_list(state, Some(body), &body_statements[split..]));

    vec![create::method_declaration(name.clone().into(), CONSTRUCTOR, parameters, has_dot_dot_dot, statements)]
}
