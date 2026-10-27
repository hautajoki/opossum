//! `macros/callMacros.ts`

use rbxts_luau::{BinaryOperator, Expr, create, globals};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::macros::types::CallMacro;
use crate::shared::diagnostics::errors;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::create_import_expression::get_import_parts;
use crate::util::create_truthiness_checks::create_truthiness_checks;

const PRIMITIVE_LUAU_TYPES: &[&str] =
    &["nil", "boolean", "string", "number", "table", "userdata", "function", "thread", "vector", "buffer"];

fn assert(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    _expression: Expr,
    mut args: Vec<Expr>,
) -> Expr {
    let value = std::mem::replace(&mut args[0], Expr::None);
    args[0] = create_truthiness_checks(state, prereqs, value, node.arguments()[0]);
    create::call(globals::assert(), args)
}

fn type_of(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, _: Expr, args: Vec<Expr>) -> Expr {
    create::call(globals::r#typeof(), args)
}

fn type_is(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, _: Expr, args: Vec<Expr>) -> Expr {
    let mut args = args.into_iter();
    let value = args.next().expect("typeIs has a value");
    let type_str = args.next().expect("typeIs has a type name");
    let type_func = match &type_str {
        Expr::String(s) if PRIMITIVE_LUAU_TYPES.contains(&s.value.as_str()) => globals::r#type(),
        _ => globals::r#typeof(),
    };
    create::binary(create::call(type_func, vec![value]), BinaryOperator::Eq, type_str)
}

fn class_is(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, _: Expr, args: Vec<Expr>) -> Expr {
    let mut args = args.into_iter();
    let value = args.next().expect("classIs has a value");
    let type_str = args.next().expect("classIs has a class name");
    create::binary(create::property(convert_to_indexable_expression(value), "ClassName"), BinaryOperator::Eq, type_str)
}

fn identity(_: &mut TransformState<'_>, _: &mut Prereqs, _: P<Node>, _: Expr, args: Vec<Expr>) -> Expr {
    args.into_iter().next().expect("identity has a value")
}

/// `$range` is only a macro as the expression of a for-of loop, which `transformForOfStatement` recognises by symbol.
fn range(state: &mut TransformState<'_>, _: &mut Prereqs, node: P<Node>, _: Expr, _: Vec<Expr>) -> Expr {
    state.add_diagnostic(errors::no_range_macro_outside_for_of(node.expression().expect("a call has a callee")));
    Expr::None
}

/// `$tuple` is only a macro as the expression of a return statement, which `transformReturnStatement` recognises by
/// symbol.
fn tuple(state: &mut TransformState<'_>, _: &mut Prereqs, node: P<Node>, _: Expr, _: Vec<Expr>) -> Expr {
    state.add_diagnostic(errors::no_tuple_macro_outside_return(node));
    Expr::None
}

fn get_module_tree(state: &mut TransformState<'_>, _: &mut Prereqs, node: P<Node>, _: Expr, _: Vec<Expr>) -> Expr {
    let source_file = tsrs_ast::get_source_file_of_node(node).expect("a call is in a source file");
    let mut parts = get_import_parts(state, source_file, node.arguments()[0]);
    // converts the flat array into { root, { "rest", "of", "path" } }
    let root = parts.remove(0);
    create::array(vec![root, create::array(parts)])
}

/// `CALL_MACROS`
pub const CALL_MACROS: &[(&str, CallMacro)] = &[
    ("assert", assert),
    ("typeOf", type_of),
    ("typeIs", type_is),
    ("classIs", class_is),
    ("identity", identity),
    ("$range", range),
    ("$tuple", tuple),
    ("$getModuleTree", get_module_tree),
];
