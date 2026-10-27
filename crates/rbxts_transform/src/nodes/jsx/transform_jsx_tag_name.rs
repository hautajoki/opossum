//! `nodes/jsx/transformJsxTagName.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use rbxts_plugin::AccessPosition;

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
            // rbxts-rs: a dotted tag is an access roblox-ts lowers itself, so plugins are offered it as any access: the
            // expression hooks, then `property_key` (DESIGN §11.5)
            if let Some(claimed) = crate::plugins::dispatch::claim_expression(state, prereqs, node) {
                return claimed;
            }
            let access = node.as_property_access_expression();
            assert_eq!(access.name.kind(), Kind::Identifier);
            let base = transform_expression(state, prereqs, access.expression);
            match crate::plugins::dispatch::rewritten_key(state, node, AccessPosition::Read) {
                Some(key) => crate::plugins::backend::keyed_access(base, &key),
                None => create::property(convert_to_indexable_expression(base), access.name.text()),
            }
        }
        Kind::JsxNamespacedName => create::string(node.text()),
        _ => transform_expression(state, prereqs, node),
    }
}

/// `transformJsxTagName`: a string for host components (`frame`, `ns:name`), else the component's expression.
///
/// rbxts-rs: JSX tag names only allow identifiers, `this`, dotted names and namespaced names, which roblox-ts lowers
/// without prerequisites (it asserts there are none). A plugin that claims a tag (or a part of one) may add some: they
/// go to `prereqs`, before the element, whose tag JavaScript evaluates first.
pub fn transform_jsx_tag_name(state: &mut TransformState<'_>, prereqs: &mut Prereqs, tag_name: P<Node>) -> Expr {
    let mut expression_prereqs = Prereqs::new();
    let expression = transform_jsx_tag_name_expression(state, &mut expression_prereqs, tag_name);
    assert!(state.plugins.is_some() || expression_prereqs.statements.is_empty());
    prereqs.push_list(expression_prereqs.statements);
    expression
}
