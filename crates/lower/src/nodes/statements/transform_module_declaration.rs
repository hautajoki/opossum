//! `nodes/statements/transformModuleDeclaration.ts`: namespaces.

use rbxts_luau::{AssignmentOperator, Expr, Stmt, create};
use rustc_hash::FxHashMap;
use tsrs_ast::{Kind, Node, Symbol};
use tsrs_core::P;

use crate::classes::prereqs::{as_any_identifier, temp_id};
use crate::classes::diagnostic_service::{OnceCache, ReportOnce};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::transform_statement_list::{ExportInfo, transform_statement_list_with_exports};
use crate::shared::diagnostics::errors;
use crate::util::has_multiple_definitions::has_multiple_definitions;
use crate::util::symbols::{is_symbol_mutable, is_symbol_of_value, validate_identifier};
use crate::util::traversal::{get_ancestor, has_declare_modifier};

fn is_declaration_of_namespace(declaration: P<Node>) -> bool {
    if !matches!(
        declaration.kind(),
        Kind::ModuleDeclaration | Kind::FunctionDeclaration | Kind::ClassDeclaration | Kind::EnumDeclaration
    ) {
        return false;
    }

    if has_declare_modifier(declaration) {
        return false;
    }

    match declaration.kind() {
        Kind::ModuleDeclaration => tsrs_ast::is_instantiated_module(declaration, false),
        Kind::FunctionDeclaration => declaration.body().is_some(),
        _ => true,
    }
}

/// The statement that declares the export's value: not an overload, a type, or a `declare`.
fn get_value_declaration_statement(symbol: P<Symbol>) -> Option<P<Node>> {
    for &declaration in symbol.declarations() {
        let statement = get_ancestor(declaration, tsrs_ast::is_statement).expect("a declaration is in a statement");
        match statement.kind() {
            Kind::FunctionDeclaration if statement.body().is_none() => continue,
            Kind::TypeAliasDeclaration | Kind::InterfaceDeclaration => continue,
            _ => {}
        }
        if tsrs_ast::can_have_modifiers(statement) && has_declare_modifier(statement) {
            continue;
        }
        return Some(statement);
    }
    None
}

/// `transformNamespace`: `local Name = {}` and a `do` block that fills it through `_container`.
fn transform_namespace(state: &mut TransformState<'_>, name: P<Node>, body: P<Node>) -> Vec<Stmt> {
    let symbol = state.checker.get_symbol_at_location_exported(name).expect("a namespace has a symbol");

    validate_identifier(state, name);

    let name_exp = transform_identifier_defined(state, name);

    let mut statements = Vec::new();
    let mut do_statements = Vec::new();

    let container_id = temp_id("container");
    state.set_module_id_by_symbol(symbol, container_id.clone().into());

    if state.is_hoisted.get(&symbol) == Some(&true) {
        statements.push(create::assignment(name_exp.clone(), AssignmentOperator::Assign, create::map(Vec::new())));
    } else {
        statements.push(create::local(as_any_identifier(name_exp.clone()), Some(create::map(Vec::new()))));
    }

    let module_exports = state.get_module_exports(symbol);
    if !module_exports.is_empty() {
        do_statements.push(create::local(container_id.clone(), Some(name_exp)));
    }

    if body.kind() == Kind::ModuleBlock {
        let mut exports_map: FxHashMap<P<Node>, Vec<String>> = FxHashMap::default();
        for &export_symbol in module_exports.iter() {
            let original_symbol = state.checker.skip_alias(export_symbol);
            if is_symbol_of_value(original_symbol)
                && !is_symbol_mutable(state, original_symbol)
                && let Some(statement) = get_value_declaration_statement(export_symbol)
            {
                exports_map.entry(statement).or_default().push(export_symbol.name().to_owned());
            }
        }
        let export_info = ExportInfo { id: container_id.into(), mapping: &exports_map };
        do_statements.extend(transform_statement_list_with_exports(
            state,
            Some(body),
            body.statements(),
            Some(&export_info),
        ));
    } else {
        // `namespace A.B {}`: the inner namespace is declared in the outer one's block
        let inner = body.as_module_declaration();
        let inner_body = body.body().expect("a nested namespace has a body");
        do_statements.extend(transform_namespace(state, inner.name, inner_body));
        let inner_value = transform_identifier_defined(state, inner.name);
        do_statements.push(create::assignment(
            create::property(Expr::TemporaryIdentifier(container_id), inner.name.text()),
            AssignmentOperator::Assign,
            inner_value,
        ));
    }

    statements.push(create::do_stmt(do_statements));

    statements
}

/// `transformModuleDeclaration`
pub fn transform_module_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    // type-only namespace
    if !tsrs_ast::is_instantiated_module(node, false) {
        return Vec::new();
    }

    // disallow merging
    let declaration = node.as_module_declaration();
    let symbol = state.checker.get_symbol_at_location_exported(declaration.name);
    if let Some(symbol) = symbol
        && has_multiple_definitions(symbol, is_declaration_of_namespace)
    {
        // `DiagnosticService.addDiagnosticWithCache`
        let key = ReportOnce::new(OnceCache::MultipleDefinitions, symbol, state.source_file);
        if state.multi_transform_state.reported_once.insert(key) {
            state.diagnostics.add_diagnostic_once(key, errors::no_namespace_merging(node));
        }
        return Vec::new();
    }

    // a string literal name is only `declare module "x" {}`, which is filtered out above
    assert!(declaration.name.kind() != Kind::StringLiteral, "a namespace has an identifier name");
    let body = node.body().expect("an instantiated namespace has a body");
    assert!(body.kind() != Kind::Identifier, "a namespace body is a block or a nested namespace");
    transform_namespace(state, declaration.name, body)
}
