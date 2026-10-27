//! `nodes/statements/transformIfStatement.ts`

use rbxts_luau::{ElseBody, IfStatement, Stmt};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_statement_list::transform_body;
use crate::util::create_truthiness_checks::create_truthiness_checks;

/// `transformIfStatementInner`
pub fn transform_if_statement_inner(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> IfStatement {
    let if_statement = node.as_if_statement();
    let expression = transform_expression(state, prereqs, if_statement.expression);
    let condition = create_truthiness_checks(state, prereqs, expression, if_statement.expression);

    let then_statement = if_statement.then_statement;
    let statements = transform_body(state, then_statement);

    let else_body = match if_statement.else_statement {
        None => ElseBody::default(),
        Some(else_statement) if else_statement.kind() == Kind::IfStatement => {
            // rbxts-rs: the `else if` is a statement a TS transformer sees, which roblox-ts lowers into this `if`: it is
            // offered to the plugins' statement hooks first (DESIGN §11.5), and a claim is the `else` block
            if let Some(claimed) = crate::plugins::dispatch::claim_statement(state, else_statement) {
                let mut else_statements = Vec::new();
                crate::plugins::dispatch::else_if_before_statement(state, else_statement, &mut else_statements);
                else_statements.extend(claimed);
                return IfStatement { condition, statements, else_body: ElseBody::Block(else_statements) };
            }
            let mut else_if_prereqs = Prereqs::new();
            let else_if = transform_if_statement_inner(state, &mut else_if_prereqs, else_statement);
            // rbxts-rs: a plugin's `insert_before_statement` for the `else if` (DESIGN §11)
            crate::plugins::dispatch::else_if_before_statement(state, else_statement, &mut else_if_prereqs.statements);
            if else_if_prereqs.statements.is_empty() {
                ElseBody::If(Box::new(else_if))
            } else {
                let mut else_if_statements = else_if_prereqs.statements;
                else_if_statements.push(Stmt::If(Box::new(else_if)));
                ElseBody::Block(else_if_statements)
            }
        }
        Some(else_statement) => {
            ElseBody::Block(transform_body(state, else_statement))
        }
    };

    IfStatement { condition, statements, else_body }
}

/// `transformIfStatement`
pub fn transform_if_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut prereqs = Prereqs::new();
    let statement = transform_if_statement_inner(state, &mut prereqs, node);
    let mut statements = prereqs.statements;
    statements.push(Stmt::If(Box::new(statement)));
    statements
}
