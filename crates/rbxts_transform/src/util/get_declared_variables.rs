//! `util/getDeclaredVariables.ts`

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

fn get_declared_variables_from_binding_name(node: P<Node>, list: &mut Vec<P<Node>>) {
    match node.kind() {
        Kind::Identifier => list.push(node),
        Kind::ObjectBindingPattern => {
            for &element in node.elements() {
                if let Some(name) = element.name() {
                    get_declared_variables_from_binding_name(name, list);
                }
            }
        }
        _ => {
            for &element in node.elements() {
                if element.kind() != Kind::OmittedExpression
                    && let Some(name) = element.name()
                {
                    get_declared_variables_from_binding_name(name, list);
                }
            }
        }
    }
}

/// `getDeclaredVariables`: the identifiers a variable declaration list (or one declaration) declares, through
/// binding patterns.
pub fn get_declared_variables(node: P<Node>) -> Vec<P<Node>> {
    let mut list = Vec::new();
    if node.kind() == Kind::VariableDeclarationList {
        for &declaration in node.as_variable_declaration_list().declarations.nodes() {
            get_declared_variables_from_binding_name(declaration.as_variable_declaration().name, &mut list);
        }
    } else {
        get_declared_variables_from_binding_name(node.as_variable_declaration().name, &mut list);
    }
    list
}
