//! `nodes/expressions/transformOmittedExpression.ts`

use rbxts_luau::{Expr, create};

/// `transformOmittedExpression`: an array hole (`[a, , b]`) is `nil`.
pub fn transform_omitted_expression() -> Expr {
    create::nil()
}
