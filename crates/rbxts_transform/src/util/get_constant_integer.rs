//! `util/getConstantInteger.ts`: integers a numeric `for` loop can read once, before its body.

use rustc_hash::FxHashMap;
use tsrs_ast::{Kind, ModifierFlags, Node, NodeFlags};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::traversal::{get_ancestor, is_ancestor_of, skip_downwards, source_file_of};

/// `getBindingInitializer`: the initializer of a variable declaration, or of an element of an array binding pattern
/// whose declaration is initialized by an array literal without spreads.
fn get_binding_initializer(declaration: P<Node>) -> Option<P<Node>> {
    if declaration.kind() == Kind::VariableDeclaration {
        return declaration.as_variable_declaration().initializer();
    }

    let pattern = declaration.parent()?;
    let parent = pattern.parent()?;
    if pattern.kind() == Kind::ArrayBindingPattern
        && parent.kind() == Kind::VariableDeclaration
        && let Some(parent_initializer) = parent.as_variable_declaration().initializer()
        && declaration.as_binding_element().dot_dot_dot_token().is_none()
    {
        let initializer = skip_downwards(parent_initializer);
        if initializer.kind() == Kind::ArrayLiteralExpression
            && !initializer.elements().iter().any(|e| e.kind() == Kind::SpreadElement)
        {
            let index = pattern.elements().iter().position(|&e| e == declaration)?;
            if let Some(&element) = initializer.elements().get(index)
                && element.kind() != Kind::OmittedExpression
            {
                return Some(element);
            }
        }
    }
    None
}

fn is_function_like(node: P<Node>) -> bool {
    tsrs_ast::is_function_like(Some(node))
}

/// `Number.isSafeInteger`
fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value && value.abs() <= 9_007_199_254_740_991.0
}

struct ConstantIntegers<'s, 'a> {
    state: &'s mut TransformState<'a>,
    require_initialized: bool,
    // share initializer results within this analysis, including failures to prove an integer
    // separate calls retain their own initialization requirements and reference checks
    cache: FxHashMap<P<Node>, Option<f64>>,
}

impl ConstantIntegers<'_, '_> {
    fn get_integer(&mut self, expression: P<Node>) -> Option<f64> {
        let expression = skip_downwards(expression);
        if let Some(&cached) = self.cache.get(&expression) {
            return cached;
        }
        let value = self.get_constant_number(expression);
        let integer = value.filter(|&v| is_safe_integer(v));
        self.cache.insert(expression, integer);
        integer
    }

    /// `getConstantNumber`
    fn get_constant_number(&mut self, expression: P<Node>) -> Option<f64> {
        match expression.kind() {
            Kind::NumericLiteral => {
                return Some(rbxts_luau::js_string_to_number(&expression.text().replace('_', "")));
            }
            Kind::PrefixUnaryExpression => {
                let unary = expression.as_prefix_unary_expression();
                if unary.operator == Kind::MinusToken
                    && let Some(value) = self.get_integer(unary.operand)
                {
                    return Some(-value);
                }
            }
            Kind::BinaryExpression => {
                let binary = expression.as_binary_expression();
                let left = self.get_integer(binary.left);
                let right = self.get_integer(binary.right());
                if let (Some(left), Some(right)) = (left, right) {
                    match binary.operator_token.kind() {
                        Kind::PlusToken => return Some(left + right),
                        Kind::MinusToken => return Some(left - right),
                        Kind::AsteriskToken => return Some(left * right),
                        Kind::SlashToken => return Some(left / right),
                        Kind::AsteriskAsteriskToken => return Some(left.powf(right)),
                        _ => {}
                    }
                }
            }
            Kind::Identifier => return self.get_identifier_number(expression),
            _ => {}
        }
        None
    }

    fn get_identifier_number(&mut self, expression: P<Node>) -> Option<f64> {
        // rbxts-rs: roblox-ts asserts the symbol exists; a missing one is simply not a constant here
        let symbol = self.state.checker.get_symbol_at_location_exported(expression)?;
        let declaration = symbol.value_declaration()?;
        if !matches!(declaration.kind(), Kind::VariableDeclaration | Kind::BindingElement) {
            return None;
        }

        let mut variable = declaration;
        while variable.kind() == Kind::BindingElement {
            variable = variable.parent()?.parent()?;
        }
        if variable.kind() != Kind::VariableDeclaration
            || source_file_of(variable) != source_file_of(expression)
            || !variable.parent()?.flags().intersects(NodeFlags::Const)
            || tsrs_ast::get_combined_modifier_flags(variable).intersects(ModifierFlags::Ambient)
            || variable.as_variable_declaration().initializer().is_none()
        {
            return None;
        }

        // steps are read before the body in a numeric loop, even if the source would break first
        // captured bindings may still be uninitialized when their function is called
        if self.require_initialized {
            let declaration_statement = variable.parent()?.parent()?;
            if variable.end().expect_source("variable") > expression.pos().expect_source("expression")
                || !is_ancestor_of(declaration_statement.parent()?, expression)
                || get_ancestor(variable, is_function_like) != get_ancestor(expression, is_function_like)
            {
                return None;
            }
        }

        let initializer = get_binding_initializer(declaration)?;
        self.get_integer(initializer)
    }
}

/// `getConstantInteger`: the value of an expression that is provably a safe integer.
///
/// Numeric loops hoist bound and step reads, so effects and mutable references are rejected. Values are inferred from
/// initializers, since literal-typed properties can change through wider aliases.
pub fn get_constant_integer(
    state: &mut TransformState<'_>,
    expression: P<Node>,
    require_initialized: bool,
) -> Option<f64> {
    ConstantIntegers { state, require_initialized, cache: FxHashMap::default() }.get_integer(expression)
}
