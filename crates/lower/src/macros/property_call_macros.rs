//! `macros/propertyCallMacros.ts`: methods of the compiler-types classes that compile to inline Luau.
//!
//! rbxts-rs: TypeScript builds the math and string macros with closures (`makeMathMethod`, `makeStringCallback`)
//! and wraps every macro in `wrapComments` when the module loads. Table entries here are plain functions, one per
//! operator or string function, and [`wrap_comments`] is applied where the macro is called, with the entry's name.

use rbxts_luau::{AnyIdentifier, AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, UnaryOperator, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::macros::types::PropertyCallMacro;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::offset::offset;
use crate::util::symbols::is_used_as_statement;
use crate::util::types::{is_definitely_type, is_number_type, is_possibly_undefined, is_string_type};

type Args = Vec<Expr>;

fn len(expression: &Expr) -> Expr {
    create::unary(UnaryOperator::Len, expression.clone())
}

fn index(expression: &Expr, index: Expr) -> Expr {
    create::computed_index(convert_to_indexable_expression(expression.clone()), index)
}

fn assign(left: Expr, right: Expr) -> Stmt {
    create::assignment(left, AssignmentOperator::Assign, right)
}

fn increment(left: Expr) -> Stmt {
    create::assignment(left, AssignmentOperator::Add, create::number(1.0))
}

fn if_then(condition: Expr, statements: Vec<Stmt>) -> Stmt {
    create::if_stmt(condition, statements, ElseBody::Block(Vec::new()))
}

fn ids(ids: &[&rbxts_luau::TemporaryIdentifier]) -> Vec<AnyIdentifier> {
    ids.iter().map(|&id| id.clone().into()).collect()
}

/// `luau.nil()` when the call's value is used, `luau.none()` otherwise.
fn nil_if_used(node: P<Node>) -> Expr {
    if !is_used_as_statement(node) { Expr::Nil } else { Expr::None }
}

// math classes (`makeMathMethod`)

macro_rules! math_methods {
    ($($name:ident = $operator:ident),* $(,)?) => {
        $(fn $name(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
            create::binary(expression, BinaryOperator::$operator, args.into_iter().next().expect("an operand"))
        })*
    };
}

math_methods!(math_add = Add, math_sub = Sub, math_mul = Mul, math_div = Div, math_idiv = FloorDiv);

// strings (`makeStringCallback`)

macro_rules! string_callbacks {
    ($($name:ident = $function:path),* $(,)?) => {
        $(fn $name(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
            let mut call_args = vec![expression];
            call_args.extend(args);
            create::call($function(), call_args)
        })*
    };
}

string_callbacks!(
    string_byte = globals::string::byte,
    string_find = globals::string::find,
    string_format = globals::string::format,
    string_gmatch = globals::string::gmatch,
    string_gsub = globals::string::gsub,
    string_lower = globals::string::lower,
    string_match = globals::string::r#match,
    string_rep = globals::string::rep,
    string_reverse = globals::string::reverse,
    string_split = globals::string::split,
    string_sub = globals::string::sub,
    string_upper = globals::string::upper,
);

/// `size` of strings and `ArrayLike`
fn length(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, _: Args) -> Expr {
    create::unary(UnaryOperator::Len, expression)
}

// ReadonlyArray

/// `makeEveryOrSomeMethod`, with the callback arguments `(value, key - 1, array)` both methods use.
fn every_or_some(prereqs: &mut Prereqs, expression: Expr, args: Args, initial_state: bool) -> Expr {
    let result_id = prereqs.push_to_var(create::bool(initial_state), "result");
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));

    let key_id = temp_id("k");
    let value_id = temp_id("v");

    let call_callback = create::call(
        callback_id,
        vec![value_id.clone().into(), offset(key_id.clone().into(), -1.0), expression.clone()],
    );
    prereqs.push(create::for_in(
        ids(&[&key_id, &value_id]),
        expression,
        vec![if_then(
            if initial_state { create::unary(UnaryOperator::Not, call_callback) } else { call_callback },
            vec![assign(result_id.clone(), create::bool(!initial_state)), create::break_stmt()],
        )],
    ));

    result_id
}

