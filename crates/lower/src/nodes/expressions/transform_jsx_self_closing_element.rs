//! `nodes/expressions/transformJsxSelfClosingElement.ts`

use rbxts_luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::jsx::transform_jsx::transform_jsx;

/// `transformJsxSelfClosingElement`: `<tag attributes />`
pub fn transform_jsx_self_closing_element(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    let element = node.as_jsx_self_closing_element();
    transform_jsx(state, prereqs, node, element.tag_name(), element.attributes(), &[])
}
