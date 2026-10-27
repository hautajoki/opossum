//! `nodes/statements/transformStatement.ts`: statement dispatch by syntax kind.

use rbxts_luau::Stmt;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::kind_name;
use crate::shared::diagnostics::errors;
use crate::util::traversal::has_declare_modifier;

use super::transform_block::transform_block;
use super::transform_break_statement::transform_break_statement;
use super::transform_class_declaration::transform_class_declaration;
use super::transform_continue_statement::transform_continue_statement;
use super::transform_do_statement::transform_do_statement;
use super::transform_enum_declaration::transform_enum_declaration;
use super::transform_export_assignment::transform_export_assignment;
use super::transform_export_declaration::transform_export_declaration;
use super::transform_expression_statement::transform_expression_statement;
use super::transform_for_of_statement::transform_for_of_statement;
use super::transform_for_statement::transform_for_statement;
use super::transform_function_declaration::transform_function_declaration;
use super::transform_if_statement::transform_if_statement;
use super::transform_import_declaration::transform_import_declaration;
use super::transform_import_equals_declaration::transform_import_equals_declaration;
use super::transform_module_declaration::transform_module_declaration;
use super::transform_return_statement::transform_return_statement;
use super::transform_switch_statement::transform_switch_statement;
use super::transform_throw_statement::transform_throw_statement;
use super::transform_try_statement::transform_try_statement;
use super::transform_variable_statement::transform_variable_statement;
use super::transform_while_statement::transform_while_statement;

/// `transformStatement`
pub fn transform_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    // rbxts-rs: the plugins' by-kind fallback (DESIGN §11.2), before roblox-ts's own lowering: every statement of a
    // statement list, those roblox-ts emits nothing for (types, `declare`) included, as a TS transformer sees them
    if let Some(claimed) = crate::plugins::dispatch::claim_statement(state, node) {
        return claimed;
    }
    // `declare` says the name is defined elsewhere; trust it and emit nothing
    if tsrs_ast::can_have_modifiers(node) && has_declare_modifier(node) {
        return Vec::new();
    }

    match node.kind() {
        // no emit
        Kind::InterfaceDeclaration | Kind::TypeAliasDeclaration | Kind::EmptyStatement => Vec::new(),

        // banned statements
        Kind::ForInStatement => {
            state.add_diagnostic(errors::no_for_in_statement(node));
            Vec::new()
        }
        Kind::LabeledStatement => {
            state.add_diagnostic(errors::no_labeled_statement(node));
            Vec::new()
        }
        Kind::DebuggerStatement => {
            state.add_diagnostic(errors::no_debugger_statement(node));
            Vec::new()
        }

        // regular transforms
        Kind::Block => transform_block(state, node),
        Kind::BreakStatement => transform_break_statement(state, node),
        Kind::ClassDeclaration => transform_class_declaration(state, node),
        Kind::ContinueStatement => transform_continue_statement(state, node),
        Kind::DoStatement => transform_do_statement(state, node),
        Kind::EnumDeclaration => transform_enum_declaration(state, node),
        Kind::ExportAssignment => transform_export_assignment(state, node),
        Kind::ExportDeclaration => transform_export_declaration(state, node),
        Kind::ExpressionStatement => transform_expression_statement(state, node),
        Kind::ForOfStatement => transform_for_of_statement(state, node),
        Kind::ForStatement => transform_for_statement(state, node),
        Kind::FunctionDeclaration => transform_function_declaration(state, node),
        Kind::IfStatement => transform_if_statement(state, node),
        Kind::ImportDeclaration => transform_import_declaration(state, node),
        Kind::ImportEqualsDeclaration => transform_import_equals_declaration(state, node),
        Kind::ModuleDeclaration => transform_module_declaration(state, node),
        Kind::ReturnStatement => transform_return_statement(state, node),
        Kind::SwitchStatement => transform_switch_statement(state, node),
        Kind::ThrowStatement => transform_throw_statement(state, node),
        Kind::TryStatement => transform_try_statement(state, node),
        Kind::VariableStatement => transform_variable_statement(state, node),
        Kind::WhileStatement => transform_while_statement(state, node),

        kind => {
            // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
            state.internal_error(node, &format!("Unknown statement: {}", kind_name(kind)));
            Vec::new()
        }
    }
}
