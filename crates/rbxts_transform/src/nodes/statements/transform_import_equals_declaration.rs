//! `nodes/statements/transformImportEqualsDeclaration.ts`

use rbxts_luau::{Expr, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::statements::transform_variable_statement::transform_variable;
use crate::nodes::transform_entity_name::transform_entity_name;
use crate::util::create_import_expression::create_import_expression;
use crate::util::symbols::is_symbol_of_value;
use crate::util::traversal::source_file_of;

/// `import x = require("y")` and `import x = A.B`.
pub fn transform_import_equals_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let declaration = node.as_import_equals_declaration();
    // rbxts-rs: or elided for a plugin
    if declaration.is_type_only || crate::plugins::dispatch::import_override(state, node) == Some(false) {
        return Vec::new();
    }

    let module_reference = declaration.module_reference;
    if module_reference.kind() == Kind::ExternalModuleReference {
        let mut statements = Vec::new();
        let alias_symbol =
            state.checker.get_symbol_at_location_exported(declaration.name).expect("an import alias has a symbol");
        let original = state.checker.skip_alias(alias_symbol);
        let is_value = is_symbol_of_value(original);
        let verbatim = state.compiler_options.verbatim_module_syntax.is_true();
        if !is_value && !verbatim {
            return statements;
        }

        let specifier = module_reference.expression().expect("an external module reference has an expression");
        assert!(specifier.kind() == Kind::StringLiteral, "a require specifier is a string literal");
        let import_exp = create_import_expression(state, source_file_of(node), specifier);
        if is_value {
            let mut import_prereqs = Prereqs::new();
            transform_variable(state, &mut import_prereqs, declaration.name, Some(import_exp.clone()));
            statements.extend(import_prereqs.statements);
        }

        // ensure we emit something
        if verbatim && statements.is_empty() && matches!(import_exp, Expr::Call(_)) {
            statements.push(create::call_statement(import_exp));
        }

        statements
    } else {
        // Identifier | QualifiedName (roblox-ts issue 1895)
        let mut import_prereqs = Prereqs::new();
        let value = transform_entity_name(state, module_reference);
        transform_variable(state, &mut import_prereqs, declaration.name, Some(value));
        import_prereqs.statements
    }
}
