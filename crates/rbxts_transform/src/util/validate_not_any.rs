//! `util/validateNotAny.ts`

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::diagnostic_service::{OnceCache, ReportOnce};
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::symbols::get_original_symbol_of_node;
use crate::util::traversal::skip_downwards;
use crate::util::types::{is_any_type, is_array_type, is_definitely_type};

/// `validateNotAnyType`: values of type `any` (or arrays of it) are not supported.
pub fn validate_not_any_type(state: &mut TransformState<'_>, mut node: P<Node>) {
    if node.kind() == Kind::SpreadElement {
        node = skip_downwards(node.expression().expect("a spread has an expression"));
    }

    let t = state.get_type(node);
    let is_any = match state.multi_transform_state.is_any_or_any_array_cache.get(&t) {
        Some(&cached) => cached,
        None => {
            let mut checked_type = t;
            if is_definitely_type(state, t, &[&is_array_type]) {
                // Array<T> -> T
                let number_type = state.checker.get_number_type();
                if let Some(index_type) = state.checker.get_index_type_of_type_exported(t, number_type) {
                    checked_type = index_type;
                }
            }
            let result = is_definitely_type(state, checked_type, &[&is_any_type]);
            state.multi_transform_state.is_any_or_any_array_cache.insert(t, result);
            result
        }
    };

    if is_any {
        // given a type like `a: { [index: string]: any }`, `a["b"]` will not have a symbol
        match get_original_symbol_of_node(state.checker, node) {
            Some(symbol) => {
                let key = ReportOnce::new(OnceCache::NoAny, symbol, state.source_file);
                if state.multi_transform_state.reported_once.insert(key) {
                    state.diagnostics.add_diagnostic_once(key, errors::no_any(node));
                }
            }
            None => state.add_diagnostic(errors::no_any(node)),
        }
    }
}
