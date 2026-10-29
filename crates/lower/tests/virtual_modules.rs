//! `opossum/<name>` resolves to the module's in-memory types, never to a file on disk.

use std::path::PathBuf;

use lower::{LoadOptions, Location, Locator, Realm};
use luau::ModulePath;

const GLOBALS: &str = "interface Array<T> { length: number; [n: number]: T }
interface Boolean {} interface Function {} interface CallableFunction {} interface NewableFunction {}
interface IArguments {} interface Number {} interface Object {} interface RegExp {} interface String {}
";

#[test]
fn an_opossum_import_reads_the_overlay_before_src() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("virtual_modules");
    std::fs::create_dir_all(root.join("src/opossum")).unwrap();
    std::fs::write(root.join("src/globals.d.ts"), GLOBALS).unwrap();
    std::fs::write(
        root.join("src/main.ts"),
        "import assets from \"opossum/assets\";\nexport const crack: string = assets.decals.crack;\n",
    )
    .unwrap();
    // `*` maps `opossum/assets` here too; the longer `opossum/*` pattern must win
    std::fs::write(root.join("src/opossum/assets.ts"), "export const decoy = 1;\n").unwrap();
    let root = root.to_string_lossy().into_owned();
    let main = format!("{root}/src/main.ts");
    let types = lower::program::module_file(&root, "assets");
    let mut overlay = lower::program::declarations(&root);
    overlay.push((
        types.clone(),
        "declare const assets: { readonly decals: { readonly crack: string } }\nexport = assets\n".to_owned(),
    ));
    let load = LoadOptions {
        root: root.clone(),
        files: vec![main.clone(), format!("{root}/src/globals.d.ts")],
        checkers: 1,
        flags: Vec::new(),
        overlay,
    };
    let (luau, diagnostics) = std::thread::Builder::new()
        .stack_size(lower::STACK_SIZE)
        .spawn(move || {
            let project = lower::load(&load, None).unwrap();
            let module = ModulePath { names: vec!["ReplicatedStorage".into(), "main".into()], root: 1, init: false };
            let assets = ModulePath {
                names: vec!["ReplicatedStorage".into(), "rbxts".into(), "opossum".into(), "assets".into()],
                root: 2,
                init: false,
            };
            let mut locator = Locator::default();
            locator.files.insert(main, Location { module: module.clone(), realm: Realm::Shared });
            locator.files.insert(types, Location { module: assets, realm: Realm::Shared });
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
    assert!(luau.contains("opossum"), "{luau}");
    assert!(!luau.contains("decoy"), "{luau}");
}
