//! Incremental and watch builds (DESIGN §14) on `tests/watch_fixtures/basic`:
//! - the W1 harness (`rbxts_transform::build::parity`) runs `basic.script.json` as watch rebuilds with the `stamp`
//!   plugin (artifacts, a state file, late strings re-rendered in cached files), without plugins, and as
//!   incremental builds in new sessions; after every step the output must equal a clean build's (JSON modules
//!   included: an edit re-copies the asset and re-transforms only the importers);
//! - `attack.script.json` on `tests/watch_fixtures/attack`: the review of the branch's attack sequence;
//! - `references.script.json` on `tests/watch_fixtures/references`: TypeScript project references (edits in a
//!   referenced project, errors, new and deleted files, hand-deleted outputs, a referenced tsconfig edit, projects
//!   skipped while not dirty, edits made while a reference fails); `midsolution.script.json` on
//!   `tests/watch_fixtures/midsolution`: the same through a solution config in the middle of the graph;
//! - the kept-`Shared` contract, with a plugin that counts its `setup` and `rebuild_start` calls;
//! - the clean of a declaration build over a used output directory, against roblox-ts's keep set, with a junction.
//!
//! Like `tests/plugins.rs`, the fixture links a `node_modules` (`RBXTS_RS_FIXTURE_NODE_MODULES`, else the roblox-ts
//! tests project's); without one the tests are skipped.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rbxts_plugin::erased::{DynPlugin, erase};
use rbxts_plugin::{
    BuildKind, ChangeSet, CheckerInfo, Cx, Fatal, FileIndex, FinishCx, Interest, PResult, Plugin, PluginInfo,
    PluginInstance, RebuildPlan, SetupCx, StateOrigin,
};
use rbxts_project::{ProjectOptionLayer, STACK_SIZE};
use rbxts_transform::build::parity::{self, ParityOptions, StepResult};
use rbxts_test_plugins::scan;
use rbxts_transform::{BuildSession, BuildSummary, CompileOptions, PluginOptions};

fn node_modules() -> Option<PathBuf> {
    let path = rbxts_test_support::node_modules().unwrap_or_default();
    path.join("@rbxts").join("compiler-types").is_dir().then_some(path)
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/watch_fixtures")
}

/// Runs `f` on a thread with the compiler's stack.
fn big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new().stack_size(STACK_SIZE).spawn_scoped(scope, f).unwrap().join().unwrap()
    })
}

fn run_basic(label: &str, plugins: &[&str], incremental: bool) -> Option<Vec<StepResult>> {
    run_fixture("basic", label, plugins, incremental)
}

/// The entries the harness copies: the sources and configs.
const COPY: &[&str] = &["src", "tsconfig.json", "default.project.json", "package.json"];

fn run_fixture(fixture: &str, label: &str, plugins: &[&str], incremental: bool) -> Option<Vec<StepResult>> {
    run_fixture_copying(fixture, label, plugins, incremental, COPY)
}

/// [`run_fixture`] copying `copy` (the entries holding sources and configs).
fn run_fixture_copying(
    fixture: &str,
    label: &str,
    plugins: &[&str],
    incremental: bool,
    copy: &[&str],
) -> Option<Vec<StepResult>> {
    run_fixture_ignoring(fixture, label, plugins, incremental, copy, plugins.is_empty())
}

/// [`run_fixture_copying`], ignoring the tsconfig's transformers no active plugin answers when `ignore_unknown`.
fn run_fixture_ignoring(
    fixture: &str,
    label: &str,
    plugins: &[&str],
    incremental: bool,
    copy: &[&str],
    ignore_unknown: bool,
) -> Option<Vec<StepResult>> {
    run_script_copying(fixture, fixture, label, plugins, incremental, copy, ignore_unknown)
}

/// [`run_fixture`] with the script `<script>.script.json` over the fixture `fixture`.
fn run_script(fixture: &str, script: &str, label: &str, plugins: &[&str], incremental: bool) -> Option<Vec<StepResult>> {
    run_script_copying(fixture, script, label, plugins, incremental, COPY, plugins.is_empty())
}

fn run_script_copying(
    fixture: &str,
    script: &str,
    label: &str,
    plugins: &[&str],
    incremental: bool,
    copy: &[&str],
    ignore_unknown: bool,
) -> Option<Vec<StepResult>> {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return None;
    };
    let script =
        parity::parse_script(&std::fs::read_to_string(fixtures().join(format!("{script}.script.json"))).unwrap())
            .unwrap();
    let scratch = std::env::temp_dir().join(format!("rbxts-rs-watch-{label}-{}", std::process::id()));
    let plugins: Vec<Box<dyn DynPlugin>> =
        plugins.iter().map(|name| rbxts_test_plugins::by_name(name).expect("a test plugin")).collect();
    let results = big_stack(|| {
        let options = ParityOptions {
            source: fixtures().join(fixture),
            tsconfig: "tsconfig.json".into(),
            copy: copy.iter().map(|c| (*c).to_owned()).collect(),
            scratch: scratch.clone(),
            // truthiness warnings: a rebuild must report the warnings of the files it does not transform again
            project_options: ProjectOptionLayer { log_truthy_changes: Some(true), ..ProjectOptionLayer::default() },
            plugins: &plugins,
            ignore_unknown_plugins: ignore_unknown,
            node_modules: Some(node_modules),
            node_modules_mode: parity::NodeModules::LinkPackages,
            incremental,
            keep_clean_copies: false,
            check_clean: true,
            memory: None,
            checkers: None,
        };
        parity::run(&options, &script)
    })
    .unwrap();
    parity::remove_tree(&scratch);
    for (index, result) in results.iter().enumerate() {
        assert!(result.problems.is_empty(), "step {index} `{}`: {:#?}", result.name, result.problems);
    }
    assert_eq!(results.len(), script.steps.len() + 1, "every step ran");
    Some(results)
}

fn step<'r>(results: &'r [StepResult], name: &str) -> &'r StepResult {
    results.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no step {name}"))
}

#[test]
fn watch_rebuilds_equal_clean_builds_with_the_stamp_plugin() {
    let Some(results) = run_basic("stamp", &["stamp"], false) else { return };
    // the closure, not every file
    let chain = step(&results, "C → B → D: Inner.a array to map");
    assert!(!chain.all && chain.transformed > 0);
    assert!(matches!(chain.kind, Some(BuildKind::WatchRebuild { .. })));
    // the global rule: every file
    assert!(step(&results, "S1: a global .d.ts interface").all);
    assert!(step(&results, "S1b: declare global in a module").all);
    // the augmentation edge reaches useAug.ts, which does not import aug.ts (checked by the parity itself)
    assert!(!step(&results, "S2: a module augmentation").all);
    // a late-string renumbering touches one source file
    assert_eq!(step(&results, "a new late string renumbers cached files").transformed, 1);
    // a tsconfig change re-runs setup: every file
    assert!(step(&results, "a tsconfig edit (the plugin's options)").all);
    assert_eq!(step(&results, "an unchanged save").transformed, 0);
    // a JSON module's edit: its importer, not every file; a JSON file nothing imports is only an asset
    let json = step(&results, JSON_TYPE_EDIT);
    assert!(!json.all && json.transformed == 1);
    assert_eq!(step(&results, "a JSON module: a new JSON file nothing imports").transformed, 0);
}

/// The basic script's step that turns a JSON module's number into a string, which changes its importer's Luau.
const JSON_TYPE_EDIT: &str = "a JSON module: a number becomes a string (the importer's `+` becomes `..`)";

