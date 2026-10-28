//! Lowering on small projects written to a temporary directory, with just enough global declarations for the code
//! under test.

use std::sync::atomic::{AtomicUsize, Ordering};

use luau::ModulePath;
use rustc_hash::FxHashMap;

use crate::{LoadOptions, Location, Locator, Options, Realm};

const GLOBALS_D_TS: &str = r#"
interface Boolean {}
interface Function {}
interface CallableFunction extends Function {}
interface NewableFunction extends Function {}
interface IArguments {}
interface Number {}
interface Object {}
interface RegExp {}
interface String { size(): number; }
interface ArrayLike<T> { size(): number; readonly [index: number]: T; }
interface ReadonlyArray<T> extends ArrayLike<T> {}
interface Array<T> extends ReadonlyArray<T> { push(...items: Array<T>): number; [index: number]: T; }
declare namespace math { function ceil(x: number): number; function floor(x: number): number; }
declare function print(...args: Array<unknown>): void;
"#;

/// Lowers `files` (`name` under `src/`, or under the project root for a `node_modules/` package file; source text)
/// and renders each source, by name. Panics on a lowering error.
/// `options.native_files` name files under `src/` too.
pub fn lower(files: &[(&str, &str)], mut options: Options) -> FxHashMap<String, String> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "opossum-lower-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let types = root.join("node_modules/@rbxts/compiler-types");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(&types).unwrap();
    std::fs::write(types.join("package.json"), r#"{ "name": "@rbxts/compiler-types", "types": "index.d.ts" }"#)
        .unwrap();
    std::fs::write(types.join("index.d.ts"), GLOBALS_D_TS).unwrap();
    let root_name = root.to_string_lossy().replace('\\', "/");
    options.native_files = options.native_files.iter().map(|name| format!("{root_name}/src/{name}")).collect();
    let mut paths = Vec::new();
    for (name, text) in files {
        if name.starts_with("node_modules/") {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
            continue;
        }
        let path = format!("{root_name}/src/{name}");
        std::fs::write(&path, text).unwrap();
        paths.push(path);
    }

    let result = std::thread::Builder::new()
        .stack_size(crate::STACK_SIZE)
        .spawn(move || {
            let load = LoadOptions { root: root_name.clone(), files: paths, checkers: 1, flags: Vec::new() };
            let project = crate::load(&load, None).unwrap();
            let mut locator = Locator::default();
            for &file in project.program.source_files() {
                let name = file.file_name();
                if let Some(rel) = name.strip_prefix(&format!("{}/src/", project.root)) {
                    let stem = rel.trim_end_matches(".ts").to_owned();
                    let module = ModulePath {
                        names: vec!["ReplicatedStorage".into(), "shared".into(), stem],
                        root: 2,
                        init: false,
                    };
                    locator.files.insert(name.to_owned(), Location { module, realm: Realm::Shared });
                }
            }
            let runtime =
                ModulePath { names: vec!["ReplicatedStorage".into(), "RuntimeLib".into()], root: 1, init: false };
            locator.runtime_lib = Some(Location { module: runtime, realm: Realm::Shared });
            let sources = project.project_files();
            // import elision reads what checking marked as referenced
            project.check(&tsrs_compiler::Context::background(), &sources);
            let lowered = crate::lower_files(&project, &locator, &options, &sources);
            let mut rendered = FxHashMap::default();
            for l in lowered {
                let name = l.file.file_name();
                let messages: Vec<String> = l.diagnostics.iter().map(|d| d.localize()).collect();
                assert!(!l.has_errors(), "{name}: {messages:?}");
                let ast = l.ast.unwrap();
                let rel = name.rsplit('/').next().unwrap_or(name).to_owned();
                rendered.insert(rel, luau::render(&ast, &locator.get(name).unwrap().module));
            }
            rendered
        })
        .unwrap()
        .join();
    let _ = std::fs::remove_dir_all(&root);
    result.unwrap()
}

/// [`lower`] of one file `a.ts`.
pub fn lower_one(text: &str) -> String {
    lower(&[("a.ts", text)], Options::default()).remove("a.ts").unwrap()
}
