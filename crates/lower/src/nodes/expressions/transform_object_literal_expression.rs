//! `nodes/expressions/transformObjectLiteralExpression.ts`

use rbxts_luau::{AnyIdentifier, AssignmentOperator, ElseBody, Expr, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_method_declaration::transform_method_declaration;
use crate::nodes::transform_property_name::transform_property_name;
use crate::shared::diagnostics::errors;
use crate::util::create_truthiness_checks::create_truthiness_checks;
use crate::util::pointer::{MapPointer, assign_to_map_pointer, create_map_pointer, disable_map_inline};
use crate::util::types::{get_first_defined_symbol, is_definitely_type, is_object_type};
use crate::util::validate_method_assignment::validate_method_assignment;

/// `transformPropertyAssignment`: `name: initializer` (or a shorthand `name`, whose initializer is the name).
fn transform_property_assignment(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    ptr: &mut MapPointer,
    name: P<Node>,
    initializer: P<Node>,
    edit: FieldEdit,
) {
    let mut left_prereqs = Prereqs::new();
    let mut left = match edit.key {
        Some(key) => key,
        None => transform_property_name(state, &mut left_prereqs, name),
    };
    let mut right_prereqs = Prereqs::new();
    let right = match edit.value {
        Some(value) => value,
        None => transform_expression(state, &mut right_prereqs, initializer),
    };

    if !left_prereqs.statements.is_empty() || !right_prereqs.statements.is_empty() {
        disable_map_inline(prereqs, ptr);
        prereqs.push_list(left_prereqs.statements);
        left = prereqs.push_to_var(left, "left");
    }

    prereqs.push_list(right_prereqs.statements);

    assign_to_map_pointer(prereqs, ptr, left, right);
}

/// `transformSpreadAssignment`: `...expression`, cloned when it starts the object, else copied key by key.
fn transform_spread_assignment(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    ptr: &mut MapPointer,
    property: P<Node>,
) {
    let expression = property.expression().expect("a spread assignment has an expression");
    let expression_type = state.get_type(expression);
    let exp_type = state.checker.get_non_optional_type(expression_type);
    if let Some(symbol) = get_first_defined_symbol(state, exp_type)
        && state.macro_manager.is_macro_only_class(symbol)
    {
        state.add_diagnostic(errors::no_macro_object_spread(property));
    }

    let t = state.get_type(expression);
    let definitely_object = is_definitely_type(state, t, &[&is_object_type]);

    if definitely_object && matches!(&ptr.value, Expr::Map(fields) if fields.is_empty()) {
        let spread = transform_expression(state, prereqs, expression);
        ptr.value = prereqs.push_to_var(create::call(globals::table::clone(), vec![spread]), &ptr.name);
        // explicitly remove the metatable, because things like classes can be spread
        prereqs.push(create::call_statement(create::call(
            globals::setmetatable(),
            vec![ptr.value.clone(), create::nil()],
        )));
        return;
    }

    disable_map_inline(prereqs, ptr);
    let mut spread_exp = transform_expression(state, prereqs, expression);
    if !definitely_object {
        spread_exp = prereqs.push_to_var_if_complex(spread_exp, "spread");
    }

    let key_id = temp_id("k");
    let value_id = temp_id("v");
    let mut statement = create::for_in(
        vec![AnyIdentifier::Temporary(key_id.clone()), AnyIdentifier::Temporary(value_id.clone())],
        spread_exp.clone(),
        vec![create::assignment(
            create::computed_index(ptr.value.clone(), Expr::TemporaryIdentifier(key_id)),
            AssignmentOperator::Assign,
            Expr::TemporaryIdentifier(value_id),
        )],
    );

    if !definitely_object {
        let condition = create_truthiness_checks(state, prereqs, spread_exp, expression);
        statement = create::if_stmt(condition, vec![statement], ElseBody::default());
    }

    prereqs.push(statement);
}

/// `transformObjectLiteralExpression`: an inline map for as long as no member needs statements first, then a
/// temporary filled member by member.
pub fn transform_object_literal_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
) -> Expr {
    transform_object_literal_with(state, prereqs, node, &mut [], Vec::new())
}

/// A plugin's edit of one written field (rbxts-rs: `compile_object_literal`): the key emitted instead of the
/// written one, and the value used instead of compiling the initializer (which is then not evaluated).
#[derive(Clone, Debug, Default)]
pub struct FieldEdit {
    /// The key to emit instead of the written one.
    pub key: Option<Expr>,
    /// The value to use instead of the initializer.
    pub value: Option<Expr>,
}

/// `transformObjectLiteralExpression` with edits (rbxts-rs: `compile_object_literal`): `edits[i]` applies to
/// property `i` (none when `edits` is empty), and `append` adds fields after the written ones.
pub fn transform_object_literal_with(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    edits: &mut [FieldEdit],
    append: Vec<(Expr, Expr)>,
) -> Expr {
    let mut edit_of = |i: usize| edits.get_mut(i).map(std::mem::take).unwrap_or_default();
    // starts as a map, becomes a temporary identifier once `disable_map_inline` is called
    let mut ptr = create_map_pointer("object");
    for (i, &property) in node.properties().iter().enumerate() {
        validate_method_assignment(state, property);
        match property.kind() {
            Kind::PropertyAssignment => {
                let name = property.name().expect("a property assignment has a name");
                if name.kind() == Kind::PrivateIdentifier {
                    state.add_diagnostic(errors::no_private_identifier(name));
                    continue;
                }
                let initializer = property.initializer().expect("a property assignment has an initializer");
                transform_property_assignment(state, prereqs, &mut ptr, name, initializer, edit_of(i));
            }
            Kind::ShorthandPropertyAssignment => {
                let name = property.name().expect("a shorthand property assignment has a name");
                transform_property_assignment(state, prereqs, &mut ptr, name, name, edit_of(i));
            }
            Kind::SpreadAssignment => transform_spread_assignment(state, prereqs, &mut ptr, property),
            Kind::MethodDeclaration => {
                let statements = transform_method_declaration(state, prereqs, property, &mut ptr);
                prereqs.push_list(statements);
            }
            // must be an accessor declaration, which is banned
            _ => state.add_diagnostic(errors::no_getter_setter(property)),
        }
    }
    for (key, value) in append {
        assign_to_map_pointer(prereqs, &mut ptr, key, value);
    }
    ptr.value
}