/// The review's attack sequence (globals reached through the closure, augmentation and export hops, const enums,
/// JSDoc macro modes, renames, deletions and re-adds, a twin .d.ts, tsconfig and Rojo edits, a case-only rename).
#[test]
fn the_review_attack_sequence_equals_clean_builds() {
    run_fixture("attack", "attack", &["stamp"], false);
    run_fixture("attack", "attack-inc", &[], true);
}

/// The `references` fixture's sources: each referenced project's sources and tsconfig, not its output folder.
const REFERENCES_COPY: &[&str] = &[
    "src",
    "tsconfig.json",
    "default.project.json",
    "package.json",
    "projects/common/src",
    "projects/common/tsconfig.json",
    "projects/shared/src",
    "projects/shared/tsconfig.json",
    "projects/external",
];

/// TypeScript project references (`references.script.json`): after every watch rebuild and every incremental build,
/// every project's outputs equal a clean build's (which builds the whole graph in a copy); an error in a referenced
/// project builds nothing.
#[test]
fn project_references_rebuild_as_clean_builds() {
    let Some(results) = run_fixture_copying("references", "references", &[], false, REFERENCES_COPY) else { return };
    // a declaration-neutral edit transforms the edited file only (the root's program sees the same declarations)
    assert_eq!(step(&results, "a body edit in a referenced project (its declarations unchanged)").transformed, 1);
    // a type change reaches the files that import it, across the reference boundary
    assert!(step(&results, "a referenced type change reaches the root's Luau (`+` becomes `..`)").transformed >= 3);
    assert_eq!(step(&results, "an unchanged save in a referenced project").transformed, 0);
    // a project that is not dirty is not built (roblox-ts's `ProjectBuild`); one that failed is built again
    assert_eq!(step(&results, "an edit in the root alone: both referenced projects are skipped").skipped, 2);
    assert_eq!(step(&results, "an edit in shared alone: common is skipped").skipped, 1);
    assert_eq!(step(&results, "a root edit while common still fails: common is built again and reports its error").skipped, 0);
    run_fixture_copying("references", "references-inc", &[], true, REFERENCES_COPY);
}

const MIDSOLUTION_COPY: &[&str] = &["src", "tsconfig.json", "base.json", "default.project.json", "package.json", "projects"];

/// Project references through a solution config in the middle of the graph (`midsolution.script.json`): a failure
/// below it blocks everything above it, a project that is not dirty is skipped, and every step equals a clean build.
#[test]
fn a_solution_config_in_the_middle_passes_failures_on() {
    let Some(results) = run_fixture_copying("midsolution", "midsolution", &[], false, MIDSOLUTION_COPY) else { return };
    assert_eq!(step(&results, "an edit in the root alone: every referenced project is skipped").skipped, 3);
    let right = "an edit in right: left and common are skipped, the root rebuilt through the solution config";
    assert_eq!(step(&results, right).skipped, 2);
    assert_eq!(step(&results, "an unchanged save of the solution config: every project is dirty").skipped, 0);
    run_fixture_copying("midsolution", "midsolution-inc", &[], true, MIDSOLUTION_COPY);
}

/// A plugin diagnostic two files report at a node in a third (`plugindiag.script.json`): every watch rebuild and
/// incremental build reports the copy a clean build keeps, when the file holding it stops reporting it, is deleted,
/// or comes back, and when the copy with related information moves to the other file. With probe alone (its parts
/// persist), an incremental build transforms only the edited file and makes the other's diagnostics again from the
/// incremental state, the one at the third file's node included.
#[test]
fn plugin_diagnostic_duplicates_rebuild_as_clean_builds() {
    run_fixture("plugindiag", "plugindiag", &["stamp", "probe"], false);
    run_fixture("plugindiag", "plugindiag-inc", &["stamp", "probe"], true);
    let Some(results) = run_fixture_ignoring("plugindiag", "plugindiag-inc-probe", &["probe"], true, COPY, true) else {
        return;
    };
    let edit = results.iter().find(|r| r.name == "seamdiag.ts: an unrelated edit").expect("the step");
    assert_eq!(edit.transformed, 1, "the other file's diagnostics are replayed, not transformed again");
    // ties on related information, and file-name case (`a3.ts`, `B.ts`)
    run_script("plugindiag", "plugindiag-ties", "plugindiag-ties", &["stamp", "probe"], false);
    run_script("plugindiag", "plugindiag-ties", "plugindiag-ties-inc", &["stamp", "probe"], true);
    // roblox-ts errors in the file holding the kept copy, and in the other; the incremental builds after a failing
    // one start from the last committed state (the stamp plugin's build counter)
    run_script("plugindiag", "plugindiag-errors", "plugindiag-errors", &["stamp", "probe"], false);
    run_script("plugindiag", "plugindiag-errors", "plugindiag-errors-inc", &["stamp", "probe"], true);
}

/// Stock's first-error rule (`firsterror-s*.script.json`, over `plugindiag` with a ReplicatedFirst file whose
/// runtime-library warning is cut after an earlier error): check and roblox-ts errors early and late, a cached file's
/// warnings after the first error, plugin errors with roblox-ts and check errors (only the plugins' diagnostics).
#[test]
fn first_error_cuts_rebuild_as_clean_builds() {
    for script in ["firsterror-s1", "firsterror-s2", "firsterror-s3", "firsterror-s4"] {
        run_script("plugindiag", script, script, &["stamp", "probe"], false);
        run_script("plugindiag", script, &format!("{script}-inc"), &["stamp", "probe"], true);
    }
    run_script("plugindiag", "firsterror-s2", "firsterror-s2-plain", &[], false);
    run_script("plugindiag", "firsterror-s2", "firsterror-s2-plain-inc", &[], true);
}

/// A tsconfig without `types` (`autotypes.script.json`): every package of the type roots is in the program, as in
/// TypeScript 5, and a package appearing, changing, turning `"typings": null` or going is a change the watcher and the
/// incremental state see.
#[test]
fn automatic_type_packages_rebuild_as_clean_builds() {
    run_fixture("autotypes", "autotypes", &[], false);
    run_fixture("autotypes", "autotypes-inc", &[], true);
}

#[test]
fn watch_rebuilds_equal_clean_builds_without_plugins() {
    run_basic("plain", &[], false);
}

#[test]
fn incremental_builds_equal_clean_builds() {
    let Some(results) = run_basic("incremental", &[], true) else { return };
    assert_eq!(results[0].kind, Some(BuildKind::Clean));
    let chain = step(&results, "C → B → D: Inner.a array to map");
    assert_eq!(chain.kind, Some(BuildKind::Incremental));
    assert!(!chain.all && chain.transformed > 0);
    let json = step(&results, JSON_TYPE_EDIT);
    assert_eq!(json.kind, Some(BuildKind::Incremental));
    assert!(!json.all && json.transformed == 1);
}

// --- the kept-Shared contract ---

#[derive(Default)]
struct Counts {
    setups: AtomicUsize,
    rebuild_starts: AtomicUsize,
    memory_origins: AtomicUsize,
}

/// Counts its `setup` and `rebuild_start` calls. Reads `counter-input.txt` (a build input); asks for a new `Shared`
/// when a changed file's name contains `rerun`; its state counts finished builds, written to `counter.txt`.
struct Counter(Arc<Counts>);

struct CounterInstance;

