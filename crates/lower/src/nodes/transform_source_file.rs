//! `nodes/transformSourceFile.ts`: a whole file, with its exports, header and runtime library import.

use luau::{AssignmentOperator, Expr, Stmt, create, globals};
use rustc_hash::FxHashSet;
use tsrs_ast::{Kind, Node, SourceFile, Symbol};
use tsrs_core::P;

use crate::classes::prereqs::as_any_identifier;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_identifier::{renamed_binding, transform_identifier_defined};
use crate::nodes::transform_property_name::property_name_key;
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::shared::diagnostics::errors;
use crate::util::symbols::{get_original_symbol_of_node, is_symbol_mutable, is_symbol_of_value};
use crate::util::traversal::{get_ancestor, has_declare_modifier};

fn get_export_declarations(export_symbol: P<Symbol>) -> &'static [P<Node>] {
    let declarations = export_symbol.declarations();
    assert!(!declarations.is_empty(), "an export has declarations");
    declarations
}

fn get_export_pair(state: &mut TransformState<'_>, export_symbol: P<Symbol>) -> (Expr, Expr) {
    let declaration = get_export_declarations(export_symbol)[0];
    if declaration.kind() == Kind::ExportSpecifier {
        let specifier = declaration.as_export_specifier();
        let export_name = specifier.property_name.unwrap_or(specifier.name);
        // the name is only a string literal for re-exports, which handleExports filters out
        assert!(export_name.kind() == Kind::Identifier, "a local export names an identifier");
        let name = property_name_key(specifier.name);
        (name, transform_identifier_defined(state, export_name))
    } else {
        let mut name = export_symbol.name();
        if export_symbol.name() == "default"
            && matches!(declaration.kind(), Kind::FunctionDeclaration | Kind::ClassDeclaration)
            && let Some(declaration_name) = declaration.name()
        {
            name = declaration_name.text();
        }
        let value = match renamed_binding(state, export_symbol, name) {
            Some(id) => Expr::TemporaryIdentifier(id),
            None => create::id(name),
        };
        // an export named by a string (`export { x as "a\b" }`) has its decoded value as its name
        (crate::util::create_string_from_literal::create_string_from_value(export_symbol.name()), value)
    }
}

fn is_export_symbol_from_export_from(export_symbol: P<Symbol>) -> bool {
    get_export_declarations(export_symbol).iter().any(|&specifier| {
        specifier.kind() == Kind::ExportSpecifier
            && specifier.parent().and_then(|p| p.parent()).is_some_and(|export_dec| {
                export_dec.kind() == Kind::ExportDeclaration
                    && export_dec.as_export_declaration().module_specifier.is_some()
            })
    })
}

fn get_ignored_export_symbols(state: &mut TransformState<'_>, source_file: P<SourceFile>) -> FxHashSet<P<Symbol>> {
    let mut ignored = FxHashSet::default();
    for &statement in source_file.get().statements.nodes() {
        if statement.kind() != Kind::ExportDeclaration {
            continue;
        }
        let declaration = statement.as_export_declaration();
        let Some(module_specifier) = declaration.module_specifier else { continue };
        match declaration.export_clause {
            None => {
                // export * from "./module";
                if let Some(module_symbol) = get_original_symbol_of_node(state.checker, module_specifier) {
                    ignored.extend(state.get_module_exports(module_symbol).iter().copied());
                }
            }
            Some(clause) if clause.kind() == Kind::NamespaceExport => {
                // export * as id from "./module";
                let id_symbol = state
                    .checker
                    .get_symbol_at_location_exported(clause.as_namespace_export().name())
                    .expect("a namespace export has a symbol");
                ignored.insert(id_symbol);
            }
            Some(_) => {}
        }
    }
    ignored
}

/// Ignores exports in the form of `export declare const x: T;`, but keeps `declare const x: T; export { x };`, as
/// TypeScript does.
fn is_export_symbol_only_from_declare(export_symbol: P<Symbol>) -> bool {
    get_export_declarations(export_symbol).iter().all(|&declaration| {
        let statement = get_ancestor(declaration, tsrs_ast::is_statement).expect("a declaration is in a statement");
        assert!(tsrs_ast::can_have_modifiers(statement), "an exported statement can have modifiers");
        has_declare_modifier(statement)
    })
}

