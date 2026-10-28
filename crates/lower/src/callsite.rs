//! Parameters filled in at the call site. A call that leaves out trailing parameters gets, in their place:
//!
//! - for an optional parameter typed opossum's `Callsite`: `"path/to/file.ts:line:col"`, the file relative to the
//!   project root and the 1-based line and column (UTF-16, as editors count) where the call starts. Hooks key their
//!   state by it. It changes when lines move, so it identifies a call site within one build, not across edits.
//! - for a parameter whose type is one of Flamework's metadata types, in a function tagged `@metadata macro`: the
//!   type's value, evaluated at compile time ([`crate::flamework`]). Untagged, it is `metadata.untagged`.
//!
//! Left-out parameters before a filled one are padded with `nil`. An argument passed explicitly (a hook forwarding
//! its own callsite) is left alone, and a call with a spread argument is never filled.

use luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::flamework;
use crate::shared::diagnostics::errors;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::traversal::{get_start, source_file_of};

/// The declaration of `Callsite`, injected with the macro declarations. Plain strings stay assignable (an
/// intersection member with only optional properties is not checked against primitives), so a callsite can be
/// forwarded or made up by hand.
pub const CALLSITE_D_TS: &str = r#"
/** Filled in by opossum when left out: `"src/path/file.ts:line:col"` of the call. */
type Callsite = string & { readonly __callsite?: never };
"#;

/// Which parameters of a declaration (counted without `this`, at most 64) a call may have filled in, worked out once
/// per declaration.
#[derive(Clone, Copy, Default)]
pub struct Plan {
    /// Optional parameters typed `Callsite`.
    callsites: u64,
    /// Optional parameters whose type may hold Flamework metadata: all of a `@metadata macro`'s, otherwise those whose
    /// declared type holds it (an error when left out).
    flamework: u64,
    /// The declaration is tagged `@metadata macro`.
    tagged: bool,
}

/// Appends the values of the parameters `call` leaves out and its signature fills in (see the module docs).
/// `arguments` are the call's own (TypeScript) arguments, `args` their lowered values.
pub fn append_callsite(state: &mut TransformState<'_>, call: P<Node>, arguments: &[P<Node>], args: &mut Vec<Expr>) {
    if arguments.iter().any(|a| a.kind() == Kind::SpreadElement) {
        return;
    }
    let signature = state.checker.get_resolved_signature_exported(call);
    let Some(declaration) = signature.declaration() else { return };
    if arguments.len() >= declaration.parameters().len() {
        return;
    }
    let plan = match state.multi_transform_state.callsite_plans.get(&declaration) {
        Some(&plan) => plan,
        None => {
            let plan = plan(state, declaration);
            state.multi_transform_state.callsite_plans.insert(declaration, plan);
            plan
        }
    };
    // the plan's parameters from the first one left out on
    let first = arguments.len() as u32;
    let mut pending = (plan.callsites | plan.flamework).checked_shr(first).unwrap_or(0) << first;
    let mut filled: Vec<(usize, Expr)> = Vec::new();
    while pending != 0 {
        let index = pending.trailing_zeros() as usize;
        pending &= pending - 1;
        if plan.callsites & (1 << index) != 0 {
            filled.push((index, create_string_from_value(&callsite(state, call))));
            continue;
        }
        let parameter = state.checker.get_type_at_position_exported(signature, index);
        let parameter = state.checker.get_non_nullable_type(parameter);
        let Some(t) = flamework::metadata_type(state, parameter) else { continue };
        if !plan.tagged {
            state.add_diagnostic(errors::untagged_metadata(call));
            return;
        }
        match flamework::evaluate(state, call, t) {
            Ok(value) => filled.push((index, value.unwrap_or(Expr::Nil))),
            Err(part) => {
                state.add_diagnostic(errors::unsupported_metadata(call, &part));
                return;
            }
        }
    }
    for (index, value) in filled {
        args.resize(index, create::nil());
        args.push(value);
    }
}

/// The [`Plan`] of the function-like `declaration`.
fn plan(state: &mut TransformState<'_>, declaration: P<Node>) -> Plan {
    let mut parameters = declaration.parameters();
    // a `this` parameter takes no argument
    let this = |p: &P<Node>| p.name().is_some_and(|n| n.kind() == Kind::Identifier && n.text() == "this");
    if parameters.first().is_some_and(this) {
        parameters = &parameters[1..];
    }
    let tagged = flamework::is_metadata_macro(declaration);
    let mut plan = Plan { tagged, ..Plan::default() };
    for (index, &parameter) in parameters.iter().enumerate().take(64) {
        if parameter.kind() != Kind::Parameter {
            continue;
        }
        let parameter = parameter.as_parameter_declaration();
        if parameter.dot_dot_dot_token().is_some() {
            break;
        }
        // a call leaves out only optional parameters
        if parameter.question_token().is_none() && parameter.initializer().is_none() {
            continue;
        }
        let type_node = parameter.type_();
        if type_node.is_some_and(|type_node| is_callsite_type(state, type_node)) {
            plan.callsites |= 1 << index;
            continue;
        }
        // a macro's parameter types are evaluated at each call, as instantiated there
        let metadata = tagged
            || type_node.is_some_and(|type_node| {
                let t = state.checker.get_type_from_type_node_exported(type_node);
                flamework::metadata_type(state, t).is_some()
            });
        if metadata {
            plan.flamework |= 1 << index;
        }
    }
    plan
}

/// `type_node` names opossum's `Callsite`.
fn is_callsite_type(state: &mut TransformState<'_>, type_node: P<Node>) -> bool {
    if type_node.kind() != Kind::TypeReference {
        return false;
    }
    let name = type_node.as_type_reference_node().type_name;
    if name.kind() != Kind::Identifier || name.text() != "Callsite" {
        return false;
    }
    let Some(symbol) = state.checker.get_symbol_at_location_exported(name) else { return false };
    let macros = crate::program::macros_file(state.root);
    symbol.declarations().iter().any(|&d| source_file_of(d).get().file_name() == macros)
}

/// `file_name` relative to the project root (unchanged outside it).
pub fn relative<'f>(state: &TransformState<'_>, file_name: &'f str) -> &'f str {
    let root = state.root.trim_end_matches('/');
    file_name.strip_prefix(root).and_then(|p| p.strip_prefix('/')).unwrap_or(file_name)
}

/// `"path:line:col"` of the call's first token.
pub fn callsite(state: &TransformState<'_>, call: P<Node>) -> String {
    let file = source_file_of(call);
    let text = file.get().text();
    let start = (get_start(call) as usize).min(text.len());
    let before = &text[..start];
    let line = before.matches('\n').count() + 1;
    let column = before[before.rfind('\n').map_or(0, |nl| nl + 1)..].encode_utf16().count() + 1;
    format!("{}:{line}:{column}", relative(state, file.get().file_name()))
}