impl Plugin for Counter {
    type Shared = String;
    type State = u32;
    type FilePart = ();
    type Instance = CounterInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "counter", version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, cx: &mut SetupCx<'_>, _state: &mut u32) -> Result<Arc<String>, Fatal> {
        self.0.setups.fetch_add(1, Ordering::SeqCst);
        if cx.state_origin() == StateOrigin::Memory {
            self.0.memory_origins.fetch_add(1, Ordering::SeqCst);
        }
        let input = cx.read_input("counter-input.txt").map_err(|e| Fatal::new(e.to_string()))?;
        Ok(Arc::new(input.unwrap_or_default()))
    }

    fn rebuild_start(&self, _shared: &String, _state: &u32, changes: &ChangeSet) -> RebuildPlan {
        self.0.rebuild_starts.fetch_add(1, Ordering::SeqCst);
        if changes.changed.iter().chain(&changes.added).any(|f| f.contains("rerun")) {
            RebuildPlan::RerunSetup
        } else {
            RebuildPlan::KeepShared
        }
    }

    fn instance(&self, _shared: Arc<String>, _checker: CheckerInfo) -> CounterInstance {
        CounterInstance
    }

    fn finish(
        &self,
        shared: &String,
        state: &mut u32,
        _parts: &[(FileIndex, &())],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        *state += 1;
        cx.write_artifact("counter.txt", format!("{state} {}", shared.trim()).into_bytes());
        Ok(())
    }
}

impl PluginInstance for CounterInstance {
    type FilePart = ();

    fn interest(&self, _interest: &mut Interest) {}

    fn end_file(&self, _cx: &mut Cx<'_>) -> PResult<()> {
        Ok(())
    }
}

/// A copy of the basic fixture with a linked node_modules; removed on drop.
struct Copy(PathBuf);

impl Drop for Copy {
    fn drop(&mut self) {
        parity::remove_tree(&self.0);
    }
}

#[test]
fn a_kept_shared_survives_failures_and_input_changes_rerun_setup() {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    };
    let root = std::env::temp_dir().join(format!("rbxts-rs-watch-shared-{}", std::process::id()));
    parity::remove_tree(&root);
    let copy = Copy(root.clone());
    parity::copy_tree(&fixtures().join("basic"), &root).unwrap();
    parity::junction(&root.join("node_modules"), &node_modules).unwrap();
    std::fs::write(root.join("counter-input.txt"), "one").unwrap();
    let counts = Arc::new(Counts::default());
    let plugins = vec![erase(Counter(Arc::clone(&counts)))];
    let path = |p: &str| root.join(p).to_string_lossy().replace('\\', "/");
    let write = |p: &str, text: &str| std::fs::write(root.join(p), text).unwrap();
    let artifact = || std::fs::read_to_string(root.join("counter.txt")).unwrap_or_default();
    let setups = || counts.setups.load(Ordering::SeqCst);
    let starts = || counts.rebuild_starts.load(Ordering::SeqCst);
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = BuildSession::new(
            CompileOptions {
                tsconfig: "tsconfig.json".into(),
                cwd: root.to_string_lossy().into_owned(),
                project_options: ProjectOptionLayer::default(),
                out_dir_override: None,
                write_partial: false,
                no_write: false,
                checkers: None,
            },
            &options,
            true,
        );
        let (report, summary) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert_eq!(summary.kind, Some(BuildKind::Clean));
        assert_eq!((setups(), starts()), (1, 0));
        assert_eq!(artifact(), "1 one");

        // an edit: rebuild_start keeps the Shared, only the closure is transformed
        write(
            "src/shared/d.ts",
            "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.size() + 1;\n}\n",
        );
        let (report, summary) = session.build(&[path("src/shared/d.ts")]);
        assert!(!report.has_errors());
        assert_eq!((setups(), starts()), (1, 1));
        assert!(!summary.all);
        assert_eq!(artifact(), "2 one", "the committed state is handed on");

        // a build input changed: setup runs again (no rebuild_start), on the committed state, and every file is
        // transformed
        write("counter-input.txt", "two");
        let (report, summary) = session.build(&[path("counter-input.txt")]);
        assert!(!report.has_errors());
        assert_eq!((setups(), starts()), (2, 1));
        assert!(summary.all);
        assert_eq!(counts.memory_origins.load(Ordering::SeqCst), 1);
        assert_eq!(artifact(), "3 two");

        // a failing build keeps the Shared it was handed and writes nothing
        write(
            "src/shared/d.ts",
            "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.sizee();\n}\n",
        );
        let (report, _) = session.build(&[path("src/shared/d.ts")]);
        assert!(report.has_errors());
        assert_eq!((setups(), starts()), (2, 2));
        assert_eq!(artifact(), "3 two");

        // the fix: the kept Shared is still there
        write(
            "src/shared/d.ts",
            "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.size() + 2;\n}\n",
        );
        let (report, _) = session.build(&[path("src/shared/d.ts")]);
        assert!(!report.has_errors());
        assert_eq!((setups(), starts()), (2, 3));
        assert_eq!(artifact(), "4 two");

        // RerunSetup re-transforms everything
        write("src/shared/rerun.ts", "export const rerun = 1;\n");
        let (report, summary) = session.build(&[path("src/shared/rerun.ts")]);
        assert!(!report.has_errors());
        assert_eq!((setups(), starts()), (3, 4));
        assert!(summary.all);
        assert_eq!(artifact(), "5 two");

        // a failing build that made a new Shared drops it: the next build runs setup again
        write("counter-input.txt", "three");
        write(
            "src/shared/d.ts",
            "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.sizee();\n}\n",
        );
        let (report, _) = session.build(&[path("counter-input.txt"), path("src/shared/d.ts")]);
        assert!(report.has_errors());
        assert_eq!((setups(), starts()), (4, 4));
        assert_eq!(artifact(), "5 two");
        write(
            "src/shared/d.ts",
            "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.size();\n}\n",
        );
        let (report, summary) = session.build(&[path("src/shared/d.ts")]);
        assert!(!report.has_errors());
        assert_eq!((setups(), starts()), (5, 4), "the input still differs from the committed one: setup again");
        assert!(summary.all);
        assert_eq!(artifact(), "6 three");
    });
    drop(copy);
}

// --- state-file integrity ---

const KEEPER_STATE: &str = "keeper-state.txt";

#[derive(Default)]
struct KeeperCounts {
    setups: AtomicUsize,
    fresh_watch_setups: AtomicUsize,
}

/// Counts finished builds in its state, persisted to `keeper-state.txt` and reloaded by a setup that starts fresh
/// outside a clean build (an incremental build, or a watch rebuild after its state file was lost). Its parts persist,
/// so an incremental build transforms only what changed. With `reads_own`, setup also reads its own state file as a
/// build input, as Flamework's candidate scan reads its own `flamework.build`.
struct Keeper {
    counts: Arc<KeeperCounts>,
    reads_own: bool,
}

struct KeeperInstance;

impl Plugin for Keeper {
    type Shared = ();
    type State = u32;
    type FilePart = ();
    type Instance = KeeperInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "keeper", version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, cx: &mut SetupCx<'_>, state: &mut u32) -> Result<Arc<()>, Fatal> {
        self.counts.setups.fetch_add(1, Ordering::SeqCst);
        if self.reads_own {
            cx.read_input(KEEPER_STATE).map_err(|e| Fatal::new(e.to_string()))?;
        }
        if cx.state_origin() == StateOrigin::Fresh && cx.build_kind() != BuildKind::Clean {
            if matches!(cx.build_kind(), BuildKind::WatchRebuild { .. }) {
                self.counts.fresh_watch_setups.fetch_add(1, Ordering::SeqCst);
            }
            let bytes = cx.read_persisted(KEEPER_STATE).map_err(|e| Fatal::new(e.to_string()))?;
            *state = bytes.map_or(0, |b| String::from_utf8_lossy(&b).trim().parse().unwrap_or(0));
        }
        Ok(Arc::new(()))
    }

    fn instance(&self, _shared: Arc<()>, _checker: CheckerInfo) -> KeeperInstance {
        KeeperInstance
    }

    fn finish(
        &self,
        _shared: &(),
        state: &mut u32,
        _parts: &[(FileIndex, &())],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        *state += 1;
        cx.write_state_file(KEEPER_STATE, state.to_string().into_bytes());
        Ok(())
    }

    fn encode_file_part(&self, _part: &()) -> Option<Vec<u8>> {
        Some(Vec::new())
    }

    fn decode_file_part(&self, _bytes: &[u8]) -> Option<()> {
        Some(())
    }
}

