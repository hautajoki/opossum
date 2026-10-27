//! `util/offset.ts`

use rbxts_luau::{BinaryOperator, Expr, create};

use crate::util::get_literal_number_value::get_literal_number_value;

/// `expression + value`, folded into literals and into a trailing `+ n` / `- n` (`array[i - 1 + 1]` → `array[i]`).
pub fn offset(expression: Expr, value: f64) -> Expr {
    if value == 0.0 {
        return expression;
    }

    if let Expr::Binary(binary) = &expression
        && matches!(binary.operator, BinaryOperator::Add | BinaryOperator::Sub)
        && let Some(right_value) = get_literal_number_value(&binary.right)
    {
        let sign = if binary.operator == BinaryOperator::Sub { -1.0 } else { 1.0 };
        let new_right_value = right_value + value * sign;
        let Expr::Binary(binary) = expression else { unreachable!() };
        let binary = *binary;
        return if new_right_value == 0.0 {
            binary.left
        } else {
            create::binary(binary.left, binary.operator, create::number(new_right_value))
        };
    }

    if let Some(literal_value) = get_literal_number_value(&expression) {
        create::number(literal_value + value)
    } else {
        let operator = if value > 0.0 { BinaryOperator::Add } else { BinaryOperator::Sub };
        create::binary(expression, operator, create::number(value.abs()))
    }
}
