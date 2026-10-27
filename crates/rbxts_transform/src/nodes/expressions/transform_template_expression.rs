//! `nodes/expressions/transformTemplateExpression.ts`

use rbxts_luau::{Expr, InterpolatedStringPart, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::transform_interpolated_string_part::transform_interpolated_string_part;
use crate::util::ensure_transform_order::ensure_transform_order;

/// `` `a${b}c` `` as a Luau interpolated string.
pub fn transform_template_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let template = node.as_template_expression();
    let mut parts = Vec::new();

    if !template.head.text().is_empty() {
        parts.push(transform_interpolated_string_part(template.head));
    }

    let spans = template.template_spans.nodes();
    let expressions: Vec<P<Node>> = spans.iter().map(|s| s.as_template_span().expression).collect();
    let ordered_expressions = ensure_transform_order(state, prereqs, &expressions);

    for (span, expression) in spans.iter().zip(ordered_expressions) {
        parts.push(InterpolatedStringPart::Expression(expression));
        let literal = span.as_template_span().literal;
        if !literal.text().is_empty() {
            parts.push(transform_interpolated_string_part(literal));
        }
    }

    create::interpolated_string(parts)
}
