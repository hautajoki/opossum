//! `nodes/transformMacroCall.ts`: a call or property call that a macro expands, with its operands planned.

use rbxts_luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::macros::property_call_macros::wrap_comments;
use crate::macros::types::{CallMacro, Macro, PropertyCallMacro};
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::shared::diagnostics::errors;
use crate::util::evaluation::plan::{EvaluationOperand, plan_evaluation};
use crate::util::types::is_possibly_undefined;
use crate::util::wrap_return_if_lua_tuple::wrap_return_if_lua_tuple;

/// Which table the macro came from.
#[derive(Clone, Copy)]
pub enum MacroKind {
    /// A call macro (`typeIs(…)`).
    Call(Macro<CallMacro>),
    /// A property call macro (`array.push(…)`).
    PropertyCall(Macro<PropertyCallMacro>),
}

/// `transformMacroCall`
pub fn transform_macro_call(
    r#macro: MacroKind,
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    expression: Expr,
    node_arguments: &[P<Node>],
) -> Expr {
    // rbxts-rs: an operand that transformed to nothing (an unsupported node, or an error such as `$tuple()` as an
    // argument) has already been reported and the file is not written. TypeScript expands the macro over the
    // shifted arguments anyway; here the expansion is skipped, so macros can rely on their operands.
    let mut missing_operand = expression.is_none();
    let mut operands = vec![EvaluationOperand { expression, prereqs: Vec::new() }];
    for &argument in node_arguments {
        let mut value_prereqs = Prereqs::new();
        let mut value = transform_expression(state, &mut value_prereqs, argument);
        if argument.kind() != Kind::SpreadElement {
            // scalar arguments must supply one nil even when an inlined call returns no values
            let argument_type = state.get_type(argument);
            if value.is_call() && is_possibly_undefined(state, argument_type) {
                value = create::paren(value);
            }
            missing_operand |= value.is_none();
            operands.push(EvaluationOperand { expression: value, prereqs: value_prereqs.statements });
            continue;
        }
        let signature = state.checker.get_resolved_signature_exported(node);
        let last_parameter = signature.parameters().last().and_then(|p| p.value_declaration());
        if let Some(parameter) = last_parameter
            && parameter.kind() == Kind::Parameter
            && parameter.as_parameter_declaration().dot_dot_dot_token().is_some()
        {
            state.add_diagnostic(errors::no_var_args_macro_spread(argument));
            return Expr::None;
        }
        let spread_expression = argument.expression().expect("a spread has an expression");
        let t = state.get_type(spread_expression);
        assert!(t.is_tuple_type(), "a spread macro argument is a tuple");
        let count = t.target_tuple_type().element_infos().len();
        let ids: Vec<_> = (0..count).map(|i| temp_id(format!("spread{i}"))).collect();
        if ids.is_empty() {
            // an empty spread may still have effects
            value_prereqs.push(create::local(temp_id(""), Some(value)));
            operands.push(EvaluationOperand { expression: Expr::None, prereqs: value_prereqs.statements });
        } else {
            value_prereqs.push(create::locals(ids.iter().cloned().map(Into::into).collect(), vec![value]));
            let mut first = Some(value_prereqs.statements);
            for id in ids {
                operands.push(EvaluationOperand {
                    expression: Expr::TemporaryIdentifier(id),
                    prereqs: first.take().unwrap_or_default(),
                });
            }
        }
    }

    let (name, transform) = match r#macro {
        MacroKind::Call(m) => (m.name, m.transform),
        MacroKind::PropertyCall(m) => (m.name, m.transform),
    };

    if missing_operand {
        return Expr::None;
    }

    let result = plan_evaluation(prereqs, operands, |expansion_prereqs, references| {
        let mut references = references.into_iter();
        let receiver = references.next().expect("the receiver operand");
        let args: Vec<Expr> = references.filter(|arg| !arg.is_none()).collect();
        match r#macro {
            MacroKind::Call(_) => transform(state, expansion_prereqs, node, receiver, args),
            MacroKind::PropertyCall(_) => wrap_comments(name, transform, state, expansion_prereqs, node, receiver, args),
        }
    });
    wrap_return_if_lua_tuple(state, node, result)
}
