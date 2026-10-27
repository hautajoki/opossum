//! `util/valueToIdStr.ts`

use rbxts_luau::Expr;

fn expression_to_str(expression: &Expr) -> Option<&str> {
    match expression {
        // X -> "X", A.B -> "B"
        Expr::Identifier(id) => Some(&id.name),
        Expr::PropertyAccess(p) => Some(&p.name),
        // X.new() -> "X"
        Expr::Call(call) => match &call.expression {
            Expr::PropertyAccess(p) if p.name == "new" => expression_to_str(&p.expression),
            _ => None,
        },
        _ => None,
    }
}

/// A temporary's name derived from its value: `foo` for `Foo`, `a.foo`, `Foo.new()`; else empty.
pub fn value_to_id_str(value: &Expr) -> String {
    match expression_to_str(value) {
        Some(s) if rbxts_luau::is_valid_identifier(s) => {
            // `str.charAt(0).toLowerCase() + str.slice(1)`
            let mut chars = s.chars();
            let first = chars.next().map(|c| c.to_lowercase().collect::<String>()).unwrap_or_default();
            first + chars.as_str()
        }
        _ => String::new(),
    }
}
