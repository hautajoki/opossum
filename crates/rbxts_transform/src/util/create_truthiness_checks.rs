//! `util/createTruthinessChecks.ts`: JavaScript truthiness in Luau, where `0`, `NaN` and `""` are truthy.

use rbxts_luau::{BinaryOperator, Expr, create};
use tsrs_ast::Node;
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::warnings;
use crate::util::expression_chain::binary_expression_chain;
use crate::util::types::{is_empty_string_type, is_nan_type, is_number_literal_type, is_possibly_type};

/// `willCreateTruthinessChecks`
pub fn will_create_truthiness_checks(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    is_possibly_type(state, t, &[&is_number_literal_type(0.0)])
        || is_possibly_type(state, t, &[&is_nan_type])
        || is_possibly_type(state, t, &[&is_empty_string_type])
}

/// `createTruthinessChecks`: `exp ~= 0 and exp == exp and exp ~= "" and exp` as the type requires.
pub fn create_truthiness_checks(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    mut exp: Expr,
    node: P<Node>,
) -> Expr {
    let t = state.get_type(node);
    let is_assignable_to_zero = is_possibly_type(state, t, &[&is_number_literal_type(0.0)]);
    let is_assignable_to_nan = is_possibly_type(state, t, &[&is_nan_type]);
    let is_assignable_to_empty_string = is_possibly_type(state, t, &[&is_empty_string_type]);

    if is_assignable_to_zero || is_assignable_to_nan || is_assignable_to_empty_string {
        exp = prereqs.push_to_var_if_complex(exp, "value");
    }

    let mut checks = Vec::new();

    if is_assignable_to_zero {
        checks.push(create::binary(exp.clone(), BinaryOperator::Ne, create::number(0.0)));
    }

    // workaround for https://github.com/microsoft/TypeScript/issues/32778
    if is_assignable_to_zero || is_assignable_to_nan {
        checks.push(create::binary(exp.clone(), BinaryOperator::Eq, exp.clone()));
    }

    if is_assignable_to_empty_string {
        checks.push(create::binary(exp.clone(), BinaryOperator::Ne, create::string("")));
    }

    let any_check = is_assignable_to_zero || is_assignable_to_nan || is_assignable_to_empty_string;
    checks.push(exp);

    if state.services.data.options.log_truthy_changes && any_check {
        let mut check_strs = Vec::new();
        if is_assignable_to_zero {
            check_strs.push("0");
        }
        if is_assignable_to_zero || is_assignable_to_nan {
            check_strs.push("NaN");
        }
        if is_assignable_to_empty_string {
            check_strs.push("\"\"");
        }
        state.add_diagnostic(warnings::truthy_change(&check_strs.join(", "), node));
    }

    binary_expression_chain(checks, BinaryOperator::And)
}
