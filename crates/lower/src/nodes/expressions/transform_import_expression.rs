//! `nodes/expressions/transformImportExpression.ts`

use luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::create_import_expression::create_import_expression;

/// `import("x")`: `TS.Promise.new(function(resolve) resolve(require("x")) end)`.
pub fn transform_import_expression(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    let module_specifier = node.arguments().first().copied();
    let Some(module_specifier) = module_specifier.filter(|n| n.kind() == Kind::StringLiteral) else {
        state.add_diagnostic(errors::no_non_string_module_specifier(node));
        return Expr::None;
    };

    let import_expression = create_import_expression(state, module_specifier);
    let resolve_id = create::any_id("resolve");

    let promise = state.ts("Promise");
    create::call(
        create::property(promise, "new"),
        vec![create::function(
            vec![resolve_id.clone()],
            false,
            vec![create::call_statement(create::call(Expr::from(resolve_id), vec![import_expression]))],
        )],
    )
}
