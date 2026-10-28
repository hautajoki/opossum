//! `util/wrapReturnIfLuaTuple.ts`

use luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_checker::{SignatureFlags, SignatureKind, TypeFlags};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
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

/// Whether the function `call` calls may return undefined besides its `LuaTuple`. The call's own type is no guide: an
/// optional call (`a.f?.()`) adds the chain's undefined to it and to its resolved signature, so then the callee's own
/// signature answers.
fn returns_nullable(state: &mut TransformState<'_>, call: P<Node>) -> bool {
    if call.kind() != Kind::CallExpression {
        return false;
    }
    let mut signature = state.checker.get_resolved_signature_exported(call);
    if signature.flags().intersects(SignatureFlags::CallChainFlags) {
        let Some(callee) = call.expression() else { return false };
        let t = state.get_type(callee);
        let t = state.checker.get_non_nullable_type(t);
        let declaration = signature.declaration();
        let own = state.checker.get_signatures_of_type_exported(t, SignatureKind::Call);
        match own.iter().find(|s| s.declaration() == declaration) {
            Some(&s) => signature = s,
            None => return false,
        }
    }
    let returns = state.checker.get_return_type_of_signature_exported(signature);
    let nullable = TypeFlags::Undefined | TypeFlags::Void | TypeFlags::Null;
    returns.flags().intersects(TypeFlags::Union) && returns.types().iter().any(|m| m.flags().intersects(nullable))
}

/// `wrapReturnIfLuaTuple`: a call returning a `LuaTuple` used as a value is wrapped in `{ … }`. One that may return
/// undefined instead is an error (`lower.nullableLuaTuple`): the table is never nil, so its undefined checks would
/// compile to constants.
pub fn wrap_return_if_lua_tuple(state: &mut TransformState<'_>, node: P<Node>, exp: Expr) -> Expr {
    let t = state.get_type(node);
    let non_nullable = state.checker.get_non_nullable_type(t);
    if is_lua_tuple_type(state, non_nullable) && should_wrap_lua_tuple(state, node, &exp) {
        if returns_nullable(state, node) {
            state.add_diagnostic(errors::nullable_lua_tuple(node));
        }
        return create::array(vec![exp]);
    }
    exp
}
