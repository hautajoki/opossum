//! What a numeric `for` needs proven about a `for (let i = a; i < b; i += k)` loop before it may read `b` once:
//! [`is_invariant`] (the bound has the same value at every condition check) and [`is_integral`] (`i < b` is
//! `i <= b - 1`). Both answer `false` when in doubt.

use tsrs_ast::{Kind, ModifierFlags, Node, NodeFlags, Symbol, SymbolFlags};
use tsrs_checker::{LiteralValue, Type, TypeFlags};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::get_constant_integer::get_constant_integer;
use crate::util::traversal::{get_ancestor, has_syntactic_modifier, is_ancestor_of, skip_downwards, skip_upwards};
use crate::util::types::{get_first_defined_symbol, is_definitely_type, is_number_type};

/// How deep [`is_integral`] follows `const` initializers into other `const`s.
const MAX_DEPTH: u32 = 8;

/// A variable or parameter an identifier reads.
enum Variable {
    /// A `const` (possibly imported, possibly destructured): its declaration.
    Const(P<Node>),
    /// An unexported `let` or a parameter of this file: its symbol and the [`scope_of`] its declaration.
    Mutable(P<Symbol>, Option<P<Node>>),
}

/// The function a node runs in, or the class whose property initializers it runs in (at construction, not where the
/// class is written); `None` at the top level of the module.
fn scope_of(node: P<Node>) -> Option<P<Node>> {
    get_ancestor(node, |n| tsrs_ast::is_function_like(Some(n)) || tsrs_ast::is_class_like(n))
}

fn variable_of(state: &mut TransformState<'_>, identifier: P<Node>) -> Option<Variable> {
    let mut symbol = state.checker.get_symbol_at_location_exported(identifier)?;
    let imported = symbol.flags().intersects(SymbolFlags::Alias);
    if imported {
        symbol = state.checker.skip_alias(symbol);
    }
    let declaration = symbol.value_declaration()?;
    let mut binding = declaration;
    while binding.kind() == Kind::BindingElement {
        binding = binding.parent()?.parent()?;
    }
    match binding.kind() {
        Kind::VariableDeclaration => {
            let list = binding.parent()?;
            if list.flags().intersects(NodeFlags::Const) {
                Some(Variable::Const(declaration))
            } else if list.flags().intersects(NodeFlags::Let)
                && !imported
                // a namespace's exported `let` is a property other files can write
                && list.parent().is_some_and(|s| !has_syntactic_modifier(s, ModifierFlags::Export))
            {
                Some(Variable::Mutable(symbol, scope_of(binding)))
            } else {
                None
            }
        }
        Kind::Parameter if !imported => Some(Variable::Mutable(symbol, binding.parent())),
        _ => None,
    }
}

/// No write to the mutable variable can run while `for_node` runs: there is none, or the loop and every write run in
/// the one activation of the scope that declares it, the writes outside the loop. A write anywhere else could run
/// mid-loop: a closure called from the body, or the declaring scope itself while the loop's closure is suspended in a
/// yield.
fn is_unwritten_in(
    state: &mut TransformState<'_>,
    symbol: P<Symbol>,
    owner: Option<P<Node>>,
    for_node: P<Node>,
) -> bool {
    let file = crate::util::traversal::source_file_of(for_node);
    let Some(declaration) = symbol.value_declaration() else { return false };
    if crate::util::traversal::source_file_of(declaration) != file {
        return false;
    }
    let loop_in_owner = scope_of(for_node) == owner;
    state.checker.get_references_to_symbol_in_file(file, symbol).into_iter().all(|reference| {
        !tsrs_ast::is_write_access(reference)
            || (loop_in_owner && !is_ancestor_of(for_node, reference) && scope_of(reference) == owner)
    })
}

