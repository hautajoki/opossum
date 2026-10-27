//! `nodes/expressions/transformSpreadElement.ts`: spread in call and `new` arguments (array literals handle their
//! own spread elements).

use rbxts_luau::{Expr, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::shared::diagnostics::errors;
use crate::util::get_add_iterable_to_array_builder::get_add_iterable_to_array_builder;
use crate::util::types::{is_array_type, is_definitely_type};
use crate::util::validate_not_any::validate_not_any_type;

/// `transformSpreadElement`: `unpack(array)`, collecting other iterables into an array first.
pub fn transform_spread_element(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let expression_node = node.expression().expect("a spread element has an expression");
    validate_not_any_type(state, expression_node);

    // array literals are caught and handled separately in transform_array_literal_expression
    let parent = node.parent().expect("a spread element has a parent");
    assert!(
        matches!(parent.kind(), Kind::CallExpression | Kind::NewExpression),
        "a spread element outside an array literal is an argument"
    );
    if parent.arguments().last() != Some(&node) {
        state.add_diagnostic(errors::no_preceding_spread_element(node));
    }

    let expression = transform_expression(state, prereqs, expression_node);

    let t = state.get_type(expression_node);
    if is_definitely_type(state, t, &[&is_array_type]) {
        create::call(globals::unpack(), vec![expression])
    } else {
        let add_iterable_to_array_builder = get_add_iterable_to_array_builder(state, expression_node, t);
        let array_id = prereqs.push_to_var(create::array(Vec::new()), "array");
        let length_id = prereqs.push_to_var(create::number(0.0), "length");
        let statements = add_iterable_to_array_builder.build(
            prereqs,
            expression,
            &as_any_identifier(array_id.clone()),
            &as_any_identifier(length_id),
            0,
            false,
        );
        prereqs.push_list(statements);
        create::call(globals::unpack(), vec![array_id])
    }
}
