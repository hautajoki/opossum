//! `util/evaluation/builtins.ts`: Luau libraries and Roblox methods whose lookups are stable.

use std::sync::Arc;

use rbxts_luau::{Effects, Expr};
use tsrs_ast::{Kind, ModifierFlags, Node};
use tsrs_core::P;

use super::facts::{mark_constant_reference, mark_stable_lookup, set_call_effects};
use crate::classes::transform_state::TransformState;
use crate::util::traversal::get_ancestor;
use crate::util::types::{is_array_type, is_definitely_type, is_in_rbxts_types, is_possibly_type, is_roblox_type};

const LIBRARIES: &[&str] = &["math", "string", "table", "bit32", "utf8", "coroutine", "task", "os", "debug", "buffer"];
const PURE_MATH: &[&str] = &[
    "abs", "acos", "asin", "atan", "atan2", "ceil", "cos", "cosh", "deg", "exp", "floor", "fmod", "frexp", "ldexp",
    "log", "log10", "max", "min", "modf", "noise", "pow", "rad", "round", "sign", "sin", "sinh", "sqrt", "tan", "tanh",
];

/// `isBuiltinLibrary`: checks the declaration, not the spelling (local lookalikes are ordinary tables).
pub fn is_builtin_library(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    if node.kind() != Kind::Identifier || !LIBRARIES.contains(&node.text()) {
        return false;
    }
    let symbol = state.checker.get_symbol_at_location_exported(node).expect("a library identifier has a symbol");
    let declarations = symbol.declarations();
    !declarations.is_empty()
        && declarations.iter().all(|&d| d.kind() == Kind::ModuleDeclaration && is_in_rbxts_types(state, d))
}

/// `tryMarkBuiltinLibrary`
pub fn try_mark_builtin_library(state: &mut TransformState<'_>, node: P<Node>, expression: &mut Expr) {
    if is_builtin_library(state, node) {
        mark_constant_reference(expression);
    }
}

fn lookup_type(state: &mut TransformState<'_>, expression: P<Node>, optional_chain: bool) -> P<tsrs_checker::Type> {
    let t = state.get_type(expression);
    if optional_chain { state.checker.get_non_nullable_type(t) } else { state.checker.get_non_optional_type(t) }
}

/// `isStableBuiltinMember`: native methods are immutable even when calling them can mutate, throw or yield.
pub fn is_stable_builtin_member(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let receiver = node.expression().expect("an access has a receiver");
    if is_builtin_library(state, receiver) {
        return true;
    }
    // optional-chain accesses run inside a nil guard; getNonOptionalType alone does not remove a real
    // `T | undefined` union from the original expression
    let optional_chain = tsrs_ast::is_optional_chain(node);
    let t = lookup_type(state, receiver, optional_chain);
    let is_roblox = match state.multi_transform_state.is_roblox_type_cache.get(&t) {
        Some(&cached) => cached,
        None => {
            let result = is_definitely_type(state, t, &[&is_roblox_type]);
            state.multi_transform_state.is_roblox_type_cache.insert(t, result);
            result
        }
    };
    if !is_roblox {
        return false;
    }
    let member_type = lookup_type(state, node, optional_chain);
    let Some(symbol) = member_type.symbol() else { return false };
    let declarations = symbol.declarations();
    !declarations.is_empty()
        && declarations.iter().all(|&d| d.kind() == Kind::MethodSignature && is_in_rbxts_types(state, d))
}

/// `tryMarkBuiltinMember`
pub fn try_mark_builtin_member(state: &mut TransformState<'_>, node: P<Node>, expression: &mut Expr) {
    if !is_stable_builtin_member(state, node) {
        return;
    }
    mark_stable_lookup(expression);
    // these numeric functions are inert for their statically checked arguments; clamp can throw even for numbers,
    // random/randomseed mutate RNG state
    let receiver = node.expression().expect("an access has a receiver");
    if is_builtin_library(state, receiver) && receiver.text() == "math" {
        let name = if node.kind() == Kind::PropertyAccessExpression {
            Some(node.as_property_access_expression().name.text())
        } else {
            let argument = node.as_element_access_expression().argument_expression;
            (argument.kind() == Kind::StringLiteral).then(|| argument.text())
        };
        let nonempty_arguments = node.parent().is_some_and(|call| {
            call.kind() == Kind::CallExpression
                && call.expression() == Some(node)
                && call.arguments().first().is_some_and(|a| a.kind() != Kind::SpreadElement)
        });
        if matches!(name, Some("min" | "max")) && !nonempty_arguments {
            // the types permit empty varargs, which throw. A function value must retain that possibility until an
            // actual call supplies an argument.
            set_call_effects(expression, Arc::new(Effects::may_throw()));
        } else if name.is_some_and(|n| PURE_MATH.contains(&n)) {
            set_call_effects(expression, Arc::new(Effects::none()));
        } else if name == Some("clamp") {
            set_call_effects(expression, Arc::new(Effects::may_throw()));
        }
    }
}

