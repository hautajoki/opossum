//! `util/isValidMethodIndexWithoutCall.ts`

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::types::get_first_defined_symbol;

/// A method may be indexed without a call in `a.b !== undefined`, `!a.b`, and as the first argument of
/// `typeIs` / `typeOf`.
pub fn is_valid_method_index_without_call(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let Some(parent) = node.parent() else { return false };
    match parent.kind() {
        // a.b !== undefined
        Kind::BinaryExpression => true,
        // !a.b
        Kind::PrefixUnaryExpression => true,
        // typeIs/typeOf macros
        Kind::CallExpression => {
            let callee = parent.expression().expect("a call has a callee");
            let t = state.get_type(callee);
            let exp_type = state.checker.get_non_optional_type(t);
            match get_first_defined_symbol(state, exp_type) {
                Some(symbol) => state
                    .macro_manager
                    .get_call_macro(state.checker, symbol)
                    // typeIs will be a TypeError if usage is not the first argument
                    .is_some_and(|m| matches!(m.name, "typeIs" | "typeOf")),
                None => false,
            }
        }
        _ => false,
    }
}
