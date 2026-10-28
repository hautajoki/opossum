//! Flamework's metadata macros (`@flamework/core`'s `Modding` types), evaluated at compile time. A call to a function
//! tagged `/** @metadata macro */` that leaves out a parameter typed with one of them passes the type's value there,
//! with the type as instantiated at the call (its explicit or inferred type arguments); [`crate::callsite`] does the
//! filling. Nothing else of Flamework is supported: `import type` its declarations, never its runtime.
//!
//! ```text
//! Modding.Many<T>             T's value: a string, number or boolean literal; undefined (nil, left out of a table);
//!                             a tuple (in order); an array (a value per member of its element union, in union order;
//!                             `never[]` is empty); an object (its properties)
//! Modding.Generic<T, "id">    "src/path/file@Name": the file declaring T relative to the project root, without its
//!                             extension, and T's name; the same wherever T is referenced, on every build and machine
//! Modding.Generic<T, "text">  T as TypeScript writes it ("Name")
//! Modding.Caller<"uuid">      the call's "path:line:col", as `Callsite`
//! ```
//!
//! A parameter typed with a union takes the value of its first member that is one of them (`Modding.Caller<"uuid"> |
//! string`). Anything else inside, and the other Modding types (`Hash`, `TupleLabels`, `Generic<T, "guard">`, the
//! other `Caller` fields, intrinsics), is `metadata.unsupported` at the call. The types are recognized by where their
//! marker property is declared (`@flamework/core`), never by name.

use luau::{Expr, create};
use tsrs_ast::{Kind, Node};
use tsrs_checker::{LiteralValue, Type, TypeFlags};
use tsrs_core::P;

use crate::callsite::{callsite, relative};
use crate::classes::transform_state::TransformState;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::traversal::source_file_of;