impl PluginInstance for KeeperInstance {
    type FilePart = ();

    fn interest(&self, _interest: &mut Interest) {}

    fn end_file(&self, _cx: &mut Cx<'_>) -> PResult<()> {
        Ok(())
    }
}

/// A copy of the basic fixture for the keeper tests, with `"incremental": true` when asked.
fn keeper_copy(label: &str, incremental: bool) -> Option<(Copy, PathBuf)> {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return None;
    };
    let root = std::env::temp_dir().join(format!("rbxts-rs-watch-{label}-{}", std::process::id()));
    parity::remove_tree(&root);
    let copy = Copy(root.clone());
    parity::copy_tree(&fixtures().join("basic"), &root).unwrap();
    parity::junction(&root.join("node_modules"), &node_modules).unwrap();
    if incremental {
        let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap();
        let tsconfig =
            tsconfig.replacen("\"compilerOptions\": {", "\"compilerOptions\": {\n\t\t\"incremental\": true,", 1);
        std::fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
    }
    Some((copy, root))
}

fn keeper_session<'p>(root: &Path, options: &'p PluginOptions<'p>, watch: bool) -> BuildSession<'p> {
    BuildSession::new(
        CompileOptions {
            tsconfig: "tsconfig.json".into(),
            cwd: root.to_string_lossy().into_owned(),
            project_options: ProjectOptionLayer::default(),
            out_dir_override: None,
            write_partial: false,
            no_write: false,
            checkers: None,
        },
        options,
        watch,
    )
}

const D_TS: &str =
    "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.size() + 1;\n}\n";
const D_TS_BROKEN: &str =
    "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.sizee();\n}\n";

/// Watch: a state file the last commit left that is deleted, edited or put back to older bytes makes its plugin
/// start fresh (it reloads what is on disk, as a new process would) and every file be transformed; the plugin's own
/// write, and the same bytes written again, change nothing. A plugin that reads its own state file as a build input
/// (Flamework) does not take its own write for a changed input, once its setup has seen the file there: a build whose
/// commit created it (its setup read it missing) asks for one more build, which runs that setup again, as a watcher
/// does at once. A failing build writes nothing, and the next one still finds the file lost.
#[test]
fn a_lost_state_file_starts_its_plugin_fresh_in_watch() {
    let Some((copy, root)) = keeper_copy("sfile", false) else { return };
    let counts = Arc::new(KeeperCounts::default());
    let plugins = vec![erase(Keeper { counts: Arc::clone(&counts), reads_own: true })];
    let state_path = root.join(KEEPER_STATE);
    let key = state_path.to_string_lossy().replace('\\', "/");
    let path = |p: &str| root.join(p).to_string_lossy().replace('\\', "/");
    let write = |p: &str, text: &str| std::fs::write(root.join(p), text).unwrap();
    let state = || std::fs::read_to_string(&state_path).ok();
    let setups = || counts.setups.load(Ordering::SeqCst);
    let fresh = || counts.fresh_watch_setups.load(Ordering::SeqCst);
    let created = |summary: &BuildSummary| {
        assert_eq!(summary.setup_inputs_changed.len(), 1, "{:?}", summary.setup_inputs_changed);
        assert!(summary.setup_inputs_changed[0].replace('\\', "/").eq_ignore_ascii_case(&key));
    };
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = keeper_session(&root, &options, true);
        let (report, summary) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(summary.all);
        assert_eq!(state().as_deref(), Some("1"));
        assert_eq!(session.state_files().len(), 1);
        assert!(
            session.state_files()[0].path.replace('\\', "/").eq_ignore_ascii_case(&key),
            "{:?}",
            session.state_files()
        );
        // setup read it missing, the commit created it: one more build runs setup again (from the memory state)
        created(&summary);
        let (report, summary) = session.build(&summary.setup_inputs_changed);
        assert!(!report.has_errors());
        assert!(summary.all);
        assert!(summary.setup_inputs_changed.is_empty(), "its own write again is not a change");
        assert_eq!((setups(), fresh()), (2, 0));
        assert_eq!(state().as_deref(), Some("2"));

        // the plugin's own write (setup read the file before the commit wrote it): an edit is the closure only
        write("src/shared/d.ts", D_TS);
        let (report, summary) = session.build(&[path("src/shared/d.ts")]);
        assert!(!report.has_errors());
        assert!(!summary.all, "the plugin's own write is not a changed input");
        assert!(summary.lost_state_files.is_empty() && summary.setup_inputs_changed.is_empty());
        assert_eq!(setups(), 2, "the Shared is kept");
        assert_eq!(state().as_deref(), Some("3"));

        // deleted: fresh (nothing to reload), every file; the commit creates it again, so setup runs once more
        std::fs::remove_file(&state_path).unwrap();
        let (report, summary) = session.build(std::slice::from_ref(&key));
        assert!(!report.has_errors());
        assert!(summary.all);
        assert_eq!(summary.lost_state_files.len(), 1);
        assert_eq!(report.files.len(), report.project_files);
        assert_eq!((setups(), fresh()), (3, 1));
        assert_eq!(state().as_deref(), Some("1"));
        created(&summary);
        let (report, summary) = session.build(&summary.setup_inputs_changed);
        assert!(!report.has_errors());
        assert!(summary.all && summary.lost_state_files.is_empty() && summary.setup_inputs_changed.is_empty());
        assert_eq!((setups(), fresh()), (4, 1));
        assert_eq!(state().as_deref(), Some("2"));

        // edited: fresh, reloading the edited value (setup read it there: no build more)
        write(KEEPER_STATE, "41");
        let (report, summary) = session.build(std::slice::from_ref(&key));
        assert!(!report.has_errors());
        assert!(summary.all);
        assert!(summary.setup_inputs_changed.is_empty());
        assert_eq!((setups(), fresh()), (5, 2));
        assert_eq!(state().as_deref(), Some("42"));

        // the same bytes again: nothing transformed, the Shared kept
        write(KEEPER_STATE, "42");
        let (report, summary) = session.build(std::slice::from_ref(&key));
        assert!(!report.has_errors());
        assert!(!summary.all);
        assert!(summary.lost_state_files.is_empty());
        assert_eq!(report.files.len(), 0);
        assert_eq!(setups(), 5);
        assert_eq!(state().as_deref(), Some("43"));

        // back to the bytes before the last commit
        write(KEEPER_STATE, "42");
        let (report, summary) = session.build(std::slice::from_ref(&key));
        assert!(!report.has_errors());
        assert!(summary.all);
        assert!(summary.setup_inputs_changed.is_empty());
        assert_eq!((setups(), fresh()), (6, 3));
        assert_eq!(state().as_deref(), Some("43"));

        // deleted while a build fails: nothing written, and the next build still finds it lost
        std::fs::remove_file(&state_path).unwrap();
        write("src/shared/d.ts", D_TS_BROKEN);
        let (report, summary) = session.build(&[key.clone(), path("src/shared/d.ts")]);
        assert!(report.has_errors());
        assert_eq!(summary.lost_state_files.len(), 1);
        assert!(summary.setup_inputs_changed.is_empty(), "a failing build commits nothing");
        assert_eq!(state(), None);
        write("src/shared/d.ts", D_TS);
        let (report, summary) = session.build(&[path("src/shared/d.ts")]);
        assert!(!report.has_errors());
        assert!(summary.all);
        assert_eq!(summary.lost_state_files.len(), 1);
        assert_eq!(state().as_deref(), Some("1"));
        created(&summary);
    });
    drop(copy);
}

