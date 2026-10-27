//! `util/ensureTransformOrder.ts`: transform expressions left to right, capturing a value in a temporary when a
//! later expression's prerequisites could change it.

use rbxts_luau::{Effects, Expr, Stmt};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements, join_effects};

/// A transform of one node into an expression with prerequisites.
pub type Transformer<'f> = &'f mut dyn FnMut(&mut TransformState<'_>, &mut Prereqs, P<Node>) -> Expr;

/// `ensureTransformOrder` with `transformExpression`.
pub fn ensure_transform_order(state: &mut TransformState<'_>, prereqs: &mut Prereqs, nodes: &[P<Node>]) -> Vec<Expr> {
    ensure_transform_order_with(state, prereqs, nodes, &mut |state, prereqs, node| {
        transform_expression(state, prereqs, node)
    })
}

/// `ensureTransformOrder` with a custom transformer.
pub fn ensure_transform_order_with(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    nodes: &[P<Node>],
    transformer: Transformer<'_>,
) -> Vec<Expr> {
    let infos: Vec<(Expr, Vec<Stmt>)> = nodes
        .iter()
        .map(|&node| {
            let mut expression_prereqs = Prereqs::new();
            let expression = transformer(state, &mut expression_prereqs, node);
            (expression, expression_prereqs.statements)
        })
        .collect();
    let mut captures = vec![false; infos.len()];
    // each capture becomes a prerequisite that earlier expressions must also cross
    let mut suffix = Effects::none();
    for i in (0..infos.len()).rev() {
        let (expression, prereq_statements) = &infos[i];
        let effects = get_effects(expression);
        captures[i] = !effects_commute(&effects, &suffix);
        if captures[i] {
            suffix = join_effects(effects, &suffix);
        }
        suffix = join_effects(get_effects_of_statements(prereq_statements), &suffix);
    }
    let mut result = Vec::with_capacity(infos.len());
    for ((expression, prereq_statements), capture) in infos.into_iter().zip(captures) {
        prereqs.push_list(prereq_statements);
        if capture {
            result.push(prereqs.push_to_var(expression, "exp"));
        } else {
            result.push(expression);
        }
    }
    result
}