/// `argumentsWithDefaults`: a missing or `nil` argument takes the default; one that may be `nil` at run time is
/// checked.
fn arguments_with_defaults(state: &mut TransformState<'_>, node: P<Node>, mut args: Args, defaults: Vec<Expr>) -> Args {
    let node_arguments = node.arguments();
    for (i, default) in defaults.into_iter().enumerate() {
        let source = node_arguments.get(i).copied();
        match args.get(i) {
            None => args.push(default),
            Some(Expr::Nil) => args[i] = default,
            Some(argument) => {
                if !argument.is_simple_primitive()
                    && source.is_none_or(|source| {
                        source.kind() == Kind::SpreadElement || {
                            let t = state.get_type(source);
                            is_possibly_undefined(state, t)
                        }
                    })
                {
                    let argument = argument.clone();
                    args[i] = create::if_expr(
                        create::binary(argument.clone(), BinaryOperator::Eq, Expr::Nil),
                        default,
                        argument,
                    );
                }
            }
        }
    }
    args
}

fn array_is_empty(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, _: Args) -> Expr {
    create::binary(create::unary(UnaryOperator::Len, expression), BinaryOperator::Eq, create::number(0.0))
}

fn array_join(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    mut expression: Expr,
    args: Args,
) -> Expr {
    let args = arguments_with_defaults(state, node, args, vec![globals::strings::comma_space()]);
    let receiver = node.expression().and_then(|e| e.expression()).expect("a property call has a receiver");
    // rbxts-rs: fixes roblox-ts reading the receiver's type with `undefined` in it on an optional chain (`parts?.join()`
    // on `Array<boolean> | undefined`): the index type of that union is unknown, so the `tostring` loop was skipped and
    // `table.concat` failed on the values. The receiver is not nil where the macro runs.
    let receiver_type = state.get_type(receiver);
    let receiver_type = state.checker.get_non_nullable_type(receiver_type);
    // rbxts-rs: TypeScript 7's getIndexTypeOfType takes the key type instead of `IndexKind.Number`
    let number_type = state.checker.get_number_type();
    let index_type = state.checker.get_index_type_of_type_exported(receiver_type, number_type);

    // table.concat only works on string and number types, so call tostring() otherwise
    if let Some(index_type) = index_type
        && !is_definitely_type(state, index_type, &[&is_string_type, &is_number_type])
    {
        let id = prereqs.push_to_var(create::call(globals::table::create(), vec![len(&expression)]), "result");
        let key_id = temp_id("k");
        let value_id = temp_id("v");
        prereqs.push(create::for_in(
            ids(&[&key_id, &value_id]),
            expression,
            vec![assign(
                create::computed_index(id.clone(), key_id.into()),
                create::call(globals::tostring(), vec![value_id.into()]),
            )],
        ));

        expression = id;
    }

    let separator = args.into_iter().next().expect("join has a separator");
    create::call(globals::table::concat(), vec![expression, separator])
}

