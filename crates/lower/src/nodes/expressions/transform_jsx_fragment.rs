//! `nodes/expressions/transformJsxFragment.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::jsx::transform_jsx::transform_jsx_factory_entity;
use crate::nodes::jsx::transform_jsx_children::transform_jsx_children;
use crate::nodes::transform_entity_name::transform_entity_name;

/// `transformJsxFragment`: `factory(fragmentFactory, nil, ...children)`.
pub fn transform_jsx_fragment(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let create_element_expression = transform_jsx_factory_entity(state, node);

    // getJsxFragmentFactoryEntity() doesn't seem to default to "Fragment".. but the typechecker does, so we should
    // follow that behavior
    let fragment_factory = match state.resolver.get_jsx_fragment_factory_entity(state.checker, node) {
        Some(jsx_fragment_factory_entity) => transform_entity_name(state, jsx_fragment_factory_entity),
        // rbxts-rs: roblox-ts parses `Fragment` into a synthetic identifier, which transforms to the plain name
        None => create::id("Fragment"),
    };
    let mut args = vec![fragment_factory];

    let transformed_children = transform_jsx_children(state, prereqs, node.children().nodes());

    // props parameter
    if !transformed_children.is_empty() {
        args.push(create::nil());
    }

    args.extend(transformed_children);

    create::call(create_element_expression, args)
}
