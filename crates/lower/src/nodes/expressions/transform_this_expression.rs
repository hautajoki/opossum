//! `nodes/expressions/transformThisExpression.ts`

use rbxts_luau::{Expr, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::macro_manager::symbol_names;
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;

/// `this`: `self`, or the class itself inside static members.
pub fn transform_this_expression(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    let symbol = state.checker.get_symbol_at_location_exported(node);
    if symbol == Some(state.macro_manager.get_symbol_or_throw(symbol_names::GLOBAL_THIS)) {
        state.add_diagnostic(errors::no_global_this(node));
    }

    if symbol.is_some() {
        let container = tsrs_ast::get_this_container(node, false, false);
        // ts.hasStaticModifier doesn't work on static blocks
        let is_static =
            tsrs_ast::has_static_modifier(container) || container.kind() == Kind::ClassStaticBlockDeclaration;
        // MethodDeclaration creates its own implicit this
        if is_static
            && container.kind() != Kind::MethodDeclaration
            && let Some(class) = container.parent()
            && tsrs_ast::is_class_like(class)
        {
            match state.class_identifier_map.get(&class) {
                Some(identifier) => return identifier.clone().into(),
                // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
                None => state.internal_error(node, "no class identifier for `this` in a static member"),
            }
        }
    }

    globals::self_()
}
