//! `nodes/expressions/transformNewExpression.ts`

use luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::callsite::fill_parameters;
use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::builtins::try_mark_pure_constructor;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements};
use crate::util::types::get_first_construct_symbol;
use crate::util::validate_not_any::validate_not_any_type;

/// `transformNewExpression`: a constructor macro (`new Map()`), or `Class.new(args)`.
pub fn transform_new_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let class_expression = node.expression().expect("a new expression has an expression");
    validate_not_any_type(state, class_expression);

    if let Some(symbol) = get_first_construct_symbol(state, class_expression)
        && let Some(r#macro) = state.macro_manager.get_constructor_macro(state.checker, symbol)
    {
        return (r#macro.transform)(state, prereqs, node);
    }
    if let Some(vector) = crate::vector::new_vector(state, prereqs, node) {
        return vector;
    }

    let mut expression = convert_to_indexable_expression(transform_expression(state, prereqs, class_expression));
    // rbxts-rs: fixes roblox-ts reading the constructor after the arguments' prerequisites ran: JavaScript evaluates
    // it first, so `new C((C = B, x))` constructs the old `C`. It is captured when those prerequisites could change it.
    let mut args_prereqs = Prereqs::new();
    let mut args = ensure_transform_order(state, &mut args_prereqs, node.arguments());
    fill_parameters(state, node, node.arguments(), &mut args);
    if !effects_commute(&get_effects(&expression), &get_effects_of_statements(&args_prereqs.statements)) {
        expression = prereqs.push_to_var(expression, "class");
    }
    prereqs.push_list(args_prereqs.statements);
    let mut call = create::call(create::property(expression, "new"), args);
    // rbxts-rs: `new UDim2(…)` and the like, for the JSX ordering (`try_mark_pure_constructor`)
    try_mark_pure_constructor(state, node, class_expression, None, node.arguments(), &mut call);
    call
}
