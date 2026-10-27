//! `util/wrapReturnIfLuaTuple.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::array_binding_pattern_contains_hoists::{
    array_binding_pattern_contains_hoists, array_like_expression_contains_spread,
};
use crate::util::traversal::skip_upwards;
use crate::util::types::is_lua_tuple_type;

fn should_wrap_lua_tuple(state: &mut TransformState<'_>, node: P<Node>, exp: &Expr) -> bool {
    if !exp.is_call() {
        return true;
    }

    let child = skip_upwards(node);
    let Some(parent) = child.parent() else { return true };
    let call_is_optional = node.question_dot_token().is_some();

    match parent.kind() {
        // `foo();`
        Kind::ExpressionStatement => false,
        // part of a for statement's header, except its condition
        Kind::ForStatement if parent.as_for_statement().condition != Some(child) => false,
        // `const [a] = foo()`
        Kind::VariableDeclaration => {
            let name = parent.name().expect("a variable declaration has a name");
            !(name.kind() == Kind::ArrayBindingPattern
                && !array_binding_pattern_contains_hoists(state, name)
                && !array_like_expression_contains_spread(name)
                && !call_is_optional)
        }
        // `[a] = foo()`
        Kind::BinaryExpression
            if tsrs_ast::is_assignment_expression(parent, false)
                && parent.as_binary_expression().left.kind() == Kind::ArrayLiteralExpression
                && !array_like_expression_contains_spread(parent.as_binary_expression().left)
                && !call_is_optional =>
        {
            false
        }
        // `foo()[n]`
        Kind::ElementAccessExpression if parent.question_dot_token().is_none() => false,
        // `return foo()`
        Kind::ReturnStatement => false,
        // `void foo()`
        Kind::VoidExpression => false,
        _ => true,
    }
}

/// `wrapReturnIfLuaTuple`: a call returning a `LuaTuple` used as a value is wrapped in `{ … }`.
pub fn wrap_return_if_lua_tuple(state: &mut TransformState<'_>, node: P<Node>, exp: Expr) -> Expr {
    let t = state.get_type(node);
    let non_nullable = state.checker.get_non_nullable_type(t);
    if is_lua_tuple_type(state, non_nullable) && should_wrap_lua_tuple(state, node, &exp) {
        return create::array(vec![exp]);
    }
    exp
}
