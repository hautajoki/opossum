//! `macros/constructorMacros.ts`

use rbxts_luau::{AssignmentOperator, Expr, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::macros::types::ConstructorMacro;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::util::ensure_transform_order::ensure_transform_order;

fn wrap_weak(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>, r#macro: ConstructorMacro) -> Expr {
    create::call(
        globals::setmetatable(),
        vec![r#macro(state, prereqs, node), create::map([(globals::strings::__mode(), globals::strings::k())])],
    )
}

fn array_constructor(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let arguments = node.arguments();
    if !arguments.is_empty() {
        let args = ensure_transform_order(state, prereqs, arguments);
        return create::call(globals::table::create(), args);
    }
    create::array(Vec::new())
}

fn set_constructor(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let Some(&arg) = node.arguments().first() else {
        return create::set(Vec::new());
    };
    // spreads cause prereq array, which cannot be optimised like this
    if arg.kind() == Kind::ArrayLiteralExpression && !arg.elements().iter().any(|e| e.kind() == Kind::SpreadElement) {
        create::set(ensure_transform_order(state, prereqs, arg.elements()))
    } else {
        let id = prereqs.push_to_var(create::set(Vec::new()), "set");
        let value_id = temp_id("v");
        let ids = vec![temp_id("").into(), value_id.clone().into()];
        let expression = transform_expression(state, prereqs, arg);
        prereqs.push(create::for_in(
            ids,
            expression,
            vec![create::assignment(
                create::computed_index(id.clone(), value_id.into()),
                AssignmentOperator::Assign,
                create::bool(true),
            )],
        ));
        id
    }
}

fn map_constructor(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    let Some(&arg) = node.arguments().first() else {
        return create::map([]);
    };
    let transformed = transform_expression(state, prereqs, arg);
    match transformed {
        Expr::Array(members) if members.iter().all(|m| matches!(m, Expr::Array(_))) => {
            create::map(members.into_iter().map(|e| {
                // the map constructor's typing gives every entry two members
                let Expr::Array(entry) = e else { unreachable!() };
                let mut entry = entry.into_iter();
                let key = entry.next().expect("a map entry has a key");
                let value = entry.next().expect("a map entry has a value");
                (key, value)
            }))
        }
        transformed => {
            let id = prereqs.push_to_var(create::map([]), "map");
            let value_id = temp_id("v");
            let ids = vec![temp_id("").into(), value_id.clone().into()];
            let value: Expr = value_id.into();
            prereqs.push(create::for_in(
                ids,
                transformed,
                vec![create::assignment(
                    create::computed_index(id.clone(), create::computed_index(value.clone(), create::number(1.0))),
                    AssignmentOperator::Assign,
                    create::computed_index(value, create::number(2.0)),
                )],
            ));
            id
        }
    }
}

fn weak_set_constructor(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    wrap_weak(state, prereqs, node, set_constructor)
}

fn weak_map_constructor(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    wrap_weak(state, prereqs, node, map_constructor)
}

/// `CONSTRUCTOR_MACROS`, by constructor interface.
pub const CONSTRUCTOR_MACROS: &[(&str, ConstructorMacro)] = &[
    ("ArrayConstructor", array_constructor),
    ("SetConstructor", set_constructor),
    ("MapConstructor", map_constructor),
    ("WeakSetConstructor", weak_set_constructor),
    ("WeakMapConstructor", weak_map_constructor),
    ("ReadonlyMapConstructor", map_constructor),
    ("ReadonlySetConstructor", set_constructor),
];
