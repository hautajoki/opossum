//! `nodes/class/transformDecorators.ts`: decorators run as plain calls after the class is built, in TypeScript's
//! evaluation order (instance members, static members, then the class; parameters between a member's decorator
//! expressions and its decorator calls).

use rbxts_luau::{AnyIdentifier, AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_property_name::transform_property_name;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::expression_might_mutate::expression_might_mutate;
use crate::util::find_constructor::find_constructor;

/// The statements that apply one decorator, given its (indexable) value.
type DecoratorCallback<'f> = &'f mut dyn FnMut(&mut TransformState<'_>, Expr) -> Vec<Stmt>;

fn count_decorators(node: P<Node>) -> usize {
    node.decorators().len()
}

fn should_inline(
    state: &mut TransformState<'_>,
    is_last_decorator: bool,
    decorator: P<Node>,
    expression: &Expr,
) -> bool {
    // immutable expressions can be inlined
    let decorator_expression = decorator.expression().expect("a decorator has an expression");
    if !expression_might_mutate(state, expression, Some(decorator_expression)) {
        return true;
    }

    // if it's not the last decorator, we can't inline
    // this is because we need to initialize all decorators before running them
    if !is_last_decorator {
        return false;
    }

    let node = decorator.parent().expect("a decorator has a parent");

    // if the node is a method declaration and has a decorator on a parameter, we can't inline
    // this is because we need to run parameter decorators between initializing and running method decorators
    if node.kind() == Kind::MethodDeclaration && node.parameters().iter().any(|&p| count_decorators(p) > 0) {
        return false;
    }

    // if the node is a class declaration and has a decorator on a constructor parameter, we can't inline
    // this is because we need to run constructor parameter decorators between initializing and running class
    // decorators
    if tsrs_ast::is_class_like(node)
        && let Some(constructor) = find_constructor(node)
        && constructor.parameters().iter().any(|&p| count_decorators(p) > 0)
    {
        return false;
    }

    // if the node is a parameter and there are any parameters with decorators after it, we can't inline
    // this ensures all of the parameters are initialized before running any, including from sibling parameters
    if node.kind() == Kind::Parameter {
        let parameters = node.parent().expect("a parameter has a parent").parameters();
        let param_idx = parameters.iter().position(|&p| p == node).expect("a parameter is in its list");
        if parameters[param_idx + 1..].iter().any(|&p| count_decorators(p) > 0) {
            return false;
        }
    }

    true
}

/// `transformMemberDecorators`: the decorator expressions (initializers), and the calls (finalizers, applied
/// last decorator first).
fn transform_member_decorators(
    state: &mut TransformState<'_>,
    node: P<Node>,
    callback: DecoratorCallback<'_>,
) -> (Vec<Stmt>, Vec<Stmt>) {
    let mut initializers = Vec::new();
    let mut finalizers = Vec::new();

    let decorators = node.decorators();
    for (i, &decorator) in decorators.iter().enumerate() {
        let mut expression_prereqs = Prereqs::new();
        let decorator_expression = decorator.expression().expect("a decorator has an expression");
        // rbxts-rs: a plugin may claim the decorator before its expression is compiled (DESIGN §11.5)
        let mut expression = match crate::plugins::dispatch::claim_decorator(state, &mut expression_prereqs, decorator)
        {
            Some(claimed) => claimed,
            None => transform_expression(state, &mut expression_prereqs, decorator_expression),
        };

        initializers.extend(expression_prereqs.statements);

        let is_last_decorator = i == decorators.len() - 1;

        if !should_inline(state, is_last_decorator, decorator, &expression) {
            let temp = temp_id("decorator");
            initializers.push(create::local(temp.clone(), Some(expression)));
            expression = Expr::TemporaryIdentifier(temp);
        }

        let statements = callback(state, convert_to_indexable_expression(expression));
        finalizers.splice(0..0, statements);
    }

    (initializers, finalizers)
}

fn transform_method_decorators(state: &mut TransformState<'_>, member: P<Node>, class_id: &AnyIdentifier) -> Vec<Stmt> {
    let class: Expr = class_id.clone().into();
    let (initializers, finalizers) = transform_member_decorators(state, member, &mut |state, expression| {
        // local _descriptor = decorator(Class, "name", { value = Class.name })
        // if _descriptor then
        // 	Class.name = _descriptor.value
        // end

        let descriptor_id = temp_id("descriptor");
        let key = state.get_class_element_object_key(member).expect("Did not find method key for method decorator");

        let descriptor = create::map([(create::string("value"), create::computed_index(class.clone(), key.clone()))]);
        vec![
            create::local(
                descriptor_id.clone(),
                Some(create::call(expression, vec![class.clone(), key.clone(), descriptor])),
            ),
            create::if_stmt(
                Expr::TemporaryIdentifier(descriptor_id.clone()),
                vec![create::assignment(
                    create::computed_index(class.clone(), key),
                    AssignmentOperator::Assign,
                    create::property(Expr::TemporaryIdentifier(descriptor_id), "value"),
                )],
                ElseBody::default(),
            ),
        ]
    });

    let mut result = initializers;
    result.extend(transform_parameter_decorators(state, member, class_id));
    result.extend(finalizers);
    result
}

fn transform_property_decorators(
    state: &mut TransformState<'_>,
    member: P<Node>,
    class_id: &AnyIdentifier,
) -> Vec<Stmt> {
    let class: Expr = class_id.clone().into();
    let (initializers, finalizers) = transform_member_decorators(state, member, &mut |state, expression| {
        // typescript enforces that property keys are static, so they shouldn't have prereqs
        let mut key_prereqs = Prereqs::new();
        let key = transform_property_name(state, &mut key_prereqs, member.name().expect("a property has a name"));
        assert!(key_prereqs.statements.is_empty(), "a decorated property's key has no prerequisites");

        // decorator(Class, "name")
        vec![create::call_statement(create::call(expression, vec![class.clone(), key]))]
    });

    let mut result = initializers;
    result.extend(finalizers);
    result
}

fn transform_parameter_decorators(
    state: &mut TransformState<'_>,
    member: P<Node>,
    class_id: &AnyIdentifier,
) -> Vec<Stmt> {
    let class: Expr = class_id.clone().into();
    let mut initializers = Vec::new();
    let mut finalizers = Vec::new();

    for (i, &parameter) in member.parameters().iter().enumerate() {
        let (param_initializers, param_finalizers) =
            transform_member_decorators(state, parameter, &mut |state, expression| {
                // No member.name means it's the constructor, so the name argument should be nil
                let key = if member.name().is_some() {
                    state.get_class_element_object_key(member).expect("Did not find method key for parameter decorator")
                } else {
                    Expr::Nil
                };

                // decorator(Class, "name", 0)
                let index = create::number(i as f64);
                vec![create::call_statement(create::call(expression, vec![class.clone(), key, index]))]
            });
        initializers.extend(param_initializers);
        finalizers.splice(0..0, param_finalizers);
    }

    let mut result = initializers;
    result.extend(finalizers);
    result
}

fn transform_class_decorators(state: &mut TransformState<'_>, node: P<Node>, class_id: &AnyIdentifier) -> Vec<Stmt> {
    let class: Expr = class_id.clone().into();
    let (initializers, finalizers) = transform_member_decorators(state, node, &mut |_, expression| {
        // Class = decorator(Class) or Class
        vec![create::assignment(
            class.clone(),
            AssignmentOperator::Assign,
            create::binary(create::call(expression, vec![class.clone()]), BinaryOperator::Or, class.clone()),
        )]
    });

    let mut result = initializers;
    if let Some(constructor) = find_constructor(node) {
        result.extend(transform_parameter_decorators(state, constructor, class_id));
    }
    result.extend(finalizers);
    result
}

/// `transformDecorators`: <https://www.typescriptlang.org/docs/handbook/decorators.html#decorator-evaluation>
pub fn transform_decorators(state: &mut TransformState<'_>, node: P<Node>, class_id: &AnyIdentifier) -> Vec<Stmt> {
    let mut result = Vec::new();

    // instance decorators, then static decorators
    for is_static in [false, true] {
        for &member in node.members() {
            if tsrs_ast::has_static_modifier(member) != is_static {
                continue;
            }
            // check `member.body` to skip overload signatures because decorators are not valid on them
            if member.kind() == Kind::MethodDeclaration && member.body().is_some() {
                result.extend(transform_method_decorators(state, member, class_id));
            } else if member.kind() == Kind::PropertyDeclaration {
                result.extend(transform_property_decorators(state, member, class_id));
            }
        }
    }

    // class decorators
    result.extend(transform_class_decorators(state, node, class_id));

    result
}
