//! `util/arrayBindingPatternContainsHoists.ts` and `util/arrayLikeExpressionContainsSpread.ts`

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::check_variable_hoist::check_variable_hoist;

/// `arrayBindingPatternContainsHoists`: one of the pattern's plain names is hoisted.
pub fn array_binding_pattern_contains_hoists(state: &mut TransformState<'_>, pattern: P<Node>) -> bool {
    for &element in pattern.elements() {
        // nested patterns use a temporary here; their hoisting is handled elsewhere
        if element.kind() == Kind::BindingElement
            && let Some(name) = element.name()
            && name.kind() == Kind::Identifier
        {
            let symbol = state.checker.get_symbol_at_location_exported(name).expect("a binding name has a symbol");
            // isHoisted is marked inside checkVariableHoist
            check_variable_hoist(state, name, symbol);
            if state.is_hoisted.get(&symbol) == Some(&true) {
                return true;
            }
        }
    }
    false
}

/// `arrayLikeExpressionContainsSpread`
pub fn array_like_expression_contains_spread(expression: P<Node>) -> bool {
    expression.elements().iter().any(|&element| {
        (element.kind() == Kind::BindingElement && element.as_binding_element().dot_dot_dot_token().is_some())
            || element.kind() == Kind::SpreadElement
    })
}
