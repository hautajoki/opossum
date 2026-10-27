//! The `stamp` test plugin against the mock compiler: claims, declines, late strings with Luau escaping, checker
//! counts, positions, the persistent state and kept `Shared` across builds, and the error paths (hook errors,
//! fatal setup).

mod support;

use std::path::PathBuf;

use rbxts_plugin::erased::erase;
use rbxts_plugin::{
    Abort, BuildKind, CallKind, CallSite, Claim, Cx, Plugin, PluginInstance, SetupCx, StateOrigin,
};
use serde_json::{Map, Value, json};
use support::mock::{
    MockCompiler, MockCx, MockSetup, Outcome, calls_of, fixture, line_and_character, source,
};
use support::stamp::{Stamp, StampState};

const GOOD: &[&str] = &["main.ts", "other.ts"];

fn entry(prefix: Value) -> Option<Map<String, Value>> {
    match json!({ "transform": "rbxts-transform-stamp", "prefix": prefix }) {
        Value::Object(map) => Some(map),
        _ => unreachable!(),
    }
}

fn luau(s: &str) -> Outcome {
    Outcome::Luau(s.to_owned())
}

fn site(file: &str, text: &str, outcome: Outcome) -> (String, String, Outcome) {
    (file.to_owned(), text.to_owned(), outcome)
}

fn builds(state: &rbxts_plugin::erased::ErasedState) -> u32 {
    state
        .downcast_ref::<StampState>()
        .expect("a stamp state")
        .builds
}

#[test]
fn claims_declines_and_late_strings() {
    let plugin = erase(Stamp);
    let mut setup = MockSetup::clean(fixture());
    setup.entry = entry(json!("it's \"v1\" \\ "));
    let out = MockCompiler::new()
        .build(&*plugin, &mut setup, GOOD, 1)
        .expect("the build succeeds");

    // both quote kinds and a backslash: `"` becomes \x22, the backslash doubles, the renderer keeps `"…"`
    let id = |k: u32| luau(&format!("\"it's \\x22v1\\x22 \\\\ #{k}/3\""));
    assert_eq!(
        out.sites,
        vec![
            site("main.ts", "here()", luau("\"src/main.ts:1:18\"")),
            site("main.ts", "buildId()", id(1)),
            site("main.ts", "plain()", Outcome::Lowered),
            site("main.ts", "mystery()", Outcome::Lowered),
            site("main.ts", "new here()", Outcome::Lowered),
            site("main.ts", "buildId()", id(2)),
            // the column counts UTF-16 units: `é` is one, though two bytes
            site("other.ts", "here()", luau("\"src/other.ts:2:26\"")),
            site("other.ts", "buildId()", id(3)),
        ]
    );
    // the unknown mode is reported per call, from the hook, not from classify_symbol
    assert_eq!(
        out.diagnostics,
        vec!["warning main.ts: `mystery` has an unknown @stamp mode; the call is left as it is"]
    );
    // late keys are keyed by path, never by FileIndex
    assert_eq!(
        out.finish.resolved,
        vec![
            "site:src/main.ts:0",
            "site:src/main.ts:1",
            "site:src/other.ts:0"
        ]
    );
    assert_eq!(
        out.finish.artifacts,
        vec![(
            PathBuf::from("stamp-report.json"),
            b"{\"sites\":3,\"builds\":1}\n".to_vec()
        )]
    );
    assert_eq!(
        out.finish.state_files,
        vec![(PathBuf::from(".stamp-state"), b"1".to_vec())]
    );
    assert_eq!(builds(&out.state), 1);
    assert_eq!(setup.inputs, vec!["env:STAMP_PREFIX"]);
}

#[test]
fn output_does_not_depend_on_the_checker_count() {
    let plugin = erase(Stamp);
    let files = &["bad.ts", "main.ts", "other.ts"];
    let one = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), GOOD, 1)
        .unwrap();
    for checkers in [2, 3] {
        let many = MockCompiler::new()
            .build(&*plugin, &mut MockSetup::clean(fixture()), GOOD, checkers)
            .unwrap();
        assert_eq!(many.sites, one.sites, "{checkers} checkers");
        assert_eq!(many.diagnostics, one.diagnostics, "{checkers} checkers");
        assert_eq!(
            many.finish.artifacts, one.finish.artifacts,
            "{checkers} checkers"
        );
    }
    // with an error too: the same diagnostics, file by file
    let one = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), files, 1)
        .unwrap();
    let two = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), files, 2)
        .unwrap();
    assert_eq!(two.diagnostics, one.diagnostics);
}

#[test]
fn lines_are_counted_as_typescript_counts_them() {
    let plugin = erase(Stamp);
    let out = MockCompiler::new()
        .build(
            &*plugin,
            &mut MockSetup::clean(fixture()),
            &["breaks.ts"],
            1,
        )
        .unwrap();
    // CR, CRLF (one break), U+2028 and U+2029 each end a line
    assert_eq!(
        out.sites,
        vec![site("breaks.ts", "here()", luau("\"src/breaks.ts:5:18\""))]
    );
    assert_eq!(line_and_character("a\r\nb", 3), (1, 0));
    assert_eq!(line_and_character("a\r\rb\u{2028}\u{2029}x", 10), (4, 0));
    assert_eq!(line_and_character("é😀x", 6), (0, 3));
}

