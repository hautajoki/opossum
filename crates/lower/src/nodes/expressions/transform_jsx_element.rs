//! `nodes/expressions/transformJsxElement.ts`

use rbxts_luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::jsx::transform_jsx::transform_jsx;

/// `transformJsxElement`: `<tag attributes>children</tag>`
pub fn transform_jsx_element(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let element = node.as_jsx_element();
    let opening_element = element.opening_element().as_jsx_opening_element();
    transform_jsx(
        state,
        prereqs,
        node,
        opening_element.tag_name(),
        opening_element.attributes(),
        element.children().nodes(),
    )
}