/// The `x.size()` macro call on an array (`ArrayLike.size`) or a string (`String.size`), lowered to `#x`: the receiver
/// and whether it is a string.
fn size_call(state: &mut TransformState<'_>, call: P<Node>) -> Option<(P<Node>, bool)> {
    if call.kind() != Kind::CallExpression || !call.arguments().is_empty() {
        return None;
    }
    let callee = skip_downwards(call.expression()?);
    if callee.kind() != Kind::PropertyAccessExpression || callee.name()?.text() != "size" {
        return None;
    }
    let callee_type = state.get_type(callee);
    let callee_type = state.checker.get_non_optional_type(callee_type);
    let symbol = get_first_defined_symbol(state, callee_type)?;
    let name = state.macro_manager.get_property_call_macro(state.checker, symbol)?.name;
    match name {
        "ArrayLike.size" => Some((callee.expression()?, false)),
        "String.size" => Some((callee.expression()?, true)),
        _ => None,
    }
}

/// The value of `expression` is the same at every condition check of the `for` statement `for_node`: literals, enum
/// members, `const`s, and `let`s and parameters that nothing can write while the loop runs, combined by arithmetic on
/// numbers; and `#x` of a string or of a local array whose length provably never changes.
pub fn is_invariant(state: &mut TransformState<'_>, expression: P<Node>, for_node: P<Node>) -> bool {
    let expression = skip_downwards(expression);
    match expression.kind() {
        Kind::NumericLiteral => true,
        Kind::PrefixUnaryExpression => {
            let unary = expression.as_prefix_unary_expression();
            matches!(unary.operator, Kind::MinusToken | Kind::PlusToken) && is_invariant(state, unary.operand, for_node)
        }
        Kind::BinaryExpression => {
            let binary = expression.as_binary_expression();
            matches!(
                binary.operator_token.kind(),
                Kind::PlusToken
                    | Kind::MinusToken
                    | Kind::AsteriskToken
                    | Kind::SlashToken
                    | Kind::PercentToken
                    | Kind::AsteriskAsteriskToken
            ) && is_invariant(state, binary.left, for_node)
                && is_invariant(state, binary.right(), for_node)
        }
        Kind::Identifier => {
            let t = state.get_type(expression);
            if !is_definitely_type(state, t, &[&is_number_type]) {
                return false;
            }
            match variable_of(state, expression) {
                Some(Variable::Const(_)) => true,
                Some(Variable::Mutable(symbol, owner)) => is_unwritten_in(state, symbol, owner, for_node),
                None => false,
            }
        }
        Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
            crate::util::get_constant_value_literal::get_constant_value_literal(state, expression).is_some()
        }
        Kind::CallExpression => {
            let Some((receiver, is_string)) = size_call(state, expression) else { return false };
            let receiver = skip_downwards(receiver);
            if receiver.kind() != Kind::Identifier {
                return false;
            }
            let invariant_binding = match variable_of(state, receiver) {
                Some(Variable::Const(_)) => true,
                Some(Variable::Mutable(symbol, owner)) => is_unwritten_in(state, symbol, owner, for_node),
                None => false,
            };
            invariant_binding && (is_string || is_fixed_length_array(state, receiver))
        }
        _ => false,
    }
}

