//! `util/convertToIndexableExpression.ts`

use luau::{Expr, create};

/// Wraps an expression that cannot be called or indexed as it is in parentheses.
pub fn convert_to_indexable_expression(expression: Expr) -> Expr {
    if expression.is_indexable() { expression } else { create::paren(expression) }
}
