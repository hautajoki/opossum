//! `nodes/expressions/transformCallExpression.ts`

use rbxts_luau::{Expr, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_import_expression::transform_import_expression;
use crate::nodes::transform_macro_call::{MacroKind, transform_macro_call};
use crate::nodes::transform_optional_chain::transform_optional_chain;
use crate::plugins::backend::{keyed_access, property_key_of};
use crate::util::add_one_if_array_type::add_one_if_array_type;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::builtins::{is_stable_builtin_member, try_mark_builtin_member, try_mark_pure_constructor};
use crate::util::evaluation::effects::{
    effects_commute, get_effects, get_effects_of_statements, is_late_read, join_effects,
};
use crate::util::is_method::{is_method, is_method_from_type};
use crate::util::types::{get_first_defined_symbol, is_possibly_type, is_possibly_undefined, is_roblox_type};
use crate::util::validate_not_any::validate_not_any_type;
use crate::util::wrap_return_if_lua_tuple::wrap_return_if_lua_tuple;
use rbxts_plugin::{AccessPosition, text_literal};

/// Some C functions like `tonumber()` error when an argument is a call that returns nothing. Possibly-undefined call
/// arguments of Roblox functions are wrapped in `()` to coerce the values to `nil`.
pub(crate) fn fix_void_arguments_for_roblox_functions(
    state: &mut TransformState<'_>,
    t: P<Type>,
    args: &mut [Expr],
    node_arguments: &[P<Node>],
) {
    if is_possibly_type(state, t, &[&is_roblox_type]) {
        for (arg, &node_arg) in args.iter_mut().zip(node_arguments) {
            if node_arg.kind() == Kind::CallExpression {
                let arg_type = state.get_type(node_arg);
                if is_possibly_undefined(state, arg_type) {
                    *arg = create::paren(std::mem::replace(arg, Expr::None));
                }
            }
        }
    }
}

/// `transformCallExpressionInner`: `expression(args)` once the callee is transformed.
pub fn transform_call_expression_inner(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    mut expression: Expr,
    node_arguments: &[P<Node>],
) -> Expr {
    if tsrs_ast::is_import_call(node) {
        return transform_import_expression(state, node);
    }

    let callee = node.expression().expect("a call has a callee");
    // a in a()
    validate_not_any_type(state, callee);

    if callee.kind() == Kind::SuperKeyword {
        let mut args = vec![globals::self_()];
        args.extend(ensure_transform_order(state, prereqs, node.arguments()));
        return create::call(create::property(convert_to_indexable_expression(expression), "constructor"), args);
    }

    let callee_type = state.get_type(callee);
    let exp_type = state.checker.get_non_optional_type(callee_type);
    if let Some(symbol) = get_first_defined_symbol(state, exp_type)
        && let Some(r#macro) = state.macro_manager.get_call_macro(state.checker, symbol)
    {
        return transform_macro_call(MacroKind::Call(r#macro), state, prereqs, node, expression, node_arguments);
    }

    let mut args_prereqs = Prereqs::new();
    let mut args = ensure_transform_order(state, &mut args_prereqs, node_arguments);
    fix_void_arguments_for_roblox_functions(state, exp_type, &mut args, node_arguments);

    if !effects_commute(&get_effects(&expression), &get_effects_of_statements(&args_prereqs.statements)) {
        expression = prereqs.push_to_var(expression, "fn");
    }
    prereqs.push_list(args_prereqs.statements);

    if is_method_from_type(state, callee, exp_type) {
        // direct calls have no receiver, but the implementation still expects its slot
        args.insert(0, Expr::Nil);
    }

    let exp = create::call(convert_to_indexable_expression(expression), args);
    wrap_return_if_lua_tuple(state, node, exp)
}

/// The key of an ordered property call: a name, or a computed index.
pub enum PropertyKey {
    /// A method name (`a.name(…)`).
    Name(String),
    /// A computed index (`a[index](…)`), already transformed.
    Index(Expr),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_ordered_property_call(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    source: P<Node>,
    mut base: Expr,
    key: PropertyKey,
    args: Vec<Expr>,
    prereq_statements: Vec<rbxts_luau::Stmt>,
    method: bool,
) -> Expr {
    let property = |state: &mut TransformState<'_>, base: &Expr| {
        let mut expression = match &key {
            PropertyKey::Name(name) => create::property(convert_to_indexable_expression(base.clone()), name.clone()),
            PropertyKey::Index(index) => {
                create::computed_index(convert_to_indexable_expression(base.clone()), index.clone())
            }
        };
        try_mark_builtin_member(state, source, &mut expression);
        expression
    };
    let prereq_effects = get_effects_of_statements(&prereq_statements);
    let name: Option<String> = match &key {
        PropertyKey::Name(name) => Some(name.clone()),
        PropertyKey::Index(Expr::String(s)) => Some(s.value.clone()),
        PropertyKey::Index(_) => None,
    };
    let stable_method = method && is_stable_builtin_member(state, source);
    let mut callee = property(state, &base);
    let lookup_effects = get_effects(&callee);
    let capture_callee = !effects_commute(&lookup_effects, &prereq_effects);
    if method
        && let Some(name) = name.as_deref()
        && rbxts_luau::is_valid_identifier(name)
        && (stable_method || (!capture_callee && matches!(key, PropertyKey::Name(_))))
    {
        // a stable method lookup can stay inline even when its receiver needs a snapshot
        if stable_method && !effects_commute(&get_effects(&base), &prereq_effects) {
            base = prereqs.push_to_var(base, "self");
        }
        prereqs.push_list(prereq_statements);
        return create::method_call(convert_to_indexable_expression(base), name, args);
    }
    // an explicit self argument must denote the receiver from before the lookup
    if method
        && (!base.is_simple() || !effects_commute(&get_effects(&base), &join_effects(lookup_effects, &prereq_effects)))
    {
        base = prereqs.push_to_var(base, "self");
        callee = property(state, &base);
    }
    if capture_callee {
        callee = prereqs.push_to_var(callee, "fn");
    }
    prereqs.push_list(prereq_statements);
    let args = if method {
        let mut with_self = vec![base];
        with_self.extend(args);
        with_self
    } else {
        args
    };
    create::call(callee, args)
}

/// `transformPropertyCallExpressionInner`: `a.b(args)` once `a` is transformed.
pub fn transform_property_call_expression_inner(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: P<Node>,
    base_expression: Expr,
    name: &str,
    node_arguments: &[P<Node>],
) -> Expr {
    let callee = node.expression().expect("a call has a callee");
    // a in a.b()
    validate_not_any_type(state, expression.as_property_access_expression().expression);
    // a.b in a.b()
    validate_not_any_type(state, callee);

    if tsrs_ast::is_super_property(expression) {
        let callee_name = expression.as_property_access_expression().name.text();
        // rbxts-rs: a plugin's `property_key` (DESIGN §11.5)
        let callee_exp = match crate::plugins::dispatch::rewritten_key(state, expression, AccessPosition::Callee) {
            Some(key) => keyed_access(base_expression, &key),
            None => create::property(convert_to_indexable_expression(base_expression), callee_name),
        };
        let args = ensure_transform_order(state, prereqs, node.arguments());
        let args = if is_method(state, expression) {
            let mut with_self = vec![globals::self_()];
            with_self.extend(args);
            with_self
        } else {
            args
        };
        return create::call(callee_exp, args);
    }

    let callee_type = state.get_type(callee);
    let exp_type = state.checker.get_non_optional_type(callee_type);
    if let Some(symbol) = get_first_defined_symbol(state, exp_type)
        && let Some(r#macro) = state.macro_manager.get_property_call_macro(state.checker, symbol)
    {
        return transform_macro_call(
            MacroKind::PropertyCall(r#macro),
            state,
            prereqs,
            node,
            base_expression,
            node_arguments,
        );
    }

    let mut args_prereqs = Prereqs::new();
    let mut args = ensure_transform_order(state, &mut args_prereqs, node_arguments);
    fix_void_arguments_for_roblox_functions(state, exp_type, &mut args, node_arguments);

    let method = is_method(state, expression);
    // rbxts-rs: a plugin's `property_key` (DESIGN §11.5)
    let key = match crate::plugins::dispatch::rewritten_key(state, expression, AccessPosition::Callee) {
        Some(key) => property_key_of(&key),
        None => PropertyKey::Name(name.to_owned()),
    };
    let mut exp = create_ordered_property_call(
        state,
        prereqs,
        expression,
        base_expression,
        key,
        args,
        args_prereqs.statements,
        method,
    );
    // rbxts-rs: `UDim2.fromScale(…)` and the like, for the JSX ordering (`try_mark_pure_constructor`)
    let access = expression.as_property_access_expression();
    try_mark_pure_constructor(state, node, access.expression, Some(access.name), node_arguments, &mut exp);
    wrap_return_if_lua_tuple(state, node, exp)
}

/// `transformElementCallExpressionInner`: `a[b](args)` once `a` is transformed.
#[allow(clippy::too_many_arguments)]
pub fn transform_element_call_expression_inner(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: P<Node>,
    mut base_expression: Expr,
    argument_expression: P<Node>,
    node_arguments: &[P<Node>],
) -> Expr {
    let access = expression.as_element_access_expression();
    let callee = node.expression().expect("a call has a callee");
    // a in a[b]()
    validate_not_any_type(state, access.expression);
    // b in a[b]()
    validate_not_any_type(state, access.argument_expression);
    // a[b] in a[b]()
    validate_not_any_type(state, callee);

    if tsrs_ast::is_super_property(expression) {
        // rbxts-rs: a plugin's `property_key` (DESIGN §11.5)
        let callee_exp = match crate::plugins::dispatch::rewritten_key(state, expression, AccessPosition::Callee) {
            Some(key) => {
                // a computed argument is still evaluated (`super` has no effects to order it after)
                let statements = crate::plugins::dispatch::replaced_key_statements(state, expression);
                prereqs.push_list(statements);
                keyed_access(base_expression, &key)
            }
            None => {
                let index = transform_expression(state, prereqs, access.argument_expression);
                create::computed_index(convert_to_indexable_expression(base_expression), index)
            }
        };
        let args = ensure_transform_order(state, prereqs, node.arguments());
        let args = if is_method(state, expression) {
            let mut with_self = vec![globals::self_()];
            with_self.extend(args);
            with_self
        } else {
            args
        };
        return create::call(callee_exp, args);
    }

    let callee_type = state.get_type(callee);
    let exp_type = state.checker.get_non_optional_type(callee_type);
    if let Some(symbol) = get_first_defined_symbol(state, exp_type)
        && let Some(r#macro) = state.macro_manager.get_property_call_macro(state.checker, symbol)
    {
        return transform_macro_call(
            MacroKind::PropertyCall(r#macro),
            state,
            prereqs,
            node,
            base_expression,
            node_arguments,
        );
    }

    // rbxts-rs: a plugin's `property_key` (DESIGN §11.5)
    let rewritten = crate::plugins::dispatch::rewritten_key(state, expression, AccessPosition::Callee);
    let mut key_prereqs = Prereqs::new();
    let argument_exp = match &rewritten {
        Some(key) => {
            // a computed argument is still evaluated, after the receiver
            key_prereqs.push_list(crate::plugins::dispatch::replaced_key_statements(state, expression));
            text_literal(key)
        }
        None => transform_expression(state, &mut key_prereqs, argument_expression),
    };
    let later = join_effects(
        get_effects_of_statements(&key_prereqs.statements),
        &(if is_late_read(&base_expression) { get_effects(&argument_exp) } else { rbxts_luau::Effects::none() }),
    );
    if !effects_commute(&get_effects(&base_expression), &later) {
        base_expression = prereqs.push_to_var(base_expression, "exp");
    }
    prereqs.push_list(key_prereqs.statements);
    let mut args_prereqs = Prereqs::new();
    let mut args = ensure_transform_order(state, &mut args_prereqs, node_arguments);
    fix_void_arguments_for_roblox_functions(state, exp_type, &mut args, node_arguments);
    let key = match rewritten {
        Some(key) => property_key_of(&key),
        None => {
            let receiver_type = state.get_type(access.expression);
            let receiver_type = state.checker.get_non_optional_type(receiver_type);
            PropertyKey::Index(add_one_if_array_type(state, receiver_type, argument_exp))
        }
    };
    let method = is_method(state, expression);
    let exp = create_ordered_property_call(
        state,
        prereqs,
        expression,
        base_expression,
        key,
        args,
        args_prereqs.statements,
        method,
    );
    wrap_return_if_lua_tuple(state, node, exp)
}

/// `transformCallExpression`
pub fn transform_call_expression(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Expr {
    transform_optional_chain(state, prereqs, node)
}
