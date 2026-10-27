//! `util/expressionMightMutate.ts`

use rbxts_luau::Expr;
use tsrs_ast::{Kind, Node, SymbolFlags};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::symbols::is_symbol_mutable;
use crate::util::traversal::skip_downwards;

/// Whether `node` reads a member of a (non-const) enum: `E.A`, `E["A"]`.
pub(crate) fn is_enum_member_access(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let member_node = match node.kind() {
        Kind::PropertyAccessExpression => node.as_property_access_expression().name,
        Kind::ElementAccessExpression => node.as_element_access_expression().argument_expression,
        _ => return false,
    };
    let member_symbol = state.checker.get_symbol_at_location_exported(member_node);
    let object = skip_downwards(node.expression().expect("an access has an expression"));
    let enum_symbol = state.checker.get_symbol_at_location_exported(object);
    member_symbol.is_some_and(|s| s.flags().intersects(SymbolFlags::EnumMember))
        && enum_symbol.is_some_and(|s| state.checker.skip_alias(s).flags().intersects(SymbolFlags::Enum))
}

/// `expressionMightMutate`: whether the value of `expression` might change between its evaluation and a later use
/// (so it must be captured first). `node` is the TypeScript expression it came from, if any.
pub fn expression_might_mutate(state: &mut TransformState<'_>, expression: &Expr, node: Option<P<Node>>) -> bool {
    match expression {
        // assume temporary identifiers are never reassigned after being returned
        Expr::TemporaryIdentifier(_) => false,
        Expr::Parenthesized(parenthesized) => expression_might_mutate(state, &parenthesized.expression, None),
        _ if expression.is_simple_primitive() => false,
        Expr::Function(_) | Expr::VarArgs => false,
        Expr::If(if_expression) => {
            expression_might_mutate(state, &if_expression.condition, None)
                || expression_might_mutate(state, &if_expression.expression, None)
                || expression_might_mutate(state, &if_expression.alternative, None)
        }
        Expr::Binary(binary) => {
            expression_might_mutate(state, &binary.left, None) || expression_might_mutate(state, &binary.right, None)
        }
        Expr::Unary(unary) => expression_might_mutate(state, &unary.expression, None),
        Expr::Array(members) | Expr::Set(members) => {
            members.iter().any(|member| expression_might_mutate(state, member, None))
        }
        Expr::Map(fields) => fields.iter().any(|field| {
            expression_might_mutate(state, &field.index, None) || expression_might_mutate(state, &field.value, None)
        }),
        // identifiers, indexing and calls
        _ => {
            if let Some(node) = node {
                let node = skip_downwards(node);
                if is_enum_member_access(state, node) {
                    return false;
                } else if node.kind() == Kind::Identifier
                    && let Some(symbol) = state.checker.get_symbol_at_location_exported(node)
                    && !is_symbol_mutable(state, symbol)
                {
                    return false;
                }
            }
            true
        }
    }
}
