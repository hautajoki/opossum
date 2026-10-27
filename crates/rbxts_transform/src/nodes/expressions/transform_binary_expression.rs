//! `nodes/expressions/transformBinaryExpression.ts`

use rbxts_luau::{AnyIdentifier, Assignment, AssignmentOperator, BinaryOperator, Expr, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_array_assignment_pattern::transform_array_assignment_pattern;
use crate::nodes::binding::transform_object_assignment_pattern::transform_object_assignment_pattern;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_initializer::transform_initializer;
use crate::nodes::transform_logical::transform_logical;
use crate::nodes::transform_logical_or_coalescing_assignment_expression::transform_logical_or_coalescing_assignment_expression;
use crate::nodes::transform_writable::{transform_writable_assignment, transform_writable_expression};
use crate::shared::diagnostics::errors;
use crate::util::array_binding_pattern_contains_hoists::array_like_expression_contains_spread;
use crate::util::binding::get_assignment_pattern_type::type_of_assignment_pattern;
use crate::util::binding::object_accessor::object_accessor;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::create_binary_from_operator::{
    create_assignment_expression, create_binary_from_operator, create_bitwise_from_operator,
    create_compound_assignment_expression, get_assignable_value, get_simple_assignment_operator, is_bitwise_operator,
};
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements, join_effects};
use crate::util::symbols::is_used_as_statement;
use crate::util::traversal::skip_downwards;
use crate::util::types::{
    is_definitely_type, is_lua_tuple_type, is_number_type, is_possibly_type, is_roblox_type, is_string_type,
};
use crate::util::validate_not_any::validate_not_any_type;

/// The right-hand side of an optimized array assignment: one expression (a call returning a LuaTuple), or a list
/// (an array literal's members).
enum OptimizedRhs {
    Single(Expr),
    List(Vec<Expr>),
}

/// `transformOptimizedArrayAssignmentPattern`: `a, b.c = rhs` in one multiple assignment.
fn transform_optimized_array_assignment_pattern(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    assignment_pattern: P<Node>,
    rhs: OptimizedRhs,
) {
    let mut variables: Vec<AnyIdentifier> = Vec::new();
    let mut writes: Vec<Expr> = Vec::new();
    let mut writes_prereqs: Vec<Stmt> = Vec::new();
    let mut binding_prereqs = Prereqs::new();
    for &element in assignment_pattern.elements() {
        if element.kind() == Kind::OmittedExpression {
            // rbxts-rs: fixes roblox-ts never declaring a hole's temporary: `[, q, , p] = tup()` wrote the globals `_`
            // and `_1`. It is declared with the pattern's other temporaries (`local _, _1`).
            let hole = temp_id("");
            variables.push(AnyIdentifier::Temporary(hole.clone()));
            writes.push(Expr::TemporaryIdentifier(hole));
            continue;
        }
        // callers only select this optimization for patterns without spread elements
        assert!(element.kind() != Kind::SpreadElement, "Cannot optimize-assign spread element");
        let mut element = element;
        let mut initializer = None;
        if element.kind() == Kind::BinaryExpression {
            let binary = element.as_binary_expression();
            initializer = Some(skip_downwards(binary.right()));
            element = skip_downwards(binary.left);
        }

        match element.kind() {
            Kind::Identifier | Kind::ElementAccessExpression | Kind::PropertyAccessExpression => {
                let mut id_prereqs = Prereqs::new();
                let id = transform_writable_expression(state, &mut id_prereqs, element, true);
                writes_prereqs.extend(id_prereqs.statements);
                writes.push(id.clone());
                if let Some(initializer) = initializer {
                    let statement = transform_initializer(state, id, initializer);
                    binding_prereqs.push(statement);
                }
            }
            Kind::ArrayLiteralExpression | Kind::ObjectLiteralExpression => {
                let temp = temp_id("binding");
                variables.push(AnyIdentifier::Temporary(temp.clone()));
                let id = Expr::TemporaryIdentifier(temp);
                writes.push(id.clone());
                if let Some(initializer) = initializer {
                    let statement = transform_initializer(state, id.clone(), initializer);
                    binding_prereqs.push(statement);
                }
                if element.kind() == Kind::ArrayLiteralExpression {
                    transform_array_assignment_pattern(state, &mut binding_prereqs, element, id);
                } else {
                    transform_object_assignment_pattern(state, &mut binding_prereqs, element, id);
                }
            }
            _ => {
                // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
                state.internal_error(element, "Expected object assignment pattern");
                return;
            }
        }
    }
    if !variables.is_empty() {
        prereqs.push(create::locals(variables, Vec::new()));
    }

    let mut target_effects = get_effects_of_statements(&writes_prereqs);
    for target in &writes {
        match target {
            Expr::PropertyAccess(access) => {
                target_effects = join_effects(target_effects, &get_effects(&access.expression));
            }
            Expr::ComputedIndex(access) => {
                target_effects = join_effects(target_effects, &get_effects(&access.expression));
                target_effects = join_effects(target_effects, &get_effects(&access.index));
            }
            _ => {}
        }
    }

    // the complete right-hand side precedes destination evaluation, including newly hoisted captures
    let rhs = match rhs {
        OptimizedRhs::List(expressions) => {
            let mut captures = vec![false; expressions.len()];
            for i in (0..expressions.len()).rev() {
                let effects = get_effects(&expressions[i]);
                captures[i] = !effects_commute(&effects, &target_effects);
                if captures[i] {
                    target_effects = join_effects(effects, &target_effects);
                }
            }
            let count = expressions.len();
            let mut result = Vec::new();
            for (index, expression) in expressions.into_iter().enumerate() {
                if !captures[index] {
                    result.push(expression);
                    continue;
                }

                // a final array member can supply multiple return values to the remaining destinations
                if index == count - 1 && expression.is_call() {
                    let remaining = writes.len().saturating_sub(index);
                    if remaining > 1 {
                        let ids: Vec<_> = (0..remaining).map(|_| temp_id("binding")).collect();
                        prereqs.push(create::locals(
                            ids.iter().cloned().map(AnyIdentifier::Temporary).collect(),
                            vec![expression],
                        ));
                        result.extend(ids.into_iter().map(Expr::TemporaryIdentifier));
                        continue;
                    }
                }

                result.push(prereqs.push_to_var(expression, "binding"));
            }
            result
        }
        OptimizedRhs::Single(rhs) => {
            if !effects_commute(&get_effects(&rhs), &target_effects) {
                let captures: Vec<_> = writes.iter().map(|_| temp_id("binding")).collect();
                prereqs.push(create::locals(
                    captures.iter().cloned().map(AnyIdentifier::Temporary).collect(),
                    vec![rhs],
                ));
                captures.into_iter().map(Expr::TemporaryIdentifier).collect()
            } else {
                vec![rhs]
            }
        }
    };

    prereqs.push_list(writes_prereqs);
    assert!(!writes.is_empty(), "an optimized assignment pattern writes something");
    prereqs.push(Stmt::Assignment(Box::new(Assignment {
        left: writes,
        operator: AssignmentOperator::Assign,
        right: rhs,
    })));
    prereqs.push_list(binding_prereqs.statements);
}

