//! `nodes/statements/transformSwitchStatement.ts`: a `repeat … until true` block of `if` statements, with a flag for
//! fallthrough.

use rbxts_luau::{
    AssignmentOperator, BinaryOperator, ElseBody, Expr, IfStatement, Stmt, TemporaryIdentifier, UnaryOperator, create,
};
use tsrs_ast::{Kind, Node, SymbolFlags};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::statements::transform_continue_statement::create_continue;
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::util::create_hoist_declaration::create_hoist_declaration;
use crate::util::expression_might_mutate::expression_might_mutate;
use crate::util::traversal::skip_downwards;

fn can_compare_identifier_directly(state: &mut TransformState<'_>, node: P<Node>, expression: &Expr) -> bool {
    let switch = node.as_switch_statement();
    let identifier = skip_downwards(switch.expression);
    if identifier.kind() != Kind::Identifier || !matches!(expression, Expr::Identifier(_)) {
        return false;
    }

    // declarations shared by case bodies can be hoisted ahead of their comparisons
    let resolved = state.checker.resolve_name_exported(identifier.text(), switch.case_block, SymbolFlags::Value, false);
    if resolved != state.checker.get_symbol_at_location_exported(identifier) {
        return false;
    }

    // literal labels cannot mutate the operand, and fallthrough skips later comparisons
    switch.case_block.as_case_block().clauses.nodes().iter().all(|&clause| {
        if clause.kind() == Kind::DefaultClause {
            return true;
        }
        let label = skip_downwards(clause.expression().expect("a case clause has an expression"));
        tsrs_ast::is_string_literal_like(label)
            || matches!(label.kind(), Kind::NumericLiteral | Kind::TrueKeyword | Kind::FalseKeyword)
    })
}

/// The condition of a case clause, and the statements it needs before it.
struct CaseClauseExpression {
    condition: Expr,
    prereq_statements: Vec<Stmt>,
}

fn transform_case_clause_expression(
    state: &mut TransformState<'_>,
    case_clause_expression: P<Node>,
    switch_expression: &Expr,
    fall_through_flag_id: &TemporaryIdentifier,
    can_fall_through_to: bool,
) -> CaseClauseExpression {
    let case_value_id = temp_id("caseValue");
    let mut case_prereqs = Prereqs::new();
    let expression = transform_expression(state, &mut case_prereqs, case_clause_expression);
    let mut prereq_statements = case_prereqs.statements;
    let case_value_might_mutate = expression_might_mutate(state, &expression, Some(case_clause_expression));

    let compared = if case_value_might_mutate {
        prereq_statements.push(create::local(case_value_id.clone(), Some(expression)));
        Expr::TemporaryIdentifier(case_value_id)
    } else {
        expression
    };
    let mut condition = create::binary(switch_expression.clone(), BinaryOperator::Eq, compared);

    if can_fall_through_to {
        let fall_through_flag = Expr::TemporaryIdentifier(fall_through_flag_id.clone());
        if !prereq_statements.is_empty() {
            let no_fall_through_condition = create::unary(UnaryOperator::Not, fall_through_flag.clone());
            prereq_statements.push(create::assignment(
                fall_through_flag.clone(),
                AssignmentOperator::Assign,
                condition,
            ));
            prereq_statements =
                vec![create::if_stmt(no_fall_through_condition, prereq_statements, ElseBody::default())];
            condition = fall_through_flag;
        } else {
            condition = create::binary(fall_through_flag, BinaryOperator::Or, condition);
        }
    }

    CaseClauseExpression { condition, prereq_statements }
}

/// A transformed case clause.
struct CaseClause {
    can_fall_through_from: bool,
    prereqs: Vec<Stmt>,
    clause_statements: Vec<Stmt>,
}

fn transform_case_clause(
    state: &mut TransformState<'_>,
    node: P<Node>,
    switch_expression: &Expr,
    fall_through_flag_id: &TemporaryIdentifier,
    can_fall_through_to: bool,
    should_update_fall_through_flag: bool,
) -> CaseClause {
    let CaseClauseExpression { condition, prereq_statements } = transform_case_clause_expression(
        state,
        node.expression().expect("a case clause has an expression"),
        switch_expression,
        fall_through_flag_id,
        can_fall_through_to,
    );

    let (clause_statements, can_fall_through_from) =
        transform_case_clause_body(state, node, condition, fall_through_flag_id, should_update_fall_through_flag);
    CaseClause { can_fall_through_from, prereqs: prereq_statements, clause_statements }
}

