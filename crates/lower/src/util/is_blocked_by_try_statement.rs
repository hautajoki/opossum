//! `util/isBlockedByTryStatement.ts`

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::util::traversal::get_ancestor;

/// A `return` inside a try statement of the same function.
pub fn is_return_blocked_by_try_statement(node: P<Node>) -> bool {
    get_ancestor(node, |a| a.kind() == Kind::TryStatement || tsrs_ast::is_function_like_declaration(a))
        .is_some_and(|a| a.kind() == Kind::TryStatement)
}

/// A `break` / `continue` inside a try statement of the same loop or switch.
pub fn is_break_blocked_by_try_statement(node: P<Node>) -> bool {
    get_ancestor(node, |a| {
        a.kind() == Kind::TryStatement
            || tsrs_ast::is_iteration_statement(a, false)
            || a.kind() == Kind::SwitchStatement
    })
    .is_some_and(|a| a.kind() == Kind::TryStatement)
}
