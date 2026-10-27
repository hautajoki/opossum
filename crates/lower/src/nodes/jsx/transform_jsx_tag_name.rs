//! `nodes/jsx/transformJsxTagName.ts`

use luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;

fn transform_jsx_tag_name_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    // host component
    // rbxts-rs: fixes roblox-ts taking every name whose first character lower-cases to itself (`c === c.toLowerCase()`)
    // as a host component: `<_Inner />`, `<$x />` or `<éa />` became strings, where TypeScript checks (and emits) them
    // as the components they name, and `<Foo-bar />` failed an assertion (an invalid Luau identifier), where TypeScript
    // has a host element. TypeScript's own rule (`isIntrinsicJsxName`: a lower-case ASCII letter first, or a `-`)
    // decides now.
    if node.kind() == Kind::Identifier && tsrs_scanner::is_intrinsic_jsx_name(node.text()) {
        return create::string(node.text());
    }

    match node.kind() {
        Kind::PropertyAccessExpression => {
            let access = node.as_property_access_expression();
            assert_eq!(access.name.kind(), Kind::Identifier);
            let base = transform_expression(state, prereqs, access.expression);
            create::property(convert_to_indexable_expression(base), access.name.text())
        }
        Kind::JsxNamespacedName => create::string(node.text()),
        _ => transform_expression(state, prereqs, node),
    }
}

/// `transformJsxTagName`: a string for host components (`frame`, `ns:name`), else the component's expression.
pub fn transform_jsx_tag_name(state: &mut TransformState<'_>, prereqs: &mut Prereqs, tag_name: P<Node>) -> Expr {
    transform_jsx_tag_name_expression(state, prereqs, tag_name)
}