/// `array` names a local of this file, not exported, made from an array literal, that nothing could ever resize:
/// every use of it only reads elements, takes `size()` or iterates it, so no alias of it escapes either.
fn is_fixed_length_array(state: &mut TransformState<'_>, array: P<Node>) -> bool {
    let Some(symbol) = state.checker.get_symbol_at_location_exported(array) else { return false };
    if symbol.flags().intersects(SymbolFlags::Alias) {
        return false;
    }
    let Some(declaration) = symbol.value_declaration() else { return false };
    if declaration.kind() != Kind::VariableDeclaration
        || declaration.as_variable_declaration().initializer().map(skip_downwards).map(|i| i.kind())
            != Some(Kind::ArrayLiteralExpression)
    {
        return false;
    }
    let statement = declaration.parent().and_then(|list| list.parent());
    if statement.is_none_or(|s| has_syntactic_modifier(s, ModifierFlags::Export)) {
        return false;
    }
    let file = crate::util::traversal::source_file_of(declaration);
    let name = declaration.name();
    state.checker.get_references_to_symbol_in_file(file, symbol).into_iter().all(|reference| {
        if Some(reference) == name || tsrs_ast::is_part_of_type_node(reference) {
            return true;
        }
        let up = skip_upwards(reference);
        let Some(parent) = up.parent() else { return false };
        match parent.kind() {
            Kind::ElementAccessExpression => {
                parent.expression() == Some(up)
                    && !tsrs_ast::is_assignment_target(skip_upwards(parent))
                    && skip_upwards(parent).parent().is_some_and(|p| p.kind() != Kind::DeleteExpression)
            }
            Kind::PropertyAccessExpression => {
                let call = skip_upwards(parent).parent();
                parent.expression() == Some(up)
                    && parent.name().is_some_and(|n| n.text() == "size")
                    && call.is_some_and(|c| {
                        c.kind() == Kind::CallExpression
                            && c.expression().map(skip_downwards) == Some(parent)
                            && c.arguments().is_empty()
                    })
            }
            Kind::ForOfStatement => parent.expression() == Some(up),
            _ => false,
        }
    })
}

fn is_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value
}

fn is_integer_literal_type(_: &mut TransformState<'_>, t: P<Type>) -> bool {
    t.flags().intersects(TypeFlags::NumberLiteral)
        && matches!(t.as_literal_type().value(), Some(LiteralValue::Number(n)) if is_integer(n.0))
}

/// A global library function (`math.floor`, `bit32.band`, `buffer.len`) whose result is an integer, ±inf or NaN.
fn is_integral_builtin(state: &mut TransformState<'_>, callee: P<Node>) -> bool {
    let callee = skip_downwards(callee);
    if callee.kind() != Kind::PropertyAccessExpression {
        return false;
    }
    let Some(library) = callee.expression().map(skip_downwards).filter(|l| l.kind() == Kind::Identifier) else {
        return false;
    };
    let Some(name) = callee.name() else { return false };
    let integral = match library.text() {
        "math" => matches!(name.text(), "floor" | "ceil" | "round"),
        "bit32" => name.text() != "btest",
        "buffer" => name.text() == "len",
        _ => false,
    };
    // the global library, not a project binding of that name
    integral
        && state.checker.get_symbol_at_location_exported(library).is_some_and(|s| {
            s.declarations()
                .iter()
                .all(|&d| tsrs_ast::get_source_file_of_node(d).is_some_and(|f| f.is_declaration_file()))
        })
}

/// `expression` is an integer (or ±inf or NaN, for which `x - 1` is `ceil(x) - 1` all the same): integer literals,
/// integer-valued `const`s and enum members, sizes, `math.floor` and friends, and `+ - * %` of those.
pub fn is_integral(state: &mut TransformState<'_>, expression: P<Node>) -> bool {
    is_integral_at(state, expression, 0)
}

