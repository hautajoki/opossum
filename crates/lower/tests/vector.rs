//! The Vector3 → `vector` lowering on a fixture project (`tests/fixtures/vector`): what is rewritten, what the
//! checker cannot prove and keeps the Vector3 API, and that the output compiles at `-O2`.

use lower::{LoadOptions, Location, Locator, Realm};
use luau::ModulePath;

fn lower_fixture() -> String {
    let root = format!("{}/tests/fixtures/vector", env!("CARGO_MANIFEST_DIR"));
    let main = format!("{root}/src/main.ts");
    let options = LoadOptions {
        root: root.clone(),
        files: vec![main.clone()],
        checkers: 1,
        flags: Vec::new(),
        overlay: lower::program::declarations(&root),
    };
    let project = lower::load(&options, None).expect("the fixture loads");
    let files = project.project_files();
    let diagnostics = project.check(&tsrs_compiler::Context::background(), &files);
    assert!(diagnostics.is_empty(), "the fixture type-checks: {} diagnostics", diagnostics.len());

    let module = ModulePath { names: vec!["ReplicatedStorage".into(), "main".into()], root: 1, init: false };
    let mut locator = Locator::default();
    locator.files.insert(main, Location { module: module.clone(), realm: Realm::Shared });
    let lowered = lower::lower_files(&project, &locator, &lower::Options::default(), &files);
    assert_eq!(lowered.len(), 1);
    assert!(lowered[0].diagnostics.is_empty(), "the fixture lowers without diagnostics");
    luau::render(lowered[0].ast.as_ref().expect("a lowered tree"), &module)
}

#[test]
fn vector3_lowers_to_the_vector_library_only_where_the_checker_proves_it() {
    let out = std::thread::Builder::new().stack_size(lower::STACK_SIZE).spawn(lower_fixture).unwrap().join().unwrap();
    for expected in [
        // a binding named `vector` is renamed, so the library stays reachable inside its scope
        "return vector.create(x, 2, 0)",
        // `Vector3.new(nil)` is 0 where `vector.create(nil)` throws
        "Vector3.new(maybe)",
        "vector.zero",
        "Vector3.xAxis",
        "vector.dot(vector.normalize(v), w)",
        // Max/Min pick the other operand on a NaN or a signed zero than vector.max/min do
        "v:Max(w, v)",
        "v:Lerp(w, 0.5)",
        // a union receiver and a userland lookalike are not the @rbxts/types Vector3
        "either.Magnitude",
        "lookalike.Magnitude",
        // the injected declarations: operator macros and `vector.lerp`
        "(a + a) * 2",
        "vector.lerp(a, a, 0.5)",
    ] {
        assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
    }
    assert!(!out.contains("local vector ="), "the library is shadowed in:\n{out}");
    // the receiver is evaluated before the argument's assignment, as in TypeScript
    let receiver = out.find("receiver()").expect("a receiver call");
    let assignment = out.find("current = argument()").expect("the argument's assignment");
    let dot = out.rfind("vector.dot(").expect("the dot product");
    assert!(receiver < assignment && assignment < dot && !out[dot..].starts_with("vector.dot(receiver()"), "{out}");
    if let Err(error) = luau::gate::compile(&out, 2, 0) {
        panic!("-O2 rejects line {}: {}\n{out}", error.line, error.message);
    }
}
