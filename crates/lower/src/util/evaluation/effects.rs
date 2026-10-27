//! `util/evaluation/effects.ts`: what running emitted Luau may do, and whether two evaluations can be reordered.
//!
//! The effect sets are [`rbxts_luau::Effects`]; the compiler-owned builtins' contracts (`markBuiltinReference` on
//! `luau.globals`) are marked by `rbxts_luau::globals` itself.

use std::collections::HashSet;

use rbxts_luau::{AnyIdentifier, Binding, Effects, Expr, InterpolatedStringPart, Stmt, UnaryOperator};

use super::facts::{
    get_binding_of_id, get_builtin_call_effects, get_call_effects, is_constant_reference, is_primitive_value,
    is_stable_lookup,
};
use super::node::{LuauNode, for_each_child};

/// `NO_EFFECTS`
pub fn no_effects() -> Effects {
    Effects::none()
}

fn binding_of_expr(e: &Expr) -> Option<Binding> {
    super::facts::get_binding(e)
}

/// `getFunctionEffects`: summarise the lowered body; parameters and locals belong to the invocation.
pub fn get_function_effects(parameters: &[AnyIdentifier], statements: &[Stmt]) -> Effects {
    let mut locals: HashSet<Binding> = parameters.iter().map(get_binding_of_id).collect();
    fn collect(node: LuauNode<'_>, locals: &mut HashSet<Binding>) {
        match node {
            LuauNode::Expr(Expr::Function(_)) => return,
            LuauNode::Stmt(Stmt::FunctionDeclaration(_)) => return,
            LuauNode::Stmt(Stmt::VariableDeclaration(n)) => {
                for id in &n.left {
                    locals.insert(get_binding_of_id(id));
                }
            }
            LuauNode::Stmt(Stmt::NumericFor(n)) => {
                locals.insert(get_binding_of_id(&n.id));
            }
            LuauNode::Stmt(Stmt::For(n)) => {
                for id in &n.ids {
                    locals.insert(get_binding_of_id(id));
                }
            }
            _ => {}
        }
        for_each_child(node, &mut |child| collect(child, locals));
    }
    for s in statements {
        collect(LuauNode::Stmt(s), &mut locals);
    }
    let mut effects = get_effects_of_statements(statements);
    effects.reads.retain(|key| !locals.contains(key) && !matches!(key, Binding::Temp(_)));
    effects.writes.retain(|key| !locals.contains(key) && !matches!(key, Binding::Temp(_)));
    effects
}

/// `joinEffects`
pub fn join_effects(mut a: Effects, b: &Effects) -> Effects {
    a.reads.extend(b.reads.iter().cloned());
    a.writes.extend(b.writes.iter().cloned());
    a.reads_heap |= b.reads_heap;
    a.writes_heap |= b.writes_heap;
    a.throws |= b.throws;
    a.allocates |= b.allocates;
    a
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wildcard {
    Reads,
    Writes,
}

fn is_exposed_binding(key: &Binding, wildcard: Wildcard) -> bool {
    match key {
        Binding::Key(k) => match wildcard {
            Wildcard::Writes => k.written_by_closure,
            Wildcard::Reads => k.captured,
        },
        // compiler temporaries are inaccessible to user code; untracked names fail closed
        Binding::Temp(_) => false,
        Binding::Name(_) | Binding::UserReads | Binding::UserWrites => true,
    }
}

fn has_exposed_binding(bindings: &HashSet<Binding>, wildcard: Wildcard) -> bool {
    bindings.iter().any(|k| is_exposed_binding(k, wildcard))
}

fn intersects(a: &HashSet<Binding>, b: &HashSet<Binding>) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    for (symbol, wildcard) in [(Binding::UserReads, Wildcard::Reads), (Binding::UserWrites, Wildcard::Writes)] {
        if a.contains(&symbol) && has_exposed_binding(b, wildcard) {
            return true;
        }
        if b.contains(&symbol) && has_exposed_binding(a, wildcard) {
            return true;
        }
    }
    a.iter().any(|k| b.contains(k))
}

fn has_writes(effects: &Effects) -> bool {
    effects.writes_heap || effects.writes.iter().any(|k| !matches!(k, Binding::Temp(_)))
}