/// rbxts-rs: the Roblox datatypes whose constructors (`new X(…)`, `X.new(…)`, `X.fromRGB(…)`, …) allocate an
/// immutable value from their arguments and touch no other state. The JSX ordering (`get_jsx_effects`) lets them
/// commute with later prerequisites (the ones that can throw only with prerequisites that have no effects:
/// [`NEVER_THROWING_CONSTRUCTORS`]); `Instance.new` and every other call stay unknown.
const PURE_DATATYPES: &[&str] = &[
    "Axes", "BrickColor", "CFrame", "Color3", "ColorSequence", "ColorSequenceKeypoint", "Faces", "Font",
    "NumberRange", "NumberSequence", "NumberSequenceKeypoint", "PhysicalProperties", "Ray", "Rect", "Region3",
    "TweenInfo", "UDim", "UDim2", "Vector2", "Vector3",
];

/// rbxts-rs: the constructors of [`PURE_DATATYPES`] that never throw for arguments of their declared types (`new` is
/// `new X(…)`): component-wise values with no validation. The others can reject a value of the right type
/// (`new NumberRange(5, 1)`, `Color3.fromHex("zz")`, unsorted sequence keypoints, an unknown font or brick colour
/// name, out-of-range keypoint times, tween and material values), or their validation is not documented closely
/// enough to rule it out, so they count as possibly throwing.
const NEVER_THROWING_CONSTRUCTORS: &[(&str, &[&str])] = &[
    ("Vector2", &["new"]),
    ("Vector3", &["new", "FromNormalId", "FromAxis"]),
    ("UDim", &["new"]),
    ("UDim2", &["new", "fromScale", "fromOffset"]),
    (
        "CFrame",
        &[
            "new",
            "Angles",
            "fromEulerAngles",
            "fromEulerAnglesXYZ",
            "fromEulerAnglesYXZ",
            "fromOrientation",
            "fromAxisAngle",
            "lookAt",
            "lookAlong",
        ],
    ),
    ("Color3", &["new", "fromRGB", "fromHSV"]),
    ("Rect", &["new"]),
    ("Ray", &["new"]),
];

/// rbxts-rs: every declaration of `node`'s symbol is in `@rbxts/types` (the globals, not a user's shadow of them).
fn is_declared_in_rbxts_types(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let Some(symbol) = state.checker.get_symbol_at_location_exported(node) else { return false };
    let declarations = symbol.declarations();
    !declarations.is_empty() && declarations.iter().all(|&d| is_in_rbxts_types(state, d))
}

/// rbxts-rs: `declaration` belongs to `receiver`'s own constructor type in `@rbxts/types`: its nearest interface or
/// type alias is `<receiver>Constructor` (`interface UDim2Constructor`, `type UDimConstructor = new (…) => UDim`).
/// A member that interface inherits (a user's `interface ColorSequenceConstructor extends SharedTableConstructor`
/// gives it `SharedTableConstructor`'s `new (t: object)`) belongs to another type, so it does not count.
fn is_declared_by_constructor_type(state: &mut TransformState<'_>, declaration: P<Node>, receiver: &str) -> bool {
    let owner = get_ancestor(declaration, |a| {
        matches!(a.kind(), Kind::InterfaceDeclaration | Kind::TypeAliasDeclaration)
    });
    owner.is_some_and(|owner| {
        owner.name().is_some_and(|name| name.text().strip_suffix("Constructor") == Some(receiver))
            && is_in_rbxts_types(state, owner)
    })
}