/// `tryTransformOptimizedObjectAssignmentPattern`: `({ a } = rhs)` with one named property and an identifier target
/// assigns `a = rhs.a` directly.
fn try_transform_optimized_object_assignment_pattern(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    assignment_pattern: P<Node>,
    rhs: &Expr,
) -> bool {
    let properties = assignment_pattern.properties();
    if properties.len() != 1 {
        return false;
    }

    let property = properties[0];
    let shorthand = property.kind() == Kind::ShorthandPropertyAssignment;
    if !(shorthand || property.kind() == Kind::PropertyAssignment) {
        return false;
    }
    let name = if shorthand {
        property.as_shorthand_property_assignment().name()
    } else {
        property.as_property_assignment().name()
    };
    if name.kind() != Kind::Identifier {
        return false;
    }

    let (mut target, mut initializer) = if shorthand {
        (name, property.as_shorthand_property_assignment().object_assignment_initializer())
    } else {
        (property.as_property_assignment().initializer(), None)
    };
    if target.kind() == Kind::BinaryExpression {
        let binary = target.as_binary_expression();
        initializer = Some(skip_downwards(binary.right()));
        target = skip_downwards(binary.left);
    }

    // property and indexed targets can evaluate before an inline receiver
    if target.kind() != Kind::Identifier {
        return false;
    }

    let receiver_type = type_of_assignment_pattern(state, assignment_pattern);
    let value =
        object_accessor(state, prereqs, convert_to_indexable_expression(rhs.clone()), receiver_type, name).value;
    let writable = transform_writable_expression(state, prereqs, target, initializer.is_some());
    prereqs.push(create::assignment(writable.clone(), AssignmentOperator::Assign, value));
    if let Some(initializer) = initializer {
        let statement = transform_initializer(state, writable, initializer);
        prereqs.push(statement);
    }

    true
}