/// A case clause's body (or, rbxts-rs, a default clause's) as `if condition then … end`, after its hoisted
/// declarations, and whether control can fall through from it.
fn transform_case_clause_body(
    state: &mut TransformState<'_>,
    node: P<Node>,
    condition: Expr,
    fall_through_flag_id: &TemporaryIdentifier,
    should_update_fall_through_flag: bool,
) -> (Vec<Stmt>, bool) {
    let clause_body = node.statements();
    let non_empty_statements: Vec<P<Node>> =
        clause_body.iter().copied().filter(|s| s.kind() != Kind::EmptyStatement).collect();
    let mut statements = match non_empty_statements.as_slice() {
        [first] if first.kind() == Kind::Block => crate::nodes::transform_statement_list::transform_body(state, *first),
        _ => transform_statement_list(state, Some(node), clause_body),
    };

    let can_fall_through_from = statements.last().is_none_or(|last| !last.is_final());
    if can_fall_through_from && should_update_fall_through_flag {
        statements.push(create::assignment(
            Expr::TemporaryIdentifier(fall_through_flag_id.clone()),
            AssignmentOperator::Assign,
            Expr::True,
        ));
    }

    let mut clause_statements = Vec::new();
    if let Some(hoist_declaration) = create_hoist_declaration(state, node) {
        clause_statements.push(hoist_declaration);
    }
    clause_statements.push(create::if_stmt(condition, statements, ElseBody::default()));
    (clause_statements, can_fall_through_from)
}

/// rbxts-rs: fixes roblox-ts dropping every case after a `default` that is not the last clause (it lowers the default
/// as the end of the switch). JavaScript tests every case before it runs the default, and a default in the middle
/// falls through into the cases after it. The clauses before the default are lowered as roblox-ts lowers them; at
/// the default, unless control fell into it, the later cases are tested in order and the one that matched is kept in
/// `_case`; the default runs when none did, and each later case runs when it matched or control falls into it.
fn transform_switch_middle_default(
    state: &mut TransformState<'_>,
    expression: &Expr,
    clauses: &[P<Node>],
    default_index: usize,
) -> Vec<Stmt> {
    let fall_through_flag_id = temp_id("fallthrough");
    let fall_through_flag = Expr::TemporaryIdentifier(fall_through_flag_id.clone());
    let case_id = temp_id("case");
    let matched_case = Expr::TemporaryIdentifier(case_id.clone());
    let mut statements = Vec::new();
    let mut is_fall_through_flag_needed = false;

    // the clauses before the default, each setting the flag when control can fall through to the next
    let mut can_fall_through_to = false;
    for &case_clause_node in &clauses[..default_index] {
        let CaseClause { can_fall_through_from, prereqs, clause_statements } = transform_case_clause(
            state,
            case_clause_node,
            expression,
            &fall_through_flag_id,
            can_fall_through_to,
            true,
        );
        statements.extend(prereqs);
        statements.extend(clause_statements);
        can_fall_through_to = can_fall_through_from;
        is_fall_through_flag_needed |= can_fall_through_from;
    }

    // which later case matches: `if x == a then _case = 1 elseif x == b then _case = 2 end`, a case with
    // prerequisites in the `else` block before its test
    let later = &clauses[default_index + 1..];
    let tests: Vec<CaseClauseExpression> = later
        .iter()
        .map(|clause| {
            let case_expression = clause.expression().expect("a case clause has an expression");
            transform_case_clause_expression(state, case_expression, expression, &fall_through_flag_id, false)
        })
        .collect();
    let mut chain: Option<(Vec<Stmt>, IfStatement)> = None;
    for (i, CaseClauseExpression { condition, prereq_statements }) in tests.into_iter().enumerate().rev() {
        let else_body = match chain.take() {
            None => ElseBody::default(),
            Some((prereqs, next)) if prereqs.is_empty() => ElseBody::If(Box::new(next)),
            Some((mut prereqs, next)) => {
                prereqs.push(Stmt::If(Box::new(next)));
                ElseBody::Block(prereqs)
            }
        };
        let assign_case = create::assignment(
            matched_case.clone(),
            AssignmentOperator::Assign,
            create::number(f64::from(u32::try_from(i + 1).expect("a case count"))),
        );
        chain = Some((prereq_statements, IfStatement { condition, statements: vec![assign_case], else_body }));
    }
    statements.push(create::local(case_id, None));
    if let Some((mut tests, first)) = chain {
        tests.push(Stmt::If(Box::new(first)));
        if can_fall_through_to {
            // a case that fell into the default skips the tests (`_case` stays nil)
            tests = vec![create::if_stmt(create::unary(UnaryOperator::Not, fall_through_flag.clone()), tests, ElseBody::default())];
        }
        statements.extend(tests);
    }

    // the default: when no later case matched (or control fell into it)
    let default_clause = clauses[default_index];
    let (default_statements, can_fall_through_from) = transform_case_clause_body(
        state,
        default_clause,
        create::binary(matched_case.clone(), BinaryOperator::Eq, Expr::Nil),
        &fall_through_flag_id,
        true,
    );
    statements.extend(default_statements);
    can_fall_through_to = can_fall_through_from;
    is_fall_through_flag_needed |= can_fall_through_from;

    // the later cases: when matched, or when control falls into them
    for (i, &case_clause_node) in later.iter().enumerate() {
        let mut condition = create::binary(
            matched_case.clone(),
            BinaryOperator::Eq,
            create::number(f64::from(u32::try_from(i + 1).expect("a case count"))),
        );
        if can_fall_through_to {
            condition = create::binary(fall_through_flag.clone(), BinaryOperator::Or, condition);
        }
        let (clause_statements, can_fall_through_from) = transform_case_clause_body(
            state,
            case_clause_node,
            condition,
            &fall_through_flag_id,
            i + 1 < later.len(),
        );
        statements.extend(clause_statements);
        can_fall_through_to = can_fall_through_from;
        is_fall_through_flag_needed |= can_fall_through_from && i + 1 < later.len();
    }

    if is_fall_through_flag_needed {
        statements.insert(0, create::local(fall_through_flag_id, Some(Expr::False)));
    }
    statements
}