/// Incremental builds (a new session per build): the state files are in the incremental state; one deleted or
/// edited since makes every file be transformed (the parts on disk were made with the state it held); the same
/// bytes transform nothing.
#[test]
fn a_lost_state_file_transforms_everything_in_an_incremental_build() {
    let Some((copy, root)) = keeper_copy("sfile-inc", true) else { return };
    let counts = Arc::new(KeeperCounts::default());
    let plugins = vec![erase(Keeper { counts: Arc::clone(&counts), reads_own: false })];
    let state_path = root.join(KEEPER_STATE);
    let state = || std::fs::read_to_string(&state_path).ok();
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let build = || {
            let mut session = keeper_session(&root, &options, false);
            let (report, summary) = session.build(&[]);
            assert!(!report.has_errors(), "{:?}", report.fatal);
            (report.files.len(), summary)
        };
        let (_, summary) = build();
        assert_eq!(summary.kind, Some(BuildKind::Clean));
        assert_eq!(state().as_deref(), Some("1"));

        let (transformed, summary) = build();
        assert_eq!(summary.kind, Some(BuildKind::Incremental));
        assert_eq!((transformed, summary.all), (0, false));
        assert_eq!(state().as_deref(), Some("2"));

        std::fs::remove_file(&state_path).unwrap();
        let (transformed, summary) = build();
        assert!(summary.all && transformed > 0);
        assert_eq!(summary.lost_state_files.len(), 1);
        assert_eq!(state().as_deref(), Some("1"));

        std::fs::write(&state_path, "41").unwrap();
        let (_, summary) = build();
        assert!(summary.all);
        assert_eq!(state().as_deref(), Some("42"));

        std::fs::write(&state_path, "42").unwrap();
        let (transformed, summary) = build();
        assert_eq!((transformed, summary.all), (0, false));
        assert!(summary.lost_state_files.is_empty());
        assert_eq!(state().as_deref(), Some("43"));
    });
    drop(copy);
}

// --- a Shared that depends on whether the plugin's own state file exists (Flamework's candidate scan) ---

const SCAN_DTS: &str = "/** The call becomes the scan plugin's answer. @scan */\ndeclare function scanned(): string;\n";
const SCANNED_TS: &str = "export const answer = scanned();\n";

/// A copy of the basic fixture with the `scan` plugin's declaration, a file calling it, and its ancestor file.
fn scan_copy(label: &str, incremental: bool) -> Option<(Copy, PathBuf)> {
    let (copy, root) = keeper_copy(label, incremental)?;
    std::fs::write(root.join("src/shared/scan.d.ts"), SCAN_DTS).unwrap();
    std::fs::write(root.join("src/shared/scanned.ts"), SCANNED_TS).unwrap();
    std::fs::write(root.join(scan::ANCESTOR_FILE), "first").unwrap();
    Some((copy, root))
}

type Tree = std::collections::BTreeMap<String, Vec<u8>>;

/// Every file under `dir`, by relative path.
fn tree(dir: &Path) -> Tree {
    fn walk(dir: &Path, base: &Path, out: &mut Tree) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.map(Result::unwrap) {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(&path, base, out);
            } else {
                let relative = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = Tree::new();
    walk(dir, dir, &mut out);
    out
}

/// The outputs of a clean build of a copy of `root`'s sources, seeded with its state file and its ancestor file as
/// they are now: what a new process builds from what the watcher left.
fn seeded_clean(root: &Path, options: &PluginOptions<'_>) -> Tree {
    let clean = root.with_file_name(format!("{}-clean", root.file_name().unwrap().to_string_lossy()));
    parity::remove_tree(&clean);
    let copy = Copy(clean.clone());
    let names = ["src", "tsconfig.json", "default.project.json", "package.json", scan::STATE_FILE, scan::ANCESTOR_FILE];
    for name in names {
        let from = root.join(name);
        if from.is_dir() {
            parity::copy_tree(&from, &clean.join(name)).unwrap();
        } else if from.exists() {
            std::fs::copy(&from, clean.join(name)).unwrap();
        }
    }
    parity::junction(&clean.join("node_modules"), &node_modules().unwrap()).unwrap();
    let tsconfig = std::fs::read_to_string(clean.join("tsconfig.json")).unwrap();
    std::fs::write(clean.join("tsconfig.json"), tsconfig.replace("\"incremental\": true,", "")).unwrap();
    let mut session = keeper_session(&clean, options, false);
    let (report, summary) = session.build(&[]);
    assert!(!report.has_errors(), "{:?}", report.fatal);
    assert_eq!(summary.kind, Some(BuildKind::Clean));
    let out = tree(&clean.join("out"));
    drop(copy);
    out
}

/// What the scanned file compiled to.
fn scanned_output(root: &Path) -> String {
    std::fs::read_to_string(root.join("out/shared/scanned.luau")).unwrap()
}

/// Watch, with a plugin whose `Shared` depends on whether its own state file exists (`scan`, as Flamework's candidate
/// scan loads an ancestor `flamework.build` while its own is missing): a build whose commit creates the file its
/// setup read as missing asks for one more build (`setup_inputs_changed`), and that one runs setup again, so the
/// outputs equal a clean build seeded with the state file from then on; that build asks for no other (its new bytes
/// are not a change), nor does an edit. Deleting the file does the same once more; the ancestor changing while the
/// file is there changes nothing, as in a clean build.
#[test]
fn a_state_file_the_commit_creates_runs_setup_again_at_once() {
    let Some((copy, root)) = scan_copy("scan", false) else { return };
    let plugins = vec![erase(scan::Scan)];
    let state_path = root.join(scan::STATE_FILE);
    let key = state_path.to_string_lossy().replace('\\', "/");
    let path = |p: &str| root.join(p).to_string_lossy().replace('\\', "/");
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = keeper_session(&root, &options, true);
        // as a watcher: a build, then one more at once when it asks
        let mut build = |touched: &[String]| {
            let (report, summary) = session.build(touched);
            assert!(!report.has_errors(), "{:?}", report.fatal);
            if summary.setup_inputs_changed.is_empty() {
                return (summary, None);
            }
            assert_eq!(summary.setup_inputs_changed.len(), 1, "{:?}", summary.setup_inputs_changed);
            assert!(summary.setup_inputs_changed[0].replace('\\', "/").eq_ignore_ascii_case(&key));
            let (report, again) = session.build(&summary.setup_inputs_changed);
            assert!(!report.has_errors(), "{:?}", report.fatal);
            assert!(again.all, "setup ran again: every file");
            assert!(again.setup_inputs_changed.is_empty(), "a build started that way asks for no other");
            (summary, Some(again))
        };

        // the first build reads the ancestor; the one after it, the state file the first created
        let (summary, again) = build(&[]);
        assert!(summary.all && again.is_some());
        assert!(scanned_output(&root).contains("\"own\""), "{}", scanned_output(&root));
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));

        // an edit: the closure, no build more
        std::fs::write(root.join("src/shared/d.ts"), D_TS).unwrap();
        let (summary, again) = build(&[path("src/shared/d.ts")]);
        assert!(!summary.all && again.is_none());
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));

        // the ancestor changes while the state file is there: setup did not read it
        std::fs::write(root.join(scan::ANCESTOR_FILE), "second").unwrap();
        let (summary, again) = build(&[path(scan::ANCESTOR_FILE)]);
        assert!(!summary.all && again.is_none());
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));

        // deleted: the plugin starts fresh, reads the ancestor, and the commit creates the file again
        std::fs::remove_file(&state_path).unwrap();
        let (summary, again) = build(std::slice::from_ref(&key));
        assert_eq!(summary.lost_state_files.len(), 1);
        assert!(again.is_some());
        assert!(scanned_output(&root).contains("\"own\""));
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));

        // edited by hand: fresh, but setup reads it there: no build more
        std::fs::write(&state_path, "41\n").unwrap();
        let (summary, again) = build(std::slice::from_ref(&key));
        assert!(summary.all && again.is_none());
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));
    });
    drop(copy);
}

