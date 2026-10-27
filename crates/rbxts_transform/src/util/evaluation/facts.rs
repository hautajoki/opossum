//! `util/evaluation/facts.ts`: reading and writing the facts carried on Luau nodes ([`rbxts_luau::facts`]).

use std::sync::Arc;

use rbxts_luau::{AnyIdentifier, Binding, BindingKey, Effects, Expr};
use tsrs_ast::{Kind, NodeFlags, Symbol};
use tsrs_core::P;

/// `markConstantReference`
pub fn mark_constant_reference(expression: &mut Expr) {
    if let Some(facts) = expression.facts_mut() {
        facts.set_constant(true);
    }
}

/// `markStableLookup`: the lookup is inert; evaluating the receiver and invoking the member are not.
pub fn mark_stable_lookup(expression: &mut Expr) {
    if let Some(facts) = expression.facts_mut() {
        facts.set_stable_lookup();
    }
}

/// `markPrimitiveValue`
pub fn mark_primitive_value(expression: &mut Expr) {
    if let Some(facts) = expression.facts_mut() {
        facts.set_primitive();
    }
}

/// `copyValueFacts`
pub fn copy_value_facts(from: &Expr, to: &mut Expr) {
    if let (Some(from), Some(to)) = (from.facts(), to.facts_mut()) {
        to.copy_value_facts(from);
    }
}

/// `setCallEffects`
pub fn set_call_effects(expression: &mut Expr, effects: Arc<Effects>) {
    if let Some(facts) = expression.facts_mut() {
        facts.set_call_effects(effects);
    }
}

/// `getCallEffects`
pub fn get_call_effects(expression: &Expr) -> Option<Arc<Effects>> {
    expression.facts()?.call_effects().cloned()
}

/// `isConstantReference`
pub fn is_constant_reference(expression: &Expr) -> bool {
    expression.facts().is_some_and(|f| f.is_constant())
}

/// `isPrimitiveValue`
pub fn is_primitive_value(node: &Expr) -> bool {
    if node.facts().is_some_and(|f| f.is_primitive()) || node.is_simple_primitive() {
        return true;
    }
    if let Expr::Parenthesized(p) = node {
        return is_primitive_value(&p.expression);
    }
    false
}

/// `markBinding`: the identifier refers to `symbol`'s binding. Imports and function declarations are constant
/// references, and so are `const` variables (other variables are explicitly not).
pub fn mark_binding(identifier: &mut Expr, symbol: P<Symbol>, key: BindingKey) {
    let Some(facts) = identifier.facts_mut() else { return };
    facts.set_binding(key);
    let declarations = symbol.declarations();
    if declarations.iter().any(|d| {
        matches!(
            d.kind(),
            Kind::ImportSpecifier | Kind::ImportClause | Kind::NamespaceImport | Kind::ImportEqualsDeclaration
        )
    }) {
        facts.set_constant(true);
    }
    let declaration = symbol.value_declaration();
    if declaration.is_some_and(|d| d.kind() == Kind::FunctionDeclaration) {
        facts.set_constant(true);
    }
    let mut parent = declaration;
    while let Some(p) = parent
        && matches!(p.kind(), Kind::BindingElement | Kind::ObjectBindingPattern | Kind::ArrayBindingPattern)
    {
        parent = p.parent();
    }
    if let Some(p) = parent
        && p.kind() == Kind::VariableDeclaration
        && let Some(list) = p.parent()
        && list.kind() == Kind::VariableDeclarationList
    {
        facts.set_constant(list.flags().intersects(NodeFlags::Const));
    }
}

/// `getBinding`
pub fn get_binding_of_id(identifier: &AnyIdentifier) -> Binding {
    if let Some(key) = identifier.facts().binding() {
        return Binding::Key(key);
    }
    match identifier {
        AnyIdentifier::Identifier(id) => Binding::Name(id.name.clone()),
        AnyIdentifier::Temporary(id) => Binding::Temp(id.id),
    }
}

/// `getBinding` for an identifier expression; `None` for other expressions.
pub fn get_binding(expression: &Expr) -> Option<Binding> {
    match expression {
        Expr::Identifier(id) => Some(match id.facts.binding() {
            Some(key) => Binding::Key(key),
            None => Binding::Name(id.name.clone()),
        }),
        Expr::TemporaryIdentifier(id) => Some(match id.facts.binding() {
            Some(key) => Binding::Key(key),
            None => Binding::Temp(id.id),
        }),
        _ => None,
    }
}

/// `getBuiltinCallEffects`
pub fn get_builtin_call_effects(expression: &Expr) -> Option<Arc<Effects>> {
    expression.facts()?.builtin().cloned()
}

/// `isStableLookup`
pub fn is_stable_lookup(expression: &Expr) -> bool {
    expression.facts().is_some_and(|f| f.is_stable_lookup())
}
