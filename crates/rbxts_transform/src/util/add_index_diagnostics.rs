//! `util/addIndexDiagnostics.ts`

use tsrs_ast::{Kind, Node};
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::is_method::is_method;
use crate::util::is_valid_method_index_without_call::is_valid_method_index_without_call;
use crate::util::traversal::skip_upwards;
use crate::util::types::{get_first_defined_symbol, is_possibly_type, string_literal_value};

fn is_length_literal(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    string_literal_value(t) == Some("length")
}

fn is_tuple(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.is_tuple_type()
}

fn is_length_property_name(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    if node.kind() == Kind::ComputedPropertyName {
        let argument_type = state.get_type(node.expression().expect("a computed name has an expression"));
        return is_possibly_type(state, argument_type, &[&is_length_literal]);
    }
    matches!(node.kind(), Kind::Identifier | Kind::StringLiteral) && node.text() == "length"
}

/// `addIndexDiagnostics`: macros and methods indexed without a call, `prototype`, and `length` of tuples.
pub fn add_index_diagnostics(
    state: &mut TransformState<'_>,
    node: P<Node>,
    member_type: P<Type>,
    receiver_type: P<Type>,
) {
    let symbol = get_first_defined_symbol(state, member_type);
    let is_property_call_macro =
        symbol.is_some_and(|s| state.macro_manager.get_property_call_macro(state.checker, s).is_some());
    if is_property_call_macro
        || (!is_valid_method_index_without_call(state, skip_upwards(node)) && is_method(state, node))
    {
        state.add_diagnostic(errors::no_index_without_call(node));
    }

    if tsrs_ast::is_prototype_access(node) {
        state.add_diagnostic(errors::no_prototype(node));
    }

    if is_possibly_type(state, receiver_type, &[&is_tuple]) {
        if node.kind() == Kind::PropertyAccessExpression && node.as_property_access_expression().name.text() == "length"
        {
            state.add_diagnostic(errors::no_length_index_in_tuples(node));
        }

        if tsrs_ast::is_property_name(node) && is_length_property_name(state, node) {
            state.add_diagnostic(errors::no_length_index_in_tuples(node));
        }

        if node.kind() == Kind::ElementAccessExpression {
            let argument_type = state.get_type(node.as_element_access_expression().argument_expression);
            if is_possibly_type(state, argument_type, &[&is_length_literal]) {
                state.add_diagnostic(errors::no_length_index_in_tuples(node));
            }
        }
    }
}
