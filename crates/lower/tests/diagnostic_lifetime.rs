//! Lowering diagnostics outlive the program that first reported them: watch mode frees each program after the next
//! one is made, and a factory's message is made once per process.

use std::path::PathBuf;

use lower::{LoadOptions, Location, Locator, Realm};
use luau::ModulePath;

const GLOBALS: &str = "interface Array<T> { length: number; [n: number]: T }
interface Boolean {} interface Function {} interface CallableFunction {} interface NewableFunction {}
interface IArguments {} interface Number {} interface Object {} interface RegExp {} interface String {}
";

/// Loads the project, lowers `main.ts` and returns its diagnostics' text, then frees the program.
fn lower_and_free(root: &str) -> Vec<String> {
    let main = format!("{root}/src/main.ts");
    let load = LoadOptions {
        root: root.to_owned(),
        files: vec![main.clone(), format!("{root}/src/globals.d.ts")],
        checkers: 1,
        flags: Vec::new(),
        overlay: lower::program::declarations(root),
    };
    let project = lower::load(&load, None).unwrap();
    let module = ModulePath { names: vec!["ReplicatedStorage".into(), "main".into()], root: 1, init: false };
    let mut locator = Locator::default();
    locator.files.insert(main, Location { module, realm: Realm::Shared });
    let sources = project.project_files();
    let lowered = lower::lower_files(&project, &locator, &lower::Options::default(), &sources);
    let texts = lowered.iter().flat_map(|l| l.diagnostics.iter().map(|d| d.localize())).collect();
    // SAFETY: every diagnostic was copied into text above, and nothing else made under the program is read again.
    unsafe { project.free() };
    texts
}

#[test]
fn a_factory_reports_the_same_message_after_the_program_that_first_made_it_is_freed() {
    // freed program memory is filled and never reused, so a stale read shows instead of reading what was there
    // SAFETY: this binary's only test sets it before anything reads the environment.
    unsafe { std::env::set_var("TSRS_ARENA_POISON", "1") };
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("diagnostic_lifetime");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/globals.d.ts"), GLOBALS).unwrap();
    std::fs::write(root.join("src/main.ts"), "const t = { a: 1 };\nfor (const k in t) {}\nexport {};\n").unwrap();
    let root = root.to_string_lossy().into_owned();
    let texts = std::thread::Builder::new()
        .stack_size(lower::STACK_SIZE)
        .spawn(move || (0..4).map(|_| lower_and_free(&root)).collect::<Vec<_>>())
        .unwrap()
        .join()
        .unwrap();
    assert!(texts[0].iter().any(|t| t.contains("lower.noForInStatement")), "{:?}", texts[0]);
    for later in &texts[1..] {
        assert_eq!(later, &texts[0]);
    }
}
