//! `nodes/statements/transformTryStatement.ts`: `TS.try(try, catch, finally)`, and the flow control (return, break,
//! continue) its functions report back.

use rbxts_luau::{AnyIdentifier, BinaryOperator, ElseBody, Expr, IfStatement, ReturnStatement, Stmt, create, globals};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::{as_any_identifier, temp_id};
use crate::classes::transform_state::{TransformState, TryUse, TryUses};
use crate::nodes::binding::transform_binding_name::transform_binding_name;
use crate::nodes::transform_statement_list::transform_statement_list;
use crate::nodes::statements::transform_continue_statement::create_continue;
use crate::util::is_blocked_by_try_statement::{is_break_blocked_by_try_statement, is_return_blocked_by_try_statement};

fn transform_catch_clause(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    let clause = node.as_catch_clause();
    let mut parameters: Vec<AnyIdentifier> = Vec::new();
    let mut statements = Vec::new();

    if let Some(variable_declaration) = clause.variable_declaration {
        let name = variable_declaration.name().expect("a catch variable has a name");
        parameters.push(transform_binding_name(state, name, &mut statements));
    }

    statements.extend(transform_statement_list(state, Some(clause.block), clause.block.statements()));

    create::function(parameters, false, statements)
}

/// The `TS.try` call's arguments: the try function, the catch function (or `nil`), and the finally function.
fn transform_try_call_args(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Expr> {
    let statement = node.as_try_statement();
    let mut try_call_args = Vec::new();

    try_call_args.push(create::function(
        Vec::new(),
        false,
        transform_statement_list(state, Some(statement.try_block), statement.try_block.statements()),
    ));

    match statement.catch_clause {
        Some(catch_clause) => try_call_args.push(transform_catch_clause(state, catch_clause)),
        None => {
            assert!(statement.finally_block.is_some(), "a try statement has a catch or a finally block");
            try_call_args.push(Expr::Nil);
        }
    }

    if let Some(finally_block) = statement.finally_block {
        try_call_args.push(create::function(
            Vec::new(),
            false,
            transform_statement_list(state, Some(finally_block), finally_block.statements()),
        ));
    }

    try_call_args
}

fn uses_any(try_uses: TryUses) -> bool {
    try_uses.uses_return || try_uses.uses_break || try_uses.uses_continue
}

/// `transformIntoTryCall`, given the transformed arguments: the try uses are final once the functions are
/// transformed.
fn transform_into_try_call(
    state: &mut TransformState<'_>,
    node: P<Node>,
    exit_type_id: &Expr,
    returns_id: &Expr,
    try_uses: TryUses,
    try_call_args: Vec<Expr>,
) -> Stmt {
    let try_call = create::call(state.ts(node, "try"), try_call_args);
    if !uses_any(try_uses) {
        return create::call_statement(try_call);
    }

    create::locals(vec![as_any_identifier(exit_type_id.clone()), as_any_identifier(returns_id.clone())], vec![try_call])
}

fn create_flow_control_condition(
    state: &mut TransformState<'_>,
    node: P<Node>,
    exit_type_id: &Expr,
    flow_control_constant: &str,
) -> Expr {
    create::binary(exit_type_id.clone(), BinaryOperator::Eq, state.ts(node, flow_control_constant))
}

/// `FlowControlCase`
struct FlowControlCase {
    condition: Expr,
    statements: Vec<Stmt>,
}

fn collapse_flow_control_cases(exit_type_id: &Expr, mut cases: Vec<FlowControlCase>) -> Vec<Stmt> {
    let last = cases.pop().expect("there is a flow control case");
    let mut next_statement =
        IfStatement { condition: exit_type_id.clone(), statements: last.statements, else_body: ElseBody::default() };

    while let Some(case) = cases.pop() {
        next_statement = IfStatement {
            condition: case.condition,
            statements: case.statements,
            else_body: ElseBody::If(Box::new(next_statement)),
        };
    }

    vec![Stmt::If(Box::new(next_statement))]
}

fn transform_flow_control(
    state: &mut TransformState<'_>,
    node: P<Node>,
    exit_type_id: &Expr,
    returns_id: &Expr,
    try_uses: TryUses,
) -> Vec<Stmt> {
    let mut flow_control_cases = Vec::new();

    if !uses_any(try_uses) {
        return Vec::new();
    }

    let parent = node.parent().expect("a try statement has a parent");
    let return_blocked = is_return_blocked_by_try_statement(parent);
    let break_blocked = is_break_blocked_by_try_statement(parent);

    if try_uses.uses_return && return_blocked {
        state.mark_try_uses(TryUse::Return);
    }
    if try_uses.uses_break && break_blocked {
        state.mark_try_uses(TryUse::Break);
    }
    if try_uses.uses_continue && break_blocked {
        state.mark_try_uses(TryUse::Continue);
    }

    if try_uses.uses_return {
        if return_blocked {
            flow_control_cases.push(FlowControlCase {
                condition: create_flow_control_condition(state, node, exit_type_id, "TRY_RETURN"),
                statements: vec![Stmt::Return(ReturnStatement {
                    expressions: vec![exit_type_id.clone(), returns_id.clone()],
                })],
            });
            if break_blocked {
                return collapse_flow_control_cases(exit_type_id, flow_control_cases);
            }
        } else {
            flow_control_cases.push(FlowControlCase {
                condition: create_flow_control_condition(state, node, exit_type_id, "TRY_RETURN"),
                statements: vec![create::return_stmt(create::call(globals::unpack(), vec![returns_id.clone()]))],
            });
        }
    }

    if try_uses.uses_break || try_uses.uses_continue {
        if break_blocked {
            flow_control_cases.push(FlowControlCase {
                condition: exit_type_id.clone(),
                statements: vec![create::return_stmt(exit_type_id.clone())],
            });
        } else {
            if try_uses.uses_break {
                flow_control_cases.push(FlowControlCase {
                    condition: create_flow_control_condition(state, node, exit_type_id, "TRY_BREAK"),
                    statements: vec![create::break_stmt()],
                });
            }
            if try_uses.uses_continue {
                flow_control_cases.push(FlowControlCase {
                    condition: create_flow_control_condition(state, node, exit_type_id, "TRY_CONTINUE"),
                    // rbxts-rs: a try statement inside a switch continues the loop, not the switch (`create_continue`)
                    statements: create_continue(state, node),
                });
            }
        }
    }

    collapse_flow_control_cases(exit_type_id, flow_control_cases)
}

/// `transformTryStatement`
pub fn transform_try_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let mut statements = Vec::new();
    let exit_type_id = Expr::TemporaryIdentifier(temp_id("exitType"));
    let returns_id = Expr::TemporaryIdentifier(temp_id("returns"));

    // rbxts-rs: roblox-ts reads the pushed `TryUses` object after its functions are transformed; here the record is
    // popped at that point, which gives the same values.
    state.push_try_uses_stack();
    let try_call_args = transform_try_call_args(state, node);
    let try_uses = state.pop_try_uses_stack().expect("the try uses were pushed");
    statements.push(transform_into_try_call(state, node, &exit_type_id, &returns_id, try_uses, try_call_args));

    statements.extend(transform_flow_control(state, node, &exit_type_id, &returns_id, try_uses));

    statements
}
