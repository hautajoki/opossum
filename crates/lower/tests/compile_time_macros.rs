//! The compile-time macros, lowered end to end on a tiny project.

use std::path::PathBuf;

use lower::{Input, LoadOptions, Location, Locator, Realm};
use luau::ModulePath;

/// The globals a `noLib` program needs, and `Extract` for the `$keys` declaration.
const GLOBALS: &str = "interface Array<T> { length: number; [n: number]: T }
interface Boolean {} interface Function {} interface CallableFunction {} interface NewableFunction {}
interface IArguments {} interface Number {} interface Object {} interface RegExp {} interface String {}
type Extract<T, U> = T extends U ? T : never;
";

/// Lowers `source` as `src/main.ts` of a project named `name` holding `files` (relative path, text): the rendered
/// Luau, the macro inputs and every diagnostic's text.
fn lower_one(
    name: &str,
    source: &str,
    files: &[(&str, &str)],
    options: lower::Options,
) -> (String, Vec<Input>, Vec<String>) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/globals.d.ts"), GLOBALS).unwrap();
    std::fs::write(root.join("src/main.ts"), source).unwrap();
    for (path, text) in files {
        std::fs::write(root.join(path), text).unwrap();
    }
    let root = root.to_string_lossy().into_owned();
    let main = format!("{root}/src/main.ts");
    let load = LoadOptions {
        root: root.clone(),
        files: vec![main.clone(), format!("{root}/src/globals.d.ts")],
        checkers: 1,
        flags: Vec::new(),
    };
    std::thread::Builder::new()
        .stack_size(lower::STACK_SIZE)
        .spawn(move || {
            let project = lower::load(&load, None).unwrap();
            let module = ModulePath { names: vec!["ReplicatedStorage".into(), "main".into()], root: 1, init: false };
            let mut locator = Locator::default();
            locator.files.insert(main, Location { module: module.clone(), realm: Realm::Shared });
            let sources = project.project_files();
            let ctx = tsrs_compiler::Context::background();
            let mut diagnostics: Vec<String> = project.check(&ctx, &sources).iter().map(|d| d.localize()).collect();
            let mut lowered = lower::lower_files(&project, &locator, &options, &sources);
            let lowered = lowered.pop().unwrap();
            diagnostics.extend(lowered.diagnostics.iter().map(|d| d.localize()));
            (luau::render(lowered.ast.as_deref().unwrap_or_default(), &module), lowered.inputs, diagnostics)
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn file_json_becomes_a_table_and_a_path_into_it_a_literal() {
    let json = r#"{ "name": "deep", "list": [1, null, 2.5], "gone": null, "inner": { "on": true } }"#;
    let (luau, inputs, diagnostics) = lower_one(
        "file_json",
        "export const data = $file(\"../data.json\");\nexport const on: boolean = $file(\"../data.json\").inner.on;\n",
        &[("data.json", json)],
        lower::Options::default(),
    );
    assert_eq!(diagnostics, Vec::<String>::new());
    // members in file order, a null member left out, a null element kept as nil
    assert!(luau.contains("name = \"deep\""), "{luau}");
    assert!(luau.contains("list = { 1, nil, 2.5 }"), "{luau}");
    assert!(!luau.contains("gone"), "{luau}");
    assert!(luau.contains("local on = true"), "{luau}");
    assert!(matches!(inputs.as_slice(), [Input::File(path)] if path.ends_with("/file_json/data.json")), "{inputs:?}");
}

#[test]
fn env_falls_back_only_when_the_variable_is_unset() {
    let mut options = lower::Options::default();
    options.env.insert("SET".into(), "from env".into());
    let (luau, inputs, diagnostics) = lower_one(
        "env_fallback",
        "export const a = $env(\"SET\", \"no\");\nexport const b = $env(\"UNSET\", \"yes\");\nexport const c = $env(\"UNSET\");\n",
        &[],
        options,
    );
    assert_eq!(diagnostics, Vec::<String>::new());
    assert!(luau.contains("local a = \"from env\""), "{luau}");
    assert!(luau.contains("local b = \"yes\""), "{luau}");
    assert!(luau.contains("local c = nil"), "{luau}");
    assert_eq!(inputs, [Input::Env("SET".into()), Input::Env("UNSET".into())]);
}

#[test]
fn keys_lists_optional_members_in_declaration_order() {
    let (luau, inputs, diagnostics) = lower_one(
        "keys_optional",
        "interface Shape { b: number; a?: string; readonly c?: boolean; 0: number }\nexport const k = $keys<Shape>();\n",
        &[],
        lower::Options::default(),
    );
    assert_eq!(diagnostics, Vec::<String>::new());
    assert!(luau.contains("local k = { \"b\", \"a\", \"c\" }"), "{luau}");
    assert!(inputs.is_empty());
}
