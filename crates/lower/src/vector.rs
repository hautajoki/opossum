//! Vector3 as Luau's native `vector` library. A Roblox `Vector3` is a Luau vector at runtime, so the rewrite only
//! changes call shapes, and the library calls are VM builtins (fastcalls, and `vector.create` with constant
//! arguments folds to a constant at `-O2`):
//!
//! ```text
//! new Vector3(x, y, z)          vector.create(x, y, z)        missing arguments are 0
//! Vector3.zero / Vector3.one    vector.zero / vector.one
//! v.Magnitude / v.Unit          vector.magnitude(v) / vector.normalize(v)
//! v.Abs() .Ceil() .Floor() .Sign()           vector.abs/ceil/floor/sign(v)
//! v.Cross(w) .Dot(w)                        vector.cross/dot(v, w)
//! ```
//!
//! The arithmetic macros (`v.add(w)`, `v.mul(2)`) are already native operators (`macros::property_call_macros`).
//!
//! Each rewrite is proven by the checker, and anything unproven keeps the plain lowering:
//! - the receiver's type is exactly the `@rbxts/types` `Vector3` interface: not a union, an intersection, a type
//!   parameter, `any` or a userland lookalike (an interface named `Vector3`, or a `_nominal_Vector3` brand);
//! - the member's every declaration is in `@rbxts/types`, so a user overload (`declare global`) never takes it;
//! - `new Vector3` resolves to `Vector3Constructor`'s own signature and every argument is definitely a number:
//!   `Vector3.new(nil)` is 0 where `vector.create(nil)` throws, so an optional number keeps `Vector3.new`;
//! - no optional chain link (`v?.Unit`): it runs inside the chain's nil check, which the plain lowering owns.
//!
//! Left on the Vector3 API: `Lerp` (`vector.lerp` returns `b` exactly at `t == 1`, where `a + (b - a) * t` may not),
//! `Angle` (Roblox rounds the result to float, `vector.angle` returns the double), `Max`/`Min` (they compare the other
//! way round, so a NaN or a signed zero picks the other operand; measured in Studio),
//! `FuzzyEq` (no library equivalent), the axis constants and `FromNormalId`/`FromAxis`, and `.X/.Y/.Z` (the VM reads
//! either case of a vector's components on the same fast path).

use std::sync::Arc;

use luau::{Effects, Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_checker::TypeFlags;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements};
use crate::util::types::{is_definitely_type, is_in_rbxts_types, is_number_type};

/// What `@rbxts/types` leaves out of the `vector` library: the operator macros (as `Vector3` has them, lowered by
/// `macros::property_call_macros`) and `vector.lerp`. A file of its own ([`declarations_file`]): opossum's `$macro`
/// detection reads the file of a symbol's first declaration, and these merge into the global `vector`.
pub const VECTOR_D_TS: &str = r#"interface vector {
	/** macro for vector + vector */
	add(this: vector, v: vector): vector;
	/** macro for vector - vector */
	sub(this: vector, v: vector): vector;
	/** macro for vector * vector | number */
	mul(this: vector, other: vector | number): vector;
	/** macro for vector / vector | number */
	div(this: vector, other: vector | number): vector;
	/** macro for vector // vector | number */
	idiv(this: vector, other: vector | number): vector;
}

declare namespace vector {
	/** Linearly interpolates every component from `a` to `b` by `t`; exactly `b` at `t == 1`. */
	function lerp(a: vector, b: vector, t: number): vector;
}
"#;

/// The in-memory file holding [`VECTOR_D_TS`].
pub fn declarations_file(root: &str) -> String {
    tsrs_core::tspath::combine_paths(root, &[".opossum/vector.d.ts"])
}

/// `Vector3` methods with a `vector` library function of the same semantics: `v.Method(args)` is
/// `vector.function(v, args)`.
const METHODS: &[(&str, &str)] = &[
    ("Abs", "abs"),
    ("Ceil", "ceil"),
    ("Cross", "cross"),
    ("Dot", "dot"),
    ("Floor", "floor"),
    ("Sign", "sign"),
];