/// rbxts-rs: marks `call` (the Luau call of `node`, `receiver.member(arguments)`, or `new receiver(arguments)`
/// when `member` is `None`) as a pure datatype constructor when `receiver` is one of [`PURE_DATATYPES`]'s
/// `@rbxts/types` globals, the member is not `BrickColor.random` (the RNG's state), the signature the call resolves to
/// is declared by the receiver's own constructor type there (a user's `declare global` augmentation can add an
/// overload, or inherit one, that takes a table), and no argument is an array or a spread
/// (`ColorSequence.new(keypoints)` reads the table). A constructor not in [`NEVER_THROWING_CONSTRUCTORS`] is marked
/// as possibly throwing.
pub fn try_mark_pure_constructor(
    state: &mut TransformState<'_>,
    node: P<Node>,
    receiver: P<Node>,
    member: Option<P<Node>>,
    arguments: &[P<Node>],
    call: &mut Expr,
) {
    if !matches!(call, Expr::Call(_))
        || receiver.kind() != Kind::Identifier
        || !PURE_DATATYPES.contains(&receiver.text())
        || !is_declared_in_rbxts_types(state, receiver)
    {
        return;
    }
    let datatype = receiver.text();
    if member.is_some_and(|member| member.text() == "random") {
        return;
    }
    let signature = state.checker.get_resolved_signature_exported(node);
    if !signature.declaration().is_some_and(|d| is_declared_by_constructor_type(state, d, datatype)) {
        return;
    }
    for &argument in arguments {
        if argument.kind() == Kind::SpreadElement {
            return;
        }
        let t = state.get_type(argument);
        if is_possibly_type(state, t, &[&is_array_type]) {
            return;
        }
    }
    let name = member.map_or("new", |member| member.text());
    let may_throw = !NEVER_THROWING_CONSTRUCTORS
        .iter()
        .any(|&(d, members)| d == datatype && members.contains(&name));
    if let Some(facts) = call.facts_mut() {
        facts.set_pure_constructor(may_throw);
    }
}

/// rbxts-rs: every declaration of `name`'s symbol is a `readonly` property of `receiver`'s own constructor interface
/// in `@rbxts/types` (`Vector2Constructor.zero`, `CFrameConstructor.identity`).
fn is_readonly_constructor_property(state: &mut TransformState<'_>, name: P<Node>, receiver: &str) -> bool {
    let Some(symbol) = state.checker.get_symbol_at_location_exported(name) else { return false };
    let declarations = symbol.declarations();
    !declarations.is_empty()
        && declarations.iter().all(|&d| {
            d.kind() == Kind::PropertySignature
                && tsrs_ast::has_syntactic_modifier(d, ModifierFlags::Readonly)
                && is_declared_by_constructor_type(state, d, receiver)
        })
}

/// rbxts-rs: marks `expression` (the Luau lookup of the property access `node`) as a Roblox constant when it is an
/// Enum item, `Enum.X.Y` with `Enum` the `@rbxts/types` global (not a user's variable or shadow) and `X`, `Y`
/// declared there, or a datatype constant, `X.name` with `X` one of [`PURE_DATATYPES`]'s `@rbxts/types` globals
/// and `name` a `readonly` property of its constructor interface there (`Vector2.zero`).
pub fn try_mark_roblox_constant(state: &mut TransformState<'_>, node: P<Node>, expression: &mut Expr) {
    if node.kind() != Kind::PropertyAccessExpression || !matches!(expression, Expr::PropertyAccess(_)) {
        return;
    }
    let access = node.as_property_access_expression();
    let receiver = access.expression;
    let constant = if receiver.kind() == Kind::PropertyAccessExpression {
        let inner = receiver.as_property_access_expression();
        inner.expression.kind() == Kind::Identifier
            && inner.expression.text() == "Enum"
            && is_declared_in_rbxts_types(state, inner.expression)
            && is_declared_in_rbxts_types(state, inner.name)
            && is_declared_in_rbxts_types(state, access.name)
    } else {
        receiver.kind() == Kind::Identifier
            && PURE_DATATYPES.contains(&receiver.text())
            && is_declared_in_rbxts_types(state, receiver)
            && is_readonly_constructor_property(state, access.name, receiver.text())
    };
    if constant && let Some(facts) = expression.facts_mut() {
        facts.set_roblox_constant();
    }
}
