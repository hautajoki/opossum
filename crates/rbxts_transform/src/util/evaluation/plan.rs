//! `util/evaluation/plan.ts`: expand a macro over opaque operand references, then decide where each operand's value
//! must be captured so evaluation order is preserved.

use rustc_hash::FxHashMap;

use rbxts_luau::{Effects, Expr, Stmt};

use super::effects::{can_discard, can_repeat, effects_commute, get_effects, get_effects_of_statements, join_effects};
use super::events::{EvaluationReference, get_evaluation_events};
use super::facts::copy_value_facts;
use super::substitute::{substitute_expr, substitute_stmts};
use crate::classes::prereqs::{Prereqs, temp_id};
use crate::util::wrap_expression_statement::wrap_expression_statement;

/// `EvaluationOperand`
pub struct EvaluationOperand {
    /// The operand's value.
    pub expression: Expr,
    /// The statements that compute it.
    pub prereqs: Vec<Stmt>,
}

/// `planEvaluation`. Opaque references keep macros from decomposing or duplicating an operand's computation before
/// it is decided where its value must be captured.
pub fn plan_evaluation(
    prereqs: &mut Prereqs,
    operands: Vec<EvaluationOperand>,
    expand: impl FnOnce(&mut Prereqs, Vec<Expr>) -> Expr,
) -> Expr {
    let effects: Vec<Effects> = operands.iter().map(|o| get_effects(&o.expression)).collect();
    let mut slots: FxHashMap<u32, EvaluationReference> = FxHashMap::default();
    let references: Vec<Expr> = operands
        .iter()
        .enumerate()
        .map(|(index, operand)| {
            // literal inspection (typeIs, default arguments, index folding) is harmless
            if operand.expression.is_simple_primitive() || operand.expression.is_none() {
                return operand.expression.clone();
            }
            let mut reference = Expr::TemporaryIdentifier(temp_id("operand"));
            copy_value_facts(&operand.expression, &mut reference);
            let Expr::TemporaryIdentifier(t) = &reference else { unreachable!() };
            slots.insert(
                t.id,
                EvaluationReference {
                    operand_index: index,
                    expression: operand.expression.clone(),
                    effects: effects[index].clone(),
                },
            );
            reference
        })
        .collect();
    let mut expansion_prereqs = Prereqs::new();
    let mut result = expand(&mut expansion_prereqs, references.clone());
    let events = get_evaluation_events(&expansion_prereqs.statements, &result, &slots);

    let mut uses: Vec<Vec<usize>> = vec![Vec::new(); operands.len()];
    for (index, event) in events.iter().enumerate() {
        if let Some(operand) = event.operand_index {
            uses[operand].push(index);
        }
    }
    let mut captures = vec![false; operands.len()];
    let mut hoisted_effects = Effects::none();
    for i in (0..operands.len()).rev() {
        let positions = &uses[i];
        let mut intervening_effects = hoisted_effects.clone();
        // earlier operands are checked on later iterations; captured later operands already contribute their
        // effects at the hoisted position
        let last = positions.last().map_or(-1, |&p| p as isize);
        let uncertain = positions.iter().any(|&p| events[p].conditional);
        for (j, event) in events.iter().enumerate() {
            if let Some(op) = event.operand_index
                && (op <= i || captures[op])
            {
                continue;
            }
            if (j as isize) < last || uncertain {
                intervening_effects = join_effects(intervening_effects, &event.effects);
            }
        }
        captures[i] = if positions.is_empty() {
            !can_discard(&effects[i])
        } else {
            ((positions.len() > 1 || uncertain) && !can_repeat(&effects[i]))
                || !effects_commute(&effects[i], &intervening_effects)
        };
        if captures[i] {
            hoisted_effects = join_effects(effects[i].clone(), &hoisted_effects);
        }
        hoisted_effects = join_effects(get_effects_of_statements(&operands[i].prereqs), &hoisted_effects);
    }

    let mut replacements: FxHashMap<u32, Expr> = FxHashMap::default();
    for (i, operand) in operands.into_iter().enumerate() {
        prereqs.push_list(operand.prereqs);
        let mut expression = operand.expression;
        if captures[i] {
            if uses[i].is_empty() {
                prereqs.push_list(wrap_expression_statement(expression));
                expression = Expr::None;
            } else {
                let name = if i == 0 { "exp".to_owned() } else { format!("arg{}", i - 1) };
                let mut captured = prereqs.push_to_var(expression.clone(), &name);
                // working temporaries may change value; only immutable captures inherit these facts
                copy_value_facts(&expression, &mut captured);
                expression = captured;
            }
        }
        if let Expr::TemporaryIdentifier(t) = &references[i]
            && slots.contains_key(&t.id)
        {
            replacements.insert(t.id, expression);
        }
    }

    let mut expansion = expansion_prereqs.statements;
    substitute_stmts(&mut expansion, &replacements);
    prereqs.push_list(expansion);
    substitute_expr(&mut result, &replacements);
    result
}