/// A lowered `Vector3` member access.
pub enum Access {
    /// `Vector3.zero` as `vector.zero`.
    Constant(&'static str),
    /// `v.Magnitude` as `vector.magnitude(v)`.
    Function(&'static str),
}

/// `vector.<name>`, a lookup of the library that never changes.
fn library_member(name: &str) -> Expr {
    let mut member = create::property(create::id("vector"), name);
    if let Some(facts) = member.facts_mut() {
        facts.set_constant(true);
        facts.set_roblox_constant();
    }
    member
}

/// `vector.<name>(args)`. Library functions read only their arguments: they can throw (for a value the types lie
/// about), and `create` makes a new value instead.
fn library_call(name: &str, args: Vec<Expr>) -> Expr {
    let effects = if name == "create" { Effects::allocation() } else { Effects::may_throw() };
    let mut callee = create::property(create::id("vector"), name);
    if let Some(facts) = callee.facts_mut() {
        facts.set_builtin(Arc::new(effects));
    }
    let mut call = create::call(callee, args);
    if name == "create"
        && let Some(facts) = call.facts_mut()
    {
        facts.set_pure_constructor(false);
    }
    call
}

/// `declaration` is a member of the `@rbxts/types` interface named `owner`.
fn member_of(state: &mut TransformState<'_>, declaration: P<Node>, owner: &str) -> bool {
    declaration.parent().is_some_and(|parent| {
        parent.kind() == Kind::InterfaceDeclaration
            && parent.name().is_some_and(|name| name.text() == owner)
            && is_in_rbxts_types(state, parent)
    })
}

/// `node` evaluates to exactly the `@rbxts/types` `Vector3` (an object type: never a union, an intersection, a type
/// parameter or `any`).
fn is_vector3(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let t = state.get_type(node);
    if !t.flags().intersects(TypeFlags::Object) {
        return false;
    }
    let Some(symbol) = t.symbol() else { return false };
    symbol.name() == "Vector3"
        && symbol.declarations().iter().any(|&d| d.kind() == Kind::InterfaceDeclaration && is_in_rbxts_types(state, d))
}

/// `node` is the global `Vector3` value of `@rbxts/types`.
fn is_vector3_global(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    if node.kind() != Kind::Identifier || node.text() != "Vector3" {
        return false;
    }
    let Some(symbol) = state.checker.get_symbol_at_location_exported(node) else { return false };
    symbol.value_declaration().is_some_and(|d| is_in_rbxts_types(state, d))
}

/// Every declaration of the member named by the property access `access` is in the `@rbxts/types` interface `owner`.
fn is_own_member(state: &mut TransformState<'_>, access: P<Node>, owner: &str) -> bool {
    let name = access.as_property_access_expression().name;
    let Some(symbol) = state.checker.get_symbol_at_location_exported(name) else { return false };
    let declarations = symbol.declarations();
    !declarations.is_empty() && declarations.iter().all(|&d| member_of(state, d, owner))
}

/// `new Vector3(args)` as `vector.create(x, y, z)`, or `None` for the plain lowering.
pub fn new_vector(state: &mut TransformState<'_>, prereqs: &mut Prereqs, node: P<Node>) -> Option<Expr> {
    let class = node.expression()?;
    if !is_vector3_global(state, class) {
        return None;
    }
    let signature = state.checker.get_resolved_signature_exported(node);
    if !signature.declaration().is_some_and(|d| member_of(state, d, "Vector3Constructor")) {
        return None;
    }
    let arguments = node.arguments();
    for &argument in arguments {
        let t = state.get_type(argument);
        if argument.kind() == Kind::SpreadElement || !is_definitely_type(state, t, &[&is_number_type]) {
            return None;
        }
    }
    let mut args = ensure_transform_order(state, prereqs, arguments);
    args.resize_with(3, || create::number(0.0));
    Some(library_call("create", args))
}

/// The native lowering of the property access `node` (`Vector3.zero`, `v.Magnitude`), or `None`.
pub fn access(state: &mut TransformState<'_>, node: P<Node>, name: &str) -> Option<Access> {
    if tsrs_ast::is_optional_chain(node) {
        return None;
    }
    let receiver = node.as_property_access_expression().expression;
    match name {
        "Magnitude" | "Unit" if is_vector3(state, receiver) && is_own_member(state, node, "Vector3") => {
            Some(Access::Function(if name == "Unit" { "normalize" } else { "magnitude" }))
        }
        "zero" | "one" if is_vector3_global(state, receiver) && is_own_member(state, node, "Vector3Constructor") => {
            Some(Access::Constant(if name == "zero" { "zero" } else { "one" }))
        }
        _ => None,
    }
}

/// `access` applied to the lowered receiver (dropped for a constant: it is the `Vector3` global).
pub fn lower_access(access: Access, receiver: Expr) -> Expr {
    match access {
        Access::Constant(name) => library_member(name),
        Access::Function(name) => library_call(name, vec![receiver]),
    }
}

/// The library function for the method call `access(...)` (`v.Dot(w)`), or `None`.
pub fn method(state: &mut TransformState<'_>, access: P<Node>, name: &str) -> Option<&'static str> {
    let &(_, function) = METHODS.iter().find(|&&(method, _)| method == name)?;
    if tsrs_ast::is_optional_chain(access) {
        return None;
    }
    let receiver = access.as_property_access_expression().expression;
    (is_vector3(state, receiver) && is_own_member(state, access, "Vector3")).then_some(function)
}

/// `vector.function(receiver, args)`. The receiver is evaluated first, as in TypeScript: it goes to a local when the
/// arguments' prerequisites could change it.
pub fn lower_method(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    function: &str,
    mut receiver: Expr,
    arguments: &[P<Node>],
) -> Expr {
    let mut args_prereqs = Prereqs::new();
    let args = ensure_transform_order(state, &mut args_prereqs, arguments);
    if !effects_commute(&get_effects(&receiver), &get_effects_of_statements(&args_prereqs.statements)) {
        receiver = prereqs.push_to_var(receiver, "vec");
    }
    prereqs.push_list(args_prereqs.statements);
    library_call(function, std::iter::once(receiver).chain(args).collect())
}