fn is_integral_at(state: &mut TransformState<'_>, expression: P<Node>, depth: u32) -> bool {
    let expression = skip_downwards(expression);
    if depth > MAX_DEPTH {
        return false;
    }
    if get_constant_integer(state, expression, false).is_some() {
        return true;
    }
    let integral = match expression.kind() {
        Kind::NumericLiteral => is_integer(luau::js_string_to_number(&expression.text().replace('_', ""))),
        Kind::PrefixUnaryExpression => {
            let unary = expression.as_prefix_unary_expression();
            matches!(unary.operator, Kind::MinusToken | Kind::PlusToken)
                && is_integral_at(state, unary.operand, depth + 1)
        }
        Kind::BinaryExpression => {
            let binary = expression.as_binary_expression();
            matches!(
                binary.operator_token.kind(),
                Kind::PlusToken | Kind::MinusToken | Kind::AsteriskToken | Kind::PercentToken
            ) && is_integral_at(state, binary.left, depth + 1)
                && is_integral_at(state, binary.right(), depth + 1)
        }
        Kind::Identifier => match variable_of(state, expression) {
            Some(Variable::Const(declaration)) if declaration.kind() == Kind::VariableDeclaration => declaration
                .as_variable_declaration()
                .initializer()
                .is_some_and(|initializer| is_integral_at(state, initializer, depth + 1)),
            _ => false,
        },
        Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
            matches!(
                crate::util::get_constant_value_literal::get_constant_value_literal(state, expression),
                Some(luau::Expr::Number(n)) if is_integer(luau::js_string_to_number(&n.value))
            )
        }
        Kind::CallExpression => {
            size_call(state, expression).is_some()
                || expression.expression().is_some_and(|callee| is_integral_builtin(state, callee))
        }
        _ => false,
    };
    integral || {
        let t = state.get_type(expression);
        is_definitely_type(state, t, &[&is_integer_literal_type])
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::lower_one;

    /// Loops a numeric `for` would change: each reads a bound that can change, writes the loop variable, or
    /// compares a non-integer start strictly.
    #[test]
    fn loops_that_must_stay_while_loops() {
        let text = lower_one(
            r#"
            export function writtenBound(n: number) { for (let i = 0; i < n; i++) { n -= 1; } }
            export function closureWritesBound(n: number) {
                const shrink = () => { n -= 1; };
                for (let i = 0; i < n; i++) shrink();
            }
            export function propertyBound(o: { n: number }) { for (let i = 0; i < o.n; i++) print(i); }
            export function grownArray() {
                const items = [1, 2, 3];
                for (let i = 0; i < items.size(); i++) items.push(i);
            }
            export function aliasedArray(items: Array<number>) { for (let i = 0; i < items.size(); i++) print(items[i]); }
            export function escapingArray(f: (a: Array<number>) => void) {
                const items = [1, 2, 3];
                f(items);
                for (let i = 0; i < items.size(); i++) print(items[i]);
            }
            export function writtenVariable() { for (let i = 0; i < 10; i++) { i += 1; } }
            export function fractionalStart() { for (let i = 0.5; i < 3; i++) print(i); }
            export function initializerWritesBound(n: number) {
                class A { x = (n = 0); }
                for (let i = 0; i < n; i++) new A();
            }
            export function ownerWritesMidLoop(n: number, spawn: (f: () => void) => void, wait: () => void) {
                spawn(() => { for (let i = 0; i < n; i++) wait(); });
                n = 0;
            }
            export namespace N {
                export let m = 3;
                export function f() { for (let i = 0; i < m; i++) print(i); }
            }
            "#,
        );
        assert!(!text.contains("for i ="), "{text}");
        assert_eq!(text.matches("while ").count(), 11, "{text}");
    }

    #[test]
    fn invariant_bounds_become_numeric_fors() {
        let text = lower_one(
            r#"
            const ITEMS = [1, 2, 3];
            export function parameter(n: number) { for (let i = 0; i < n; i++) print(i); }
            export function fixedArray() { for (let i = 0; i < ITEMS.size(); i++) print(ITEMS[i]); }
            export function down(n: number) { for (let i = n; i >= 0; i -= 2) print(i); }
            export function captured(n: number) {
                const fns: Array<() => number> = [];
                for (let i = 1; i <= n; i++) fns.push(() => i);
                return fns;
            }
            "#,
        );
        for header in
            ["for i = 0, math.ceil(n) - 1 do", "for i = 0, #ITEMS - 1 do", "for i = n, 0, -2 do", "for i = 1, n do"]
        {
            assert!(text.contains(header), "{header} in {text}");
        }
        assert!(!text.contains("while "), "{text}");
    }
}
