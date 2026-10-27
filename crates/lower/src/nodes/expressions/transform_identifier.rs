//! `nodes/expressions/transformIdentifier.ts`

use luau::{Expr, TemporaryIdentifier, create};
use tsrs_ast::{Kind, ModifierFlags, Node, Symbol};
use tsrs_core::P;

use crate::classes::macro_manager::symbol_names;
use crate::classes::prereqs::renamed_id;
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::evaluation::bindings::get_binding_key;
use crate::util::evaluation::builtins::try_mark_builtin_library;
use crate::util::evaluation::facts::{mark_binding, set_call_effects};
use crate::util::symbols::{get_extends_node, is_block_like, is_namespace, is_symbol_mutable};
use crate::util::traversal::{get_ancestor, has_syntactic_modifier, is_ancestor_of, skip_downwards, skip_upwards};
use crate::util::types::get_first_construct_symbol;

/// The symbol an identifier refers to: for a shorthand property `{ a }`, the value `a`.
pub fn identifier_symbol(state: &mut TransformState<'_>, node: P<Node>) -> Option<P<Symbol>> {
    match node.parent() {
        Some(parent) if parent.kind() == Kind::ShorthandPropertyAssignment => {
            state.checker.get_shorthand_assignment_value_symbol(Some(parent))
        }
        _ => state.checker.get_symbol_at_location_exported(node),
    }
}

/// The temporary that stands for a project binding named after a Luau keyword or reserved global (`local`, `type`,
/// `error`): one per declaration and file, which the name solver renames (`type_`). `None` for any other name, and
/// for declarations outside the project's sources, which name real Luau globals.
pub fn renamed_binding(state: &mut TransformState<'_>, symbol: P<Symbol>, name: &str) -> Option<TemporaryIdentifier> {
    if !luau::LUAU_RESERVED_KEYWORDS.contains(&name) && !luau::is_reserved_identifier(name) {
        return None;
    }
    let declaration = symbol.declarations().first().copied()?;
    let file = tsrs_ast::get_source_file_of_node(declaration)?;
    if file.is_declaration_file() || declaration.flags().intersects(tsrs_ast::NodeFlags::Ambient) {
        return None;
    }
    let id = state.renames.entry(declaration).or_insert_with(|| renamed_id(name));
    Some(id.clone())
}

/// `transformIdentifierDefined`: the identifier as a Luau name, with its binding facts.
pub fn transform_identifier_defined(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    let symbol = identifier_symbol(state, node).expect("a defined identifier has a symbol");

    let replacement = match state.symbol_to_id_map.get(&symbol).cloned() {
        Some(id) => Some(id),
        None => renamed_binding(state, symbol, node.text()),
    };
    if let Some(replacement_id) = replacement {
        let mut identifier = Expr::TemporaryIdentifier(replacement_id);
        let key = get_binding_key(state, symbol, node);
        mark_binding(&mut identifier, symbol, key);
        return identifier;
    }

    let mut identifier = create::id(node.text());
    let key = get_binding_key(state, symbol, node);
    mark_binding(&mut identifier, symbol, key);
    try_mark_builtin_library(state, node, &mut identifier);
    if let Some(effects) = state.multi_transform_state.function_effects.get(&symbol).cloned() {
        set_call_effects(&mut identifier, effects);
    }
    identifier
}

fn get_ancestor_which_is_child_of(parent: P<Node>, mut node: P<Node>) -> Option<P<Node>> {
    while let Some(p) = node.parent()
        && p != parent
    {
        node = p;
    }
    node.parent().map(|_| node)
}

/// `symbol.valueDeclaration` does not point to imports.
pub(crate) fn get_declaration_from_import(symbol: P<Symbol>) -> Option<P<Node>> {
    symbol.declarations().iter().copied().find(|&d| get_ancestor(d, tsrs_ast::is_any_import_syntax).is_some())
}

