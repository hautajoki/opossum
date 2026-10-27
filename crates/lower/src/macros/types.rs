//! `macros/types.ts`: the macro signatures.

use rbxts_luau::Expr;
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;

/// `IdentifierMacro`
pub type IdentifierMacro = fn(&mut TransformState<'_>, P<Node>) -> Expr;

/// `ConstructorMacro`
pub type ConstructorMacro = fn(&mut TransformState<'_>, &mut Prereqs, P<Node>) -> Expr;

/// `CallMacro`. Operands are opaque values: assigning to them or transforming their source again bypasses
/// evaluation planning. Use `push_to_var` only for the macro's own working state.
pub type CallMacro = fn(&mut TransformState<'_>, &mut Prereqs, P<Node>, Expr, Vec<Expr>) -> Expr;

/// `PropertyCallMacro`: the node is a call whose expression is a property or element access.
pub type PropertyCallMacro = fn(&mut TransformState<'_>, &mut Prereqs, P<Node>, Expr, Vec<Expr>) -> Expr;

/// A macro table entry: its name (`Array.push`, `typeIs`; plugins see it) and its transform.
#[derive(Clone, Copy)]
pub struct Macro<F> {
    /// The name: the function (`typeIs`), class (`ArrayConstructor`) or `Class.method` (`Array.push`).
    pub name: &'static str,
    /// Its transform.
    pub transform: F,
}
