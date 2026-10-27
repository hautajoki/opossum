//! `util/getAddIterableToArrayBuilder.ts`: the statements that append every value of an iterable to an array being
//! built (array spread, call-argument spread, and anything else that collects an iterable into an array).

use rbxts_luau::{
    AnyIdentifier, AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, UnaryOperator, create, globals,
};
use tsrs_ast::Node;
use tsrs_checker::{Type, TypeFlags};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::types::{
    is_array_type, is_definitely_type, is_generator_type, is_iterable_function_lua_tuple_type,
    is_iterable_function_type, is_iterable_type, is_map_type, is_set_type, is_shared_table_type, is_string_type,
};
use crate::util::value_to_id_str::value_to_id_str;

/// `AddIterableToArrayBuilder`: how an iterable of a given type is appended to an array. Pick one with
/// [`get_add_iterable_to_array_builder`], then [`AddIterableToArrayBuilder::build`] the statements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddIterableToArrayBuilder {
    /// `addArray`: `table.move`.
    Array,
    /// `addString`: one element per UTF-8 character.
    String,
    /// `addSet`: the set's keys.
    Set,
    /// `addMap`: `{ key, value }` pairs (maps and shared tables).
    Map,
    /// `addIterableFunctionLuaTuple`: each call's results as an array, until the first is `nil`.
    IterableFunctionLuaTuple,
    /// `addIterableFunction`: each value the function returns.
    IterableFunction,
    /// `addGenerator`: each value until the generator is done.
    Generator,
    /// A type that cannot be iterated (already reported): no statements.
    Unsupported,
}

impl AddIterableToArrayBuilder {
    /// Appends the values of `expression` to `array_id`, whose length is `length_id` plus `amt_elements_since_update`
    /// (elements written since `length_id` was last updated). With `should_update_length_id`, `length_id` is the
    /// array's length afterwards; the loop builders always keep it up to date.
    pub fn build(
        self,
        prereqs: &mut Prereqs,
        expression: Expr,
        array_id: &AnyIdentifier,
        length_id: &AnyIdentifier,
        amt_elements_since_update: u32,
        should_update_length_id: bool,
    ) -> Vec<Stmt> {
        let array_id: Expr = array_id.clone().into();
        let length_id: Expr = length_id.clone().into();
        match self {
            Self::Array => {
                add_array(prereqs, expression, array_id, length_id, amt_elements_since_update, should_update_length_id)
            }
            Self::String => {
                let expression =
                    create::call(globals::string::gmatch(), vec![expression, globals::utf8::charpattern()]);
                add_for_loop(expression, "char", array_id, length_id, amt_elements_since_update)
            }
            Self::Set => add_for_loop(expression, "v", array_id, length_id, amt_elements_since_update),
            Self::IterableFunction => {
                add_for_loop(expression, "result", array_id, length_id, amt_elements_since_update)
            }
            Self::Map => add_map(expression, array_id, length_id, amt_elements_since_update),
            Self::IterableFunctionLuaTuple => {
                add_iterable_function_lua_tuple(prereqs, expression, array_id, length_id, amt_elements_since_update)
            }
            Self::Generator => add_generator(expression, array_id, length_id, amt_elements_since_update),
            Self::Unsupported => Vec::new(),
        }
    }
}

/// `length += 1; array[length] = value`
fn append_statements(array_id: &Expr, length_id: &Expr, value: Expr) -> [Stmt; 2] {
    [
        create::assignment(length_id.clone(), AssignmentOperator::Add, create::number(1.0)),
        create::assignment(
            create::computed_index(array_id.clone(), length_id.clone()),
            AssignmentOperator::Assign,
            value,
        ),
    ]
}

/// `length += amtElementsSinceUpdate`, when there are any.
fn catch_up_length(result: &mut Vec<Stmt>, length_id: &Expr, amt_elements_since_update: u32) {
    if amt_elements_since_update > 0 {
        result.push(create::assignment(
            length_id.clone(),
            AssignmentOperator::Add,
            create::number(f64::from(amt_elements_since_update)),
        ));
    }
}

/// `addArray`
fn add_array(
    prereqs: &mut Prereqs,
    expression: Expr,
    array_id: Expr,
    length_id: Expr,
    amt_elements_since_update: u32,
    should_update_length_id: bool,
) -> Vec<Stmt> {
    let mut result = Vec::new();

    let input_array = prereqs.push_to_var_if_non_id(expression, "array");
    let mut input_length = create::unary(UnaryOperator::Len, input_array.clone());
    if should_update_length_id {
        input_length = prereqs.push_to_var(input_length, &(value_to_id_str(&input_array) + "Length"));
    }

    result.push(create::call_statement(create::call(
        globals::table::r#move(),
        vec![
            input_array,
            create::number(1.0),
            input_length.clone(),
            create::binary(
                length_id.clone(),
                BinaryOperator::Add,
                create::number(f64::from(amt_elements_since_update + 1)),
            ),
            array_id,
        ],
    )));

    if should_update_length_id {
        result.push(create::assignment(length_id, AssignmentOperator::Add, input_length));
    }

    result
}