/// `effectsCommute`, including errors: a failed operand must not allow a later mutation to happen.
pub fn effects_commute(a: &Effects, b: &Effects) -> bool {
    !(intersects(&a.reads, &b.writes)
        || intersects(&a.writes, &b.reads)
        || intersects(&a.writes, &b.writes)
        || (a.reads_heap && b.writes_heap)
        || (a.writes_heap && (b.reads_heap || b.writes_heap))
        || (a.throws && (b.throws || has_writes(b)))
        || (b.throws && has_writes(a)))
}

/// `canDiscard`
pub fn can_discard(effects: &Effects) -> bool {
    !effects.throws && !has_writes(effects)
}

/// `canRepeat`
pub fn can_repeat(effects: &Effects) -> bool {
    can_discard(effects) && !effects.allocates
}

fn reads_of(binding: Binding) -> Effects {
    Effects { reads: HashSet::from([binding]), ..Effects::none() }
}

/// `getIntrinsicEffects`: the node's direct effect, excluding its children. Unknown syntax fails closed.
pub fn get_intrinsic_effects(node: LuauNode<'_>) -> Effects {
    match node {
        LuauNode::Id(id) => reads_of(get_binding_of_id(id)),
        LuauNode::StringPart => Effects::none(),
        LuauNode::MapField(m) => match &m.index {
            Expr::String(_) => Effects::none(),
            Expr::Number(n) if rbxts_luau::js_string_to_number(&n.value).is_finite() => Effects::none(),
            _ => Effects::may_throw(),
        },
        LuauNode::Expr(e) => match e {
            Expr::Identifier(_) | Expr::TemporaryIdentifier(_) => {
                reads_of(binding_of_expr(e).expect("an identifier has a binding"))
            }
            Expr::Nil | Expr::True | Expr::False | Expr::Number(_) | Expr::String(_) | Expr::LateString(_) => {
                Effects::none()
            }
            Expr::None | Expr::VarArgs => Effects::none(),
            Expr::Call(n) => get_builtin_call_effects(&n.expression)
                .or_else(|| get_call_effects(&n.expression))
                .map_or_else(Effects::unknown, |e| (*e).clone()),
            Expr::MethodCall(_) => Effects::unknown(),
            Expr::Array(_) | Expr::Map(_) | Expr::MixedTable(_) => Effects::allocation(),
            Expr::Set(_) => Effects { throws: true, ..Effects::allocation() },
            Expr::PropertyAccess(_) | Expr::ComputedIndex(_) => {
                if is_stable_lookup(e) {
                    Effects::none()
                } else {
                    Effects::unknown()
                }
            }
            Expr::InterpolatedString(n) => {
                let inert = n.parts.iter().all(|p| match p {
                    InterpolatedStringPart::Text(_) => true,
                    InterpolatedStringPart::Expression(e) => is_primitive_value(e),
                });
                if inert { Effects::none() } else { Effects::unknown() }
            }
            Expr::Unary(n) => {
                if n.operator == UnaryOperator::Len && !is_primitive_value(&n.expression) {
                    Effects::unknown()
                } else {
                    Effects::none()
                }
            }
            Expr::Binary(n) => {
                use rbxts_luau::BinaryOperator::{Eq, Ne};
                if matches!(n.operator, Eq | Ne) && !is_primitive_value(&n.left) && !is_primitive_value(&n.right) {
                    Effects::unknown()
                } else {
                    Effects::none()
                }
            }
            Expr::Parenthesized(_) | Expr::If(_) => Effects::none(),
            // a function expression is not a statement and not one of the kinds above
            Expr::Function(_) => Effects::unknown(),
        },
        LuauNode::ElseIf(_) => Effects::none(),
        LuauNode::Stmt(s) => match s {
            Stmt::Assignment(n) => {
                // property writes can invoke __newindex or immediate Instance signal handlers
                let mut effects = Effects::none();
                for target in &n.left {
                    match binding_of_expr(target) {
                        Some(binding) => {
                            effects.writes.insert(binding);
                        }
                        None => effects = join_effects(effects, &Effects::unknown()),
                    }
                }
                effects
            }
            Stmt::VariableDeclaration(n) => {
                let mut effects = Effects::none();
                for target in &n.left {
                    effects.writes.insert(get_binding_of_id(target));
                }
                effects
            }
            // generalized iteration can call __iter
            Stmt::For(_) => Effects::unknown(),
            // moving an operand into a nonterminating loop can prevent its evaluation
            Stmt::While(_) | Stmt::Repeat(_) => Effects::unknown(),
            // non-finite bounds can fail to terminate
            Stmt::NumericFor(_) => Effects::may_throw(),
            Stmt::Call(_)
            | Stmt::Return(_)
            | Stmt::If(_)
            | Stmt::Do(_)
            | Stmt::Comment(_)
            | Stmt::Break
            | Stmt::Continue => Effects::none(),
            Stmt::FunctionDeclaration(_) | Stmt::MethodDeclaration(_) => Effects::unknown(),
        },
    }
}