fn array_move(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let mut args = args.into_iter();
    let mut move_args = vec![expression];
    for _ in 0..3 {
        move_args.push(offset(args.next().expect("move has three indices"), 1.0));
    }
    if let Some(destination) = args.next() {
        move_args.push(destination);
    }
    create::call(globals::table::r#move(), move_args)
}

fn array_includes(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let mut args = args.into_iter();
    let mut call_args = vec![expression, args.next().expect("includes has a value")];
    if let Some(from_index) = args.next() {
        call_args.push(offset(from_index, 1.0));
    }
    create::binary(create::call(globals::table::find(), call_args), BinaryOperator::Ne, Expr::Nil)
}

fn array_index_of(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let mut args = args.into_iter();
    let mut find_args = vec![expression, args.next().expect("indexOf has a value")];
    if let Some(from_index) = args.next() {
        find_args.push(offset(from_index, 1.0));
    }
    offset(
        create::binary(create::call(globals::table::find(), find_args), BinaryOperator::Or, create::number(0.0)),
        -1.0,
    )
}

fn array_every(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    every_or_some(prereqs, expression, args, true)
}

fn array_some(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    every_or_some(prereqs, expression, args, false)
}

fn array_for_each(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let key_id = temp_id("k");
    let value_id = temp_id("v");
    prereqs.push(create::for_in(
        ids(&[&key_id, &value_id]),
        expression.clone(),
        vec![create::call_statement(create::call(
            callback_id,
            vec![value_id.into(), offset(key_id.into(), -1.0), expression],
        ))],
    ));

    nil_if_used(node)
}

fn array_map(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let new_value_id =
        prereqs.push_to_var(create::call(globals::table::create(), vec![len(&expression)]), "newValue");
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let key_id = temp_id("k");
    let value_id = temp_id("v");
    prereqs.push(create::for_in(
        ids(&[&key_id, &value_id]),
        expression.clone(),
        vec![assign(
            create::computed_index(new_value_id.clone(), key_id.clone().into()),
            create::call(callback_id, vec![value_id.into(), offset(key_id.into(), -1.0), expression]),
        )],
    ));

    new_value_id
}

fn array_map_filtered(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    _: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let new_value_id = prereqs.push_to_var(create::array(Vec::new()), "newValue");
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let length_id = prereqs.push_to_var(create::number(0.0), "length");
    let key_id = temp_id("k");
    let value_id = temp_id("v");
    let result_id = temp_id("result");
    prereqs.push(create::for_in(
        ids(&[&key_id, &value_id]),
        expression.clone(),
        vec![
            create::local(
                result_id.clone(),
                Some(create::call(callback_id, vec![value_id.into(), offset(key_id.into(), -1.0), expression])),
            ),
            if_then(
                create::binary(result_id.clone().into(), BinaryOperator::Ne, Expr::Nil),
                vec![
                    increment(length_id.clone()),
                    assign(create::computed_index(new_value_id.clone(), length_id), result_id.into()),
                ],
            ),
        ],
    ));

    new_value_id
}

fn array_filter_undefined(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    _: P<Node>,
    expression: Expr,
    _: Args,
) -> Expr {
    let length_id = prereqs.push_to_var(create::number(0.0), "length");
    let index_id1 = temp_id("i");
    prereqs.push(create::for_in(
        ids(&[&index_id1]),
        expression.clone(),
        vec![if_then(
            create::binary(index_id1.clone().into(), BinaryOperator::Gt, length_id.clone()),
            vec![assign(length_id.clone(), index_id1.into())],
        )],
    ));

    let result_id = prereqs.push_to_var(create::array(Vec::new()), "result");
    let result_length_id = prereqs.push_to_var(create::number(0.0), "resultLength");
    let index_id2 = temp_id("i");
    let value_id = temp_id("v");
    prereqs.push(create::numeric_for(
        index_id2.clone().into(),
        create::number(1.0),
        length_id,
        None,
        vec![
            create::local(value_id.clone(), Some(index(&expression, index_id2.into()))),
            if_then(
                create::binary(value_id.clone().into(), BinaryOperator::Ne, Expr::Nil),
                vec![
                    increment(result_length_id.clone()),
                    assign(create::computed_index(result_id.clone(), result_length_id), value_id.into()),
                ],
            ),
        ],
    ));

    result_id
}

fn array_filter(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let new_value_id = prereqs.push_to_var(create::array(Vec::new()), "newValue");
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let length_id = prereqs.push_to_var(create::number(0.0), "length");
    let key_id = temp_id("k");
    let value_id = temp_id("v");
    prereqs.push(create::for_in(
        ids(&[&key_id, &value_id]),
        expression.clone(),
        vec![if_then(
            create::binary(
                create::call(callback_id, vec![value_id.clone().into(), offset(key_id.into(), -1.0), expression]),
                BinaryOperator::Eq,
                Expr::True,
            ),
            vec![
                increment(length_id.clone()),
                assign(create::computed_index(new_value_id.clone(), length_id), value_id.into()),
            ],
        )],
    ));

    new_value_id
}

fn array_reduce(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let mut start = create::number(1.0);
    let end = len(&expression);

    let length_exp = len(&expression);

    let has_initial_value = args.len() >= 2;
    let mut args = args.into_iter();
    let callback = args.next().expect("a callback");
    let result_id;
    // if there was no initialValue supplied
    if !has_initial_value {
        prereqs.push(if_then(
            create::binary(length_exp, BinaryOperator::Eq, create::number(0.0)),
            vec![create::call_statement(create::call(
                globals::error(),
                vec![create::string(
                    "Attempted to call `ReadonlyArray.reduce()` on an empty array without an initialValue.",
                )],
            ))],
        ));
        result_id = prereqs.push_to_var(index(&expression, start.clone()), "result");
        start = offset(start, 1.0);
    } else {
        result_id = prereqs.push_to_var(args.next().expect("an initial value"), "result");
    }
    let callback_id = convert_to_indexable_expression(callback);

    let iterator_id = temp_id("i");
    prereqs.push(create::numeric_for(
        iterator_id.clone().into(),
        start,
        end,
        None,
        vec![assign(
            result_id.clone(),
            create::call(
                callback_id,
                vec![
                    result_id.clone(),
                    index(&expression, iterator_id.clone().into()),
                    offset(iterator_id.into(), -1.0),
                    expression,
                ],
            ),
        )],
    ));

    result_id
}

fn array_find(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let loop_id = temp_id("i");
    let value_id = temp_id("v");
    let result_id = prereqs.push_to_var_empty("result");

    prereqs.push(create::for_in(
        ids(&[&loop_id, &value_id]),
        expression.clone(),
        vec![if_then(
            create::binary(
                create::call(callback_id, vec![value_id.clone().into(), offset(loop_id.into(), -1.0), expression]),
                BinaryOperator::Eq,
                Expr::True,
            ),
            vec![assign(result_id.clone(), value_id.into()), create::break_stmt()],
        )],
    ));

    result_id
}

fn array_find_index(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    _: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let loop_id = temp_id("i");
    let value_id = temp_id("v");
    let result_id = prereqs.push_to_var(create::number(-1.0), "result");

    prereqs.push(create::for_in(
        ids(&[&loop_id, &value_id]),
        expression.clone(),
        vec![if_then(
            create::binary(
                create::call(callback_id, vec![value_id.into(), offset(loop_id.clone().into(), -1.0), expression]),
                BinaryOperator::Eq,
                Expr::True,
            ),
            vec![assign(result_id.clone(), offset(loop_id.into(), -1.0)), create::break_stmt()],
        )],
    ));

    result_id
}

// Array

fn array_push(_: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, expression: Expr, args: Args) -> Expr {
    // for `a.push()` always emit luau.unary so the call doesn't disappear in emit
    if args.is_empty() {
        return len(&expression);
    }

    for arg in args {
        prereqs.push(create::call_statement(create::call(globals::table::insert(), vec![expression.clone(), arg])));
    }

    if !is_used_as_statement(node) { len(&expression) } else { Expr::None }
}

fn array_pop(_: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, expression: Expr, _: Args) -> Expr {
    let mut length_exp = len(&expression);

    let return_value_is_used = !is_used_as_statement(node);
    let mut ret_value = Expr::None;
    if return_value_is_used {
        length_exp = prereqs.push_to_var(length_exp, "length");
        ret_value = prereqs.push_to_var(index(&expression, length_exp.clone()), "result");
    }

    prereqs.push(assign(index(&expression, length_exp), Expr::Nil));

    ret_value
}

fn array_shift(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, _: Args) -> Expr {
    create::call(globals::table::remove(), vec![expression, create::number(1.0)])
}

fn array_unshift(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    for arg in args.into_iter().rev() {
        prereqs.push(create::call_statement(create::call(
            globals::table::insert(),
            vec![expression.clone(), create::number(1.0), arg],
        )));
    }

    if !is_used_as_statement(node) { len(&expression) } else { Expr::None }
}

fn array_insert(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let mut args = args.into_iter();
    let index = offset(args.next().expect("insert has an index"), 1.0);
    let value = args.next().expect("insert has a value");
    create::call(globals::table::insert(), vec![expression, index, value])
}

fn array_remove(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let index = offset(args.into_iter().next().expect("remove has an index"), 1.0);
    create::call(globals::table::remove(), vec![expression, index])
}

fn array_unordered_remove(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let index_exp =
        prereqs.push_to_var_if_complex(offset(args.into_iter().next().expect("an index"), 1.0), "index");

    let length_id = prereqs.push_to_var(len(&expression), "length");

    let value_is_used = !is_used_as_statement(node);
    let value_id = prereqs.push_to_var(index(&expression, index_exp.clone()), "value");

    prereqs.push(if_then(
        create::binary(value_id.clone(), BinaryOperator::Ne, Expr::Nil),
        vec![
            assign(index(&expression, index_exp), index(&expression, length_id.clone())),
            assign(index(&expression, length_id), Expr::Nil),
        ],
    ));

    if value_is_used { value_id } else { Expr::None }
}

fn array_sort(_: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, expression: Expr, args: Args) -> Expr {
    let value_is_used = !is_used_as_statement(node);

    let mut sort_args = vec![expression.clone()];
    sort_args.extend(args);

    prereqs.push(create::call_statement(create::call(globals::table::sort(), sort_args)));

    if value_is_used { expression } else { Expr::None }
}

/// `clear` of arrays, sets and maps
fn clear(_: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, expression: Expr, _: Args) -> Expr {
    prereqs.push(create::call_statement(create::call(globals::table::clear(), vec![expression])));
    nil_if_used(node)
}

// ReadonlySet and ReadonlyMap

fn set_map_is_empty(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, _: Args) -> Expr {
    create::binary(create::call(globals::next(), vec![expression]), BinaryOperator::Eq, Expr::Nil)
}

fn set_map_size(_: &mut TransformState<'_>, prereqs: &mut Prereqs, _: P<Node>, expression: Expr, _: Args) -> Expr {
    let size_id = prereqs.push_to_var(create::number(0.0), "size");
    prereqs.push(create::for_in(vec![temp_id("").into()], expression, vec![increment(size_id.clone())]));
    size_id
}

fn set_map_has(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    let left = index(&expression, args.into_iter().next().expect("has has a key"));
    create::binary(left, BinaryOperator::Ne, Expr::Nil)
}

// Set and Map

fn set_map_delete(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let arg = args.into_iter().next().expect("delete has a key");
    let value_is_used = !is_used_as_statement(node);
    let mut value_existed_id = Expr::None;
    if value_is_used {
        value_existed_id = prereqs.push_to_var(
            create::binary(index(&expression, arg.clone()), BinaryOperator::Ne, Expr::Nil),
            "valueExisted",
        );
    }

    prereqs.push(assign(index(&expression, arg), Expr::Nil));

    value_existed_id
}

fn set_for_each(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let value_id = temp_id("v");
    prereqs.push(create::for_in(
        ids(&[&value_id]),
        expression.clone(),
        vec![create::call_statement(create::call(
            callback_id,
            vec![value_id.clone().into(), value_id.into(), expression],
        ))],
    ));

    nil_if_used(node)
}

fn set_add(_: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, expression: Expr, args: Args) -> Expr {
    let value_is_used = !is_used_as_statement(node);
    prereqs.push(assign(index(&expression, args.into_iter().next().expect("add has a value")), Expr::True));
    if value_is_used { expression } else { Expr::None }
}

fn map_for_each(
    _: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    args: Args,
) -> Expr {
    let callback_id = convert_to_indexable_expression(args.into_iter().next().expect("a callback"));
    let key_id = temp_id("k");
    let value_id = temp_id("v");
    prereqs.push(create::for_in(
        ids(&[&key_id, &value_id]),
        expression.clone(),
        vec![create::call_statement(create::call(callback_id, vec![value_id.into(), key_id.into(), expression]))],
    ));

    nil_if_used(node)
}

fn map_get(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    index(&expression, args.into_iter().next().expect("get has a key"))
}

fn map_set(_: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, expression: Expr, args: Args) -> Expr {
    let mut args = args.into_iter();
    let key_exp = args.next().expect("set has a key");
    let value_exp = args.next().expect("set has a value");
    let value_is_used = !is_used_as_statement(node);
    prereqs.push(assign(index(&expression, key_exp), value_exp));
    if value_is_used { expression } else { Expr::None }
}

// Promise

fn promise_then(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, expression: Expr, args: Args) -> Expr {
    create::method_call(convert_to_indexable_expression(expression), "andThen", args)
}

type Methods = &'static [(&'static str, PropertyCallMacro)];

