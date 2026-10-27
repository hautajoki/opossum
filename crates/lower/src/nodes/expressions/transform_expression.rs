//! `nodes/expressions/transformExpression.ts`: expression dispatch by syntax kind.

use rbxts_luau::Expr;
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::evaluation::facts::mark_primitive_value;
use crate::util::types::{is_boolean_type, is_definitely_type, is_number_type, is_string_type, is_undefined_type};
use crate::util::validate_method_assignment::validate_method_expression;

use super::transform_array_literal_expression::transform_array_literal_expression;
use super::transform_await_expression::transform_await_expression;
use super::transform_binary_expression::transform_binary_expression;
use super::transform_call_expression::transform_call_expression;
use super::transform_class_expression::transform_class_expression;
use super::transform_conditional_expression::transform_conditional_expression;
use super::transform_delete_expression::transform_delete_expression;
use super::transform_element_access_expression::transform_element_access_expression;
use super::transform_function_expression::transform_function_expression;
use super::transform_identifier::transform_identifier;
use super::transform_jsx_element::transform_jsx_element;
use super::transform_jsx_expression::transform_jsx_expression;
use super::transform_jsx_fragment::transform_jsx_fragment;
use super::transform_jsx_self_closing_element::transform_jsx_self_closing_element;
use super::transform_new_expression::transform_new_expression;
use super::transform_no_substitution_template_literal::transform_no_substitution_template_literal;
use super::transform_numeric_literal::transform_numeric_literal;
use super::transform_object_literal_expression::transform_object_literal_expression;
use super::transform_omitted_expression::transform_omitted_expression;
use super::transform_parenthesized_expression::transform_parenthesized_expression;
use super::transform_property_access_expression::transform_property_access_expression;
use super::transform_spread_element::transform_spread_element;
use super::transform_string_literal::transform_string_literal;
use super::transform_super_keyword::transform_super_keyword;
use super::transform_tagged_template_expression::transform_tagged_template_expression;
use super::transform_template_expression::transform_template_expression;
use super::transform_this_expression::transform_this_expression;
use super::transform_type_expression::transform_type_expression;
use super::transform_unary_expression::{transform_postfix_unary_expression, transform_prefix_unary_expression};
use super::transform_void_expression::transform_void_expression;
use super::transform_yield_expression::transform_yield_expression;

/// The label of a node kind in diagnostics and the parity report (`getKindName`).
pub fn kind_name(kind: Kind) -> String {
    format!("{kind:?}")
}

/// `transformExpression`
pub fn transform_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    // rbxts-rs: the plugins' by-kind fallback (DESIGN §11.2)
    if let Some(claimed) = crate::plugins::dispatch::claim_expression(state, prereqs, node) {
        return claimed;
    }
    let mut expression = match node.kind() {
        // banned expressions
        Kind::BigIntLiteral => {
            state.add_diagnostic(errors::no_big_int(node));
            Expr::None
        }
        Kind::NullKeyword => {
            state.add_diagnostic(errors::no_null_literal(node));
            Expr::None
        }
        Kind::PrivateIdentifier => {
            state.add_diagnostic(errors::no_private_identifier(node));
            Expr::None
        }
        Kind::RegularExpressionLiteral => {
            state.add_diagnostic(errors::no_regex(node));
            Expr::None
        }
        Kind::TypeOfExpression => {
            state.add_diagnostic(errors::no_type_of_expression(node));
            Expr::None
        }

        // skip transforms
        Kind::ImportKeyword => Expr::None,

        // regular transforms
        Kind::ArrayLiteralExpression => transform_array_literal_expression(state, prereqs, node),
        Kind::ArrowFunction | Kind::FunctionExpression => transform_function_expression(state, node),
        Kind::AsExpression
        | Kind::ExpressionWithTypeArguments
        | Kind::NonNullExpression
        | Kind::SatisfiesExpression
        | Kind::TypeAssertionExpression => transform_type_expression(state, prereqs, node),
        Kind::AwaitExpression => transform_await_expression(state, prereqs, node),
        Kind::BinaryExpression => transform_binary_expression(state, prereqs, node),
        Kind::CallExpression => transform_call_expression(state, prereqs, node),
        Kind::ClassExpression => transform_class_expression(state, prereqs, node),
        Kind::ConditionalExpression => transform_conditional_expression(state, prereqs, node),
        Kind::DeleteExpression => transform_delete_expression(state, prereqs, node),
        Kind::ElementAccessExpression => transform_element_access_expression(state, prereqs, node),
        Kind::FalseKeyword => Expr::False,
        Kind::TrueKeyword => Expr::True,
        Kind::Identifier => transform_identifier(state, node),
        Kind::JsxElement => transform_jsx_element(state, prereqs, node),
        Kind::JsxExpression => transform_jsx_expression(state, prereqs, node),
        Kind::JsxFragment => transform_jsx_fragment(state, prereqs, node),
        Kind::JsxSelfClosingElement => transform_jsx_self_closing_element(state, prereqs, node),
        Kind::NewExpression => transform_new_expression(state, prereqs, node),
        Kind::NoSubstitutionTemplateLiteral => transform_no_substitution_template_literal(node),
        Kind::NumericLiteral => transform_numeric_literal(state, node),
        Kind::ObjectLiteralExpression => transform_object_literal_expression(state, prereqs, node),
        Kind::OmittedExpression => transform_omitted_expression(),
        Kind::ParenthesizedExpression => transform_parenthesized_expression(state, prereqs, node),
        Kind::PostfixUnaryExpression => transform_postfix_unary_expression(state, prereqs, node),
        Kind::PrefixUnaryExpression => transform_prefix_unary_expression(state, prereqs, node),
        Kind::PropertyAccessExpression => transform_property_access_expression(state, prereqs, node),
        Kind::SpreadElement => transform_spread_element(state, prereqs, node),
        Kind::StringLiteral => transform_string_literal(node),
        Kind::SuperKeyword => transform_super_keyword(),
        Kind::TaggedTemplateExpression => transform_tagged_template_expression(state, prereqs, node),
        Kind::TemplateExpression => transform_template_expression(state, prereqs, node),
        Kind::ThisKeyword => transform_this_expression(state, node),
        Kind::VoidExpression => transform_void_expression(state, prereqs, node),
        Kind::YieldExpression => transform_yield_expression(state, prereqs, node),

        kind => {
            // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
            state.internal_error(node, &format!("Unknown expression: {}", kind_name(kind)));
            return Expr::None;
        }
    };

    let t = state.get_type(node);
    validate_method_expression(state, node, t);
    let is_primitive = match state.multi_transform_state.is_primitive_type_cache.get(&t) {
        Some(&cached) => cached,
        None => {
            let result =
                is_definitely_type(state, t, &[&is_boolean_type, &is_number_type, &is_string_type, &is_undefined_type]);
            state.multi_transform_state.is_primitive_type_cache.insert(t, result);
            result
        }
    };
    if is_primitive {
        mark_primitive_value(&mut expression);
    }
    expression
}
