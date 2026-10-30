//! `export =` beside type exports: tsgo promotes the types onto the assignment, and its value must still be imported.

use std::path::PathBuf;

use lower::{LoadOptions, Location, Locator, Realm};
use luau::ModulePath;

const GLOBALS: &str = "interface Array<T> { length: number; [n: number]: T }
interface Boolean {} interface Function {} interface CallableFunction {} interface NewableFunction {}
interface IArguments {} interface Number {} interface Object {} interface RegExp {} interface String {}
";

#[test]
fn a_default_import_of_an_export_assignment_beside_an_interface_requires_the_module() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export_assignment");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/globals.d.ts"), GLOBALS).unwrap();
    std::fs::write(
        root.join("src/data.d.ts"),
        "export interface Entry { readonly id: number }\ndeclare const data: { readonly fps: number }\nexport = data;\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.ts"), "import data from \"./data\";\nexport const fps = data.fps;\n").unwrap();
    let root = root.to_string_lossy().into_owned();
    let main = format!("{root}/src/main.ts");
    let types = format!("{root}/src/data.d.ts");
    let load = LoadOptions {
        root: root.clone(),
        files: vec![main.clone(), types.clone(), format!("{root}/src/globals.d.ts")],
        checkers: 1,
        flags: Vec::new(),
        overlay: lower::program::declarations(&root),
    };
    let (luau, diagnostics) = std::thread::Builder::new()
        .stack_size(lower::STACK_SIZE)
        .spawn(move || {
            let project = lower::load(&load, None).unwrap();
            let module = ModulePath { names: vec!["ReplicatedStorage".into(), "main".into()], root: 1, init: false };
            let data = ModulePath { names: vec!["ReplicatedStorage".into(), "data".into()], root: 1, init: false };
            let mut locator = Locator::default();
            locator.files.insert(main, Location { module: module.clone(), realm: Realm::Shared });
            locator.files.insert(types, Location { module: data, realm: Realm::Shared });
            let sources = project.project_files();
            let ctx = tsrs_compiler::Context::background();
            let mut diagnostics: Vec<String> = project.check(&ctx, &sources).iter().map(|d| d.localize()).collect();
            let lowered = lower::lower_files(&project, &locator, &lower::Options::default(), &sources);
            let main = lowered.iter().find(|l| l.file.file_name().ends_with("/main.ts")).unwrap();
            diagnostics.extend(main.diagnostics.iter().map(|d| d.localize()));
            (luau::render(main.ast.as_deref().unwrap_or_default(), &module), diagnostics)
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(diagnostics, Vec::<String>::new());
    assert!(luau.contains("require"), "{luau}");
}