/// `createForLoopArrayBuilder(valueName, getLoopExpression)`: `for _value in expression do … end`.
fn add_for_loop(
    loop_expression: Expr,
    value_name: &str,
    array_id: Expr,
    length_id: Expr,
    amt_elements_since_update: u32,
) -> Vec<Stmt> {
    let mut result = Vec::new();
    catch_up_length(&mut result, &length_id, amt_elements_since_update);

    let value_id = temp_id(value_name);
    let statements = append_statements(&array_id, &length_id, Expr::TemporaryIdentifier(value_id.clone()));
    result.push(create::for_in(vec![AnyIdentifier::Temporary(value_id)], loop_expression, statements.into()));

    result
}

/// `addMap`
fn add_map(expression: Expr, array_id: Expr, length_id: Expr, amt_elements_since_update: u32) -> Vec<Stmt> {
    let mut result = Vec::new();
    catch_up_length(&mut result, &length_id, amt_elements_since_update);

    let key_id = temp_id("k");
    let value_id = temp_id("v");
    let pair =
        create::array(vec![Expr::TemporaryIdentifier(key_id.clone()), Expr::TemporaryIdentifier(value_id.clone())]);
    let statements = append_statements(&array_id, &length_id, pair);
    result.push(create::for_in(
        vec![AnyIdentifier::Temporary(key_id), AnyIdentifier::Temporary(value_id)],
        expression,
        statements.into(),
    ));

    result
}

/// `addIterableFunctionLuaTuple`
fn add_iterable_function_lua_tuple(
    prereqs: &mut Prereqs,
    expression: Expr,
    array_id: Expr,
    length_id: Expr,
    amt_elements_since_update: u32,
) -> Vec<Stmt> {
    let mut result = Vec::new();
    catch_up_length(&mut result, &length_id, amt_elements_since_update);

    let iter_func_id = prereqs.push_to_var(expression, "iterFunc");
    let value_id = temp_id("results");
    let value = Expr::TemporaryIdentifier(value_id.clone());
    let mut statements = vec![
        create::local(value_id, Some(create::array(vec![create::call(iter_func_id, Vec::new())]))),
        // later return values do not keep an iterator alive after its first return is nil
        create::if_stmt(
            create::binary(
                create::computed_index(value.clone(), create::number(1.0)),
                BinaryOperator::Eq,
                create::nil(),
            ),
            vec![create::break_stmt()],
            ElseBody::default(),
        ),
    ];
    statements.extend(append_statements(&array_id, &length_id, value));
    result.push(create::while_stmt(create::bool(true), statements));

    result
}

/// `addGenerator`
fn add_generator(expression: Expr, array_id: Expr, length_id: Expr, amt_elements_since_update: u32) -> Vec<Stmt> {
    let mut result = Vec::new();
    catch_up_length(&mut result, &length_id, amt_elements_since_update);

    let iter_id = temp_id("result");
    let iter = Expr::TemporaryIdentifier(iter_id.clone());
    let mut statements =
        vec![create::if_stmt(create::property(iter.clone(), "done"), vec![create::break_stmt()], ElseBody::default())];
    statements.extend(append_statements(&array_id, &length_id, create::property(iter, "value")));
    result.push(create::for_in(
        vec![AnyIdentifier::Temporary(iter_id)],
        create::property(convert_to_indexable_expression(expression), "next"),
        statements,
    ));

    result
}

/// `getAddIterableToArrayBuilder`: the builder for an iterable of type `t`; reports types that cannot be iterated at
/// `node` (and returns [`AddIterableToArrayBuilder::Unsupported`]).
pub fn get_add_iterable_to_array_builder(
    state: &mut TransformState<'_>,
    node: P<Node>,
    t: P<Type>,
) -> AddIterableToArrayBuilder {
    if is_definitely_type(state, t, &[&is_array_type]) {
        AddIterableToArrayBuilder::Array
    } else if is_definitely_type(state, t, &[&is_string_type]) {
        AddIterableToArrayBuilder::String
    } else if is_definitely_type(state, t, &[&is_set_type]) {
        AddIterableToArrayBuilder::Set
    } else if is_definitely_type(state, t, &[&is_map_type]) || is_definitely_type(state, t, &[&is_shared_table_type]) {
        AddIterableToArrayBuilder::Map
    } else if is_definitely_type(state, t, &[&is_iterable_function_lua_tuple_type]) {
        AddIterableToArrayBuilder::IterableFunctionLuaTuple
    } else if is_definitely_type(state, t, &[&is_iterable_function_type]) {
        AddIterableToArrayBuilder::IterableFunction
    } else if is_definitely_type(state, t, &[&is_generator_type]) {
        AddIterableToArrayBuilder::Generator
    } else if is_definitely_type(state, t, &[&is_iterable_type]) {
        state.add_diagnostic(errors::no_iterable_iteration(node));
        AddIterableToArrayBuilder::Unsupported
    } else if t.flags().intersects(TypeFlags::Union) {
        state.add_diagnostic(errors::no_macro_union(node));
        AddIterableToArrayBuilder::Unsupported
    } else {
        state.add_diagnostic(errors::no_unsupported_iteration(node));
        AddIterableToArrayBuilder::Unsupported
    }
}
