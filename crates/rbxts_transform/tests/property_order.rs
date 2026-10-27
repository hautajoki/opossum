//! `util::ts5_property_order` on a synthetic project (`tests/property_order`): inheritance chains with redeclared
//! members, bases listed against their declaration order, a generic base, computed (late-bound) members, classes, an
//! intersection, declaration merging with a module augmentation, a base in another file whose members' offsets fall
//! inside the derived interface's range, type literals (alone, in an intersection, instantiated), object literal
//! types with a spread (alone and as an interface's base; class instances, falsy and object unions and generic operands
//! spread, a generic operand between same-named parts, optional and intersected generic operands, union members with
//! the same names) and homomorphic mapped types (filtering and renaming `as` clauses included).
//!
//! - The rebuilt order must equal TypeScript 5's, `expected-ts5.txt`, which `tests/property_order/ts5props.cjs`
//!   writes with TypeScript 5.9.3 (the lines are compared as sorted sets: the two compilers list files in different
//!   orders).
//! - TypeScript 7's own order is pinned for the cases where it differs, so a change in tsrs shows up here.

use std::path::Path;

use rbxts_project::{LoadOptions, STACK_SIZE};
use rbxts_transform::util::ts5_property_order::ts5_property_order;
use tsrs_ast::{Kind, Node, SourceFile};
use tsrs_checker::{Checker, Type};
use tsrs_compiler::Context;
use tsrs_core::P;

fn source_text(file: &SourceFile, node: P<Node>) -> &'static str {
    let (pos, end) = (node.pos().expect_source("parsed"), node.end().expect_source("parsed"));
    file.text()[pos as usize..end as usize].trim()
}

/// The names, a unique symbol's spelled as `@name` (TypeScript 5: `__@name@<id>`, TypeScript 7: `\x7f@name@<id>`).
fn names(symbols: &[P<tsrs_ast::Symbol>]) -> String {
    symbols.iter().map(|s| normalize(s.name())).collect::<Vec<_>>().join(", ")
}

/// `__@name@123` / `\x7f@name@123` → `@name`.
fn normalize(name: &str) -> String {
    let rest = name.strip_prefix("__@").or_else(|| name.strip_prefix("\u{7f}@"));
    match rest.and_then(|r| r.rsplit_once('@')) {
        Some((base, id)) if id.bytes().all(|b| b.is_ascii_digit()) => format!("@{base}"),
        _ => name.to_owned(),
    }
}

/// One line per interface or class declaration and per variable annotated with a type reference, in
/// `ts5props.cjs`'s format: (TypeScript 7 order, rebuilt TypeScript 5 order).
fn visit(checker: &mut Checker, file: &'static SourceFile, rel: &str, node: P<Node>, out: &mut Vec<(String, String)>) {
    let mut line = |checker: &mut Checker, head: String, t: P<Type>| {
        let raw = names(checker.get_properties_of_type_exported(t));
        let ts5 = names(&ts5_property_order(checker, t));
        out.push((format!("{head}: {raw}"), format!("{head}: {ts5}")));
    };
    match node.kind() {
        Kind::InterfaceDeclaration | Kind::ClassDeclaration => {
            let name = node.name().expect("a named declaration");
            let symbol = checker.get_symbol_at_location_exported(name).expect("a declared symbol");
            let t = checker.get_declared_type_of_symbol_exported(symbol);
            line(checker, format!("{rel} decl {}", name.text()), t);
        }
        Kind::VariableDeclaration if node.type_node().is_some_and(|t| t.kind() == Kind::TypeReference) => {
            let name = node.name().expect("a named variable");
            let t = checker.get_type_at_location(name);
            line(checker, format!("{rel} var {}", source_text(file, name)), t);
        }
        _ => {}
    }
    node.for_each_child(&mut |child| {
        visit(checker, file, rel, child, out);
        false
    });
}

fn collect(check_first: bool) -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/property_order");
    let tsconfig = dir.join("tsconfig.json").to_string_lossy().replace('\\', "/");
    let cwd = dir.to_string_lossy().replace('\\', "/");
    let project = rbxts_project::load(&LoadOptions { tsconfig, cwd, checkers: 1, lazy_dts: false, regions: false })
        .unwrap_or_else(|d| panic!("the fixture's tsconfig: {} diagnostics", d.len()));
    let ctx = Context::background();
    project.program.bind_source_files();
    let mut checker = project.program.get_type_checker(&ctx);
    if check_first {
        for &file in project.program.source_files() {
            let diagnostics = project.program.get_semantic_diagnostics_with_checker(&ctx, &mut checker, file);
            assert!(diagnostics.is_empty(), "{}: {} diagnostics", file.file_name(), diagnostics.len());
        }
    }
    let mut out = Vec::new();
    for &file in project.program.source_files() {
        let name = file.file_name();
        let Some(index) = name.find("/property_order/src/") else { continue };
        if name.ends_with("globals.d.ts") {
            continue;
        }
        let rel = name[index + "/property_order/".len()..].to_owned();
        visit(&mut checker, file.get(), &rel, file.as_node(), &mut out);
    }
    out.sort();
    out
}