/// The same in incremental builds (a new session per build): the incremental state keeps what setup read, so the
/// build after the one that created the state file finds that read changed and transforms everything with a new
/// setup; the outputs equal a clean build seeded with the state file from then on.
#[test]
fn a_state_file_the_commit_creates_reruns_setup_in_the_next_incremental_build() {
    let Some((copy, root)) = scan_copy("scan-inc", true) else { return };
    let plugins = vec![erase(scan::Scan)];
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let build = || {
            let mut session = keeper_session(&root, &options, false);
            let (report, summary) = session.build(&[]);
            assert!(!report.has_errors(), "{:?}", report.fatal);
            summary
        };
        let summary = build();
        assert_eq!(summary.kind, Some(BuildKind::Clean));
        assert_eq!(summary.setup_inputs_changed.len(), 1);
        assert!(scanned_output(&root).contains("\"ancestor first\""), "{}", scanned_output(&root));

        let summary = build();
        assert_eq!(summary.kind, Some(BuildKind::Incremental));
        assert!(summary.all, "what setup read changed");
        assert!(summary.setup_inputs_changed.is_empty());
        assert!(scanned_output(&root).contains("\"own\""));
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));

        // a new process starts the count over, so it wrote the same bytes: nothing changed, nothing transformed
        let summary = build();
        assert!(!summary.all && summary.setup_inputs_changed.is_empty());
        assert_eq!(tree(&root.join("out")), seeded_clean(&root, &options));
    });
    drop(copy);
}

/// Another plugin's state file as a setup input (`scan-reader` reads the one `scan` writes, new bytes at every
/// build): a commit that changes what the reader read asks for one more build (`setup_inputs_changed`, as for a
/// plugin's own state file created), and the reader's read is kept as it was, so the next build runs its setup again
/// and its output follows the bytes that build started with. `scan`'s own new bytes alone still ask for nothing.
#[test]
fn another_plugins_state_file_is_a_setup_input_like_any_other() {
    let Some((copy, root)) = scan_copy("scan-reader", false) else { return };
    std::fs::write(root.join("src/shared/reader.d.ts"), "/** @scanreader */\ndeclare function readState(): string;\n")
        .unwrap();
    std::fs::write(root.join("src/shared/read.ts"), "export const text = readState();\n").unwrap();
    let plugins = vec![erase(scan::Scan), erase(scan::ScanReader)];
    let state_path = root.join(scan::STATE_FILE);
    let path = |p: &str| root.join(p).to_string_lossy().replace('\\', "/");
    let read_output = || std::fs::read_to_string(root.join("out/shared/read.luau")).unwrap();
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = keeper_session(&root, &options, true);
        // both read the state file as missing; scan's commit creates it
        let (report, summary) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(read_output().contains("\"reader none\""), "{}", read_output());
        assert_eq!(summary.setup_inputs_changed.len(), 1, "{:?}", summary.setup_inputs_changed);

        // the build a watcher starts at once: the reader reads "1", scan's commit leaves "2", which the reader's
        // setup did not read
        let (report, summary) = session.build(&summary.setup_inputs_changed);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(read_output().contains("\"reader 1\""), "{}", read_output());
        assert_eq!(std::fs::read_to_string(&state_path).unwrap().trim(), "2");
        assert_eq!(summary.setup_inputs_changed.len(), 1, "{:?}", summary.setup_inputs_changed);
        assert!(
            report
                .plugin_messages
                .iter()
                .any(|m| m.starts_with("scan-reader: ") && m.contains("the state file of scan")),
            "{:?}",
            report.plugin_messages
        );

        // the next build (an edit) runs the reader's setup again: it reads "2"
        std::fs::write(root.join("src/shared/d.ts"), D_TS).unwrap();
        let (report, _) = session.build(&[path("src/shared/d.ts")]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(read_output().contains("\"reader 2\""), "{}", read_output());
    });
    drop(copy);
}

// --- the clean keeps what roblox-ts keeps ---

/// A declaration build (`declaration`, `declarationMap`, `declarationDir`, an explicit `tsBuildInfoFile` under
/// `outDir`) over a used output directory: the clean keeps every file roblox-ts's `getProjectOutputs` keeps (the
/// `.d.ts` and `.d.ts.map` TypeScript emits for each file, `index.d.ts` beside `init.luau` included; the outputs of
/// project `.d.ts` assets; tsc's build info and roblox-ts's), deletes what it does not, and removes a junction inside
/// the output directory as a link, leaving its target alone.
#[test]
fn the_clean_keeps_what_roblox_ts_keeps_and_removes_links_as_links() {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    };
    let root = std::env::temp_dir().join(format!("rbxts-rs-watch-keep-{}", std::process::id()));
    let sentinel = std::env::temp_dir().join(format!("rbxts-rs-watch-keep-sentinel-{}", std::process::id()));
    parity::remove_tree(&root);
    let copy = Copy(root.clone());
    parity::copy_tree(&fixtures().join("basic"), &root).unwrap();
    parity::junction(&root.join("node_modules"), &node_modules).unwrap();
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap().replace(
        "\"compilerOptions\": {",
        "\"compilerOptions\": {\n\t\t\"declaration\": true,\n\t\t\"declarationMap\": true,\n\t\t\"declarationDir\": \"types\",\n\t\t\"incremental\": true,\n\t\t\"tsBuildInfoFile\": \"out/info.tsbuildinfo\",",
    );
    std::fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
    std::fs::write(root.join("src/shared/index.ts"), "export const idx = 1;\n").unwrap();
    let put = |p: &str| {
        let path = root.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "user file\n").unwrap();
    };
    let kept = [
        "types/shared/index.d.ts",
        "types/shared/index.d.ts.map",
        "types/shared/c.d.ts",
        "types/shared/c.d.ts.map",
        "out/shared/globals.d.ts",
        "out/info.tsbuildinfo",
        "out/info.rbxtsc.tsbuildinfo",
    ];
    let stale = ["types/shared/gone.d.ts", "out/shared/gone.luau", "out/shared/init.d.ts"];
    for p in kept.iter().chain(&stale) {
        put(p);
    }
    std::fs::create_dir_all(&sentinel).unwrap();
    std::fs::write(sentinel.join("precious.txt"), "keep me\n").unwrap();
    parity::junction(&root.join("out/linked"), &sentinel).unwrap();

    let plugins: Vec<Box<dyn DynPlugin>> = Vec::new();
    let (report, _) = big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = BuildSession::new(
            CompileOptions {
                tsconfig: "tsconfig.json".into(),
                cwd: root.to_string_lossy().into_owned(),
                project_options: ProjectOptionLayer::default(),
                out_dir_override: None,
                write_partial: false,
                no_write: false,
                checkers: None,
            },
            &options,
            false,
        );
        let (report, summary) = session.build(&[]);
        (report.fatal.clone(), summary)
    });
    assert!(report.is_empty(), "{report:?}");
    for p in kept {
        assert!(root.join(p).is_file(), "{p} was deleted");
    }
    for p in stale {
        assert!(!root.join(p).exists(), "{p} was kept");
    }
    assert!(root.join("out/shared/init.luau").is_file());
    assert!(std::fs::symlink_metadata(root.join("out/linked")).is_err(), "the junction is removed");
    assert_eq!(std::fs::read_to_string(sentinel.join("precious.txt")).unwrap(), "keep me\n", "its target is not");
    let _ = std::fs::remove_dir_all(&sentinel);
    drop(copy);
}

