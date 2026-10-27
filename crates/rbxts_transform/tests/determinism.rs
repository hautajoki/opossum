//! S3-par: what a build writes and reports does not depend on the checker count, on which checker gets which file,
//! or on the order a checker visits its files.
//!
//! Every transform fixture (`tests/fixtures/<area>` without links) and the plugin fixture (`tests/plugin_fixtures/probe`
//! with the `stamp`, `probe` and `probe-exprcall` plugins, which write artifacts and a state file) is built, with
//! writes, in a fresh copy: once with one checker, then with 2, 4 and 8 under tsrs's `random:<seed>` assignment (a
//! random checker per file and a random visit order per checker) and with 4 under the default assignment. Each build
//! must leave the same files with the same bytes (outputs, artifacts, state files; the sources and `node_modules`
//! aside) and report the same diagnostics, warnings and fatal errors as the one-checker build.
//!
//! This binary holds this one test, so it may set `TSRS_CHECKER_ASSIGNMENT` between builds: no other thread is
//! running then. Without a `node_modules` (see `plugins.rs`) it is skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rbxts_plugin::erased::DynPlugin;
use rbxts_project::{ProjectOptionLayer, STACK_SIZE};
use rbxts_transform::{CompileOptions, CompileReport, PluginOptions, compile_project_with};
use tsrs_compiler::diagnosticwriter::flatten_diagnostic_message;

/// (checkers, `TSRS_CHECKER_ASSIGNMENT`); the first is the reference.
const CONFIGS: &[(usize, Option<&str>)] =
    &[(1, None), (2, Some("random:21")), (4, Some("random:22")), (8, Some("random:23")), (4, None)];

fn node_modules() -> Option<PathBuf> {
    let path = rbxts_test_support::node_modules().unwrap_or_default();
    path.join("@rbxts").join("compiler-types").is_dir().then_some(path)
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

/// The copy's files with their bytes, but for its sources and `node_modules` (never followed).
fn tree(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if relative == "node_modules" || relative == "src" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            tree(root, &path, out);
        } else {
            out.insert(relative, std::fs::read(&path).unwrap());
        }
    }
}

/// What a build reported, one line each, with paths relative to the copy.
fn reported(report: &CompileReport, root: &Path) -> Vec<String> {
    let root = root.to_string_lossy().replace('\\', "/");
    let relative = |text: String| text.replace('\\', "/").replace(&root, "<root>");
    let mut lines: Vec<String> = report.fatal.iter().map(|m| relative(format!("fatal: {m}"))).collect();
    lines.extend(report.warnings.iter().map(|w| relative(format!("warning: {w}"))));
    for d in report.diagnostics.iter().chain(report.files.iter().flat_map(|f| f.diagnostics.iter())) {
        let file = d.file().map(|f| f.file_name().to_owned()).unwrap_or_default();
        let pos = d.pos().unwrap_or(0);
        lines.push(relative(format!("{:?} {file}@{pos}: {}", d.category(), flatten_diagnostic_message(*d, " "))));
    }
    lines.extend(report.files.iter().map(|f| relative(format!("file {} written={}", f.out_path, f.luau.is_some()))));
    lines
}