/// `handleExports`: the export table at the end of the file.
fn handle_exports(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    symbol: P<Symbol>,
    statements: &mut Vec<Stmt>,
) -> bool {
    let ignored_export_symbols = get_ignored_export_symbols(state, source_file);

    let mut must_push_exports = state.has_export_from;
    let mut export_pairs: Vec<(Expr, Expr)> = Vec::new();
    let file_statements = source_file.get().statements.nodes();
    // even an erased `export =` replaces the module's named exports
    let has_export_equals =
        file_statements.iter().any(|s| s.kind() == Kind::ExportAssignment && s.as_export_assignment().is_export_equals);
    if !has_export_equals {
        for &export_symbol in state.get_module_exports(symbol).iter() {
            if ignored_export_symbols.contains(&export_symbol) {
                continue;
            }

            // export { default as x } from "./module";
            if is_export_symbol_from_export_from(export_symbol) {
                continue;
            }

            let original_symbol = state.checker.skip_alias(export_symbol);

            // only export values
            if !is_symbol_of_value(original_symbol) {
                continue;
            }

            // handled in transformIdentifier
            if is_symbol_mutable(state, original_symbol) {
                must_push_exports = true;
                continue;
            }

            // ignore exports in the form of `export declare const x: T;`
            if is_export_symbol_only_from_declare(export_symbol) {
                continue;
            }

            export_pairs.push(get_export_pair(state, export_symbol));
        }
    }

    if state.has_export_equals {
        // the local exports variable is created in transformExportAssignment
        let final_statement = file_statements.last().copied();
        let ends_with_export_equals = final_statement
            .is_some_and(|s| s.kind() == Kind::ExportAssignment && s.as_export_assignment().is_export_equals);
        if !ends_with_export_equals {
            statements.push(create::return_stmt(globals::exports()));
        }
        false
    } else if must_push_exports {
        // with an export let or export from, `local exports = {}` goes at the top of the file
        statements.insert(0, create::local(as_any_identifier(globals::exports()), Some(create::map(Vec::new()))));
        for (export_key, export_id) in export_pairs {
            statements.push(create::assignment(
                create::computed_index(globals::exports(), export_key),
                AssignmentOperator::Assign,
                export_id,
            ));
        }
        statements.push(create::return_stmt(globals::exports()));
        true
    } else {
        if !export_pairs.is_empty() {
            // only regular exports: return a table at the bottom of the file
            statements.push(create::return_stmt(create::map(export_pairs)));
        }
        false
    }
}

fn last_non_comment_statement(statements: &[Stmt]) -> Option<&Stmt> {
    statements.iter().rev().find(|s| !s.is_comment())
}

/// `transformSourceFile`: the file's Luau statements.
pub fn transform_source_file(state: &mut TransformState<'_>, node: P<SourceFile>) -> Vec<Stmt> {
    if state.location.is_none() {
        state.add_diagnostic(errors::unrouted(node.as_node(), node.get().file_name()));
    }
    let symbol = state.checker.get_symbol_at_location_exported(node.as_node()).expect("a module file has a symbol");
    state.set_module_id_by_symbol(symbol, as_any_identifier(globals::exports()));

    // the statements of the source file
    let file_node = node.as_node();
    let mut statements = transform_statement_list(state, Some(file_node), node.get().statements.nodes());
    handle_exports(state, node, symbol, &mut statements);

    // a module returns nil when it exports no values
    let ends_with_return = last_non_comment_statement(&statements).is_some_and(|s| matches!(s, Stmt::Return(_)));
    if !ends_with_return {
        statements.push(create::return_stmt(Expr::Nil));
    }

    crate::types::finish(state, &mut statements);

    let mut header_statements = vec![create::comment("!optimize 2")];
    if state.uses_runtime_lib
        && let Some(import) = state.create_runtime_lib_import()
    {
        header_statements.push(import);
    }

    // Luau directive comments like --!strict go before the header
    let directive_count =
        statements.iter().take_while(|s| matches!(s, Stmt::Comment(text) if text.starts_with('!'))).count();
    let directives: Vec<Stmt> = statements.drain(..directive_count).collect();
    statements.splice(0..0, header_statements);
    statements.splice(0..0, directives);
    statements
}
