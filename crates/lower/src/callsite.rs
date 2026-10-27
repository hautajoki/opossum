//! `Callsite`: a call that leaves out a trailing optional parameter typed `Callsite` passes
//! `"path/to/file.ts:line:col"` there instead: the file relative to the project root, and the 1-based line and
//! column (UTF-16, as editors count) where the call starts. Hooks key their state by it. It changes when lines move,
//! so it identifies a call site within one build, not across edits. An argument passed explicitly (a hook forwarding
//! its own callsite) is left alone.

use luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::traversal::{get_start, source_file_of};

/// The declaration of `Callsite`, injected with the macro declarations. Plain strings stay assignable (an
/// intersection member with only optional properties is not checked against primitives), so a callsite can be
/// forwarded or made up by hand.
pub const CALLSITE_D_TS: &str = r#"
/** Filled in by opossum when left out: `"src/path/file.ts:line:col"` of the call. */
type Callsite = string & { readonly __callsite?: never };
"#;

/// Appends the callsite string to `args` when `call`'s signature ends in an optional `Callsite` parameter that the
/// call leaves out, padding skipped optional parameters with `nil`. `arguments` are the call's own (TypeScript)
/// arguments, `args` their lowered values.
pub fn append_callsite(state: &mut TransformState<'_>, call: P<Node>, arguments: &[P<Node>], args: &mut Vec<Expr>) {
    if arguments.iter().any(|a| a.kind() == Kind::SpreadElement) {
        return;
    }
    let Some(declaration) = state.checker.get_resolved_signature_exported(call).declaration() else { return };
    let mut parameters = declaration.parameters();
    // a `this` parameter takes no argument
    let this = |p: &P<Node>| p.name().is_some_and(|n| n.kind() == Kind::Identifier && n.text() == "this");
    if parameters.first().is_some_and(this) {
        parameters = &parameters[1..];
    }
    let Some(&last) = parameters.last() else { return };
    let index = parameters.len() - 1;
    if arguments.len() > index || !is_callsite_parameter(state, last) {
        return;
    }
    args.resize(index, create::nil());
    args.push(create_string_from_value(&callsite(state, call)));
}

fn is_callsite_parameter(state: &mut TransformState<'_>, parameter: P<Node>) -> bool {
    if parameter.kind() != Kind::Parameter {
        return false;
    }
    let declaration = parameter.as_parameter_declaration();
    if declaration.dot_dot_dot_token().is_some()
        || (declaration.question_token().is_none() && declaration.initializer().is_none())
    {
        return false;
    }
    let Some(type_node) = declaration.type_() else { return false };
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

/// `"path:line:col"` of the call's first token.
fn callsite(state: &TransformState<'_>, call: P<Node>) -> String {
    let file = source_file_of(call);
    let text = file.get().text();
    let start = (get_start(call) as usize).min(text.len());
    let before = &text[..start];
    let line = before.matches('\n').count() + 1;
    let column = before[before.rfind('\n').map_or(0, |nl| nl + 1)..].encode_utf16().count() + 1;
    let name = file.get().file_name();
    let root = state.root.trim_end_matches('/');
    let path = name.strip_prefix(root).and_then(|p| p.strip_prefix('/')).unwrap_or(name);
    format!("{path}:{line}:{column}")
}
