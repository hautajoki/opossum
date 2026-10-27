//! The compiler side of the plugin seam against the real transform: the `stamp` and `probe` test plugins
//! (`rbxts_test_plugins`) over `tests/plugin_fixtures/probe`, whose `expected/` Luau is hand-checked (plugin output
//! has no stock roblox-ts reference).
//!
//! Covered: calls by symbol, by receiver (plain and inside an optional chain) and by callee marker; `new` and
//! decorator claims; `compile_call` with `Original` / `Luau` / `Nil` slots and `Function` / `Method` callees;
//! `compile_in_order`, prereqs, comments, captures, hoists, root insertions and the header; `import_named` (new and
//! reused imports) and `request_import_elision` / `mark_import_used`; the class hook; the expression and statement
//! fallbacks; late strings; source facts (UTF-16 positions, `{@link}`, `declaring_package`, `resolve_module`); the
//! artifact and state-file commit; claim order and `RBXTS_PLUGIN_CLAIMS=all`; the phase-guard refusals; a
//! panicking hook; `Err` without an error; an unresolved late string; an artifact path outside the project;
//! `explain_checker_diagnostic`; and tsconfig activation. From S2: optional method calls and `super.m()` (with
//! `compile_receiver` binding a non-method optional call's receiver); `property_key` in every position; the
//! `assignment` hook with `compile_binary`; `compile_object_literal` (edits and a permuted order); `locals()`;
//! `hoist_spillable` and the main chunk's local limit; expression hooks on chain links and statement assignments,
//! asked once; and the misuse the compiler refuses. From the seam additions: computed keys (`AccessSite.key`
//! `None`), `JsDocLink.text`, `runtime_member`, `synthesized_string`, identical diagnostics kept once,
//! `insert_before_statement` in every statement position, `hoist_spillable_named` (spilled too), the `import_named`
//! `name_1` fallback and import order, `file_identifier_texts`, TS5 numeric literal text, the header comment above
//! plugin imports, a leading plugin comment above its statement's comments, and `ts5_properties_of_type`
//! (`seam*.ts`). From the seam's last round (`plugin_fixtures/seamrest`, with a Lune runtime check): optional and `super`
//! calls with their short-circuit and `self`, every link and call of a chain for the expression hooks, asked once,
//! block bodies, `else if`, static blocks and `declare` statements for the statement hooks, dotted JSX tags; a hoist of
//! what its statement declares refused; `classify_symbol` diagnostics the same for any checker count; and
//! `decline-all` and the identity plugins changing no output of the area fixtures.
//!
//! Like `tests/fixtures.rs`, the fixture is copied to a temporary directory with a `node_modules` junction
//! (`RBXTS_RS_FIXTURE_NODE_MODULES`, else the roblox-ts tests project's); without one the tests are skipped.

#![cfg(windows)]

use std::path::{Path, PathBuf};

use rbxts_luau_check::LUAU_VERSION;
use rbxts_plugin::erased::DynPlugin;
use rbxts_project::{ProjectOptionLayer, STACK_SIZE};
use rbxts_transform::{CompileOptions, CompileReport, PluginOptions, compile_project_with};
use tsrs_compiler::diagnosticwriter::flatten_diagnostic_message;
use tsrs_diagnostics::Category;

fn node_modules() -> Option<PathBuf> {
    let path = rbxts_test_support::node_modules().unwrap_or_default();
    path.join("@rbxts").join("compiler-types").is_dir().then_some(path)
}

fn fixture_dir() -> PathBuf {
    fixture_dir_of("probe")
}

fn fixture_dir_of(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plugin_fixtures").join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the fixture in a temporary directory, removed on drop (the junction with `rmdir`, never followed).
struct Project {
    root: PathBuf,
}

impl Project {
    /// `None` (the test is skipped) without a `node_modules` to link.
    fn new(label: &str) -> Option<Project> {
        Project::of("probe", label)
    }

    /// A copy of the fixture `plugin_fixtures/<fixture>`.
    fn of(fixture: &str, label: &str) -> Option<Project> {
        Project::copy_of(&fixture_dir_of(fixture), &["src", "tsconfig.json", "default.project.json", "package.json"], label)
    }

    /// A copy of the entries `names` of the fixture at `fixture`, with a `node_modules` junction.
    fn copy_of(fixture: &Path, names: &[&str], label: &str) -> Option<Project> {
        let Some(node_modules) = node_modules() else {
            rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
            return None;
        };
        let root = std::env::temp_dir().join(format!("rbxts-rs-plugins-{label}-{}", std::process::id()));
        let project = Project { root };
        project.clean();
        std::fs::create_dir_all(&project.root).unwrap();
        for &name in names {
            let from = fixture.join(name);
            if from.is_dir() {
                copy_dir(&from, &project.root.join(name));
            } else {
                std::fs::copy(&from, project.root.join(name)).unwrap();
            }
        }
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(project.root.join("node_modules"))
            .arg(node_modules.to_string_lossy().replace('/', "\\"))
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "could not link node_modules");
        Some(project)
    }

    fn clean(&self) {
        let link = self.root.join("node_modules");
        if link.exists() {
            let _ = std::process::Command::new("cmd").args(["/C", "rmdir"]).arg(&link).status();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }

    fn write(&self, relative: &str, text: &str) {
        std::fs::write(self.root.join(relative), text).unwrap();
    }

    /// Builds the copy with the named test plugins (in registration order).
    fn build(&self, plugins: &[&str], configure: impl FnOnce(&mut PluginOptions<'_>)) -> CompileReport {
        self.build_with(plugins, None, configure)
    }

    /// [`Project::build`] with a checker count (`None`: the default).
    fn build_with(
        &self,
        plugins: &[&str],
        checkers: Option<usize>,
        configure: impl FnOnce(&mut PluginOptions<'_>),
    ) -> CompileReport {
        let plugins: Vec<Box<dyn DynPlugin>> =
            plugins.iter().map(|name| rbxts_test_plugins::by_name(name).expect("a test plugin")).collect();
        let cwd = self.root.to_string_lossy().into_owned();
        let mut options = PluginOptions { plugins: &plugins, ..PluginOptions::default() };
        configure(&mut options);
        let ignore = options.ignore_unknown_tsconfig_plugins;
        let claims_all = options.claims_all;
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(STACK_SIZE)
                .spawn_scoped(scope, move || {
                    let options =
                        PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: ignore, claims_all };
                    compile_project_with(
                        &CompileOptions {
                            tsconfig: "tsconfig.json".into(),
                            cwd,
                            project_options: ProjectOptionLayer::default(),
                            out_dir_override: None,
                            write_partial: false,
                            no_write: false,
                            checkers,
                        },
                        &options,
                    )
                })
                .unwrap()
                .join()
                .unwrap()
        })
    }

    fn out_exists(&self) -> bool {
        self.root.join("out").exists()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        self.clean();
    }
}

/// Every diagnostic of the build, as `error|warning <file>: <message>`.
fn diagnostics(report: &CompileReport) -> Vec<String> {
    let mut all = report.diagnostics.clone();
    for file in &report.files {
        all.extend(file.diagnostics.iter().copied());
    }
    all.iter()
        .map(|d| {
            let category = if d.category() == Category::Error { "error" } else { "warning" };
            let file = d.file().map(|f| f.file_name().rsplit('/').next().unwrap_or_default().to_owned());
            format!("{category} {}: {}", file.unwrap_or_default(), flatten_diagnostic_message(*d, "\n"))
        })
        .collect()
}

fn errors(report: &CompileReport) -> Vec<String> {
    diagnostics(report).into_iter().filter(|d| d.starts_with("error")).collect()
}

/// Compares the build's Luau with `expected/`.
fn assert_expected(report: &CompileReport) {
    assert_expected_except(report, &[]);
}

/// Compares the build's Luau with `expected/`, but for the files in `except`.
fn assert_expected_except(report: &CompileReport, except: &[&str]) {
    assert_expected_of("probe", report, except);
}

