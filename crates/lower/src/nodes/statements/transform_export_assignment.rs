//! `nodes/statements/transformExportAssignment.ts`

use rbxts_luau::{Stmt, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::shared::diagnostics::errors;
use crate::util::symbols::{is_symbol_mutable, is_symbol_of_value};
use crate::util::traversal::source_file_of;

fn transform_export_equals(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    state.has_export_equals = true;

    let mut prereqs = Prereqs::new();
    let expression = node.as_export_assignment().expression();

    let source_file = source_file_of(node);
    let final_statement = source_file.get().statements.nodes().last().copied();
    let statement = if final_statement == Some(node) {
        create::return_stmt(transform_expression(state, &mut prereqs, expression))
    } else {
        let left = state.get_module_id_from_node(node);
        create::local(left, Some(transform_expression(state, &mut prereqs, expression)))
    };

    let mut statements = prereqs.statements;
    statements.push(statement);
    statements
}

fn transform_export_default(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut expression_prereqs = Prereqs::new();
    let expression = transform_expression(state, &mut expression_prereqs, node.as_export_assignment().expression());
    let mut statements = expression_prereqs.statements;
    statements.push(create::local(create::any_id("default"), Some(expression)));
    statements
}

/// `transformExportAssignment`: `export = x` and `export default x`.
pub fn transform_export_assignment(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let assignment = node.as_export_assignment();
    let symbol = state.checker.get_symbol_at_location_exported(assignment.expression());
    if let Some(symbol) = symbol
        && is_symbol_mutable(state, symbol)
    {
        state.add_diagnostic(errors::no_export_assignment_let(node));
    }

    if let Some(symbol) = symbol {
        let original = state.checker.skip_alias(symbol);
        let erased = !is_symbol_of_value(original)
            || (!state.compiler_options.verbatim_module_syntax.is_true()
                && state.checker.get_type_only_alias_declaration_exported(symbol).is_some());
        if erased {
            return Vec::new();
        }
    }

    if assignment.is_export_equals {
        transform_export_equals(state, node)
    } else {
        transform_export_default(state, node)
    }
}