fn run(check_first: bool) {
    let out = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || collect(check_first))
        .expect("spawn the test thread")
        .join()
        .expect("the test thread");

    let expected = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/property_order/expected-ts5.txt"))
        .expect("expected-ts5.txt");
    let normalized = |line: &str| {
        let Some((head, list)) = line.split_once(": ") else { return line.to_owned() };
        format!("{head}: {}", list.split(", ").map(normalize).collect::<Vec<_>>().join(", "))
    };
    let expected: Vec<String> =
        expected.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).map(normalized).collect();
    let mut expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    expected.sort_unstable();
    let mut ts5: Vec<&str> = out.iter().map(|(_, ts5)| ts5.as_str()).collect();
    ts5.sort_unstable();
    assert_eq!(ts5.join("\n"), expected.join("\n"), "ts5_property_order against TypeScript 5");

    // TypeScript 7's own order where it differs from TypeScript 5's (see `util/ts5_property_order.rs`).
    let raw: Vec<&str> = out.iter().map(|(raw, _)| raw.as_str()).collect();
    for line in [
        // Inherited members sorted by position instead of in their base's order.
        "src/chain.ts decl Leaf: size, kind, label",
        "src/chain.ts decl Alias: kind, size, label",
        "src/chain.ts decl Both: a, b",
        // A late-bound member sorted with the bound ones (TypeScript 5 lists it after them).
        "src/chain.ts decl WithComputed: dyn, tail, kind, size",
        "src/chain.ts decl Boxed: value, kind, size, label",
        "src/chain.ts decl BoxedLeaf: size, kind, label, value",
        "src/chain.ts var boxedValue: value, kind, size, label",
        "src/chain.ts decl LeafClass: x, y, z, w",
        "src/chain.ts var leafValue: size, kind, label",
        "src/chain.ts var mixedValue: size, kind, label, a, b",
        // `icon` and `rank` taken for own members: their offsets in display.ts fall inside `Item` in item.ts.
        "src/item.ts decl Item: icon, rank, a, b, c, title, body",
        // Computed members of type literals sorted with the others.
        "src/literals.ts var literalValue: z, @tag, y, late, x",
        "src/literals.ts var headLiteralValue: head, shared, z, @tag, y, late, x",
        "src/literals.ts var genericValue: value, late, first",
        // A spread sorted by declaration instead of right side first.
        "src/literals.ts var spreadValue: sb, sc, sd, sa",
        "src/literals.ts decl FromSpread: own, sb, sc, sd, sa",
        // Homomorphic mapped types sorted by their source's declarations.
        "src/literals.ts var partialLeafValue: kind, label, size",
        "src/literals.ts var partialBothValue: a, b",
        // Spreads: a falsy union part, a union of objects, a generic operand, all sorted by declaration.
        "src/spreads.ts var andSpreadValue: k1, k3, k2, k0",
        "src/spreads.ts var emptySpreadValue: t1, t3, t2, t0",
        "src/spreads.ts var countSpreadValue: n1, n3, n2, n0",
        "src/spreads.ts var unionFirstValue: u2, u1, q, r",
        "src/spreads.ts var unionSecondValue: u3, u2, q, r",
        "src/spreads.ts var madeValue: a1, a2, gz, gy, gx2, gx1, gx",
        // A filtering `as` clause sorted by declaration.
        "src/spreads.ts var filteredValue: s2, sd, s1",
        // Generic and union operands: an optional generic, a generic intersection, union members with the same
        // names, and a renaming `as` clause.
        "src/generic_spreads.ts var withOptionsValue: oa, oc, ox, ob",
        "src/generic_spreads.ts var intersectedValue: tz, rb, ra, rd, rc, re, a",
        "src/generic_spreads.ts var eitherNumberValue: x2, x1, y2, y1",
        "src/generic_spreads.ts var evGettersValue: get_onA, get_onY, get_onZ, get_w, get_x, get_y",
    ] {
        assert!(raw.contains(&line), "TypeScript 7 order {line:?} not found in:\n{}", raw.join("\n"));
    }
}

#[test]
fn property_order_on_demand() {
    run(false);
}

/// The same with every file checked first, so types are resolved in check order rather than on demand.
#[test]
fn property_order_after_check() {
    run(true);
}
