//! `nodes/expressions/transformSuperKeyword.ts`

use luau::{Expr, globals};

/// `transformSuperKeyword`: `super`, the local the class boilerplate binds to the base class.
pub fn transform_super_keyword() -> Expr {
    globals::super_()
}