const MATH_ADD_SUB_MUL: Methods = &[("add", math_add), ("sub", math_sub), ("mul", math_mul)];
const MATH_ADD_SUB: Methods = &[("add", math_add), ("sub", math_sub)];
const MATH_ALL: Methods = &[
    ("add", math_add),
    ("sub", math_sub),
    ("mul", math_mul),
    ("div", math_div),
    ("idiv", math_idiv),
];
const MATH_NO_IDIV: Methods =
    &[("add", math_add), ("sub", math_sub), ("mul", math_mul), ("div", math_div)];

/// `PROPERTY_CALL_MACROS`, by interface, in roblox-ts's order.
pub const PROPERTY_CALL_MACROS: &[(&str, Methods)] = &[
    // math classes
    ("CFrame", MATH_ADD_SUB_MUL),
    ("UDim", MATH_ADD_SUB),
    ("UDim2", MATH_ADD_SUB),
    ("Vector2", MATH_ALL),
    ("Vector2int16", MATH_NO_IDIV),
    ("Vector3", MATH_ALL),
    ("Vector3int16", MATH_NO_IDIV),
    ("vector", MATH_ALL),
    ("Number", &[("idiv", math_idiv)]),
    (
        "String",
        &[
            ("size", length),
            ("byte", string_byte),
            ("find", string_find),
            ("format", string_format),
            ("gmatch", string_gmatch),
            ("gsub", string_gsub),
            ("lower", string_lower),
            ("match", string_match),
            ("rep", string_rep),
            ("reverse", string_reverse),
            ("split", string_split),
            ("sub", string_sub),
            ("upper", string_upper),
        ],
    ),
    ("ArrayLike", &[("size", length)]),
    (
        "ReadonlyArray",
        &[
            ("isEmpty", array_is_empty),
            ("join", array_join),
            ("move", array_move),
            ("includes", array_includes),
            ("indexOf", array_index_of),
            ("every", array_every),
            ("some", array_some),
            ("forEach", array_for_each),
            ("map", array_map),
            ("mapFiltered", array_map_filtered),
            ("filterUndefined", array_filter_undefined),
            ("filter", array_filter),
            ("reduce", array_reduce),
            ("find", array_find),
            ("findIndex", array_find_index),
        ],
    ),
    (
        "Array",
        &[
            ("push", array_push),
            ("pop", array_pop),
            ("shift", array_shift),
            ("unshift", array_unshift),
            ("insert", array_insert),
            ("remove", array_remove),
            ("unorderedRemove", array_unordered_remove),
            ("sort", array_sort),
            ("clear", clear),
        ],
    ),
    (
        "ReadonlySet",
        &[
            ("isEmpty", set_map_is_empty),
            ("size", set_map_size),
            ("has", set_map_has),
            ("forEach", set_for_each),
        ],
    ),
    ("Set", &[("delete", set_map_delete), ("clear", clear), ("add", set_add)]),
    (
        "ReadonlyMap",
        &[
            ("isEmpty", set_map_is_empty),
            ("size", set_map_size),
            ("has", set_map_has),
            ("forEach", map_for_each),
            ("get", map_get),
        ],
    ),
    ("Map", &[("delete", set_map_delete), ("clear", clear), ("set", map_set)]),
    ("Promise", &[("then", promise_then)]),
];

// comment logic

fn header(text: &str) -> Stmt {
    create::comment(format!(" ▼ {text} ▼"))
}

fn footer(text: &str) -> Stmt {
    create::comment(format!(" ▲ {text} ▲"))
}

/// `wrapComments`: a macro expansion of more than one statement is framed by comments naming the method
/// (`Class.method`).
pub fn wrap_comments(
    method_name: &str,
    callback: PropertyCallMacro,
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    call_node: P<Node>,
    call_exp: Expr,
    args: Args,
) -> Expr {
    let mut expression_prereqs = Prereqs::new();
    let expression = callback(state, &mut expression_prereqs, call_node, call_exp, args);
    if expression_prereqs.statements.len() > 1 {
        expression_prereqs.unshift(header(method_name));
        expression_prereqs.push(footer(method_name));
    }
    prereqs.push_list(expression_prereqs.statements);
    expression
}