/// Writes one artifact at a fixed path.
struct ArtifactAt(&'static str);

impl Plugin for ArtifactAt {
    type Shared = ();
    type State = ();
    type FilePart = ();
    type Instance = CounterInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "artifact-at", version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, _cx: &mut SetupCx<'_>, _state: &mut ()) -> Result<Arc<()>, Fatal> {
        Ok(Arc::new(()))
    }

    fn instance(&self, _shared: Arc<()>, _checker: CheckerInfo) -> CounterInstance {
        CounterInstance
    }

    fn finish(
        &self,
        _shared: &(),
        _state: &mut (),
        _parts: &[(FileIndex, &())],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        cx.write_artifact(self.0, b"plugin bytes\n".to_vec());
        Ok(())
    }
}

/// A plugin artifact at an asset's output path fails the build, naming the plugin, and the asset is not overwritten.
#[test]
fn an_artifact_may_not_overwrite_an_asset_output() {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    };
    let root = std::env::temp_dir().join(format!("rbxts-rs-watch-artifact-{}", std::process::id()));
    parity::remove_tree(&root);
    let copy = Copy(root.clone());
    parity::copy_tree(&fixtures().join("basic"), &root).unwrap();
    parity::junction(&root.join("node_modules"), &node_modules).unwrap();
    let plugins = vec![erase(ArtifactAt("out/shared/asset.lua"))];
    let fatal = big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = BuildSession::new(
            CompileOptions {
                tsconfig: "tsconfig.json".into(),
                cwd: root.to_string_lossy().into_owned(),
                project_options: ProjectOptionLayer::default(),
                out_dir_override: None,
                write_partial: false,
                no_write: false,
                checkers: None,
            },
            &options,
            false,
        );
        session.build(&[]).0.fatal
    });
    assert!(fatal.iter().any(|f| f.contains("artifact-at") && f.contains("is an output of this build")), "{fatal:?}");
    assert!(!root.join("out").exists(), "a failing build writes nothing");
    drop(copy);
}

#[test]
fn a_watch_rebuild_load_checks_what_a_plugin_emitted_into() {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    };
    let root = std::env::temp_dir().join(format!("rbxts-rs-watch-luau-{}", std::process::id()));
    parity::remove_tree(&root);
    let copy = Copy(root.clone());
    parity::copy_tree(&fixtures().join("basic"), &root).unwrap();
    parity::junction(&root.join("node_modules"), &node_modules).unwrap();
    let plugins = vec![rbxts_test_plugins::by_name("identity-call").expect("the identity-call plugin")];
    let path = |p: &str| root.join(p).to_string_lossy().replace('\\', "/");
    // `count` locals from calls the plugin claims: Luau allows 200
    let big = |count: usize| {
        let locals: String = (0..count).map(|i| format!("const a{i} = mk();\n")).collect();
        std::fs::write(
            root.join("src/shared/big.ts"),
            format!("declare function mk(): number;\n{locals}export const total = a0 + a1;\n"),
        )
        .unwrap();
    };
    let output = root.join("out/shared/big.luau");
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = BuildSession::new(
            CompileOptions {
                tsconfig: "tsconfig.json".into(),
                cwd: root.to_string_lossy().into_owned(),
                project_options: ProjectOptionLayer::default(),
                out_dir_override: None,
                write_partial: false,
                no_write: false,
                checkers: None,
            },
            &options,
            true,
        );
        big(150);
        let (report, _) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(report.timings.load_checked > 0);
        let written = std::fs::read_to_string(&output).unwrap();

        // the rebuild Luau refuses fails, and the old output stays as it was
        big(205);
        let (report, _) = session.build(&[path("src/shared/big.ts")]);
        let refused: Vec<String> = report
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter())
            .map(|d| tsrs_compiler::diagnosticwriter::flatten_diagnostic_message(*d, " "))
            .collect();
        assert!(
            refused.iter().any(|m| m.contains("refuses this file") && m.contains("Out of local registers")),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_to_string(&output).unwrap(), written, "a failing rebuild writes nothing");

        // fixed, it is written
        big(190);
        let (report, _) = session.build(&[path("src/shared/big.ts")]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(std::fs::read_to_string(&output).unwrap().contains("local a189 = mk()"));
    });
    drop(copy);
}

#[test]
fn a_late_string_that_makes_a_cached_file_refused_fails_the_watch_rebuild() {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    };
    // round 7's validator case: `a.ts` compares the stamp plugin's build id (a late string) with a constant, and a
    // branch of 201 locals hangs on it; Luau folds the comparison at optimisation level 1 and drops a dead branch
    let root = std::env::temp_dir().join(format!("rbxts-rs-watch-late-{}", std::process::id()));
    parity::remove_tree(&root);
    let copy = Copy(root.clone());
    let basic = fixtures().join("basic");
    std::fs::create_dir_all(root.join("src/shared")).unwrap();
    for f in ["default.project.json", "package.json", "tsconfig.json"] {
        std::fs::copy(basic.join(f), root.join(f)).unwrap();
    }
    parity::junction(&root.join("node_modules"), &node_modules).unwrap();
    let plugins = vec![rbxts_test_plugins::by_name("stamp").expect("the stamp plugin")];
    let path = |p: &str| root.join(p).to_string_lossy().replace('\\', "/");
    let locals: String = (0..201).map(|i| format!("\tconst a{i} = mk();\n")).collect();
    let a = format!(
        "/** @stamp build */\ndeclare function buildId(): string;\ndeclare function mk(): number;\nprint(1);\n\
         if (buildId() === \"watch #1/1\") {{\n{locals}\tprint(a0, a200);\n}}\nexport {{}};\n"
    );
    std::fs::write(root.join("src/shared/a.ts"), &a).unwrap();
    let b_site = "/** @stamp build */\ndeclare function buildId(): string;\nexport const id = buildId();\n";
    std::fs::write(root.join("src/shared/b.ts"), b_site).unwrap();
    let output = root.join("out/shared/a.luau");
    let messages = |report: &rbxts_transform::CompileReport| -> Vec<(String, String)> {
        report
            .diagnostics
            .iter()
            .chain(report.files.iter().flat_map(|f| f.diagnostics.iter()))
            .map(|d| {
                let at = d.pos().map(|p| a[p as usize..].lines().next().unwrap_or("").to_owned()).unwrap_or_default();
                (tsrs_compiler::diagnosticwriter::flatten_diagnostic_message(*d, " "), at)
            })
            .collect()
    };
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = BuildSession::new(
            CompileOptions {
                tsconfig: "tsconfig.json".into(),
                cwd: root.to_string_lossy().into_owned(),
                project_options: ProjectOptionLayer::default(),
                out_dir_override: None,
                write_partial: false,
                no_write: false,
                checkers: None,
            },
            &options,
            true,
        );
        // two build sites: the id is "watch #1/2", the branch is dead (a warning: it loads only with folding)
        let (report, _) = session.build(&[]);
        assert!(!report.has_errors(), "{:?} {:?}", report.fatal, messages(&report));
        let written = std::fs::read_to_string(&output).unwrap();
        assert!(written.contains("\"watch #1/2\" == \"watch #1/1\""), "{written}");

        // only b.ts changes: a.ts is cached and re-rendered with "watch #1/1", which keeps the branch
        std::fs::write(root.join("src/shared/b.ts"), "export const id = \"none\";\n").unwrap();
        let (report, summary) = session.build(&[path("src/shared/b.ts")]);
        let refused: Vec<(String, String)> =
            messages(&report).into_iter().filter(|(m, _)| m.contains("refuses this file")).collect();
        assert_eq!(refused.len(), 1, "{:?}", messages(&report));
        assert!(refused[0].0.contains("Out of local registers"), "{refused:?}");
        assert!(refused[0].1.starts_with("if (buildId() === \"watch #1/1\")"), "anchored at the `if`: {refused:?}");
        assert!(summary.written.is_empty(), "{:?}", summary.written);
        assert_eq!(std::fs::read_to_string(&output).unwrap(), written, "a failing rebuild writes nothing");
    });
    drop(copy);
}

