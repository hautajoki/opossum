//! `util/getLiteralNumberValue.ts`

use rbxts_luau::{Expr, UnaryOperator};

/// The value of a number literal, a parenthesized one or a negated one.
pub fn get_literal_number_value(expression: &Expr) -> Option<f64> {
    match expression {
        Expr::Parenthesized(p) => get_literal_number_value(&p.expression),
        Expr::Number(n) => Some(rbxts_luau::js_string_to_number(&n.value.replace('_', ""))),
        Expr::Unary(u) if u.operator == UnaryOperator::Neg => get_literal_number_value(&u.expression).map(|v| -v),
        _ => None,
    }
}
