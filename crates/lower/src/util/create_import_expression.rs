//! Imports as requires: the imported file's routed instance path, checked against the importer's route.

use luau::{Expr, ModulePath, create};
use tsrs_ast::{Kind, Node, SourceFile};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::traversal::source_file_of;

/// `getSourceFileFromModuleSpecifier`: the file an import specifier resolves to.
pub fn get_source_file_from_module_specifier(
    state: &mut TransformState<'_>,
    module_specifier: P<Node>,
) -> Option<P<SourceFile>> {
    let symbol = state
        .checker
        .get_symbol_at_location_exported(module_specifier)
        .or_else(|| state.checker.resolve_external_module_name_exported(module_specifier, None))?;
    let declaration = symbol.value_declaration()?;
    if declaration.kind() == Kind::ModuleDeclaration
        && let Some(name) = declaration.name()
        && tsrs_ast::is_string_literal_like(name)
    {
        let source_file = source_file_of(module_specifier);
        let mode = state.program.get_mode_for_usage_location(source_file, name);
        let resolved = state.program.get_resolved_module(source_file, name.text(), mode)?;
        return if resolved.is_resolved() { state.program.get_source_file(resolved.resolved_file_name) } else { None };
    }
    (declaration.kind() == Kind::SourceFile).then(|| declaration.as_source_file_p())
}

/// Whether `file` is `@rbxts/services`, whose named imports lower to `game:GetService` calls.
pub fn is_services_file(file: P<SourceFile>) -> bool {
    file.get().file_name().contains("/node_modules/@rbxts/services/")
}

/// The instance path this file requires `module_file` by, or `None` when it cannot: this file is unrouted (reported
/// once, at the file), or, reported at `at`, the target is unrouted or out of the importer's reach (another realm,
/// or a client module named from outside its root).
pub fn require_path(state: &mut TransformState<'_>, at: P<Node>, module_file: P<SourceFile>) -> Option<ModulePath> {
    let from = state.location?;
    let Some(to) = state.locator.get(module_file.get().file_name()) else {
        state.add_diagnostic(errors::unrouted(at, module_file.get().file_name()));
        return None;
    };
    if !from.realm.reaches(to.realm) || luau::spell(&from.module, &to.module).is_none() {
        state.add_diagnostic(errors::route_boundary(at, from.realm.name(), to.realm.name()));
        return None;
    }
    Some(to.module.clone())
}

/// `createImportExpression`: `require(<path>)` for the module `module_specifier` names, or `Expr::None` after a
/// diagnostic.
pub fn create_import_expression(state: &mut TransformState<'_>, module_specifier: P<Node>) -> Expr {
    let Some(module_file) = get_source_file_from_module_specifier(state, module_specifier) else {
        state.add_diagnostic(errors::no_module_specifier_file(module_specifier));
        return Expr::None;
    };
    match require_path(state, module_specifier, module_file) {
        Some(path) => create::require(path),
        None => Expr::None,
    }
}