/// What a Flamework metadata type is, by its marker property.
#[derive(Clone, Copy)]
enum Marker {
    Many,
    Generic,
    Caller,
    /// A Modding type opossum does not evaluate.
    Other(&'static str),
}

const MARKERS: [(&str, Marker); 6] = [
    ("_flamework_macro_many", Marker::Many),
    ("_flamework_macro_generic", Marker::Generic),
    ("_flamework_macro_caller", Marker::Caller),
    ("_flamework_macro_hash", Marker::Other("Modding.Hash")),
    ("_flamework_macro_tuple_labels", Marker::Other("Modding.TupleLabels")),
    ("_flamework_intrinsic", Marker::Other("a Flamework intrinsic")),
];

/// How deep a value may nest: a recursive type is an error, not a stack overflow.
const MAX_DEPTH: u32 = 32;

/// The Flamework marker `t` carries, with the marker property's type.
fn marker(state: &mut TransformState<'_>, t: P<Type>) -> Option<(Marker, P<Type>)> {
    for (name, marker) in MARKERS {
        let Some(property) = state.checker.get_property_of_type_exported(t, name) else { continue };
        let flamework =
            property.declarations().iter().any(|&d| source_file_of(d).get().file_name().contains("/@flamework/core/"));
        if flamework {
            return Some((marker, state.checker.get_type_of_symbol_exported(property)));
        }
    }
    None
}

/// `t` when it is a Flamework metadata type, or else its first union member that is one.
pub fn metadata_type(state: &mut TransformState<'_>, t: P<Type>) -> Option<P<Type>> {
    if !t.is_union() {
        return marker(state, t).is_some().then_some(t);
    }
    t.types().iter().copied().find(|&member| marker(state, member).is_some())
}

/// `declaration` carries a `@metadata` JSDoc tag that says `macro`.
pub fn is_metadata_macro(declaration: P<Node>) -> bool {
    declaration.jsdoc(None).iter().any(|doc| {
        doc.as_jsdoc().tags.is_some_and(|tags| {
            tags.nodes().iter().any(|&tag| {
                tag.kind() == Kind::JSDocUnknownTag
                    && tag.tag_name().text() == "metadata"
                    && tag.as_jsdoc_unknown_tag().comment().is_some_and(|comment| {
                        comment.nodes().iter().any(|&part| {
                            part.kind() == Kind::JSDocText && part.text().split_whitespace().any(|word| word == "macro")
                        })
                    })
            })
        })
    })
}

/// The Luau value of the metadata type `t` (from [`metadata_type`]) at `call`, `None` for nil. `Err` names the part
/// of `t` that has no compile-time value.
pub fn evaluate(state: &mut TransformState<'_>, call: P<Node>, t: P<Type>) -> Result<Option<Expr>, String> {
    value(state, call, t, MAX_DEPTH)
}

fn value(state: &mut TransformState<'_>, call: P<Node>, t: P<Type>, depth: u32) -> Result<Option<Expr>, String> {
    if depth == 0 {
        return Err(format!("`{}`, nested too deep,", state.checker.type_to_string_exported(t)));
    }
    if let Some((marker, inner)) = marker(state, t) {
        return match marker {
            Marker::Many => value(state, call, inner, depth - 1),
            Marker::Generic => generic(state, inner).map(Some),
            Marker::Caller => match string_literal(inner) {
                Some("uuid") => Ok(Some(create_string_from_value(&callsite(state, call)))),
                field => Err(format!("Modding.Caller<\"{}\">", field.unwrap_or("?"))),
            },
            Marker::Other(name) => Err(name.to_owned()),
        };
    }
    if tsrs_checker::is_tuple_type_exported(t) {
        let mut members = Vec::new();
        for &member in state.checker.get_type_arguments_exported(t) {
            members.push(value(state, call, member, depth - 1)?.unwrap_or(Expr::Nil));
        }
        return Ok(Some(create::array(members)));
    }
    if state.checker.is_array_type_exported(t) {
        let Some(&element) = state.checker.get_type_arguments_exported(t).first() else {
            return Ok(Some(create::array(Vec::new())));
        };
        let members = if element.is_union() { element.types() } else { std::slice::from_ref(&element) };
        let mut values = Vec::new();
        for &member in members {
            if member.flags().intersects(TypeFlags::Never) {
                break;
            }
            values.push(value(state, call, member, depth - 1)?.unwrap_or(Expr::Nil));
        }
        return Ok(Some(create::array(values)));
    }
    if is_object(t) {
        let mut fields = Vec::new();
        for &property in state.checker.get_properties_of_type_exported(t) {
            let property_type = state.checker.get_type_of_symbol_exported(property);
            if let Some(field) = value(state, call, property_type, depth - 1)? {
                fields.push((create_string_from_value(property.name()), field));
            }
        }
        return Ok(Some(create::map(fields)));
    }
    let flags = t.flags();
    if flags.intersects(TypeFlags::StringLiteral | TypeFlags::NumberLiteral | TypeFlags::BooleanLiteral) {
        match t.as_literal_type().value() {
            Some(LiteralValue::String(s)) => return Ok(Some(create_string_from_value(s))),
            Some(LiteralValue::Number(n)) => return Ok(Some(create::number(n.0))),
            Some(LiteralValue::Boolean(b)) => return Ok(Some(create::bool(b))),
            _ => {}
        }
    }
    if flags.intersects(TypeFlags::Undefined) {
        return Ok(None);
    }
    Err(format!("`{}`", state.checker.type_to_string_exported(t)))
}

/// `Modding.Generic<T, M>`'s value, from its marker type `[T, M]`.
fn generic(state: &mut TransformState<'_>, marker: P<Type>) -> Result<Expr, String> {
    let &[target, field, ..] = state.checker.get_type_arguments_exported(marker) else {
        return Err("Modding.Generic".to_owned());
    };
    match string_literal(field) {
        Some("id") => match id(state, target) {
            Some(id) => Ok(create_string_from_value(&id)),
            None => Err(format!("the id of `{}`", state.checker.type_to_string_exported(target))),
        },
        Some("text") => Ok(create_string_from_value(&state.checker.type_to_string_exported(target))),
        field => Err(format!("Modding.Generic<T, \"{}\">", field.unwrap_or("?"))),
    }
}

/// `"src/path/file@Name"` of a type declared by name at the top of a file, or `None`.
fn id(state: &TransformState<'_>, t: P<Type>) -> Option<String> {
    let symbol = t.symbol()?;
    let declaration = symbol.value_declaration().or_else(|| symbol.declarations().first().copied())?;
    if declaration.name().is_none() || declaration.parent().is_none_or(|p| p.kind() != Kind::SourceFile) {
        return None;
    }
    let path = relative(state, source_file_of(declaration).get().file_name());
    let path = [".d.ts", ".tsx", ".ts"].iter().find_map(|ext| path.strip_suffix(ext)).unwrap_or(path);
    Some(format!("{path}@{}", symbol.name()))
}

/// An object type, or an intersection of them.
fn is_object(t: P<Type>) -> bool {
    if t.is_intersection() {
        return t.types().iter().all(|&member| is_object(member));
    }
    t.flags().intersects(TypeFlags::Object)
}

fn string_literal(t: P<Type>) -> Option<&'static str> {
    if !t.is_string_literal() {
        return None;
    }
    match t.as_literal_type().value() {
        Some(LiteralValue::String(s)) => Some(s),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::Options;
    use crate::tests::lower;

    const MODDING_D_TS: &str = r#"
export namespace Modding {
    export type Many<T> = T & { _flamework_macro_many: T };
    export type Generic<T, M extends "id" | "text"> = string & { _flamework_macro_generic: [T, M] };
    export type Caller<M extends "uuid"> = string & { _flamework_macro_caller: M };
}
"#;

    /// A phase's own id and the id another phase's `Before` names are the same string; the type is the one the call
    /// instantiates; undefined members are left out and skipped parameters padded.
    #[test]
    fn evaluates_metadata_types_at_the_call() {
        let a = r#"
import type { Modding } from "@flamework/core";
interface Before<P> { readonly _before: P }
export interface A {}
export interface B extends Before<A> {}
type Meta<P> = Modding.Many<{
    key: Modding.Generic<P, "id">;
    name: Modding.Generic<P, "text">;
    first: P extends Before<unknown> ? false : true;
    before: (P extends Before<infer Q> ? Modding.Generic<Q, "id"> : never)[];
    unused: undefined;
}>;
/** @metadata macro */
export function stage<P>(delta = 0, meta?: Meta<P>, site?: Modding.Caller<"uuid"> | string) {}
stage<A>(1);
stage<B>();
"#;
        let files = [("node_modules/@flamework/core/index.d.ts", MODDING_D_TS), ("a.ts", a)];
        let out = lower(&files, Options::default()).remove("a.ts").unwrap();
        let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
        let a = r#"stage(1, { key = "src/a@A", name = "A", first = true, before = {}, }, "src/a.ts:15:1")"#;
        let b =
            r#"stage(nil, { key = "src/a@B", name = "B", first = false, before = { "src/a@A" }, }, "src/a.ts:16:1")"#;
        assert!(out.contains(a) && out.contains(b), "{out}");
    }
}