fn check_identifier_hoist(state: &mut TransformState<'_>, node: P<Node>, symbol: P<Symbol>) {
    if state.is_hoisted.contains_key(&symbol) {
        return;
    }

    let Some(declaration) = symbol.value_declaration().or_else(|| get_declaration_from_import(symbol)) else {
        return;
    };

    // parameters cannot be hoisted
    if get_ancestor(declaration, |n| n.kind() == Kind::Parameter).is_some()
        || declaration.kind() == Kind::ShorthandPropertyAssignment
    {
        return;
    }

    // class expressions can self refer
    if tsrs_ast::is_class_like(declaration) && is_ancestor_of(declaration, node) {
        return;
    }

    let Some(declaration_statement) = get_ancestor(declaration, tsrs_ast::is_statement) else { return };
    if matches!(declaration_statement.kind(), Kind::ForStatement | Kind::ForOfStatement | Kind::TryStatement) {
        return;
    }

    let Some(parent) = declaration_statement.parent() else { return };
    if !is_block_like(parent) {
        return;
    }

    let Some(sibling) = get_ancestor_which_is_child_of(parent, node) else { return };
    if !tsrs_ast::is_statement(sibling) {
        return;
    }

    let statements = parent.statements();
    let declaration_idx = statements.iter().position(|&s| s == declaration_statement);
    let sibling_idx = statements.iter().position(|&s| s == sibling);

    // `indexOf` gives -1 when absent
    let as_index = |i: Option<usize>| i.map_or(-1, |i| i as isize);
    let (declaration_idx, sibling_idx) = (as_index(declaration_idx), as_index(sibling_idx));
    if sibling_idx > declaration_idx {
        return;
    }

    if sibling_idx == declaration_idx {
        // non-async function declarations, class declarations, and variable statements can self refer
        let self_reference = match declaration_statement.kind() {
            Kind::FunctionDeclaration => !has_syntactic_modifier(declaration_statement, ModifierFlags::Async),
            Kind::ClassDeclaration => true,
            Kind::VariableStatement => {
                get_ancestor(node, |n| tsrs_ast::is_statement(n) || tsrs_ast::is_function_like_declaration(n))
                    == Some(declaration_statement)
            }
            _ => false,
        };
        if self_reference {
            return;
        }
    }

    state.hoists_by_statement.entry(sibling).or_default().push(node);
    state.is_hoisted.insert(symbol, true);
}

/// A compile-time macro of opossum's declarations (`$git`, `$env`, …), none of which lowers yet.
fn is_unsupported_macro(state: &TransformState<'_>, symbol: P<Symbol>) -> bool {
    let file = symbol.declarations().first().and_then(|&d| tsrs_ast::get_source_file_of_node(d));
    file.is_some_and(|f| f.get().file_name() == crate::program::macros_file(state.root))
}

/// `transformIdentifier`
pub fn transform_identifier(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    // synthetic nodes have no parents or symbols, so skip all the symbol-related logic
    if node.parent().is_none() || node.pos().is_synthetic() {
        return create::id(node.text());
    }

    let symbol = identifier_symbol(state, node).expect("an identifier has a symbol");

    if is_unsupported_macro(state, symbol) {
        state.add_diagnostic(errors::unsupported_macro(node, node.text()));
        return Expr::None;
    }

    if state.checker.is_undefined_symbol(symbol) {
        return Expr::Nil;
    } else if state.checker.is_arguments_symbol(symbol) {
        state.add_diagnostic(errors::no_arguments(node));
    } else if state.macro_manager.is(symbol, symbol_names::GLOBAL_THIS) {
        state.add_diagnostic(errors::no_global_this(node));
    }

    if let Some(r#macro) = state.macro_manager.get_identifier_macro(state.checker, symbol) {
        return (r#macro.transform)(state, node);
    }

    if let Some(construct_symbol) = get_first_construct_symbol(state, node)
        && state.macro_manager.get_constructor_macro(state.checker, construct_symbol).is_some()
    {
        let class = node.parent().and_then(|p| p.parent()).and_then(|p| p.parent());
        let is_class_extends_node = class.is_some_and(|c| {
            tsrs_ast::is_class_like(c) && get_extends_node(c).and_then(|e| e.expression()) == Some(node)
        });
        if is_class_extends_node {
            state.add_diagnostic(errors::no_macro_extends(node));
        } else {
            state.add_diagnostic(errors::no_constructor_macro_without_new(node));
        }
    }

    let parent = skip_upwards(node).parent().expect("an identifier has a parent");
    let is_callee =
        parent.kind() == Kind::CallExpression && parent.expression().is_some_and(|e| skip_downwards(e) == node);
    if !is_callee && state.macro_manager.get_call_macro(state.checker, symbol).is_some() {
        state.add_diagnostic(errors::no_index_without_call(node));
        return Expr::None;
    }

    // exit here for export let so we don't check hoist later
    if let Some(value_declaration) = symbol.value_declaration()
        && tsrs_ast::get_source_file_of_node(value_declaration) == tsrs_ast::get_source_file_of_node(node)
        && get_ancestor(value_declaration, |n| n.kind() == Kind::ModuleDeclaration && !is_namespace(n)).is_none()
        && let Some(export_access) = state.get_module_id_property_access(symbol)
        && is_symbol_mutable(state, symbol)
    {
        return export_access;
    }

    check_identifier_hoist(state, node, symbol);

    transform_identifier_defined(state, node)
}