#[test]
fn env_overrides_the_tsconfig_entry() {
    let plugin = erase(Stamp);
    let mut setup = MockSetup::clean(fixture());
    setup.entry = entry(json!("from-tsconfig "));
    setup.env.insert("STAMP_PREFIX".into(), "env:".into());
    let out = MockCompiler::new()
        .build(&*plugin, &mut setup, &["other.ts"], 1)
        .unwrap();
    assert_eq!(
        out.sites[1],
        site("other.ts", "buildId()", luau("\"env:#1/1\""))
    );
    assert_eq!(
        out.setup_messages,
        vec!["info: stamp: prefix \"env:\", 0 earlier builds"]
    );
}

#[test]
fn a_hook_error_aborts_the_site_and_fails_the_build() {
    let plugin = erase(Stamp);
    let mut compiler = MockCompiler::new();
    let out = compiler
        .build(
            &*plugin,
            &mut MockSetup::clean(fixture()),
            &["bad.ts", "main.ts"],
            1,
        )
        .expect("no fatal error");
    assert_eq!(
        out.sites[0],
        site("bad.ts", "here(1, \"two\")", Outcome::Aborted)
    );
    assert!(
        out.diagnostics
            .contains(&"error bad.ts: `here` takes no arguments".to_owned()),
        "{:?}",
        out.diagnostics
    );
    // finish does not run: nothing resolved, nothing queued, nothing committed
    assert!(!out.committed);
    assert!(out.finish.resolved.is_empty() && out.finish.artifacts.is_empty());
    assert!(compiler.committed.is_none() && compiler.kept.is_none());
}

#[test]
fn setup_fails_with_a_fatal_error() {
    let plugin = erase(Stamp);
    let mut setup = MockSetup::clean(fixture());
    setup.entry = entry(json!(5));
    let error = MockCompiler::new()
        .build(&*plugin, &mut setup, GOOD, 1)
        .err()
        .expect("a fatal error");
    assert_eq!(
        error,
        "stamp: `prefix` must be a string, got 5\nSet it in tsconfig.json under compilerOptions.plugins."
    );
}

#[test]
fn state_and_shared_persist_across_watch_rebuilds() {
    let plugin = erase(Stamp);
    let mut compiler = MockCompiler::new();
    let mut setup = MockSetup::clean(fixture());
    assert!(
        compiler
            .build(&*plugin, &mut setup, GOOD, 1)
            .unwrap()
            .committed
    );

    // a watch rebuild with no input change asks rebuild_start and keeps the Shared, even when it fails
    setup.watch(1);
    setup.origin = StateOrigin::Memory;
    let failed = compiler
        .build(&*plugin, &mut setup, &["bad.ts"], 1)
        .unwrap();
    assert!(!failed.setup_ran && !failed.committed);
    assert_eq!(compiler.rebuild_start_calls, 1);
    assert_eq!(
        builds(compiler.committed.as_ref().unwrap()),
        1,
        "a failed build commits nothing"
    );
    assert!(
        compiler.kept.is_some(),
        "a Shared kept from a committed build stays kept"
    );

    setup.watch(2);
    let second = compiler.build(&*plugin, &mut setup, GOOD, 1).unwrap();
    assert!(!second.setup_ran && second.setup_messages.is_empty());
    assert_eq!(builds(&second.state), 2);
    assert_eq!(
        second.finish.artifacts[0].1,
        b"{\"sites\":3,\"builds\":2}\n".to_vec()
    );

    // a changed build input runs setup again without asking rebuild_start
    setup.watch(3);
    setup.input_changed = true;
    let third = compiler.build(&*plugin, &mut setup, GOOD, 1).unwrap();
    assert!(third.setup_ran);
    assert_eq!(compiler.rebuild_start_calls, 2);
    assert_eq!(builds(&third.state), 3);
}

/// The blocking case: `-w` started on a project with an error. The first build's setup ran, its state was
/// dropped, so its `Shared` must not be kept either: the next build runs setup again on the committed state.
#[test]
fn a_shared_from_a_build_that_did_not_commit_is_dropped() {
    let plugin = erase(Stamp);
    let mut compiler = MockCompiler::new();
    let mut setup = MockSetup::clean(fixture());
    let first = compiler
        .build(&*plugin, &mut setup, &["bad.ts", "main.ts"], 1)
        .unwrap();
    assert!(first.setup_ran && !first.committed);
    assert!(compiler.kept.is_none() && compiler.committed.is_none());

    setup.watch(1);
    let second = compiler.build(&*plugin, &mut setup, GOOD, 1).unwrap();
    assert!(second.setup_ran, "setup runs again");
    assert_eq!(
        compiler.rebuild_start_calls, 0,
        "no kept Shared, so no rebuild_start"
    );
    assert_eq!(builds(&second.state), 1);
}