/// `transformSwitchStatement`
pub fn transform_switch_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut result = transform_switch_statement_inner(state, node);
    // rbxts-rs: a `continue` inside set this switch's flag and broke out of its block (`create_continue`); continue
    // the loop after the block
    if let Some(flag) = state.switch_continue_flags.remove(&node) {
        let block = result.pop().expect("the switch's block");
        result.push(create::local(flag.clone(), Some(Expr::False)));
        result.push(block);
        let continue_statements = create_continue(state, node);
        result.push(create::if_stmt(Expr::TemporaryIdentifier(flag), continue_statements, ElseBody::default()));
    }
    result
}

/// The prerequisites, then the `repeat … until true` block (last).
fn transform_switch_statement_inner(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let switch = node.as_switch_statement();
    let mut prereqs = Prereqs::new();
    let switch_expression = transform_expression(state, &mut prereqs, switch.expression);
    let needs_capture = expression_might_mutate(state, &switch_expression, Some(switch.expression))
        && !can_compare_identifier_directly(state, node, &switch_expression);
    let expression = if needs_capture {
        prereqs.push_to_var(switch_expression, "exp")
    } else {
        prereqs.push_to_var_if_complex(switch_expression, "exp")
    };
    let mut result = prereqs.statements;
    let fall_through_flag_id = temp_id("fallthrough");

    let mut is_fall_through_flag_needed = false;

    let mut statements = Vec::new();
    let mut can_fall_through_to = false;
    let clauses = switch.case_block.as_case_block().clauses.nodes();
    if let Some(default_index) = clauses.iter().position(|clause| clause.kind() == Kind::DefaultClause)
        && default_index + 1 < clauses.len()
    {
        result.push(create::repeat_stmt(
            transform_switch_middle_default(state, &expression, clauses, default_index),
            Expr::True,
        ));
        return result;
    }
    for (i, &case_clause_node) in clauses.iter().enumerate() {
        if case_clause_node.kind() == Kind::CaseClause {
            let should_update_fall_through_flag = i < clauses.len() - 1 && clauses[i + 1].kind() == Kind::CaseClause;
            let CaseClause { can_fall_through_from, prereqs, clause_statements } = transform_case_clause(
                state,
                case_clause_node,
                &expression,
                &fall_through_flag_id,
                can_fall_through_to,
                should_update_fall_through_flag,
            );

            statements.extend(prereqs);
            statements.extend(clause_statements);

            can_fall_through_to = can_fall_through_from;

            if can_fall_through_from {
                is_fall_through_flag_needed = true;
            }
        } else {
            statements.extend(transform_statement_list(state, Some(case_clause_node), case_clause_node.statements()));
            break;
        }
    }

    if is_fall_through_flag_needed {
        statements.insert(0, create::local(fall_through_flag_id, Some(Expr::False)));
    }

    result.push(create::repeat_stmt(statements, Expr::True));
    result
}
