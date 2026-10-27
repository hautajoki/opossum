//! `nodes/expressions/transformArrayLiteralExpression.ts`

use rbxts_luau::{AssignmentOperator, BinaryOperator, Expr, UnaryOperator, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::get_add_iterable_to_array_builder::get_add_iterable_to_array_builder;
use crate::util::pointer::{create_array_pointer, disable_array_inline};

/// `transformArrayLiteralExpression`: an inline array, or, from the first spread (or element with prerequisites) on,
/// a temporary filled with `table.move`, loops and indexed assignments.
pub fn transform_array_literal_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    let elements = node.elements();
    if !elements.iter().any(|element| element.kind() == Kind::SpreadElement) {
        return create::array(ensure_transform_order(state, prereqs, elements));
    }

    let mut ptr = create_array_pointer("array");
    let length_id = temp_id("length");
    let length = Expr::TemporaryIdentifier(length_id.clone());
    let mut amt_elements_since_update: u32 = 0;

    // the array pointer becomes an identifier once, when inline construction ends
    let initialize_length_id = |prereqs: &mut Prereqs, ptr_value: &Expr, amt_elements_since_update: &mut u32| {
        prereqs.push(create::local(length_id.clone(), Some(create::unary(UnaryOperator::Len, ptr_value.clone()))));
        *amt_elements_since_update = 0;
    };

    for (i, &element) in elements.iter().enumerate() {
        if element.kind() == Kind::SpreadElement {
            if matches!(ptr.value, Expr::Array(_)) {
                disable_array_inline(prereqs, &mut ptr);
                initialize_length_id(prereqs, &ptr.value, &mut amt_elements_since_update);
            }
            assert!(ptr.value.is_any_identifier(), "the array is a temporary after a spread");

            let expression = element.expression().expect("a spread element has an expression");
            let t = state.get_type(expression);
            let add_iterable_to_array_builder = get_add_iterable_to_array_builder(state, expression, t);
            let spread_exp = transform_expression(state, prereqs, expression);
            let should_update_length_id = i < elements.len() - 1;
            let statements = add_iterable_to_array_builder.build(
                prereqs,
                spread_exp,
                &as_any_identifier(ptr.value.clone()),
                &as_any_identifier(length.clone()),
                amt_elements_since_update,
                should_update_length_id,
            );
            prereqs.push_list(statements);
        } else {
            let mut expression_prereqs = Prereqs::new();
            let expression = transform_expression(state, &mut expression_prereqs, element);
            if matches!(ptr.value, Expr::Array(_)) && !expression_prereqs.statements.is_empty() {
                disable_array_inline(prereqs, &mut ptr);
                initialize_length_id(prereqs, &ptr.value, &mut amt_elements_since_update);
            }
            if let Expr::Array(members) = &mut ptr.value {
                members.push(expression);
            } else {
                prereqs.push_list(expression_prereqs.statements);
                prereqs.push(create::assignment(
                    create::computed_index(
                        ptr.value.clone(),
                        create::binary(
                            length.clone(),
                            BinaryOperator::Add,
                            create::number(f64::from(amt_elements_since_update + 1)),
                        ),
                    ),
                    AssignmentOperator::Assign,
                    expression,
                ));
            }
            amt_elements_since_update += 1;
        }
    }

    ptr.value
}
