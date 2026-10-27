//! `nodes/statements/transformReturnStatement.ts`

use rbxts_luau::{Expr, ReturnStatement, Stmt, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::macro_manager::symbol_names;
use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::{TransformState, TryUse};
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::is_blocked_by_try_statement::is_return_blocked_by_try_statement;
use crate::util::traversal::skip_downwards;
use crate::util::types::{get_first_defined_symbol, is_lua_tuple_type};

fn is_tuple_returning_call(state: &mut TransformState<'_>, ts_expression: P<Node>, luau_expression: &Expr) -> bool {
    // intentionally not state.get_type(), which uses skipUpwards
    if !luau_expression.is_call() {
        return false;
    }
    let t = state.checker.get_type_at_location(skip_downwards(ts_expression));
    is_lua_tuple_type(state, t)
}

fn is_tuple_macro(state: &mut TransformState<'_>, expression: P<Node>) -> bool {
    let callee_type = state.get_type(expression.expression().expect("a call has a callee"));
    let symbol = get_first_defined_symbol(state, callee_type);
    symbol.is_some() && symbol == Some(state.macro_manager.get_symbol_or_throw(symbol_names::TUPLE))
}

/// `transformReturnStatementInner`: `return expression`, with `LuaTuple` values spread.
pub fn transform_return_statement_inner(state: &mut TransformState<'_>, return_exp: P<Node>) -> Vec<Stmt> {
    let mut prereqs = Prereqs::new();

    let inner_return_exp = skip_downwards(return_exp);
    let expressions: Vec<Expr> =
        if inner_return_exp.kind() == Kind::CallExpression && is_tuple_macro(state, inner_return_exp) {
            ensure_transform_order(state, &mut prereqs, inner_return_exp.arguments())
        } else {
            let expression = transform_expression(state, &mut prereqs, inner_return_exp);
            let return_type = state.get_type(return_exp);
            if is_lua_tuple_type(state, return_type) && !is_tuple_returning_call(state, return_exp, &expression) {
                match expression {
                    Expr::Array(members) => members,
                    other => vec![create::call(globals::unpack(), vec![other])],
                }
            } else {
                vec![expression]
            }
        };

    let mut result = prereqs.statements;

    if is_return_blocked_by_try_statement(return_exp) {
        state.mark_try_uses(TryUse::Return);
        let try_return = state.ts(return_exp, "TRY_RETURN");
        result.push(Stmt::Return(ReturnStatement { expressions: vec![try_return, create::array(expressions)] }));
    } else {
        result.push(Stmt::Return(ReturnStatement { expressions }));
    }

    result
}

/// `transformReturnStatement`
pub fn transform_return_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    match node.expression() {
        None => {
            if is_return_blocked_by_try_statement(node) {
                state.mark_try_uses(TryUse::Return);
                let try_return = state.ts(node, "TRY_RETURN");
                return vec![Stmt::Return(ReturnStatement {
                    expressions: vec![try_return, create::array(Vec::new())],
                })];
            }
            vec![create::return_stmt(Expr::Nil)]
        }
        Some(expression) => transform_return_statement_inner(state, expression),
    }
}
