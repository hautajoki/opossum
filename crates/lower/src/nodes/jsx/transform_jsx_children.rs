//! `nodes/jsx/transformJsxChildren.ts`

use std::fmt::Write;

use rbxts_luau::{Expr, Quote, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::shared::diagnostics::errors;
use crate::util::ensure_transform_order::ensure_transform_order_with;
use crate::util::fixup_whitespace_and_decode_entities::fixup_whitespace_and_decode_entities;

fn is_whitespace_only_jsx_text(child: P<Node>) -> bool {
    child.kind() == Kind::JsxText && child.as_jsx_text().contains_only_trivia_white_spaces()
}

/// A JSX text child as a double-quoted Luau string: backslashes, quotes and control characters escaped.
fn transform_jsx_text(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    // rbxts-rs: roblox-ts throws on both (a RangeError from `String.fromCodePoint`, an assertion); a diagnostic keeps
    // the build going
    let text = match fixup_whitespace_and_decode_entities(node.as_jsx_text().text()) {
        Ok(Some(text)) => text,
        Ok(None) => {
            state.internal_error(node, "Expected JSX text that is not only whitespace to have a value");
            return Expr::None;
        }
        Err(_) => {
            // rbxts-rs: fixes roblox-ts crashing the whole build (a RangeError): an error for this file, which is not
            // a compiler bug, so not an internal error
            state.add_diagnostic(errors::no_invalid_jsx_entity(node));
            return Expr::None;
        }
    };
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            // decoded entities can contain literal control characters
            '\x00'..='\x1f' | '\x7f' => {
                let _ = write!(escaped, "\\x{:02x}", u32::from(character));
            }
            _ => escaped.push(character),
        }
    }
    // decoded text needs quoted escapes even when it contains both quote characters
    create::string_quoted(escaped, Quote::Double)
}

/// `transformJsxChildren`: the children as arguments, in order: text as strings, `{expression}` as its value and
/// `{...expression}` unpacked. Whitespace-only text and empty `{}` are skipped.
pub fn transform_jsx_children(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    children: &[P<Node>],
) -> Vec<Expr> {
    let last_jsx_child_index = children.iter().rposition(|&child| !is_whitespace_only_jsx_text(child));

    if let Some(last) = last_jsx_child_index {
        for &child in &children[..last] {
            if child.kind() == Kind::JsxExpression && child.as_jsx_expression().dot_dot_dot_token().is_some() {
                state.add_diagnostic(errors::no_preceding_jsx_spread_element(child));
            }
        }
    }

    let nodes: Vec<P<Node>> = children
        .iter()
        .copied()
        // ignore jsx text that only contains whitespace
        .filter(|&child| !is_whitespace_only_jsx_text(child))
        // ignore empty jsx expressions, i.e. `{}`
        .filter(|&child| child.kind() != Kind::JsxExpression || child.as_jsx_expression().expression().is_some())
        .collect();

    ensure_transform_order_with(state, prereqs, &nodes, &mut |state, prereqs, node| {
        if node.kind() == Kind::JsxText {
            transform_jsx_text(state, node)
        } else {
            transform_expression(state, prereqs, node)
        }
    })
}
