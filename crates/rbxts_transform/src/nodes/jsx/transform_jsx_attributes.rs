//! `nodes/jsx/transformJsxAttributes.ts`

use rbxts_luau::{AnyIdentifier, AssignmentOperator, ElseBody, Expr, Stmt, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::shared::diagnostics::errors;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::create_truthiness_checks::create_truthiness_checks;
use crate::util::fixup_whitespace_and_decode_entities::decode_entities_in_text;
use crate::util::pointer::{MapPointer, assign_to_map_pointer, disable_map_inline};
use crate::util::types::{get_first_defined_symbol, is_definitely_type, is_object_type};

/// `createJsxAttributeLoop`: `for k, v in expression do attributes[k] = v end`, guarded by a truthiness check unless
/// the expression is definitely an object.
fn create_jsx_attribute_loop(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    attributes_ptr_value: Expr,
    mut expression: Expr,
    ts_expression: P<Node>,
) -> Stmt {
    let t = state.get_type(ts_expression);
    let definitely_object = is_definitely_type(state, t, &[&is_object_type]);
    if !definitely_object {
        expression = prereqs.push_to_var_if_complex(expression, "attribute");
    }

    let key_id = temp_id("k");
    let value_id = temp_id("v");
    let mut statement = create::for_in(
        vec![AnyIdentifier::Temporary(key_id.clone()), AnyIdentifier::Temporary(value_id.clone())],
        expression.clone(),
        vec![create::assignment(
            create::computed_index(attributes_ptr_value, Expr::TemporaryIdentifier(key_id)),
            AssignmentOperator::Assign,
            Expr::TemporaryIdentifier(value_id),
        )],
    );

    if !definitely_object {
        let condition = create_truthiness_checks(state, prereqs, expression, ts_expression);
        statement = create::if_stmt(condition, vec![statement], ElseBody::default());
    }

    statement
}

/// `transformJsxAttribute`: `name={value}`, `name="value"` or a bare `name` (`true`).
fn transform_jsx_attribute(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    attribute: P<Node>,
    attributes_ptr: &mut MapPointer,
) {
    let jsx_attribute = attribute.as_jsx_attribute();
    let mut initializer = jsx_attribute.initializer();
    if let Some(init) = initializer
        && init.kind() == Kind::JsxExpression
    {
        initializer = init.as_jsx_expression().expression();
    }

    let mut init_prereqs = Prereqs::new();
    let init = match initializer {
        // rbxts-rs: fixes roblox-ts reading an attribute's string (`Text="a\nb &amp; c"`) as a JavaScript string
        // literal: JSX attribute strings have no escapes (`\n` is a backslash and an `n`), and their HTML entities are
        // decoded (TypeScript's `tryDecodeEntities`). A line break inside one went into the quotes as it was, which
        // Luau rejects. A string with none of these is spelled as before.
        Some(initializer)
            if initializer.kind() == Kind::StringLiteral
                && initializer.text().contains(['\\', '&', '\n', '\r']) =>
        {
            match decode_entities_in_text(initializer.text()) {
                Ok(value) => create_string_from_value(&value),
                Err(_) => {
                    state.add_diagnostic(errors::no_invalid_jsx_entity(initializer));
                    Expr::None
                }
            }
        }
        Some(initializer) => transform_expression(state, &mut init_prereqs, initializer),
        None => create::bool(true),
    };

    if !init_prereqs.statements.is_empty() {
        disable_map_inline(prereqs, attributes_ptr);
        prereqs.push_list(init_prereqs.statements);
    }

    // an identifier, or a namespaced name as `namespace:name`
    let name = create::string(jsx_attribute.name().text());
    assign_to_map_pointer(prereqs, attributes_ptr, name, init);
}

/// `transformJsxAttributes`: fills `attributes_ptr` with the attributes and spread attributes, in order.
pub fn transform_jsx_attributes(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    attributes: P<Node>,
    attributes_ptr: &mut MapPointer,
) {
    let properties = attributes.properties();
    for &attribute in properties {
        if attribute.kind() == Kind::JsxAttribute {
            transform_jsx_attribute(state, prereqs, attribute, attributes_ptr);
        } else {
            // spread attributes: `<frame { ...x }/>`
            let attribute_expression = attribute.as_jsx_spread_attribute().expression();

            let attribute_type = state.get_type(attribute_expression);
            let exp_type = state.checker.get_non_optional_type(attribute_type);
            if let Some(symbol) = get_first_defined_symbol(state, exp_type)
                && state.macro_manager.is_macro_only_class(symbol)
            {
                state.add_diagnostic(errors::no_macro_object_spread(attribute));
            }

            let expression = transform_expression(state, prereqs, attribute_expression);

            if Some(&attribute) == properties.first() && is_definitely_type(state, exp_type, &[&is_object_type]) {
                attributes_ptr.value =
                    prereqs.push_to_var(create::call(globals::table::clone(), vec![expression]), &attributes_ptr.name);
                // Explicitly remove metatable because things like classes can be spread
                prereqs.push(create::call_statement(create::call(
                    globals::setmetatable(),
                    vec![attributes_ptr.value.clone(), create::nil()],
                )));
                continue;
            }

            disable_map_inline(prereqs, attributes_ptr);
            let statement = create_jsx_attribute_loop(
                state,
                prereqs,
                attributes_ptr.value.clone(),
                expression,
                attribute_expression,
            );
            prereqs.push(statement);
        }
    }
}
