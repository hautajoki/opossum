//! `util/evaluation/bindings.ts`: which bindings closures can observe or reassign.

use std::rc::Rc;

use rustc_hash::FxHashSet;

use rbxts_luau::BindingKey;
use tsrs_ast::{Kind, Node, SourceFile, Symbol};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::traversal::get_ancestor;

/// `BindingUsage`
#[derive(Default)]
pub struct BindingUsage {
    /// Bindings of the file used in another function than the one declaring them.
    pub captured: FxHashSet<P<Symbol>>,
    /// Those of them assigned there.
    pub written_by_closure: FxHashSet<P<Symbol>>,
}

fn enclosing_function(node: P<Node>) -> Option<P<Node>> {
    node.parent().and_then(|p| get_ancestor(p, tsrs_ast::is_function_like))
}

fn visit(state: &mut TransformState<'_>, source_file: P<SourceFile>, node: P<Node>, usage: &mut BindingUsage) {
    if node.kind() == Kind::Identifier {
        let symbol = match node.parent() {
            Some(p) if p.kind() == Kind::ShorthandPropertyAssignment => {
                state.checker.get_shorthand_assignment_value_symbol(Some(p))
            }
            _ => state.checker.get_symbol_at_location_exported(node),
        };
        if let Some(symbol) = symbol
            && let Some(declaration) = symbol.value_declaration()
            && tsrs_ast::get_source_file_of_node(declaration) == Some(source_file)
            && enclosing_function(declaration) != enclosing_function(node)
        {
            usage.captured.insert(symbol);
            if tsrs_ast::is_assignment_target(node) {
                usage.written_by_closure.insert(symbol);
            }
        }
    }
    node.for_each_child(&mut |child| {
        visit(state, source_file, child, usage);
        false
    });
}

fn get_binding_usage(state: &mut TransformState<'_>, source_file: P<SourceFile>) -> Rc<BindingUsage> {
    if let Some(usage) = state.multi_transform_state.binding_usage_by_source_file.get(&source_file) {
        return Rc::clone(usage);
    }
    let mut usage = BindingUsage::default();
    visit(state, source_file, source_file.as_node(), &mut usage);
    let usage = Rc::new(usage);
    state.multi_transform_state.binding_usage_by_source_file.insert(source_file, Rc::clone(&usage));
    usage
}

/// `getBindingKey`: unknown callees can only rebind a local if some closure has access to it.
pub fn get_binding_key(state: &mut TransformState<'_>, symbol: P<Symbol>, reference: P<Node>) -> BindingKey {
    if let Some(&key) = state.multi_transform_state.binding_keys.get(&symbol) {
        return key;
    }
    let source_file = tsrs_ast::get_source_file_of_node(reference).expect("a reference has a source file");
    let usage = get_binding_usage(state, source_file);
    let external = symbol.value_declaration().is_none_or(|d| tsrs_ast::get_source_file_of_node(d) != Some(source_file));
    let key = BindingKey {
        id: P::addr(symbol) as u64,
        captured: external || usage.captured.contains(&symbol),
        written_by_closure: external || usage.written_by_closure.contains(&symbol),
    };
    state.multi_transform_state.binding_keys.insert(symbol, key);
    key
}
