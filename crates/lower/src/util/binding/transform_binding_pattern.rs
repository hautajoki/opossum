//! `util/binding/transformBindingPattern.ts`

use rbxts_luau::Expr;
use rustc_hash::FxHashMap;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_array_binding_pattern::transform_array_binding_pattern;
use crate::nodes::binding::transform_object_binding_pattern::transform_object_binding_pattern;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements};
use crate::util::evaluation::substitute::substitute_stmts;

/// `transformBindingPattern`: destructures `value` into `pattern`. `value` is captured in a local only when it is not
/// an identifier the destructuring itself could change.
pub fn transform_binding_pattern(state: &mut TransformState<'_>, prereqs: &mut Prereqs, pattern: P<Node>, value: Expr) {
    let reference = temp_id("binding");
    let reference_id = reference.id;
    let mut body = Prereqs::new();
    if pattern.kind() == Kind::ArrayBindingPattern {
        transform_array_binding_pattern(state, &mut body, pattern, Expr::TemporaryIdentifier(reference));
    } else {
        transform_object_binding_pattern(state, &mut body, pattern, Expr::TemporaryIdentifier(reference));
    }

    // capture only when the emitted destructuring can change the source binding
    let source = if value.is_any_identifier()
        && effects_commute(&get_effects(&value), &get_effects_of_statements(&body.statements))
    {
        value
    } else {
        prereqs.push_to_var(value, "binding")
    };
    let mut replacements = FxHashMap::default();
    replacements.insert(reference_id, source);
    let mut statements = body.statements;
    substitute_stmts(&mut statements, &replacements);
    prereqs.push_list(statements);
}
