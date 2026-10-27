//! `nodes/statements/transformImportDeclaration.ts`

use rbxts_luau::{Expr, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id_expr};
use crate::classes::transform_state::TransformState;
use crate::nodes::statements::transform_variable_statement::transform_variable;
use crate::nodes::transform_property_name::transform_property_name;
use crate::util::clean_module_name::clean_module_name;
use crate::util::create_import_expression::{create_import_expression, get_source_file_from_module_specifier};
use crate::util::symbols::{get_original_symbol_of_node, is_symbol_of_value};
use crate::util::traversal::source_file_of;

/// Whether an import binding is used as a value: referenced (import elision on the original program) and naming a
/// value at run time.
///
/// rbxts-rs: at the end of a file with plugins, an alias a plugin asked to elide is unused, and one a plugin kept
/// is used (`plugins::dispatch::import_override`).
fn is_used_value_import(state: &mut TransformState<'_>, alias_declaration: P<Node>, name: P<Node>) -> bool {
    let symbol = get_original_symbol_of_node(state.checker, name);
    let referenced = match crate::plugins::dispatch::import_override(state, alias_declaration) {
        Some(kept) => kept,
        None => state.resolver.is_referenced_alias_declaration(state.checker, alias_declaration),
    };
    referenced && symbol.is_none_or(is_symbol_of_value)
}

fn count_import_exp_uses(state: &mut TransformState<'_>, import_clause: P<Node>) -> usize {
    let clause = import_clause.as_import_clause();
    let mut uses = 0;

    if let Some(name) = clause.name
        && is_used_value_import(state, import_clause, name)
    {
        uses += 1;
    }

    if let Some(named_bindings) = clause.named_bindings {
        if named_bindings.kind() == Kind::NamespaceImport {
            if crate::plugins::dispatch::import_override(state, named_bindings) != Some(false) {
                uses += 1;
            }
        } else {
            for &element in named_bindings.elements() {
                let name = element.name().expect("an import specifier has a name");
                if is_used_value_import(state, element, name) {
                    uses += 1;
                }
            }
        }
    }

    uses
}

/// `importExp`: the `TS.import` call, made on first use (roblox-ts's `Lazy`).
struct LazyImport {
    module_specifier: P<Node>,
    value: Option<Expr>,
}

impl LazyImport {
    fn get(&mut self, state: &mut TransformState<'_>) -> Expr {
        if self.value.is_none() {
            let source_file = source_file_of(self.module_specifier);
            self.value = Some(create_import_expression(state, source_file, self.module_specifier));
        }
        self.value.clone().expect("set above")
    }
}

/// `transformImportDeclaration`
pub fn transform_import_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let declaration = node.as_import_declaration();
    // no emit for type only
    let import_clause = declaration.import_clause;
    if import_clause.is_some_and(|c| c.as_import_clause().phase_modifier.get() == Kind::TypeKeyword) {
        return Vec::new();
    }

    let mut statements = Vec::new();

    let module_specifier = declaration.module_specifier;
    assert!(module_specifier.kind() == Kind::StringLiteral, "an import's module specifier is a string literal");
    let mut import_exp = LazyImport { module_specifier, value: None };

    if let Some(import_clause) = import_clause {
        let clause = import_clause.as_import_clause();
        // detect if we need to push to a new var or not
        let uses = count_import_exp_uses(state, import_clause);
        if uses > 1 {
            let specifier_text = module_specifier.text();
            let module_name = specifier_text.rsplit('/').next().unwrap_or(specifier_text);
            let id = temp_id_expr(clean_module_name(module_name));
            let value = import_exp.get(state);
            statements.push(create::local(crate::classes::prereqs::as_any_identifier(id.clone()), Some(value)));
            import_exp.value = Some(id);
        }

        let mut binding_prereqs = Prereqs::new();

        // default import logic
        if let Some(import_clause_name) = clause.name
            && is_used_value_import(state, import_clause, import_clause_name)
        {
            let module_file = get_source_file_from_module_specifier(state, module_specifier);
            let module_symbol = module_file.and_then(|f| state.checker.get_symbol_at_location_exported(f.as_node()));
            let has_default = module_symbol
                .is_some_and(|symbol| state.get_module_exports(symbol).iter().any(|v| v.name() == "default"));
            let value = import_exp.get(state);
            let value = if has_default { create::property(value, "default") } else { value };
            transform_variable(state, &mut binding_prereqs, import_clause_name, Some(value));
        }

        if let Some(named_bindings) = clause.named_bindings {
            if named_bindings.kind() == Kind::NamespaceImport {
                if crate::plugins::dispatch::import_override(state, named_bindings) == Some(false) {
                    // rbxts-rs: elided for a plugin
                } else {
                    // namespace import logic
                    let name = named_bindings.as_namespace_import().name;
                    let value = import_exp.get(state);
                    transform_variable(state, &mut binding_prereqs, name, Some(value));
                }
            } else {
                // named elements import logic
                for &element in named_bindings.elements() {
                    let name = element.name().expect("an import specifier has a name");
                    // check that the import is referenced and has a value at run time
                    if is_used_value_import(state, element, name) {
                        let property_name = element.property_name().unwrap_or(name);
                        let value = import_exp.get(state);
                        let index = transform_property_name(state, &mut binding_prereqs, property_name);
                        transform_variable(
                            state,
                            &mut binding_prereqs,
                            name,
                            Some(create::computed_index(value, index)),
                        );
                    }
                }
            }
        }

        statements.extend(binding_prereqs.statements);
    }

    // ensure we emit something
    if import_clause.is_none() || (state.compiler_options.verbatim_module_syntax.is_true() && statements.is_empty()) {
        let expression = import_exp.get(state);
        assert!(matches!(expression, Expr::Call(_)), "an import expression is a call");
        statements.push(create::call_statement(expression));
    }

    statements
}