#[test]
fn an_incremental_build_runs_setup_and_reloads_persisted_state() {
    let plugin = erase(Stamp);
    let mut compiler = MockCompiler::new();
    let mut setup = MockSetup::clean(fixture());
    compiler.build(&*plugin, &mut setup, GOOD, 1).unwrap();

    // a new process: no committed state in memory, and never rebuild_start
    let mut compiler = MockCompiler {
        kept: compiler.kept,
        ..MockCompiler::new()
    };
    setup.kind = BuildKind::Incremental;
    setup
        .persisted
        .insert(PathBuf::from(".stamp-state"), b"41".to_vec());
    let out = compiler.build(&*plugin, &mut setup, GOOD, 1).unwrap();
    assert!(out.setup_ran);
    assert_eq!(compiler.rebuild_start_calls, 0);
    assert_eq!(builds(&out.state), 42);

    let mut compiler = MockCompiler::new();
    setup
        .persisted
        .insert(PathBuf::from(".stamp-state"), b"not a number".to_vec());
    let error = compiler.build(&*plugin, &mut setup, GOOD, 1).err().unwrap();
    assert_eq!(error, "stamp: corrupt state file");
}

#[test]
fn setup_cx_resolves_modules_from_a_path() {
    let mut setup = MockSetup::clean(fixture());
    let cx = SetupCx::new(&mut setup);
    let t = cx
        .resolve_module("@rbxts/t", "E:/project/src/index.ts")
        .expect("resolves");
    assert_eq!(t.package_name.as_deref(), Some("@rbxts/t"));
    assert!(
        cx.resolve_module("missing", "E:/project/src/index.ts")
            .is_none()
    );
    assert!(cx.transforms_all_files());
}

/// The typed API directly, without the erased layer: decorator and `new` calls are declined.
#[test]
fn decorators_and_new_are_declined() {
    let program = fixture();
    let (index, file) = source(program, "main.ts");
    let call = calls_of(file)[0];
    let instance = Stamp.instance(
        std::sync::Arc::new(support::stamp::StampShared {
            prefix: String::new(),
        }),
        rbxts_plugin::CheckerInfo { index: 0, count: 1 },
    );
    let mut backend = MockCx::new(program, index, file);
    let mut cx = Cx::new(&mut backend);
    for kind in [CallKind::Decorator, CallKind::New] {
        let site = CallSite {
            node: call,
            kind,
            callee: call.expression().unwrap(),
            args: &[],
            symbol: None,
            receiver: None,
            in_optional_chain: false,
            value_used: true,
        };
        assert_eq!(
            instance.call(&mut cx, &site, support::stamp::HERE),
            Ok(Claim::Decline)
        );
    }
    assert!(backend.diagnostics.is_empty());
}

/// `Cx` itself: capture scopes (closed on `Err` and on panic), aborts from primitives, `report` categories.
#[test]
fn cx_capture_and_abort_paths() {
    let program = fixture();
    let (index, file) = source(program, "bad.ts");
    let call = calls_of(file)[0];
    let (one, two) = (call.arguments()[0], call.arguments()[1]);
    let mut backend = MockCx::new(program, index, file);
    let mut cx = Cx::new(&mut backend);

    // statements emitted inside capture come back instead of reaching the enclosing scope
    let (value, statements) = cx
        .capture(|cx| {
            let v = cx.compile_expr(one)?;
            Ok(cx.push_to_var(v, "n"))
        })
        .unwrap();
    assert_eq!(statements.len(), 1);
    assert_eq!(rbxts_luau::render_expr(&value, &Default::default()), "_n");

    // a failing primitive aborts, and the scope is still closed
    let failed: Result<((), _), Abort> = cx.capture(|cx| {
        cx.push_to_var(rbxts_luau::create::nil(), "x");
        cx.compile_expr(call).map(|_| ())
    });
    assert!(failed.is_err());

    // a panic inside capture closes the scope before it continues
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = cx.capture(|cx| -> Result<(), Abort> {
            cx.push_to_var(rbxts_luau::create::nil(), "y");
            panic!("hook bug")
        });
    }));
    assert!(panicked.is_err());
    assert_eq!(cx.compile_in_order(&[one, two]).map(|v| v.len()), Ok(2));

    // a warning does not abort; an error does
    assert!(
        cx.report(rbxts_plugin::PluginDiagnostic::warning(call, "w"))
            .is_none()
    );
    assert!(
        cx.report(rbxts_plugin::PluginDiagnostic::error(call, "e"))
            .is_some()
    );
    let _abort: Abort = cx.error(call, "boom");
    assert_eq!(backend.scopes.len(), 1, "every capture scope was closed");
    assert!(
        backend.scopes[0].is_empty(),
        "nothing leaked into the enclosing scope"
    );
    assert_eq!(
        backend.errors(),
        vec![
            "internal: the mock compiles literals and identifiers only",
            "e",
            "boom"
        ]
    );
    assert_eq!(backend.warnings(), vec!["w"]);
}
