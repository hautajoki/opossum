//! `nodes/statements/transformExportDeclaration.ts`

use rbxts_luau::{AssignmentOperator, BinaryOperator, Expr, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::transform_property_name::transform_property_name;
use crate::util::clean_module_name::clean_module_name;
use crate::util::create_import_expression::create_import_expression;
use crate::util::symbols::is_symbol_of_value;
use crate::util::traversal::source_file_of;

fn is_export_specifier_value(state: &mut TransformState<'_>, element: P<Node>) -> bool {
    if element.as_export_specifier().is_type_only {
        return false;
    }

    if state.resolver.is_referenced_alias_declaration(state.checker, element) {
        return true;
    }

    let name = element.as_export_specifier().name;
    if let Some(alias_symbol) = state.checker.get_symbol_at_location_exported(name) {
        let original = state.checker.skip_alias(alias_symbol);
        if is_symbol_of_value(original) {
            return true;
        }
    }

    false
}

fn count_import_exp_uses(state: &mut TransformState<'_>, export_clause: Option<P<Node>>) -> usize {
    match export_clause {
        Some(clause) if clause.kind() == Kind::NamedExports => {
            let mut uses = 0;
            for &element in clause.elements() {
                if is_export_specifier_value(state, element) {
                    uses += 1;
                }
            }
            uses
        }
        _ => 1,
    }
}

fn transform_export_from(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let declaration = node.as_export_declaration();
    let module_specifier = declaration.module_specifier.expect("an export from has a module specifier");
    assert!(module_specifier.kind() == Kind::StringLiteral, "an export's module specifier is a string literal");

    let mut statements = Vec::new();
    let export_clause = declaration.export_clause;

    // detect if we need to push to a new var or not
    let uses = count_import_exp_uses(state, export_clause);
    let import_exp: Expr = if uses == 1 {
        create_import_expression(state, source_file_of(node), module_specifier)
    } else if uses > 1 {
        let specifier_text = module_specifier.text();
        let module_name = specifier_text.rsplit('/').next().unwrap_or(specifier_text);
        let id = temp_id(clean_module_name(module_name));
        let value = create_import_expression(state, source_file_of(node), module_specifier);
        statements.push(create::local(id.clone(), Some(value)));
        Expr::TemporaryIdentifier(id)
    } else {
        return statements;
    };

    let module_id: Expr = state.get_module_id_from_node(node).into();
    match export_clause {
        Some(clause) if clause.kind() == Kind::NamedExports => {
            // export { a, b, c } from "./module";
            for &element in clause.elements() {
                if is_export_specifier_value(state, element) {
                    let specifier = element.as_export_specifier();
                    let mut name_prereqs = Prereqs::new();
                    let export_name = transform_property_name(state, &mut name_prereqs, specifier.name);
                    let import_name = transform_property_name(
                        state,
                        &mut name_prereqs,
                        specifier.property_name.unwrap_or(specifier.name),
                    );
                    statements.extend(name_prereqs.statements);
                    statements.push(create::assignment(
                        create::computed_index(module_id.clone(), export_name),
                        AssignmentOperator::Assign,
                        create::computed_index(import_exp.clone(), import_name),
                    ));
                }
            }
        }
        Some(clause) => {
            // export * as foo from "./module";
            let mut name_prereqs = Prereqs::new();
            let name = transform_property_name(state, &mut name_prereqs, clause.as_namespace_export().name);
            statements.extend(name_prereqs.statements);
            statements.push(create::assignment(
                create::computed_index(module_id, name),
                AssignmentOperator::Assign,
                import_exp,
            ));
        }
        None => {
            // export * from "./module";
            let key_id = temp_id("k");
            let value_id = temp_id("v");
            statements.push(create::for_in(
                vec![key_id.clone().into(), value_id.clone().into()],
                // importExp may be `nil` in .d.ts files, so default to `{}`; boolean `or` is safe, because
                // importExp can only be a table if not `nil`
                create::binary(import_exp, BinaryOperator::Or, create::map(Vec::new())),
                vec![create::assignment(
                    create::computed_index(module_id, Expr::TemporaryIdentifier(key_id)),
                    AssignmentOperator::Assign,
                    Expr::TemporaryIdentifier(value_id),
                )],
            ));
        }
    }

    state.has_export_from = true;

    statements
}

/// `transformExportDeclaration`
pub fn transform_export_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let declaration = node.as_export_declaration();
    if declaration.is_type_only {
        return Vec::new();
    }
    if declaration.module_specifier.is_some() {
        return transform_export_from(state, node);
    }
    Vec::new()
}