/// Builds a fresh copy of `fixture` (everything but `expected*/` and `links.json`) with `plugins` under `config`; returns the copy's tree and the
/// report's lines.
fn build(
    fixture: &Path,
    plugins: &[&str],
    label: &str,
    node_modules: &Path,
    (checkers, assignment): (usize, Option<&str>),
) -> (BTreeMap<String, Vec<u8>>, Vec<String>) {
    // one copy path for every config: the incremental state of a composite project (`project_references`) holds
    // absolute paths, which must be the same for its bytes to be compared
    let root = std::env::temp_dir().join(format!("rbxts-rs-determinism-{label}-{}", std::process::id()));
    remove_copy(&root);
    std::fs::create_dir_all(&root).unwrap();
    for entry in std::fs::read_dir(fixture).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("expected") || name == "links.json" || name == "plugins.json" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &root.join(&name));
        } else {
            std::fs::copy(entry.path(), root.join(&name)).unwrap();
        }
    }
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("node_modules"))
        .arg(node_modules.to_string_lossy().replace('/', "\\"))
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "could not link node_modules");
    // SAFETY: this binary's only test runs here, between builds; no other thread reads the environment now
    unsafe {
        match assignment {
            Some(assignment) => std::env::set_var("TSRS_CHECKER_ASSIGNMENT", assignment),
            None => std::env::remove_var("TSRS_CHECKER_ASSIGNMENT"),
        }
    }
    let plugins: Vec<Box<dyn DynPlugin>> =
        plugins.iter().map(|name| rbxts_test_plugins::by_name(name).expect("a test plugin")).collect();
    let cwd = root.to_string_lossy().into_owned();
    let report = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, || {
                compile_project_with(
                    &CompileOptions {
                        tsconfig: "tsconfig.json".into(),
                        cwd,
                        project_options: ProjectOptionLayer::default(),
                        out_dir_override: None,
                        // the once-per-compilation case keeps every file's diagnostics (rbxts-rs's partial mode): a
                        // failing build reports no file after the first with an error, as roblox-ts's
                        write_partial: label == "once",
                        no_write: false,
                        checkers: Some(checkers),
                    },
                    &PluginOptions { plugins: &plugins, ..PluginOptions::default() },
                )
            })
            .unwrap()
            .join()
            .unwrap()
    });
    let mut files = BTreeMap::new();
    tree(&root, &root, &mut files);
    let lines = reported(&report, &root);
    remove_copy(&root);
    (files, lines)
}

fn remove_copy(root: &Path) {
    let link = root.join("node_modules");
    if link.exists() {
        let _ = std::process::Command::new("cmd").args(["/C", "rmdir"]).arg(&link).status();
    }
    let _ = std::fs::remove_dir_all(root);
}

/// The differences of `other` from `reference`, one line each.
fn differences(
    reference: &(BTreeMap<String, Vec<u8>>, Vec<String>),
    other: &(BTreeMap<String, Vec<u8>>, Vec<String>),
) -> Vec<String> {
    let mut out = Vec::new();
    for (path, bytes) in &reference.0 {
        match other.0.get(path) {
            None => out.push(format!("missing {path}")),
            Some(theirs) if theirs != bytes => out.push(format!("differs {path}")),
            Some(_) => {}
        }
    }
    out.extend(other.0.keys().filter(|p| !reference.0.contains_key(*p)).map(|p| format!("extra {p}")));
    if reference.1 != other.1 {
        out.push(format!("reported differently:\n  one checker: {:?}\n  this: {:?}", reference.1, other.1));
    }
    out
}