/// `transformBinaryExpression`
pub fn transform_binary_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let binary = node.as_binary_expression();
    let operator_kind = binary.operator_token.kind();
    let (left_node, right_node) = (binary.left, binary.right());

    validate_not_any_type(state, left_node);
    validate_not_any_type(state, right_node);

    // banned
    if operator_kind == Kind::EqualsEqualsToken {
        state.add_diagnostic(errors::no_equals_equals(node));
        return Expr::None;
    } else if operator_kind == Kind::ExclamationEqualsToken {
        state.add_diagnostic(errors::no_exclamation_equals(node));
        return Expr::None;
    }

    // logical
    if matches!(operator_kind, Kind::AmpersandAmpersandToken | Kind::BarBarToken | Kind::QuestionQuestionToken) {
        return transform_logical(state, prereqs, node);
    }

    // rbxts-rs: an assignment to `receiver.key` a plugin claims (DESIGN §11.5)
    if tsrs_ast::is_assignment_operator(operator_kind)
        && let Some(claimed) = crate::plugins::dispatch::claim_assignment(state, prereqs, node)
    {
        return claimed;
    }

    if tsrs_ast::is_logical_or_coalescing_assignment_expression(node) {
        return transform_logical_or_coalescing_assignment_expression(state, prereqs, node);
    }

    if tsrs_ast::is_assignment_operator(operator_kind) {
        // in destructuring, the right-hand side must be executed first
        if left_node.kind() == Kind::ArrayLiteralExpression {
            let right_exp = transform_expression(state, prereqs, right_node);

            // optimize empty array destructure
            if left_node.elements().is_empty() {
                if is_used_as_statement(node) && matches!(&right_exp, Expr::Array(members) if members.is_empty()) {
                    return Expr::None;
                }
                return right_exp;
            }

            if right_exp.is_call()
                && {
                    let right_type = state.get_type(right_node);
                    is_lua_tuple_type(state, right_type)
                }
                && !array_like_expression_contains_spread(left_node)
            {
                transform_optimized_array_assignment_pattern(
                    state,
                    prereqs,
                    left_node,
                    OptimizedRhs::Single(right_exp),
                );
                if !is_used_as_statement(node) {
                    state.add_diagnostic(errors::no_lua_tuple_destructure_assignment_expression(node));
                }
                return Expr::None;
            }

            if matches!(&right_exp, Expr::Array(members) if !members.is_empty())
                && is_used_as_statement(node)
                && !array_like_expression_contains_spread(left_node)
            {
                let Expr::Array(members) = right_exp else { unreachable!() };
                transform_optimized_array_assignment_pattern(state, prereqs, left_node, OptimizedRhs::List(members));
                return Expr::None;
            }

            let parent_id = prereqs.push_to_var(right_exp, "binding");
            transform_array_assignment_pattern(state, prereqs, left_node, parent_id.clone());
            return parent_id;
        } else if left_node.kind() == Kind::ObjectLiteralExpression {
            let right_exp = transform_expression(state, prereqs, right_node);

            // optimize empty object destructure
            if left_node.properties().is_empty() {
                if is_used_as_statement(node) && matches!(&right_exp, Expr::Map(fields) if fields.is_empty()) {
                    return Expr::None;
                }
                return right_exp;
            }

            // a used assignment expression must still return the original receiver
            if is_used_as_statement(node)
                && try_transform_optimized_object_assignment_pattern(state, prereqs, left_node, &right_exp)
            {
                return Expr::None;
            }

            let parent_id = prereqs.push_to_var(right_exp, "binding");
            transform_object_assignment_pattern(state, prereqs, left_node, parent_id.clone());
            return parent_id;
        }

        let writable_type = state.get_type(left_node);
        let value_type = state.get_type(right_node);
        let operator = get_simple_assignment_operator(state, writable_type, operator_kind, value_type);
        let assignment = transform_writable_assignment(
            state,
            prereqs,
            left_node,
            right_node,
            true,
            operator != Some(rbxts_luau::AssignmentOperator::Assign),
        );
        return match operator {
            Some(operator) => {
                let value = get_assignable_value(state, operator, assignment.value, value_type);
                let readable = (!assignment.readable_is_writable).then_some(assignment.readable);
                create_assignment_expression(prereqs, assignment.writable, operator, value, readable)
            }
            None => create_compound_assignment_expression(
                state,
                prereqs,
                assignment.writable,
                writable_type,
                assignment.readable,
                operator_kind,
                assignment.value,
                value_type,
            ),
        };
    }

    if is_bitwise_operator(operator_kind) {
        return create_bitwise_from_operator(state, prereqs, operator_kind, node);
    }

    let mut ordered = ensure_transform_order(state, prereqs, &[left_node, right_node]).into_iter();
    let left = ordered.next().expect("left");
    let right = ordered.next().expect("right");

    if operator_kind == Kind::InKeyword {
        return create::binary(
            create::computed_index(convert_to_indexable_expression(right), left),
            BinaryOperator::Ne,
            Expr::Nil,
        );
    } else if operator_kind == Kind::InstanceOfKeyword {
        let right_type = state.get_type(right_node);
        if is_possibly_type(state, right_type, &[&is_roblox_type]) {
            state.add_diagnostic(errors::no_roblox_symbol_instanceof(right_node));
        }
        let callee = state.ts(node, "instanceof");
        return create::call(callee, vec![left, right]);
    }

    let left_type = state.get_type(left_node);
    let right_type = state.get_type(right_node);

    if matches!(
        operator_kind,
        Kind::LessThanToken | Kind::LessThanEqualsToken | Kind::GreaterThanToken | Kind::GreaterThanEqualsToken
    ) {
        let left_ok = is_definitely_type(state, left_type, &[&is_string_type])
            || is_definitely_type(state, left_type, &[&is_number_type]);
        // roblox-ts checks the left type's number-ness twice here; kept as written
        let right_ok = is_definitely_type(state, right_type, &[&is_string_type])
            || is_definitely_type(state, left_type, &[&is_number_type]);
        if !left_ok || !right_ok {
            state.add_diagnostic(errors::no_non_number_string_relation_operator(node));
        }
    }

    create_binary_from_operator(state, prereqs, left, left_type, operator_kind, right, right_type)
}
