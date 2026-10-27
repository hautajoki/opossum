//! `util/expressionChain.ts`

use rbxts_luau::{BinaryOperator, Expr, create};

use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;

/// `[a, b, c]` → `a op b op c`.
pub fn binary_expression_chain(expressions: Vec<Expr>, operator: BinaryOperator) -> Expr {
    let mut iter = expressions.into_iter();
    let first = iter.next().expect("a chain of at least one expression");
    iter.fold(first, |acc, current| create::binary(acc, operator, current))
}

/// `["a", "b", "c"]` → `exp.a.b.c`.
pub fn property_access_expression_chain(expression: Expr, names: &[String]) -> Expr {
    names.iter().fold(convert_to_indexable_expression(expression), |acc, name| create::property(acc, name.clone()))
}
