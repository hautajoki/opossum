//! `nodes/transformWritable.ts`: assignment targets.

use rbxts_luau::{Effects, Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::plugins::backend::keyed_access;
use crate::shared::diagnostics::errors;
use crate::util::add_one_if_array_type::add_one_if_array_type;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::effects::{
    effects_commute, get_effects, get_effects_of_statements, is_late_read, join_effects,
};
use crate::util::traversal::skip_downwards;

/// `transformWritableExpression`: the target of an assignment. With `read_after_write`, the receiver and key are
/// held in locals so the target can be read back.
pub fn transform_writable_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    read_after_write: bool,
) -> Expr {
    if tsrs_ast::is_prototype_access(node) {
        state.add_diagnostic(errors::no_prototype(node));
    }
    match node.kind() {
        Kind::PropertyAccessExpression => {
            let access = node.as_property_access_expression();
            let expression = transform_expression(state, prereqs, access.expression);
            let base = if read_after_write {
                prereqs.push_to_var_if_non_id(expression, "exp")
            } else {
                convert_to_indexable_expression(expression)
            };
            // rbxts-rs: a plugin's `property_key` (DESIGN §11.5)
            match crate::plugins::dispatch::rewritten_key(state, node, crate::plugins::dispatch::write_position(node)) {
                Some(key) => keyed_access(base, &key),
                None => create::property(base, access.name.text()),
            }
        }
        Kind::ElementAccessExpression => {
            let access = node.as_element_access_expression();
            let mut ordered =
                ensure_transform_order(state, prereqs, &[access.expression, access.argument_expression]).into_iter();
            let mut expression = ordered.next().expect("receiver");
            let index = ordered.next().expect("index");
            if is_late_read(&expression) && !effects_commute(&get_effects(&expression), &get_effects(&index)) {
                expression = prereqs.push_to_var(expression, "exp");
            }
            // rbxts-rs: a plugin's `property_key` for a string literal key (DESIGN §11.5)
            let position = crate::plugins::dispatch::write_position(node);
            if let Some(key) = crate::plugins::dispatch::rewritten_key(state, node, position) {
                // a computed argument is still evaluated, after the receiver
                let statements = crate::plugins::dispatch::discarded_key_statements(state, node, index);
                let expression = crate::plugins::dispatch::place_key_statements(prereqs, expression, statements);
                let base = if read_after_write {
                    prereqs.push_to_var_if_non_id(expression, "exp")
                } else {
                    convert_to_indexable_expression(expression)
                };
                return keyed_access(base, &key);
            }
            let receiver_type = state.get_type(access.expression);
            let index_exp = add_one_if_array_type(state, receiver_type, index);
            let base = if read_after_write {
                prereqs.push_to_var_if_non_id(expression, "exp")
            } else {
                convert_to_indexable_expression(expression)
            };
            let index = if read_after_write { prereqs.push_to_var_if_complex(index_exp, "index") } else { index_exp };
            create::computed_index(base, index)
        }
        _ => {
            let transformed = transform_expression(state, prereqs, skip_downwards(node));
            if !transformed.is_writable() && !transformed.is_none() {
                // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
                state.internal_error(node, "assignment target is not writable");
            }
            transformed
        }
    }
}

/// The result of `transformWritableAssignment`. `readable_is_writable` is TypeScript's `readable === writable`.
pub struct WritableAssignment {
    /// What to assign to.
    pub writable: Expr,
    /// What to read the current value from.
    pub readable: Expr,
    /// `readable` is `writable` itself.
    pub readable_is_writable: bool,
    /// The transformed right-hand side.
    pub value: Expr,
}

/// `transformWritableAssignment`: the target, then the value; the target is captured where the value's evaluation
/// could change it.
pub fn transform_writable_assignment(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    write_node: P<Node>,
    value_node: P<Node>,
    read_after_write: bool,
    read_before_write: bool,
) -> WritableAssignment {
    let writable = transform_writable_expression(state, prereqs, write_node, read_after_write);
    let mut value_prereqs = Prereqs::new();
    let value = transform_expression(state, &mut value_prereqs, value_node);
    let prereq_effects = get_effects_of_statements(&value_prereqs.statements);
    let value_effects = get_effects(&value);
    let effects = join_effects(prereq_effects.clone(), &value_effects);
    let writable = capture_writable_assignment_target(prereqs, writable, &prereq_effects, &value_effects);
    // the assignment target and its old value may need separate snapshots
    let (readable, readable_is_writable) = if read_before_write && !effects_commute(&get_effects(&writable), &effects) {
        (prereqs.push_to_var(writable.clone(), "readable"), false)
    } else {
        (writable.clone(), true)
    };
    prereqs.push_list(value_prereqs.statements);
    WritableAssignment { writable, readable, readable_is_writable, value }
}

/// `captureWritableAssignmentTarget`: Luau evaluates complex bases and keys before an inline right-hand side, but
/// can read locals at the store instruction; hoisted prerequisites precede both.
pub fn capture_writable_assignment_target(
    prereqs: &mut Prereqs,
    writable: Expr,
    prereq_effects: &Effects,
    value_effects: &Effects,
) -> Expr {
    let intervening = |expression: &Expr| {
        join_effects(
            prereq_effects.clone(),
            &(if is_late_read(expression) { value_effects.clone() } else { Effects::none() }),
        )
    };
    match writable {
        Expr::PropertyAccess(access) => {
            let access = *access;
            let base = access.expression;
            let base_effects = intervening(&base);
            let stable_base = if !effects_commute(&get_effects(&base), &base_effects) {
                prereqs.push_to_var(base, "exp")
            } else {
                base
            };
            create::property(stable_base, access.name)
        }
        Expr::ComputedIndex(access) => {
            let access = *access;
            let base = access.expression;
            let index = access.index;
            let capture_index = !effects_commute(&get_effects(&index), &intervening(&index));
            let base_effects =
                if capture_index { join_effects(intervening(&base), &get_effects(&index)) } else { intervening(&base) };
            let stable_base = if !effects_commute(&get_effects(&base), &base_effects) {
                prereqs.push_to_var(base, "exp")
            } else {
                base
            };
            let index = if capture_index { prereqs.push_to_var(index, "index") } else { index };
            create::computed_index(stable_base, index)
        }
        other => other,
    }
}
