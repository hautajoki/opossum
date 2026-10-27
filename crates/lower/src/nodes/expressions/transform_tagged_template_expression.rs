//! `nodes/expressions/transformTaggedTemplateExpression.ts`

use rbxts_luau::{Expr, create};
use tsrs_ast::tokenflags::TokenFlags;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::ensure_transform_order::ensure_transform_order;

/// The cooked string of a template part, as JavaScript gives it to the tag.
///
/// rbxts-rs: fixes roblox-ts passing the cooked text to `luau.string`, which renders it as Luau source text: a
/// cooked backslash (`` tag`a\\b` ``) became an escape (`"a\b"`, a backspace), a cooked leading newline was dropped by
/// the long bracket it went into, and an invalid escape (`` tag`\unicode` ``, whose cooked string is `undefined`)
/// gave invalid Luau. The value is now escaped where it would not render as itself, and an invalid escape gives `nil`.
fn cooked_string(part: P<Node>) -> Expr {
    let flags = part.template_literal_like_data().expect("a template part").template_flags;
    if flags.intersects(TokenFlags::ContainsInvalidEscape) {
        return Expr::Nil;
    }
    create_string_from_value(part.text())
}

/// `` tag`a${b}c` ``: `tag({ "a", "c" }, b)`, the strings cooked.
pub fn transform_tagged_template_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    let tagged = node.as_tagged_template_expression();
    let tag_exp = transform_expression(state, prereqs, tagged.tag);
    let template = tagged.template;

    if template.kind() == Kind::TemplateExpression {
        let template = template.as_template_expression();
        let spans = template.template_spans.nodes();
        let mut strings = vec![cooked_string(template.head)];
        strings.extend(spans.iter().map(|span| cooked_string(span.as_template_span().literal)));

        let expressions: Vec<P<Node>> = spans.iter().map(|span| span.as_template_span().expression).collect();
        let expressions = ensure_transform_order(state, prereqs, &expressions);

        let mut args = vec![create::array(strings)];
        args.extend(expressions);
        create::call(convert_to_indexable_expression(tag_exp), args)
    } else {
        create::call(convert_to_indexable_expression(tag_exp), vec![create::array(vec![cooked_string(template)])])
    }
}