/// Every entry under `dir` (files and directories), by relative path.
fn entries(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.map(Result::unwrap) {
            let path = entry.path();
            out.push(path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            if entry.file_type().unwrap().is_dir() {
                walk(&path, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The Luau outputs are written all or nothing (DESIGN §14: a failing build writes nothing): with two outputs blocked
/// by directories, the build fails naming both, and no other output is written, no temporary file is left, and no
/// directory the staging created stays. With the paths free again, the next build writes them.
#[test]
fn a_luau_write_error_writes_no_other_output() {
    let Some((copy, root)) = keeper_copy("blocked", false) else { return };
    let blocked = ["out/shared/c.luau", "out/shared/d.luau"];
    for dir in blocked {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(root.join(dir).join("keep.txt"), "x").unwrap();
    }
    let before = entries(&root.join("out"));
    big_stack(|| {
        let options = PluginOptions { ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = keeper_session(&root, &options, false);
        let (report, summary) = session.build(&[]);
        let failed: Vec<&String> = report.fatal.iter().filter(|m| m.starts_with("could not write")).collect();
        assert_eq!(failed.len(), 2, "{:?}", report.fatal);
        for (message, dir) in failed.iter().zip(blocked) {
            assert!(message.replace('\\', "/").contains(dir), "{message}");
        }
        assert!(summary.written.is_empty(), "{:?}", summary.written);
        assert_eq!(entries(&root.join("out")), before, "nothing written, no temporary file, no new directory");

        for dir in blocked {
            std::fs::remove_dir_all(root.join(dir)).unwrap();
        }
        let (report, summary) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert!(!summary.written.is_empty());
        for file in blocked {
            assert!(root.join(file).is_file(), "{file}");
        }
    });
    drop(copy);
}

/// Opens `path` the way an editor or Rojo may hold it: reading and writing shared, deleting not, so nothing can
/// rename over it or rename it.
fn hold(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(path).unwrap()
}

/// A path as a watcher reports it to the session: forward slashes.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The `.rbxts-tmp-` and `.rbxts-old-` files a commit leaves under `dir`.
fn commit_leftovers(dir: &Path) -> Vec<String> {
    entries(dir).into_iter().filter(|e| e.contains(".rbxts-tmp-") || e.contains(".rbxts-old-")).collect()
}

/// A rename into place that fails after others went through (an output another process holds past the retries)
/// rolls the commit back: the outputs already replaced get their old bytes again, no temporary file or kept copy is
/// left, and the build reports the held one. Released, the next build writes both.
#[test]
fn a_failed_rename_into_place_rolls_the_commit_back() {
    let Some((copy, root)) = keeper_copy("rollback", false) else { return };
    big_stack(|| {
        let options = PluginOptions { ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = keeper_session(&root, &options, true);
        let (report, _) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        let read = |file: &str| std::fs::read(root.join(file)).unwrap();
        let (c_before, d_before) = (read("out/shared/c.luau"), read("out/shared/d.luau"));
        let src = |file: &str| root.join("src/shared").join(file);
        for file in ["c.ts", "d.ts"] {
            let text = std::fs::read_to_string(src(file)).unwrap();
            std::fs::write(src(file), format!("{text}\nexport const added = 1;\n")).unwrap();
        }
        let touched: Vec<String> = ["c.ts", "d.ts"].iter().map(|f| slashed(&src(f))).collect();

        let held = hold(&root.join("out/shared/d.luau"));
        let (report, summary) = session.build(&touched);
        let failed: Vec<&String> = report.fatal.iter().filter(|m| m.starts_with("could not write")).collect();
        assert_eq!(failed.len(), 1, "{:?}", report.fatal);
        assert!(failed[0].replace('\\', "/").contains("out/shared/d.luau"), "{}", failed[0]);
        assert!(!failed[0].contains("rolling back left"), "{}", failed[0]);
        assert!(summary.written.is_empty(), "{:?}", summary.written);
        assert_eq!(read("out/shared/c.luau"), c_before, "c.luau has its old bytes again");
        assert_eq!(read("out/shared/d.luau"), d_before);
        assert_eq!(commit_leftovers(&root.join("out")), Vec::<String>::new());
        drop(held);

        let (report, summary) = session.build(&touched);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        assert_eq!(summary.written.len(), 2, "{:?}", summary.written);
        assert_ne!(read("out/shared/c.luau"), c_before);
        assert_ne!(read("out/shared/d.luau"), d_before);
        assert_eq!(commit_leftovers(&root.join("out")), Vec::<String>::new());
    });
    drop(copy);
}

/// Case-only renames of two sources whose outputs another process holds: both failures are reported (not only the
/// first), nothing is written, and the case renames made are undone.
#[test]
fn every_failed_case_rename_is_reported() {
    let Some((copy, root)) = keeper_copy("caserename", false) else { return };
    big_stack(|| {
        let options = PluginOptions { ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = keeper_session(&root, &options, true);
        let (report, _) = session.build(&[]);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        let shared = root.join("src/shared");
        let mut touched = Vec::new();
        for (from, to) in [("useAug.ts", "UseAug.ts"), ("useDecl.ts", "UseDecl.ts")] {
            std::fs::rename(shared.join(from), shared.join(to)).unwrap();
            touched.push(slashed(&shared.join(from)));
            touched.push(slashed(&shared.join(to)));
        }
        let before = entries(&root.join("out"));
        let held = [hold(&root.join("out/shared/useAug.luau")), hold(&root.join("out/shared/useDecl.luau"))];
        let (report, summary) = session.build(&touched);
        let failed: Vec<String> =
            report.fatal.iter().filter(|m| m.starts_with("could not rename")).map(|m| m.replace('\\', "/")).collect();
        assert_eq!(failed.len(), 2, "{:?}", report.fatal);
        assert!(failed[0].contains("out/shared/UseAug.luau"), "{failed:?}");
        assert!(failed[1].contains("out/shared/UseDecl.luau"), "{failed:?}");
        assert!(summary.written.is_empty(), "{:?}", summary.written);
        assert_eq!(entries(&root.join("out")), before, "the old names, nothing written");
        drop(held);

        let (report, _) = session.build(&touched);
        assert!(!report.has_errors(), "{:?}", report.fatal);
        let names = entries(&root.join("out/shared"));
        assert!(names.contains(&"UseAug.luau".to_owned()) && names.contains(&"UseDecl.luau".to_owned()), "{names:?}");
    });
    drop(copy);
}
