//! `nodes/transformLogical.ts`: `&&`, `||` and `??` chains.

use rbxts_luau::{AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, UnaryOperator, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id_expr};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::create_truthiness_checks::{create_truthiness_checks, will_create_truthiness_checks};
use crate::util::expression_chain::binary_expression_chain;
use crate::util::types::{is_boolean_literal_type, is_possibly_type};

struct LogicalChainItem {
    node: P<Node>,
    expression: Expr,
    statements: Vec<Stmt>,
    inline: bool,
}

/// How the condition before each following operand is built.
#[derive(Clone, Copy)]
enum Condition {
    /// `&&`: the truthiness checks.
    And,
    /// `||`: `not` the truthiness checks.
    Or,
    /// `??`: `condition == nil`.
    Coalesce,
}

fn build_condition(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    condition: Condition,
    condition_id: &Expr,
    node: P<Node>,
) -> Expr {
    match condition {
        Condition::And => create_truthiness_checks(state, prereqs, condition_id.clone(), node),
        Condition::Or => {
            create::unary(UnaryOperator::Not, create_truthiness_checks(state, prereqs, condition_id.clone(), node))
        }
        Condition::Coalesce => create::binary(condition_id.clone(), BinaryOperator::Eq, Expr::Nil),
    }
}

/// `flattenByOperator`: `a && b && c` → `[a, b, c]`.
fn flatten_by_operator(mut node: P<Node>, operator_kind: Kind) -> Vec<P<Node>> {
    let mut result = Vec::new();
    while node.kind() == Kind::BinaryExpression && node.as_binary_expression().operator_token.kind() == operator_kind {
        let binary = node.as_binary_expression();
        result.push(binary.right());
        node = binary.left;
    }
    result.push(node);
    result.reverse();
    result
}

/// `getLogicalChain`: each operand transformed with its own prerequisites (the operands as
/// `flattenByOperator` gives them).
fn get_logical_chain(
    state: &mut TransformState<'_>,
    nodes: Vec<P<Node>>,
    enable_inlining: bool,
) -> Vec<LogicalChainItem> {
    let count = nodes.len();
    nodes
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            let t = state.get_type(node);
            let mut expression_prereqs = Prereqs::new();
            let expression = transform_expression(state, &mut expression_prereqs, node);
            let mut inline = false;
            if enable_inlining {
                let will_wrap = index < count - 1 && will_create_truthiness_checks(state, t);
                inline = expression_prereqs.statements.is_empty() && !will_wrap;
            }
            LogicalChainItem { node, expression, statements: expression_prereqs.statements, inline }
        })
        .collect()
}

/// `buildLogicalChainPrereqs`: `local condition = a; if <condition> then condition = b; if … end end`.
fn build_logical_chain_prereqs(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    chain: &mut Vec<LogicalChainItem>,
    condition_id: &Expr,
    condition: Condition,
    index: usize,
) {
    let statements = std::mem::take(&mut chain[index].statements);
    prereqs.push_list(statements);
    let expression = std::mem::replace(&mut chain[index].expression, Expr::None);
    if index == 0 {
        let id = crate::classes::prereqs::as_any_identifier(condition_id.clone());
        prereqs.push(create::local(id, Some(expression)));
    } else {
        prereqs.push(create::assignment(condition_id.clone(), AssignmentOperator::Assign, expression));
    }
    if index + 1 < chain.len() {
        let node = chain[index].node;
        let condition_exp = build_condition(state, prereqs, condition, condition_id, node);
        let mut branch_prereqs = Prereqs::new();
        build_logical_chain_prereqs(state, &mut branch_prereqs, chain, condition_id, condition, index + 1);
        prereqs.push(create::if_stmt(condition_exp, branch_prereqs.statements, ElseBody::default()));
    }
}

/// `mergeInlineExpressions`: neighbouring inline items become one `a and b` item.
fn merge_inline_expressions(chain: &mut Vec<LogicalChainItem>, binary_operator: BinaryOperator) {
    let mut i = 0;
    while i < chain.len() {
        if chain[i].inline {
            let mut exps = vec![std::mem::replace(&mut chain[i].expression, Expr::None)];
            let j = i + 1;
            while j < chain.len() && chain[j].inline {
                exps.push(chain.remove(j).expression);
            }
            chain[i].expression = binary_expression_chain(exps, binary_operator);
        }
        i += 1;
    }
}

/// `buildInlineConditionExpression`: the simple case, where operands inline into `and` / `or`.
fn build_inline_condition_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    operands: Vec<P<Node>>,
    luau_operator: BinaryOperator,
    condition: Condition,
) -> Expr {
    let mut chain = get_logical_chain(state, operands, true);

    merge_inline_expressions(&mut chain, luau_operator);

    // single inline at the end, no temp variable needed
    if chain.len() == 1 && chain[0].inline {
        return chain.pop().expect("one item").expression;
    }

    let condition_id = temp_id_expr("condition");
    build_logical_chain_prereqs(state, prereqs, &mut chain, &condition_id, condition, 0);
    condition_id
}

/// `transformLogical`
pub fn transform_logical(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let operator = node.as_binary_expression().operator_token.kind();
    let coalesce_possibly_false = operator == Kind::QuestionQuestionToken && {
        let t = state.get_type(node);
        is_possibly_type(state, t, &[&is_boolean_literal_type(false)])
    };
    let operands = flatten_by_operator(node, operator);
    transform_logical_operands(state, prereqs, operator, operands, coalesce_possibly_false)
}

/// `transformLogical` on operands already flattened (`a && b && c` is `[a, b, c]`). For `??`,
/// `coalesce_possibly_false` says the whole expression's type may be `false` (then no operand is inlined).
pub fn transform_logical_operands(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    operator: Kind,
    operands: Vec<P<Node>>,
    coalesce_possibly_false: bool,
) -> Expr {
    match operator {
        Kind::AmpersandAmpersandToken => {
            build_inline_condition_expression(state, prereqs, operands, BinaryOperator::And, Condition::And)
        }
        Kind::BarBarToken => {
            build_inline_condition_expression(state, prereqs, operands, BinaryOperator::Or, Condition::Or)
        }
        Kind::QuestionQuestionToken => {
            if !coalesce_possibly_false {
                return build_inline_condition_expression(
                    state,
                    prereqs,
                    operands,
                    BinaryOperator::Or,
                    Condition::Coalesce,
                );
            }
            let mut chain = get_logical_chain(state, operands, false);
            let condition_id = temp_id_expr("condition");
            build_logical_chain_prereqs(state, prereqs, &mut chain, &condition_id, Condition::Coalesce, 0);
            condition_id
        }
        other => panic!("Operator not implemented: {other:?}"),
    }
}

/// `flattenByOperator` for a public caller: `a && b && c` → `[a, b, c]`.
pub fn flatten_logical_operands(node: P<Node>, operator_kind: Kind) -> Vec<P<Node>> {
    flatten_by_operator(node, operator_kind)
}