/// Compares the build's Luau with `plugin_fixtures/<fixture>/expected/`, but for the files in `except`.
fn assert_expected_of(fixture: &str, report: &CompileReport, except: &[&str]) {
    let mut failures = Vec::new();
    let expected_dir = fixture_dir_of(fixture).join("expected");
    for file in &report.files {
        let relative = Path::new(&file.out_path)
            .strip_prefix(&report.out_dir)
            .unwrap_or(Path::new(&file.out_path))
            .to_string_lossy()
            .replace('\\', "/");
        if except.contains(&relative.as_str()) {
            continue;
        }
        let Some(luau) = &file.luau else {
            failures.push(format!("{relative}: no output"));
            continue;
        };
        // for reviewing new expectations: the build's Luau, written where this names
        if let Some(dump) = std::env::var_os("RBXTS_RS_PLUGIN_FIXTURE_DUMP") {
            let path = Path::new(&dump).join(&relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, luau).unwrap();
        }
        let expected = match std::fs::read_to_string(expected_dir.join(&relative)) {
            Ok(text) => text.replace("\r\n", "\n"),
            Err(e) => {
                failures.push(format!("{relative}: {e}"));
                continue;
            }
        };
        if *luau != expected {
            failures.push(format!("{relative}: differs from expected\n--- ours\n{luau}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn probe_and_stamp_match_the_expected_luau() {
    let Some(project) = Project::new("expected") else { return };
    // probe-exprcall: an expression-kind hook on `probeExpr…` calls, which the call hooks still claim inside
    let report = project.build(&["stamp", "probe", "probe-exprcall"], |_| {});
    assert!(report.fatal.is_empty(), "{:?}", report.fatal);
    assert_eq!(errors(&report), Vec::<String>::new());
    assert_eq!(report.plugins, vec!["stamp", "probe", "probe-exprcall"]);
    // the unknown @stamp mode is a warning from the hook, per call
    assert_eq!(
        diagnostics(&report),
        vec![
            "warning calls.ts: `mystery` has an unknown @stamp mode; the call is left as it is",
            // asked once: the call hook compiling the call itself does not ask the expression hook again
            "warning s2chains.ts: probe-exprcall: asked about probeAskOnce",
            // a plugin warning identical to roblox-ts's own at the same node, and one at a node in another file
            // that two files report: each once in the build
            "warning seamdiag.ts: Value will be checked against 0, NaN",
            "warning probe.d.ts: probe: at the declaration, once in the build",
            // reported twice at the same node, then as a message and with related information: kept once
            "warning seamnames.ts: probe: reported twice, kept once",
            // with a hint, and the same text without one: kept once
            "warning seamnames.ts: probe: with a hint\nthe hint",
            // with another source: kept
            "warning seamnames.ts: probe: reported twice, kept once",
        ]
    );
    assert_expected(&report);
    // every output was written, then the artifacts and the state file
    assert!(report.files.iter().all(|f| f.written));
    let read = |name: &str| std::fs::read_to_string(project.root.join(name)).unwrap();
    assert_eq!(read("stamp-report.json"), "{\"sites\":1,\"builds\":1}\n");
    assert_eq!(read(".stamp-state"), "1");
    assert_eq!(
        read("probe-report.json"),
        "[{\"file\":\"src/shared/approved.ts\",\"claims\":4},{\"file\":\"src/shared/approvedAfter.ts\",\"claims\":2},\
         {\"file\":\"src/shared/calls.ts\",\"claims\":10},{\"file\":\"src/shared/capture.ts\",\"claims\":2},\
         {\"file\":\"src/shared/classes.ts\",\"claims\":2},{\"file\":\"src/shared/decorators.ts\",\"claims\":2},\
         {\"file\":\"src/shared/elide.ts\",\"claims\":3},{\"file\":\"src/shared/elideAll.ts\",\"claims\":1},\
         {\"file\":\"src/shared/elideKept.ts\",\"claims\":1},{\"file\":\"src/shared/exprcall.ts\",\"claims\":3},{\"file\":\"src/shared/facts.ts\",\"claims\":1},\
         {\"file\":\"src/shared/fallbacks.ts\",\"claims\":2},{\"file\":\"src/shared/inorder.ts\",\"claims\":5},{\"file\":\"src/shared/receivers.ts\",\"claims\":3},\
         {\"file\":\"src/shared/reuse.ts\",\"claims\":1},{\"file\":\"src/shared/shadow.ts\",\"claims\":3},\
         {\"file\":\"src/shared/s2assign.ts\",\"claims\":13},\
         {\"file\":\"src/shared/s2calls.ts\",\"claims\":11},{\"file\":\"src/shared/s2chains.ts\",\"claims\":1},\
         {\"file\":\"src/shared/s2locals.ts\",\"claims\":4},{\"file\":\"src/shared/s2objects.ts\",\"claims\":4},\
         {\"file\":\"src/shared/s2spill.ts\",\"claims\":7},{\"file\":\"src/shared/seambefore.ts\",\"claims\":16},\
         {\"file\":\"src/shared/seamdiag.ts\",\"claims\":2},{\"file\":\"src/shared/seamdiag2.ts\",\"claims\":1},\
         {\"file\":\"src/shared/seamimports.ts\",\"claims\":5},\
         {\"file\":\"src/shared/seamkeys.ts\",\"claims\":3},{\"file\":\"src/shared/seamnames.ts\",\"claims\":7},\
         {\"file\":\"src/shared/seamorder.ts\",\"claims\":2},{\"file\":\"src/shared/selfcalls.ts\",\"claims\":5}]\n"
    );
    assert_eq!(
        read("probe-exprcall-report.json"),
        "[{\"file\":\"src/shared/exprcall.ts\",\"claims\":3},{\"file\":\"src/shared/s2chains.ts\",\"claims\":2}]\n"
    );
    assert_eq!(report.artifacts_written.len(), 4, "{:?}", report.artifacts_written);
    // a second build changes no Luau and no artifact (write if changed); the stamp state counts the clean build anew
    let again = project.build(&["stamp", "probe", "probe-exprcall"], |_| {});
    assert!(again.files.iter().all(|f| !f.written));
    assert!(again.artifacts_written.is_empty(), "{:?}", again.artifacts_written);
    // setup's env read is a build input
    assert!(
        report
            .build_inputs
            .iter()
            .any(|i| matches!(i, rbxts_transform::plugins::BuildInput::Env { name, .. } if name == "STAMP_PREFIX"))
    );
    // hook timings were recorded per plugin
    let probe = report.plugin_timings.iter().find(|t| t.name == "probe").unwrap();
    let calls = probe.hooks.iter().find(|(k, _)| k.label() == "call").unwrap().1.calls;
    assert!(calls >= 23, "{calls}");
}

#[test]
fn the_first_plugin_to_take_a_call_wins_and_double_claims_are_reported_on_request() {
    let Some(project) = Project::new("order") else { return };
    let report = project.build(&["stamp", "probe", "probe-b"], |_| {});
    assert_eq!(errors(&report), Vec::<String>::new());
    // `probe` takes every node it is asked about first; `probe-b` gets only what `probe` declined (nothing here)
    assert_expected_except(
        &report,
        &["shared/classes.luau", "shared/decorators.luau", "shared/exprcall.luau", "shared/s2chains.luau"],
    );
    let read = |name: &str| std::fs::read_to_string(project.root.join(name)).unwrap();
    assert_eq!(
        read("probe-b-report.json"),
        "[]
"
    );
    // the class hook is broadcast: both plugins' statements, in registration order, before the decorators
    let classes = report.files.iter().find(|f| f.file_name.ends_with("classes.ts")).unwrap().luau.as_deref().unwrap();
    assert!(
        classes.contains(
            "	Decorated.__probed = \"probe\"
	Decorated.__probed = \"probe-b\"
	Decorated = probeDecorator(\"probed\")"
        ),
        "{classes}"
    );

    let report = project.build(&["stamp", "probe", "probe-b"], |o| o.claims_all = true);
    let doubles: Vec<String> = errors(&report).into_iter().filter(|e| e.contains("both claimed")).collect();
    assert!(!doubles.is_empty(), "{:?}", errors(&report));
    assert!(
        doubles
            .iter()
            .all(|e| e.ends_with("plugins `probe` and `probe-b` both claimed this call (RBXTS_PLUGIN_CLAIMS=all)"))
    );
}

#[test]
fn hook_failures_fail_the_build_and_write_nothing() {
    let Some(project) = Project::new("failures") else { return };
    let cases: &[(&str, &[&str])] = &[
        ("probe-fail", &["error failures.ts: probe: fail was called"]),
        (
            "probe-err",
            &[
                "error failures.ts: probe: the first err",
                "error failures.ts: rbxts-rs internal error: plugin `probe` returned Err from call without reporting an error",
            ],
        ),
        ("probe-panic", &["error failures.ts: rbxts-rs internal error: plugin `probe` panicked in call: probe: boom"]),
    ];
    for (plugin, expected) in cases {
        let report = project.build(&["stamp", plugin], |_| {});
        assert_eq!(errors(&report), *expected, "{plugin}");
        assert!(!project.out_exists(), "{plugin}: nothing is written");
        assert!(!project.root.join("probe-report.json").exists(), "{plugin}: no artifact");
        if *plugin == "probe-panic" {
            // the checker may be inconsistent after a panic: its group stops, and the report is cut at the first panic
            // in transform order, so any checker count reports what one checker does
            let names = |r: &rbxts_transform::CompileReport| -> Vec<String> { r.files.iter().map(|f| f.file_name.clone()).collect() };
            assert_eq!(report.files.last().unwrap().file_name.rsplit('/').next(), Some("failures.ts"));
            assert_eq!(report.fatal.len(), 1, "{:?}", report.fatal);
            assert!(report.fatal[0].ends_with("failures.ts; later files were not transformed"), "{:?}", report.fatal);
            let one = project.build_with(&["stamp", plugin], Some(1), |_| {});
            assert_eq!(errors(&one), *expected, "{plugin}, one checker");
            assert_eq!(names(&one), names(&report), "{plugin}: the same files for one checker and for several");
            assert_eq!(one.fatal, report.fatal, "{plugin}: the same fatal text for one checker and for several");
            assert!(!project.out_exists(), "{plugin}, one checker: nothing is written");
        }
    }
}

#[test]
fn refused_primitives_are_internal_errors_naming_the_plugin() {
    let Some(project) = Project::new("refuse") else { return };
    let report = project.build(&["stamp", "probe-refuse"], |_| {});
    let errors = errors(&report);
    for expected in [
        "error calls.ts: rbxts-rs internal error: plugin `probe` called prereq in begin_file",
        "error calls.ts: rbxts-rs internal error: plugin `probe` called hoist in end_file",
    ] {
        assert!(errors.iter().any(|e| e == expected), "{expected}\nin {errors:?}");
    }
    // classify_symbol's diagnostics are anchored at the symbol's declaration, not at the first site that asked
    // (which moves with the checker count and file order): `probeHoist` is declared in probe.d.ts
    let classify: Vec<&String> = errors.iter().filter(|e| e.contains("in classify_symbol")).collect();
    assert!(
        classify
            .iter()
            .any(|e| *e
                == "error probe.d.ts: rbxts-rs internal error: plugin `probe` called late_string in classify_symbol"),
        "{classify:?}"
    );
    assert!(classify.iter().all(|e| !e.starts_with("error failures.ts")), "{classify:?}");
    assert!(!project.out_exists());
    // each checker asks about a symbol once, from whichever of its files reaches it first: the build reports each of
    // these once, the same for any checker count (every checker used to report its own)
    let classify_at = |checkers: usize| -> Vec<String> {
        let report = project.build_with(&["stamp", "probe-refuse"], Some(checkers), |_| {});
        report
            .diagnostics
            .iter()
            .filter(|d| flatten_diagnostic_message(**d, "\n").contains("in classify_symbol"))
            .map(|d| format!("{}:{} {}", d.file().unwrap().file_name(), d.pos().unwrap(), d.localize()))
            .collect()
    };
    let one = classify_at(1);
    assert!(one.len() > 1, "{one:?}");
    let mut unique = one.clone();
    unique.dedup();
    assert_eq!(unique, one, "reported once each, sorted");
    for checkers in [2, 4, 8] {
        assert_eq!(classify_at(checkers), one, "{checkers} checkers");
    }
    // none of them is in a file's own list
    let report = project.build_with(&["stamp", "probe-refuse"], Some(4), |_| {});
    assert!(
        report.files.iter().flat_map(|f| &f.diagnostics).all(|d| !d.localize().contains("in classify_symbol")),
        "classify_symbol's diagnostics are the build's"
    );
}

#[test]
fn finish_errors_fail_the_build() {
    let Some(project) = Project::new("finish") else { return };
    let report = project.build(&["stamp", "probe-unresolved"], |_| {});
    assert_eq!(
        report.fatal,
        vec!["plugin `probe` left the late string `src/shared/elide.ts:0` unresolved (used in src/shared/elide.ts)"]
    );
    assert!(!project.out_exists());

    let report = project.build(&["stamp", "probe-artifact-outside"], |_| {});
    assert_eq!(report.fatal.len(), 1, "{:?}", report.fatal);
    assert!(report.fatal[0].starts_with("plugin `probe`: artifact "), "{:?}", report.fatal);
    assert!(report.fatal[0].ends_with("is outside the project directory, the outDir and the include path"));
    assert!(!project.out_exists());
    assert!(!project.root.join("probe-report.json").exists());
}

#[test]
fn declining_after_lowering_is_an_error_and_a_call_is_offered_once() {
    let Some(project) = Project::new("decline") else { return };
    project.write(
        "src/shared/declines.ts",
        "import { helper } from \"./macros\";\nexport const a = probeDeclineAfter(1);\nexport const b = probeHoistDecline();\nexport const c = probePrereqDecline();\n\
         export const d = probeArgDecline(probeHoist(sideEffect()));\nexport const e = void probeVoidHoist;\n\
         export const f = probeArgDecline(probeSelf(1));\n\
         export const g = capMap[probePrereq(tostring(sideEffect()))].cr(1);\n\
         export const h = void void probeVoidSelf;\nexport const i = probeLateDecline();\n\
         export const j = probeMarkDecline(helper);\nexport const k = probeTwice(1);\nprobeStmtSelf;\n",
    );
    let report = project.build(&["stamp", "probe", "probe-exprcall"], |_| {});
    let all = diagnostics(&report);
    let declines: Vec<&String> = all.iter().filter(|d| d.starts_with("error declines.ts")).collect();
    assert_eq!(
        declines,
        vec![
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after lowering it with compile_expr",
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after calling hoist",
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after emitting prerequisites (they would run twice)",
            // the hoist of the nested probeHoist claim, made while the argument was compiled, would be made twice
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after leaving statements before \
             the root statement behind (a hoist or an insertion, its own or a nested hook's)",
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined an expression after calling hoist",
            // the nested probeSelf claim would run again
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after a node inside it was \
             offered to a plugin hook (it would be offered again)",
            // the receiver's prereqs were captured and dropped: it is not reused
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after compiling the receiver \
             inside a capture",
            // the inner `void probeVoidSelf` hook ran inside the outer one: its frame does not reset the outer's
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined an expression after lowering it with \
             compile_expr",
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after calling late_string",
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after calling mark_import_used",
            "error declines.ts: rbxts-rs internal error: plugin `probe`: compiled its own call a second time (it would be \
             lowered twice)",
            "error declines.ts: rbxts-rs internal error: plugin `probe-exprcall`: a statement hook cannot compile its own \
             statement (compile_* takes expressions; lower the statement's parts)",
        ]
    );
    // the hook that compiled the call itself and declined ran once: the call is not offered again
    assert_eq!(all.iter().filter(|d| d.ends_with("probe: declineafter was offered")).count(), 1, "{all:?}");
    assert!(!project.out_exists());
}

#[test]
fn an_erroneous_decline_ends_the_offer() {
    let Some(project) = Project::new("decline-ends") else { return };
    project.write("src/shared/declines.ts", "export const a = probeDeclineAfter(1);\n");
    let report = project.build(&["stamp", "probe", "probe-b"], |_| {});
    let all = diagnostics(&report);
    // probe declined after lowering the call: an error, and probe-b is not asked (it would warn too)
    assert_eq!(all.iter().filter(|d| d.ends_with("probe: declineafter was offered")).count(), 1, "{all:?}");
    assert_eq!(
        errors(&report),
        vec![
            "error declines.ts: rbxts-rs internal error: plugin `probe` declined a call after lowering it with compile_expr"
        ]
    );
}

#[test]
fn claims_all_never_changes_the_output() {
    let Some(project) = Project::new("claims-all") else { return };
    // without the switch: probe-touched is only asked where probe declined, and changes nothing
    let report = project.build(&["stamp", "probe", "probe-touched"], |_| {});
    assert_eq!(errors(&report), Vec::<String>::new());
    assert_expected_except(&report, &["shared/exprcall.luau", "shared/s2chains.luau"]);
    // with it: probe-touched is asked about calls probe took, and its hook changes compiler state before declining,
    // so the build fails instead of writing different output
    std::fs::remove_dir_all(project.root.join("out")).unwrap();
    let report = project.build(&["stamp", "probe", "probe-touched"], |o| o.claims_all = true);
    let touched: Vec<String> = errors(&report).into_iter().filter(|e| e.contains("changed compiler state")).collect();
    assert!(!touched.is_empty(), "{:?}", errors(&report));
    assert!(touched.iter().all(|e| e.ends_with(
        "plugin `probe-touched` changed compiler state when asked about a call plugin `probe` claimed \
         (RBXTS_PLUGIN_CLAIMS=all fails such a build rather than change its output)"
    )));
    assert!(!project.out_exists());
}

#[test]
fn seam_misuse_is_refused_naming_the_plugin() {
    let Some(project) = Project::new("misuse") else { return };
    project.write(
        "src/shared/misuse.ts",
        "export const a = probeCallTwice(1);\n\
         export const b = probeExprThenCall(2);\n\
         export const c = 1 + probeAncestor();\n\
         export const d = probeBinBad(1, 2);\n\
         export class R extends SelfBase {\n\
         \tr(v: number) {\n\
         \t\treturn super.r(v);\n\
         \t}\n\
         }\n\
         export const e = keyedList[probeTakeComputed];\n\
         interface Reads {\n\
         \tcount: number;\n\
         \topt?: number;\n\
         }\n\
         export class C {\n\
         \t/** @probeattrsread */\n\
         \treads: Reads = { count: 0 };\n\
         \trun(i: string) {\n\
         \t\tthis.reads.count *= 2;\n\
         \t\tdelete this.reads.opt;\n\
         \t\t(this.reads as unknown as Record<string, number>)[i] = 1;\n\
         \t}\n\
         }\n\
         export const f = maybeRe?.re(1);\n\
         export const g = plainRe.re(2);\n",
    );
    let report = project.build(&["stamp", "probe"], |_| {});
    let prefix = "error misuse.ts: rbxts-rs internal error: plugin `probe`";
    assert_eq!(
        errors(&report),
        vec![
            format!("{prefix}: compile_call ran a second time on its call (it would be lowered twice)"),
            format!("{prefix}: compile_call after the call was lowered with compile_expr (it would be lowered twice)"),
            format!(
                "{prefix}: compiled an ancestor of its own node (everything else inside it would be lowered a second \
                 time)"
            ),
            format!(
                "{prefix}: compile_binary: PlusEqualsToken is an assignment operator; pass the operator it applies \
                 (`+=` is PlusToken)"
            ),
            format!(
                "{prefix}: compile_receiver: `super` is not a value (roblox-ts calls `super.m(self, …)`); lower the \
                 call with compile_expr or compile_call, or pass `self` yourself"
            ),
            // a computed key has no text to rewrite
            "error misuse.ts: rbxts-rs internal error: plugin `probe` took a property key whose key is computed \
             (AccessSite.key is None): such a site can only be declined or failed"
                .to_owned(),
            format!(
                "{prefix}: compiled the operand of `delete`, which roblox-ts lowers as the deletion (it would delete \
                 it, not read it)"
            ),
            // a computed key is offered to the assignment hook (`AssignSite.key` `None`); the probe refuses this target
            "error misuse.ts: probe: an attribute assignment needs `this.attrs.key`".to_owned(),
            // after `?.` the receiver is compiled already (`plainRe.re(2)`, offered before it is compiled, is fine)
            format!(
                "{prefix}: compiled the receiver or callee of its node, which is compiled already (it would be \
                 evaluated a second time): use compile_receiver, or compile_expr of the node itself"
            ),
        ]
    );
    assert!(!project.out_exists());

    // an expression hook offered a call inside a chain cannot compile the call's base again
    std::fs::remove_file(project.root.join("src/shared/misuse.ts")).unwrap();
    project.write("src/shared/eager.ts", "export const h = maybeRe?.probeExprBase(1).value;\n");
    let report = project.build(&["stamp", "probe", "probe-exprcall"], |_| {});
    assert_eq!(
        errors(&report),
        vec![
            "error eager.ts: rbxts-rs internal error: plugin `probe-exprcall`: compiled the receiver or callee of its \
             node, which is compiled already (it would be evaluated a second time): use compile_receiver, or \
             compile_expr of the node itself"
        ]
    );
    std::fs::remove_file(project.root.join("src/shared/eager.ts")).unwrap();

    // a class hook compiling its own class expression would lower it twice
    project.write("src/shared/classself.ts", "export const K = class Named {};\n");
    let report = project.build(&["stamp", "probe-classself"], |_| {});
    // (without the probe's statement hook, `debugger` in fallbacks.ts is roblox-ts's error)
    let errors: Vec<String> = errors(&report).into_iter().filter(|e| e.contains("classself.ts")).collect();
    assert_eq!(
        errors,
        vec![
            "error classself.ts: rbxts-rs internal error: plugin `probe`: a class hook cannot compile its own class \
             (roblox-ts lowers it already; it would be lowered twice)"
        ]
    );
    assert!(!project.out_exists());
}

/// A hoist is declared before the top-level statement around the hook's node: a value that uses something that
/// statement declares (a temporary of its lowering, a parameter, a local, `self`) is refused, naming the plugin;
/// values that use only what is declared before it (earlier statements, other hoists, plugin imports) are not.
#[test]
fn a_hoist_of_what_its_statement_declares_is_refused() {
    let Some(project) = Project::new("hoists") else { return };
    project.write(
        "src/shared/hoists.ts",
        "const top = mk();\n\
         export const fine1 = probeHoist(top);\n\
         export const fine2 = probeHoist([probeHoist(sideEffect())]);\n\
         export const fine3 = probeSpill(() => { const inner = mk(); return nx ?? inner; });\n\
         export const bad1 = probeHoist(nx ?? sideEffect());\n\
         export function bad2(p: number) {\n\
         \treturn probeSpill(p);\n\
         }\n\
         export class Bad3 {\n\
         \tm() {\n\
         \t\treturn probeNamed(this);\n\
         \t}\n\
         }\n\
         for (const i of [1, 2]) {\n\
         \tprint(probeHoist([i]));\n\
         }\n\
         export const bad5 = (() => { const loc = mk(); return probeHoist(loc); })();\n\
         export const bad6 = probeSpill(() => later());\n\
         export const bad7 = (() => {\n\
         \tconst q = mk();\n\
         \treturn probeSpill([(() => { const q = 2; return q; })(), q]);\n\
         })();\n\
         export const bad8 = probeSpill(() => ambientLater());\n\
         export const bad9 = probeHoist(other);\n\
         function later() { return 1; }\n\
         declare function ambientLater(): number;\n\
         import { other } from \"./runtime\";\n",
    );
    let report = project.build(&["stamp", "probe"], |_| {});
    let prefix = "error hoists.ts: rbxts-rs internal error: plugin `probe`:";
    let tail = "; a hoist is declared before the top-level statement around its node, where it would not read the \
                same (hoist only values that need nothing that statement declares)";
    let found: Vec<String> = errors(&report).into_iter().filter(|e| e.contains("hoists.ts")).collect();
    assert_eq!(
        found,
        vec![
            format!(
                "{prefix} hoist: the value uses a temporary declared in the current statement (a prerequisite's, or \
                 one roblox-ts made while lowering it){tail}"
            ),
            format!(
                "{prefix} hoist_spillable: the value uses `p`, which is declared in the current statement (a parameter \
                 or a local of the code around its node){tail}"
            ),
            format!(
                "{prefix} hoist_spillable_named: the value uses `self`, which is a local of the function or class \
                 around its node{tail}"
            ),
            format!(
                "{prefix} hoist: the value uses `i`, which is declared in the current statement (a parameter or a \
                 local of the code around its node){tail}"
            ),
            format!(
                "{prefix} hoist: the value uses `loc`, which is declared in the current statement (a parameter or a \
                 local of the code around its node){tail}"
            ),
            // a function the file declares further down: the hoist would read a global
            format!(
                "{prefix} hoist_spillable: the value uses `later`, which a later statement of the file declares (a \
                 function, class, enum or variable declared after the statement around its node){tail}"
            ),
            // the inner `q` is the nested function's own: the outer one is still the statement's
            format!(
                "{prefix} hoist_spillable: the value uses `q`, which is declared in the current statement (a \
                 parameter or a local of the code around its node){tail}"
            ),
            // an ambient function declared further down: roblox-ts declares `local ambientLater` before the statement
            // that uses it, after the hoist
            format!(
                "{prefix} hoist_spillable: the value uses `ambientLater`, which a later statement of the file declares \
                 (a function, class, enum or variable declared after the statement around its node){tail}"
            ),
            // an import below the use
            format!(
                "{prefix} hoist: the value uses `other`, which a later statement of the file declares (a function, \
                 class, enum or variable declared after the statement around its node){tail}"
            ),
        ]
    );
    assert!(!project.out_exists());

    // the values that use only what is declared before the statement are hoisted
    project.write(
        "src/shared/hoists.ts",
        "const top = mk();\n\
         export const fine1 = probeHoist(top);\n\
         export const fine2 = probeHoist([probeHoist(sideEffect())]);\n\
         export const fine3 = probeSpill(() => { const inner = mk(); return nx ?? inner; });\n\
         export const fine4 = probeSpill(() => early());\n",
    );
    // a function declared further up
    let early = "function early() { return 1; }\n";
    let source = std::fs::read_to_string(project.root.join("src/shared/hoists.ts")).unwrap();
    project.write("src/shared/hoists.ts", &format!("{early}{source}"));
    let report = project.build(&["stamp", "probe"], |_| {});
    assert_eq!(errors(&report), Vec::<String>::new());
    let file = report.files.iter().find(|f| f.file_name.ends_with("hoists.ts")).unwrap();
    assert_eq!(
        file.luau.as_deref().unwrap(),
        "-- Compiled with roblox-ts v3.0.0\nlocal function early()\n\treturn 1\nend\n\
         local top = mk()\nlocal _hoisted = top\nlocal fine1 = _hoisted\n\
         local _hoisted_1 = sideEffect()\nlocal _hoisted_2 = { _hoisted_1 }\nlocal fine2 = _hoisted_2\n\
         local _spilled = function()\n\tlocal inner = mk()\n\tlocal _condition = nx\n\tif _condition == nil then\n\
         \t\t_condition = inner\n\tend\n\treturn _condition\nend\nlocal fine3 = _spilled\n\
         local _spilled_1 = function()\n\treturn early()\nend\nlocal fine4 = _spilled_1\nreturn {\n\tfine1 = fine1,\n\
         \tfine2 = fine2,\n\tfine3 = fine3,\n\tfine4 = fine4,\n}\n"
    );

    // only a value declaration (or an import) counts, as in roblox-ts's `checkIdentifierHoist`: an interface, type
    // alias or global augmentation of the same name further down is not one
    project.write(
        "src/shared/hoists.ts",
        "const Foo = { a: 1 };\n\
         export const merged1 = probeHoist(Foo);\n\
         interface Foo { a: number }\n\
         class C2 {}\n\
         export const merged2 = probeHoist(C2);\n\
         interface C2 { x?: number }\n\
         const T4 = 1;\n\
         export const merged3 = probeHoist(T4);\n\
         type T4 = number;\n\
         export const merged4 = probeHoist(Vector3);\n\
         declare global { interface Vector3 { probeExtra?: number } }\n",
    );
    let report = project.build(&["stamp", "probe"], |_| {});
    assert_eq!(errors(&report), Vec::<String>::new());
}

#[test]
fn luau_decides_every_file_plugins_emitted_into() {
    let Some(project) = Project::new("locals") else { return };
    let lines = |count: usize, line: &dyn Fn(usize) -> String| (1..=count).map(|i| line(i) + "\n").collect::<String>();
    let tables = |count: usize| lines(count, &|i| format!("export const v{i} = [{i}];"));
    let build = |source: &str| {
        project.write("src/shared/big.ts", source);
        let report = project.build(&["stamp", "probe"], |_| {});
        let _ = std::fs::remove_dir_all(project.root.join("out"));
        report
    };
    let big = |report: &CompileReport| -> Vec<String> {
        diagnostics(report).into_iter().filter(|d| d.contains(" big.ts: ")).collect()
    };
    // Luau refuses the file at optimisation level 1, debug level 1 (`level` names a directive's level instead)
    let fails_at = |report: &CompileReport, level: &str, refused: &str| {
        let all = big(report);
        assert_eq!(all.len(), 1, "{all:?}");
        let start = format!("error big.ts: Luau {LUAU_VERSION} refuses this file at {level} and debug level 1, ");
        assert!(all[0].starts_with(&start) && all[0].contains(&format!(": \"{refused}")), "{all:?}");
        assert!(all[0].ends_with("It would build, then fail to load."), "{all:?}");
    };
    let fails = |report: &CompileReport| fails_at(report, "optimisation level 1", "Out of local registers");
    let folding_only = |report: &CompileReport| {
        let all = big(report);
        assert_eq!(all.len(), 1, "{all:?}");
        let start = format!("warning big.ts: This file loads only with Luau's constant folding: Luau {LUAU_VERSION} ");
        assert!(all[0].starts_with(&start), "{all:?}");
        assert!(errors(report).is_empty(), "{:?}", errors(report));
    };
    let at_line = |report: &CompileReport, line: &str| {
        let file = report.files.iter().find(|f| f.file_name.ends_with("big.ts")).unwrap();
        let start = file.diagnostics[0].pos().expect("a position") as usize;
        let text = std::fs::read_to_string(project.root.join("src/shared/big.ts")).unwrap();
        assert!(text[start..].trim_start().starts_with(line), "{}", &text[start..]);
    };

    // 205 tables and 2 spillable hoists: past the limit after spilling too, at the statement Luau names
    let report = build(&(tables(205) + "export const s1 = probeSpill(v1);\nexport const s2 = probeSpill(v2);\n"));
    fails(&report);
    at_line(&report, "export const v201 = [201];");
    assert!(!project.out_exists());

    // 197 tables, a hoist, an import and its use: 200, and `local TS` (added after the transform) makes 201
    let report = build(
        &("import { impl } from \"./runtime\";\n".to_owned()
            + &tables(197)
            + "print(probeHoist([0]));\nexport const u = impl(\"\", 0);\n"),
    );
    fails(&report);

    // `local la, lb = 1, mk()` takes two
    let report = build(&(tables(198) + "const [la, lb] = [1, mk()];\nprint(la, lb, probeHoist([0]));\n"));
    fails(&report);

    // locals a plugin's prereqs add: one `compile_call` binds (`nx ?? …`), one inside a `do` block, two at once
    fails(&build(&(tables(200) + "probeInject(nx ?? sideEffect());\n")));
    fails(&build(&(tables(200) + "probeDoPrereq();\n")));
    fails(&build(&(tables(199) + "probeMultiPrereq();\n")));
    // a function a plugin emitted, past the limit in its own body: at the statement that holds it
    let report = build("print(1);\nprobeFnPrereq();\nexport {};\n");
    fails(&report);
    at_line(&report, "probeFnPrereq();");

    // constants: Luau folds them (at debug level 1), so 205 of them and a hoist load only with its folding
    folding_only(&build(
        &(lines(205, &|i| format!("export const c{i} = -{i} * 2 + 1;")) + "print(probeHoist([0]));\n"),
    ));
    // … but not under --!optimize 0
    let report = build(
        &("//!optimize 0\n".to_owned()
            + &lines(205, &|i| format!("export const c{i} = {i};"))
            + "print(probeHoist([0]));\n"),
    );
    fails_at(&report, "optimisation level 0 (the file's --!optimize directive)", "Out of local registers");
    // an interpolated string of a string constant folds; of a number, or a builtin call, does not (level 1)
    let interpolated = |k: &str| {
        format!("const k = {k};\n")
            + &lines(201, &|i| format!("export const s{i} = `a${{k}}`;"))
            + "print(probeHoist([0]));\n"
    };
    folding_only(&build(&interpolated("\"s\"")));
    fails(&build(&interpolated("1")));
    fails(&build(&(lines(201, &|i| format!("export const m{i} = math.abs(1);")) + "print(probeHoist([0]));\n")));
    // a local written anywhere is no constant
    fails(&build(
        &(lines(201, &|i| format!("let w{i} = 1;"))
            + &lines(201, &|i| format!("w{i} = 2;"))
            + "print(probeHoist([0]));\n"),
    ));
    // a branch Luau knows is dead takes no register from level 1
    let report = build(
        &(tables(195)
            + "if (false as boolean) {\n"
            + &lines(10, &|i| format!("\tconst z{i} = mk();\n\tprint(z{i});"))
            + "}\nprint(probeHoist([0]));\n"),
    );
    folding_only(&report);
    // a loop variable takes a register at level 1, literal bounds or not
    fails(&build(
        &(tables(199)
            + "for (const i of $range(1, 3)) {\n\tconst z = mk();\n\tprint(i, z);\n}\nprint(probeHoist([0]));\n"),
    ));

    // every file a plugin emitted into is compiled, however few locals it has: Luau's 255 registers, held by a wide
    // call or a `..` chain (round 5's t01, t02, t08), and its nesting limit (t09: 1005 `else if`s)
    let wide = "declare function wide(...a: unknown[]): void;\ndeclare function text(): string;\n";
    let terms = |count: usize, term: &dyn Fn(usize) -> String| (0..count).map(term).collect::<Vec<_>>().join(" + ");
    let t01 = format!(
        "{wide}export function t01(x: unknown, s: string) {{\n\twide({}, {});\n}}\nprint(probeHoist([0]));\n",
        vec!["x"; 220].join(", "),
        terms(40, &|_| "s".to_owned())
    );
    fails_at(&build(&t01), "optimisation level 1", "Out of registers when trying to allocate");
    for (locals, chained) in [(160, 98), (185, 97)] {
        let source = wide.to_owned()
            + &lines(locals, &|i| format!("const a{i} = text();"))
            + &format!(
                "wide({});\nprint(probeHoist([0]));\nexport {{}};\n",
                terms(chained, &|i| format!("a{}", i + 1))
            );
        let report = build(&source);
        fails_at(&report, "optimisation level 1", "Out of registers when trying to allocate");
        at_line(&report, "wide(a1 + a2");
    }
    let t09 = format!(
        "{wide}export function pick(x: number) {{\n\t{}\n}}\nprint(probeHoist([0]));\n",
        (0..1005).map(|i| format!("if (x === {i}) {{\n\t\twide({i});\n\t}}")).collect::<Vec<_>>().join(" else ")
    );
    fails_at(&build(&t09), "optimisation level 1", "Exceeded allowed recursion depth");

    // a file no plugin emitted anything into is left as roblox-ts leaves it, however many locals it has
    let report = build(&tables(205));
    assert_eq!(big(&report), Vec::<String>::new());
}

#[test]
fn a_function_past_luau_upvalue_limit_is_an_error() {
    let Some(project) = Project::new("upvalues") else { return };
    let lines = |count: usize, line: &dyn Fn(usize) -> String| (1..=count).map(|i| line(i) + "\n").collect::<String>();
    let names = |prefix: &str| (1..=100).map(|i| format!("{prefix}{i}")).collect::<Vec<_>>().join(", ");
    let build = |hoisted: &str| {
        project.write(
            "src/shared/big.ts",
            &(lines(100, &|i| format!("const a{i} = mk();"))
                + "export function g() {\n"
                + &lines(100, &|i| format!("\tconst b{i} = mk();"))
                + &format!("\treturn () => print({}, {}{hoisted});\n}}\n", names("a"), names("b"))
                + "print(probeHoist([1]));\n"),
        );
        let report = project.build(&["stamp", "probe"], |_| {});
        let _ = std::fs::remove_dir_all(project.root.join("out"));
        diagnostics(&report).into_iter().filter(|d| d.contains(" big.ts: ")).collect::<Vec<_>>()
    };
    // 100 main-chunk locals and 100 of `g`: 200 upvalues, Luau's limit
    assert_eq!(build(""), Vec::<String>::new());
    // a hoist (a main-chunk local) used in the closure makes 201
    let all = build(", probeHoist([0])");
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].starts_with(&format!("error big.ts: Luau {LUAU_VERSION} refuses this file")), "{all:?}");
    assert!(all[0].contains(": \"Out of upvalue registers when trying to allocate "), "{all:?}");
}

#[test]
fn plugin_panics_outside_the_transform_fail_the_build() {
    let Some(project) = Project::new("panics") else { return };
    let report = project.build(&["stamp", "probe-setup-panic"], |_| {});
    assert_eq!(report.fatal, vec!["plugin `probe` panicked in setup: probe: setup panic"]);
    let report = project.build(&["stamp", "probe-finish-panic"], |_| {});
    assert_eq!(report.fatal, vec!["plugin `probe` panicked in finish: probe: finish panic"]);
    assert!(!project.out_exists());
}

#[test]
fn artifacts_cannot_escape_through_links_or_alias_a_luau_output() {
    let Some(project) = Project::new("sneaky") else { return };
    let outside = std::env::temp_dir().join(format!("rbxts-rs-plugins-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let link = project.root.join("link");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "could not make the junction");
    let report = project.build(&["stamp", "probe-artifact-sneaky"], |_| {});
    let _ = std::process::Command::new("cmd").args(["/C", "rmdir"]).arg(&link).status();
    assert_eq!(report.fatal.len(), 2, "{:?}", report.fatal);
    assert!(
        report.fatal[0].ends_with("calls.luau. has a path component ending in `.` or a space"),
        "{:?}",
        report.fatal
    );
    assert!(
        report.fatal[1]
            .ends_with("through-the-junction.txt is outside the project directory, the outDir and the include path"),
        "{:?}",
        report.fatal
    );
    assert!(!project.out_exists());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing written through the junction");
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn checker_errors_are_explained_by_plugins() {
    let Some(project) = Project::new("explain") else { return };
    project.write("src/shared/broken.ts", "export const z = probeBrokenName();\n");
    let report = project.build(&["stamp", "probe"], |_| {});
    let messages: Vec<String> = report.diagnostics.iter().map(|d| flatten_diagnostic_message(*d, "\n")).collect();
    assert_eq!(messages, vec!["probe explained: Cannot find name 'probeBrokenName'.\nDeclare probeBroken first."]);
    // as stock roblox-ts, the files before the first with a checker error are transformed (nothing is written), the
    // rest are not
    let transformed: Vec<&str> =
        report.files.iter().map(|f| f.file_name.rsplit('/').next().unwrap_or_default()).collect();
    assert_eq!(transformed, vec!["approved.ts", "approvedAfter.ts"], "only the files before broken.ts are transformed");
    assert!(!project.out_exists());
}

/// Stock roblox-ts runs its TypeScript transformers (the plugins here) before checking anything: when one reports an
/// error, only their diagnostics are reported, not the checker's of a later file, nor roblox-ts's.
#[test]
fn a_plugin_error_reports_only_the_plugins_diagnostics() {
    let Some(project) = Project::new("plugin-first") else { return };
    project.write(
        "src/shared/aaplugin.ts",
        "export const h = probeHoist(laterFn());\nexport function laterFn() {\n\treturn 1;\n}\nexport const nul = null;\n",
    );
    project.write("src/shared/zzcheck.ts", "export const bad: number = \"x\";\n");
    let report = project.build(&["stamp", "probe"], |_| {});
    let errors = errors(&report);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("error aaplugin.ts: rbxts-rs internal error: plugin `probe`: hoist"), "{errors:?}");
    assert!(!project.out_exists());
}

/// A tsconfig transformer no registered plugin claims: as in stock roblox-ts 3.0.0 (`createTransformerList`), one Node
/// can't resolve from the project directory is a warning with stock's text, and the build goes on without it; one
/// Node loads a factory function from (stock would run it) is an error; one Node fails to load is a warning with Node's
/// error.
#[test]
fn tsconfig_plugins_must_be_compiled_in() {
    let Some(project) = Project::new("activation") else { return };
    let report = project.build(&[], |_| {});
    assert!(report.fatal.is_empty(), "{:?}", report.fatal);
    let project_dir = project.root.to_string_lossy().replace('/', "\\");
    let warnings: Vec<String> = diagnostics(&report).into_iter().filter(|d| d.starts_with("warning")).collect();
    assert_eq!(
        warnings,
        vec![format!(
            "warning : Transformer `rbxts-transform-stamp` was not found!\nMore info: Error: Cannot find module \
             'rbxts-transform-stamp' from '{project_dir}'\nSuggestion: Did you forget to install the package?"
        )]
    );
    // the build without the transformer compiles the plugin calls as plain calls (the `debugger` is roblox-ts's error)
    let found = errors(&report);
    assert!(found[0].starts_with("error fallbacks.ts: `debugger` is not supported!"), "{found:?}");

    let tsconfig = std::fs::read_to_string(fixture_dir().join("tsconfig.json")).unwrap();
    // one that throws when loaded, and one resolve.sync can't find (it tries `.js` only)
    project.write("throws.js", "throw new Error(\"boom\");\n");
    project.write("data.json", "{}\n");
    let two = "[{ \"transform\": \"./throws\" }, { \"transform\": \"./data\" }]";
    project.write(
        "tsconfig.json",
        &tsconfig.replace("[{ \"transform\": \"rbxts-transform-stamp\", \"prefix\": \"fixture \" }]", two),
    );
    let report = project.build(&[], |_| {});
    assert!(report.fatal.is_empty(), "{:?}", report.fatal);
    let warnings: Vec<String> = diagnostics(&report).into_iter().filter(|d| d.starts_with("warning")).collect();
    if warnings.len() == 2 {
        assert_eq!(
            warnings,
            vec![
                "warning : Transformer `./throws` was not found!\nMore info: Error: boom\nSuggestion: Did you forget to \
                 install the package?"
                    .to_owned(),
                format!(
                    "warning : Transformer `./data` was not found!\nMore info: Error: Cannot find module './data' from \
                     '{project_dir}'\nSuggestion: Did you forget to install the package?"
                ),
            ]
        );
    } else {
        // no Node to load `./throws` with: it is taken for a transformer stock would run
        assert_eq!(report.fatal.len() + warnings.len(), 2, "{:?} {warnings:?}", report.fatal);
    }

    // a transformer Node would load
    project.write("transformer.js", "module.exports = () => (context) => (file) => file;\n");
    project.write("tsconfig.json", &tsconfig.replace("\"rbxts-transform-stamp\"", "\"./transformer\""));
    let report = project.build(&[], |_| {});
    assert_eq!(
        report.fatal,
        vec![
            "tsconfig plugin \"./transformer\" is not compiled into this toolchain (no registered plugin has that \
             tsconfig_transform)"
        ]
    );

    // a plugin with a tsconfig_transform is off without its entry
    project.write(
        "tsconfig.json",
        &tsconfig.replace(
            ",\n\t\t\"plugins\": [{ \"transform\": \"rbxts-transform-stamp\", \"prefix\": \"fixture \" }]",
            "",
        ),
    );
    let report = project.build(&["stamp"], |_| {});
    assert!(report.plugins.is_empty(), "{:?}", report.plugins);
    // and the build without plugins compiles the plugin calls as plain calls
    assert_eq!(errors(&report).len(), 1, "{:?}", errors(&report));
    assert!(
        errors(&report)[0].starts_with("error fallbacks.ts: `debugger` is not supported!"),
        "{:?}",
        errors(&report)
    );
}

#[test]
fn a_render_panic_is_the_file_s_internal_error() {
    let Some(project) = Project::new("render") else { return };
    // a claimed value that is no value at all cannot be rendered (a plugin bug): the file's error, not a crash
    project.write("src/shared/render.ts", "export const x = probeNone();\n");
    let report = project.build(&["stamp", "probe"], |_| {});
    assert_eq!(
        errors(&report),
        vec!["error render.ts: rbxts-rs internal error: rendering the Luau panicked: Cannot render None"]
    );
    assert!(report.fatal.is_empty(), "{:?}", report.fatal);
    assert!(!project.out_exists());
}

#[test]
fn an_unused_optional_chain_in_a_for_head_s_parentheses_lowers() {
    let Some(project) = Project::new("parens") else { return };
    // roblox-ts lowers the unused chain to no value, which the parentheses then wrapped (a render panic)
    project.write(
        "src/shared/parens.ts",
        "declare const maybe: { f(): number } | undefined;
for ((maybe?.f()); ; ) {
	break;
}
export {};
",
    );
    let report = project.build(&["stamp", "probe"], |_| {});
    assert_eq!(
        errors(&report).into_iter().filter(|e| e.contains("parens.ts")).collect::<Vec<_>>(),
        Vec::<String>::new()
    );
    let file = report.files.iter().find(|f| f.file_name.ends_with("parens.ts")).unwrap();
    assert_eq!(
        file.luau.as_deref().unwrap(),
        "-- Compiled with roblox-ts v3.0.0\ndo\n\tlocal _result = maybe\n\tif _result ~= nil then\n\t\t_result:f()\n\tend\n\
         \twhile true do\n\t\tbreak\n\tend\nend\nreturn nil\n"
    );
}

/// `@ts-nocheck` without `allowCommentDirectives`: one error per pragma, as stock roblox-ts
/// (`fileUsesCommentDirectives`), also when it is repeated or a later `@ts-check` overrides it.
#[test]
fn every_ts_nocheck_pragma_is_reported() {
    let Some(project) = Project::new("nocheck") else { return };
    project.write("src/shared/twice.ts", "// @ts-nocheck\n// @ts-nocheck\nexport const a = 1;\n");
    project.write("src/shared/overridden.ts", "// @ts-nocheck\n// @ts-check\nexport const b = 1;\n");
    let report = project.build(&["stamp", "probe"], |_| {});
    let errors = errors(&report);
    let count = |file: &str| {
        let prefix = format!("error {file}: Usage of `@ts-ignore`, `@ts-expect-error`, and `@ts-nocheck`");
        errors.iter().filter(|e| e.starts_with(&prefix)).count()
    };
    assert_eq!(count("twice.ts"), 2, "{errors:?}");
    assert_eq!(count("overridden.ts"), 1, "{errors:?}");
    // each spans its comment (stock's spans run from the comment on for `range.end` characters)
    let spans: Vec<(u32, u32)> = report
        .files
        .iter()
        .filter(|f| f.file_name.ends_with("twice.ts"))
        .flat_map(|f| &f.diagnostics)
        .chain(&report.diagnostics)
        .filter(|d| d.file().is_some_and(|file| file.file_name().ends_with("twice.ts")))
        .map(|d| d.loc().map(|loc| (loc.pos().expect_source("a source position"), loc.len().unwrap())).unwrap())
        .collect();
    assert_eq!(spans, [(0, 14), (15, 14)], "{errors:?}");
}

/// The offers S2 left out and the seam's last additions, against the `seamrest` fixture (`probe` and `probe-exprcall`):
/// optional method calls and non-method optional calls, `super.m()` / `super.m?.()` and a `super` callee's key, every
/// access and call inside a chain for the expression hooks, an expression hook asked once per node, block bodies,
/// `else if`, a static block and `declare` statements for the statement hooks, and dotted JSX tags. The expected Luau is
/// hand-checked; `runtime/check.luau` runs it with Lune (`lune` on the PATH, skipped and said so without it): claimed
/// optional calls and links keep short-circuiting, receivers and bases are evaluated once, `super` calls keep `self`.
#[test]
fn every_chain_link_super_call_and_body_is_offered_and_keeps_its_semantics() {
    let Some(project) = Project::of("seamrest", "seamrest") else { return };
    let report = project.build(&["probe", "probe-exprcall"], |_| {});
    assert!(report.fatal.is_empty(), "{:?}", report.fatal);
    assert_eq!(errors(&report), Vec::<String>::new());
    // asked once per node: the call hook compiling each call itself does not ask the expression hook again
    assert_eq!(
        diagnostics(&report),
        vec![
            "warning askonce.ts: probe-exprcall: asked about probeAskOnce",
            "warning askonce.ts: probe-exprcall: asked about probeAskOnce",
            "warning askonce.ts: probe-exprcall: asked about probeAskOnce",
        ]
    );
    assert_expected_of("seamrest", &report, &[]);
    let fixture = fixture_dir_of("seamrest");
    match std::process::Command::new("lune").args(["run", "runtime/check.luau"]).current_dir(&fixture).output() {
        Err(e) => eprintln!("seamrest: the Lune runtime check skipped: lune: {e}"),
        Ok(output) => assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).starts_with("ok "),
            "runtime/check.luau:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

/// Offering a node and getting a clean decline changes nothing: over the area fixtures, `decline-all` (offered every
/// call, access, assignment, class, expression and statement it can be, declining all of it) and the identity plugins
/// (claiming and lowering as roblox-ts would) write what a plugin build with nothing to do (`inert`) writes, file for
/// file, with the same diagnostics. (A build with plugins differs from one without only in numeric literals with
/// separators, which it prints as their value, as stock roblox-ts's reprint does.)
#[test]
fn declining_or_lowering_as_roblox_ts_changes_no_output() {
    let areas = ["binding", "classes", "contextual_this", "exprs", "jsx", "literals", "loops", "macros", "stmts", "stockfix"];
    let modes = ["decline-all", "identity-expr", "identity-call", "identity-key", "identity-assign", "identity-fallback"];
    for area in areas {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(area);
        let names: Vec<String> = std::fs::read_dir(&fixture)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with("expected") && n != "runtime")
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let Some(project) = Project::copy_of(&fixture, &names, &format!("decline-{area}")) else { return };
        let outputs = |plugin: &str| {
            let report = project.build(&[plugin], |_| {});
            let _ = std::fs::remove_dir_all(project.root.join("out"));
            let mut files: Vec<(String, Option<String>)> =
                report.files.iter().map(|f| (f.out_path.clone(), f.luau.clone())).collect();
            files.sort();
            (files, diagnostics(&report))
        };
        let baseline = outputs("inert");
        assert!(!baseline.0.is_empty(), "{area}: no files");
        for mode in modes {
            let (files, diagnostics) = outputs(mode);
            assert_eq!(diagnostics, baseline.1, "{area}, {mode}: diagnostics");
            for ((path, luau), (_, expected)) in files.iter().zip(&baseline.0) {
                assert_eq!(luau, expected, "{area}, {mode}: {path}");
            }
            assert_eq!(files.len(), baseline.0.len(), "{area}, {mode}: files");
        }
    }
}

/// A plugin diagnostic identical to roblox-ts's own at the same node (here its `logTruthyChanges` warning), or to
/// one another file's hook reported at a node in a third file, is kept once in the build, as TypeScript's
/// DiagnosticCollection keeps one of each; for any checker count. The copy kept is the one TypeScript's
/// `sortAndDeduplicateDiagnostics` keeps, the one with related information, whichever file reports it first.
#[test]
fn a_plugin_diagnostic_is_kept_once_in_the_build() {
    let probe = fixture_dir_of("probe");
    let Some(project) = Project::copy_of(&probe, &["default.project.json", "package.json"], "diagonce") else {
        return;
    };
    let tsconfig = std::fs::read_to_string(probe.join("tsconfig.json")).unwrap();
    project.write("tsconfig.json", &tsconfig.replacen('{', "{\n\t\"rbxts\": { \"logTruthyChanges\": true },", 1));
    std::fs::create_dir_all(project.root.join("src/shared")).unwrap();
    for name in ["probe.d.ts", "seamdiag.ts", "seamdiag2.ts"] {
        std::fs::copy(probe.join("src/shared").join(name), project.root.join("src/shared").join(name)).unwrap();
    }
    // the declaration warning's copy with related information, and the file holding it
    let kept = |report: &CompileReport| {
        let holders: Vec<(String, usize)> = report
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter().map(move |d| (f, d)))
            .filter(|(_, d)| d.localize().contains("at the declaration"))
            .map(|(f, d)| {
                let name = Path::new(&f.file_name).file_name().unwrap().to_string_lossy().into_owned();
                (name, d.related_information().len())
            })
            .collect();
        holders
    };
    for checkers in [1, 2] {
        let report = project.build_with(&["stamp", "probe"], Some(checkers), |_| {});
        assert!(report.fatal.is_empty(), "{:?}", report.fatal);
        assert_eq!(
            diagnostics(&report),
            vec![
                "warning seamdiag.ts: Value will be checked against 0, NaN",
                "warning probe.d.ts: probe: at the declaration, once in the build",
            ],
            "{checkers} checkers"
        );
        // seamdiag.ts reports it first, seamdiag2.ts with related information
        assert_eq!(kept(&report), vec![("seamdiag2.ts".to_owned(), 1)], "{checkers} checkers");
    }
    // the other way round: still the copy with related information
    let first = std::fs::read_to_string(project.root.join("src/shared/seamdiag.ts")).unwrap();
    project.write("src/shared/seamdiag.ts", &first.replace("probeDiagDecl()", "probeDiagDecl(true)"));
    let second = std::fs::read_to_string(project.root.join("src/shared/seamdiag2.ts")).unwrap();
    project.write("src/shared/seamdiag2.ts", &second.replace("probeDiagDecl(true)", "probeDiagDecl()"));
    let report = project.build_with(&["stamp", "probe"], Some(1), |_| {});
    assert_eq!(kept(&report), vec![("seamdiag.ts".to_owned(), 1)]);

    // copies whose related information differs only by file compare by path, lowercased on Windows as TypeScript
    // compares it: `a.ts`'s before `B.ts`'s (and the others'), where the file names would put `B.ts` first
    project.write("src/shared/a.ts", "export const a = probeDiagDecl(true);\n");
    project.write("src/shared/B.ts", "export const b = probeDiagDecl(true);\n");
    let report = project.build_with(&["stamp", "probe"], Some(1), |_| {});
    assert_eq!(kept(&report), vec![("a.ts".to_owned(), 1)]);
    std::fs::remove_file(project.root.join("src/shared/a.ts")).unwrap();
    std::fs::remove_file(project.root.join("src/shared/B.ts")).unwrap();

    // a plugin copy and roblox-ts's that differ only in category: the plugin's is kept, as stock adds transformer
    // diagnostics before its own and sorts stably
    project.write(
        "src/shared/category.ts",
        "declare const n: number;\nexport const c = probeDiagSame(n, \"Value will be checked against 0, NaN\", true) ? 1 : 0;\n",
    );
    let report = project.build_with(&["stamp", "probe"], Some(1), |_| {});
    let truthy: Vec<Category> = report
        .files
        .iter()
        .filter(|f| f.file_name.ends_with("category.ts"))
        .flat_map(|f| &f.diagnostics)
        .filter(|d| d.localize().starts_with("Value will be checked"))
        .map(|d| d.category())
        .collect();
    assert_eq!(truthy, vec![Category::Message]);
}