/// A project whose diagnostics roblox-ts reports once per compilation (`addDiagnosticWithCache`): an `any` export
/// used in five modules; an `any` member of a generic interface read through two instantiations from two files in
/// another folder (the validator's case: whether a checker answers with the declared symbol or an instantiated
/// one depends on what it checked before); `any` properties read through a union's synthetic property, a mapped
/// type's property and a tuple element next to their sources (stock roblox-ts 3.0.0 reports 9 there, `z.ts`); and an
/// enum and a namespace merged in one file (roblox-ts requires `moduleDetection: force`, so merged declarations never
/// span files). Its configs are `exprs`'s.
fn once_fixture(configs: &Path) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rbxts-rs-determinism-once-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["src/shared/x", "src/shared/y"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for name in ["tsconfig.json", "default.project.json", "package.json"] {
        std::fs::copy(configs.join(name), root.join(name)).unwrap();
    }
    let write = |name: &str, text: &str| std::fs::write(root.join("src/shared").join(name), text).unwrap();
    write("a.ts", "export const value: any = {};\n");
    for name in ["b", "c", "d", "e", "f"] {
        write(&format!("{name}.ts"), "import { value } from \"./a\";\nexport const x = value.x;\n");
    }
    write(
        "x/p.ts",
        "export interface Holder<T> {\n\tvalue: any;\n\tt: T;\n}\nexport declare const hs: Holder<string>;\n\
         export declare const hn: Holder<number>;\nexport function early<T>(h: Holder<T>) {\n\treturn h.value;\n}\n",
    );
    write(
        "y/q.ts",
        "import { hn, hs } from \"../x/p\";\nexport const one = hs.value.x;\nexport const two = hn.value.x;\n",
    );
    write("y/r.ts", "import { hs } from \"../x/p\";\nexport const three = hs.value.x;\n");
    // the validator's `single` case (val-s3par2)
    write(
        "z.ts",
        r#"interface H<T> { r: Record<"a", any>; tup: [any, string]; t: T }
declare const hs: H<string>;
declare const hn: H<number>;
export const z1 = hs.tup[0].x;
export const z2 = hn.tup[0].x;
export const z3 = hs.r.a.x;
export const z4 = hn.r.a.x;
interface Alpha { value: any; a: 1 }
interface Beta { value: any; b: 1 }
declare const u: Alpha | Beta;
declare const al: Alpha;
declare const be: Beta;
export const z5 = u.value.x;
export const z6 = al.value.x;
export const z7 = be.value.x;
interface Src { value: any; n: number }
declare const ps: Partial<Src>;
declare const src: Src;
export const z8 = ps.value.x;
export const z9 = src.value.x;
"#,
    );
    write(
        "g.ts",
        "export enum Merged {\n\tA = 1,\n}\nexport enum Merged {\n\tB = 2,\n}\n\
         export namespace Spread {\n\texport const a = 1;\n}\nexport namespace Spread {\n\texport const b = 2;\n}\n",
    );
    root
}

#[test]
fn outputs_and_diagnostics_do_not_depend_on_the_checkers() {
    let Some(node_modules) = node_modules() else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    };
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases: Vec<(String, PathBuf, &[&str])> = Vec::new();
    let mut areas: Vec<PathBuf> = std::fs::read_dir(manifest.join("tests/fixtures"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && !p.join("links.json").exists())
        .collect();
    areas.sort();
    for area in areas {
        let name = area.file_name().unwrap().to_string_lossy().into_owned();
        // the test plugins a fixture names in its `plugins.json` (see `fixtures.rs`)
        let plugins: Vec<String> = std::fs::read_to_string(area.join("plugins.json"))
            .ok()
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap_or_default();
        let plugins: &'static [&'static str] =
            Vec::leak(plugins.into_iter().map(|p| &*String::leak(p)).collect::<Vec<&'static str>>());
        cases.push((name, area, plugins));
    }
    cases.push((
        "plugins".to_owned(),
        manifest.join("tests/plugin_fixtures/probe"),
        &["stamp", "probe", "probe-exprcall"],
    ));
    let once = once_fixture(&manifest.join("tests/fixtures/exprs"));
    cases.push(("once".to_owned(), once.clone(), &[]));

    let mut failures = Vec::new();
    for (label, fixture, plugins) in &cases {
        let reference = build(fixture, plugins, label, &node_modules, CONFIGS[0]);
        if label == "once" {
            // a failing build, every file transformed (partial mode): each once-per-compilation diagnostic, once
            let count = |text: &str| reference.1.iter().filter(|l| l.contains(text)).count();
            assert_eq!(count("type `any` is not supported"), 2 + 9, "{:?}", reference.1);
            assert_eq!(count("Enum merging is not supported"), 1, "{:?}", reference.1);
            assert_eq!(count("Namespace merging is not supported"), 1, "{:?}", reference.1);
        } else {
            assert!(reference.0.keys().any(|p| p.ends_with(".luau")), "{label}: the reference build wrote no Luau");
        }
        for &config in &CONFIGS[1..] {
            let other = build(fixture, plugins, label, &node_modules, config);
            let diffs = differences(&reference, &other);
            if !diffs.is_empty() {
                failures.push(format!("{label} with {config:?}:\n  {}", diffs.join("\n  ")));
            }
        }
    }
    // SAFETY: as in `build`
    unsafe { std::env::remove_var("TSRS_CHECKER_ASSIGNMENT") };
    let _ = std::fs::remove_dir_all(&once);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