fn accumulate(node: LuauNode<'_>, result: &mut Effects, pure_constructors: bool) {
    if let LuauNode::Expr(e) = node {
        if is_constant_reference(e) {
            return;
        }
        if pure_constructors
            && let Expr::Call(call) = e
            && call.facts.is_pure_constructor()
        {
            // the constructor is a constant global's member; only the arguments remain, and an error for a rejected
            // value (`new NumberRange(5, 1)`) must not move past a later write
            result.allocates = true;
            result.throws |= call.facts.constructor_may_throw();
            for argument in &call.args {
                accumulate(LuauNode::Expr(argument), result, pure_constructors);
            }
            return;
        }
        if pure_constructors && let Expr::PropertyAccess(access) = e && access.facts.is_roblox_constant() {
            // an Enum item or a datatype constant: a global's immutable member
            return;
        }
        // creating a closure does not execute its body
        if matches!(e, Expr::Function(_)) {
            result.allocates = true;
            return;
        }
    }
    if let LuauNode::Id(id) = node
        && id.facts().is_constant()
    {
        return;
    }
    let effects = get_intrinsic_effects(node);
    result.reads.extend(effects.reads);
    result.writes.extend(effects.writes);
    result.reads_heap |= effects.reads_heap;
    result.writes_heap |= effects.writes_heap;
    result.throws |= effects.throws;
    result.allocates |= effects.allocates;
    for_each_child(node, &mut |child| accumulate(child, result, pure_constructors));
}

/// `getEffects` of one expression.
pub fn get_effects(node: &Expr) -> Effects {
    if is_constant_reference(node) || node.is_simple_primitive() || node.is_none() {
        return Effects::none();
    }
    if matches!(node, Expr::Function(_)) {
        return Effects::allocation();
    }
    let mut result = Effects::none();
    accumulate(LuauNode::Expr(node), &mut result, false);
    result
}

/// `getEffects` of a statement list.
pub fn get_effects_of_statements(statements: &[Stmt]) -> Effects {
    let mut result = Effects::none();
    for s in statements {
        accumulate(LuauNode::Stmt(s), &mut result, false);
    }
    result
}

/// rbxts-rs: [`get_effects`] for the JSX ordering (`transform_jsx`), where a Roblox datatype constructor
/// (`UDim2.fromScale(…)`, marked by `try_mark_pure_constructor`) only allocates (and may throw, for the ones that
/// validate their arguments), and an Enum item or a datatype
/// constant (`Enum.AutomaticSize.Y`, `Vector2.zero`, marked by `try_mark_roblox_constant`) has no effect. roblox-ts's own uses of
/// `getEffects` keep treating it as an unknown call, so their output stays stock's.
pub fn get_jsx_effects(node: &Expr) -> Effects {
    if is_constant_reference(node) || node.is_simple_primitive() || node.is_none() {
        return Effects::none();
    }
    if matches!(node, Expr::Function(_)) {
        return Effects::allocation();
    }
    let mut result = Effects::none();
    accumulate(LuauNode::Expr(node), &mut result, true);
    result
}

/// rbxts-rs: [`get_effects_of_statements`] for the JSX ordering (see [`get_jsx_effects`]).
pub fn get_jsx_effects_of_statements(statements: &[Stmt]) -> Effects {
    let mut result = Effects::none();
    for s in statements {
        accumulate(LuauNode::Stmt(s), &mut result, true);
    }
    result
}

/// `isLateRead`: Luau can use a local's register directly until the consuming instruction.
pub fn is_late_read(expression: &Expr) -> bool {
    let mut e = expression;
    while let Expr::Parenthesized(p) = e {
        e = &p.expression;
    }
    e.is_any_identifier()
}
