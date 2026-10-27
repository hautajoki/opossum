//! The hand-written transform fixtures (`tests/fixtures/<area>`) against stock roblox-ts's output for them.
//!
//! A fixture is a roblox-ts project without `node_modules` (`src`, `tsconfig.json`, `default.project.json`, …) and
//! `expected/`, the Luau rbxts-rs must produce: stock roblox-ts's output (roblox-ts master through
//! `roblox-ts-test/probe/rbxts-driver.cjs`), except where a fixture's notes say otherwise. The test copies the
//! project (everything but `expected*/`) to a temporary directory, links in a `node_modules`, builds it and checks:
//! - every output equals its `expected/` file byte for byte, and every `expected/` file is produced;
//! - the build reports nothing: no diagnostic of any category (checker, setup or transform), no warning, no fatal
//!   error, no unsupported kind, no internal error. A fixture has no expected diagnostics. As with stock roblox-ts,
//!   diagnostics in declaration files under `src` are not reported, so a fixture can't test them.
//!
//! The shared `node_modules` (with `@rbxts/compiler-types`, `@rbxts/types`, `@rbxts/testez` and `@rbxts/services`) is
//! `RBXTS_RS_FIXTURE_NODE_MODULES` when set (a path that lacks the packages fails the test), else the roblox-ts
//! tests project's, found as `roblox-ts/tests/node_modules` or `roblox-ts-test/roblox-ts/tests/node_modules` under
//! the crate's directory or any directory above it. Without one the test is skipped and says so on the terminal
//! (outside the captured output), and passes, unless `RBXTS_RS_FIXTURE_REQUIRE=1` or `CI` is set: then it fails.
//!
//! A fixture's copy links the shared `node_modules` in as its `node_modules`, or, with a `links.json`, makes the
//! links that describes, in each of its layouts (see [`check`]). Linked `@rbxts/types` means no type counts as a
//! Roblox type (`is_in_rbxts_types`, as in stock roblox-ts), so the fixtures can't exercise the Roblox-type paths
//! (wrapping, the `instanceof` diagnostic).
//!
//! - `binding`: destructuring in declarations (`export let` and hoisted names included), parameters (rest tuples
//!   too) and assignments (optimized, nested, below a rest element, used as values), over arrays, tuples, LuaTuple
//!   calls, strings, sets, maps, iterable functions, generators and `SharedTable`; computed, string and numeric keys
//!   on strings and arrays; patterns in class constructors and methods. A hole in an optimized LuaTuple assignment
//!   declares its temporary (`local _`), which stock roblox-ts leaves a global (the `stockfix` fixture).
//! - `classes`: class declarations and expressions, constructors (parameter properties, property initializers,
//!   `super` calls), methods with unusual names, static members and blocks, `this` in static members, hoisting,
//!   default exports, and every kind of decorator. TypeScript 5 lists exported functions first and TypeScript 7 in
//!   source order, so the fixture declares its exported functions first.
//! - `declarations`: a package with `declaration` and `declarationMap`: the `.d.ts` / `.d.ts.map` of each source
//!   (`paths` specifiers rewritten to relative ones, a `types` reference directive to `@rbxts/types`), a declaration
//!   file under `src` printed again as an asset, a Luau asset. Checked as a written output tree; its maps are
//!   TypeScript 7's (see its tsconfig).
//! - `contextual_this`: function expressions and object literal methods in slots whose signature has a `this`
//!   parameter (TypeScript 5 gives them its contextual `this`), next to arrows, `this` users and `this: void` slots
//!   (`thisVoid.ts`: object literal methods there are callbacks, where TypeScript 7 alone reports a method).
//! - `exprs`: conditional expressions, `||=` / `&&=` / `??=`, `yield` and `yield*`, `delete`, `await`, tagged
//!   templates, `import()`, mixed edge cases, and a server script importing the modules. A tagged template's
//!   cooked backslash is escaped (`'back\\slash'`), where stock roblox-ts writes it bare (the `stockfix` fixture).
//! - `json`: JSON modules (`resolveJsonModule`): default, namespace, named and dynamic imports, a JSON file outside
//!   the root dirs (imported at its Rojo-mapped source path), and one nothing imports. The only fixture checked as a
//!   written output tree ([`check_tree`]): its JSON files are assets, copied as they are (`cfg/crlf.json` keeps its
//!   CRLF line ends; `.gitattributes` keeps them in the checkout).
//! - `jsx`: JSX elements, attributes (string, boolean, namespaced, with prerequisites), spread props (first, later,
//!   possibly undefined, conditional), fragments, keyed, conditional, mapped and spread children, JSX text whitespace
//!   and entities, `@jsx` / `@jsxFrag` pragmas, and the JSX factory import kept alive when only JSX uses it. Two
//!   lines are rbxts-rs's, not stock's (the `stockfix` fixture): `<_Inner />` is the component, not a host string, and
//!   an attribute string's backslash is a backslash (`"a\\b"`).
//! - `junction`: static and dynamic imports of packages behind junctions (symlinks elsewhere): `@rbxts/services`
//!   in the shared `node_modules`, and `@rbxts/greeter` (the fixture's `packages/greeter`) outside any
//!   `node_modules`, with either the packages or the whole `node_modules` linked. Imports keep their `node_modules`
//!   paths, as with stock roblox-ts (`guessVirtualPath`).
//! - `junction_augmentation`: a module augmentation of a linked package, first in root order: its resolution counts
//!   like an import's, so another file's import through a second link to the package goes through the augmented one.
//! - `junction_interleaved_imports`, `junction_interleaved_type_refs`: a file's resolutions (imports, or
//!   `/// <reference types>`) all come before those of the files it reaches, so of two links to one package the one
//!   the file names wins over the one its dependency names.
//! - `junction_relative_dependency`: a package imported by relative path while also linked into `node_modules` as a
//!   dependency: TypeScript 5 learns links from resolutions only, so the import keeps the package's own path.
//! - `junction_two_links`: two links to one package: imports of both go through the one TypeScript 5 met first.
//! - `junction_workspace`: a bun / pnpm isolated workspace (`packages/game` and its sibling `packages/core`, both
//!   linking `@rbxts/services` from `node_modules/.store`): the first link met is game's own.
//!   `junction_workspace_core_first` imports `core` before `services`: game's link still comes first.
//! - `literals`: object and array literals, methods in object literals, spread in objects, arrays and calls, every
//!   iterable kind an array spread accepts. TypeScript 5 lists exported functions first and TypeScript 7 in source
//!   order, so the fixture declares its exported functions first.
//! - `loops`: numeric `for` loops under `--optimizedLoops` and the `while` fallback (copies of captured loop
//!   variables, finalizers before `continue`), every for-of loop builder and `$range`, `while`, `do`-`while`, and
//!   `continue` / `break` inside `try` in loops, roblox-ts's `loop.spec.ts` (destructured loop variables
//!   included), and parenthesized `for` initializers and incrementors (value-less ones, which crash stock roblox-ts,
//!   are dropped).
//! - `macros`: every call, identifier, constructor and property-call macro, as values and statements, on optional
//!   chains and with evaluation-order cases.
//! - `midsolution`: project references through a solution config in the middle of the graph (`projects/group`, only
//!   references): its written `out` tree (maps TypeScript 7's, `expected-ts5/` keeps stock's), and with an error in
//!   the deepest project or in one branch ([`midsolution_errors`]: the failure blocks every project above it).
//! - `project_references`: TypeScript project references (a root, `projects/shared` and `projects/common`, both
//!   `composite` with a `declarationDir`): each project's written output tree (`expected/<outDir>`, build infos
//!   left out), imports across the reference boundary included.
//! - `stmts`: enums (non-finite values too), namespaces, `switch`, and `try` / `throw` (roblox-ts tests' specs plus
//!   hand-written cases). `expected/` is roblox-ts on the TypeScript 7 checker, except the infinities, which rbxts-rs
//!   writes as `math.huge`, and the `default` before a case in `switches`, which rbxts-rs lowers so that the case
//!   still runs (the `stockfix` fixture); `expected-ts5/` (not checked) keeps the two files where stock roblox-ts on TypeScript 5
//!   differs, in the order of the export table. Its tsconfig turns on `allowCommentDirectives` (for
//!   `@ts-expect-error` in enums).
//! - `stockfix`: the roblox-ts bugs rbxts-rs fixes (each source file names its bug). `expected/` is rbxts-rs's output,
//!   which differs from stock roblox-ts's exactly by those fixes, and `runtime/check.luau` runs it with Lune (when
//!   `lune` is on the PATH) against JavaScript's results for the same calls (`runtime/<file>.luau`). Its tsconfig sets
//!   `preserveSymlinks`, so the linked `@rbxts/types` counts as `@rbxts/types` (`jsxConstructors` needs its Roblox
//!   datatype globals recognised); nothing else in the fixture depends on it.
//!   `tests/error_fixtures/jsx_entity` holds an input stock roblox-ts crashes on, which rbxts-rs reports as errors
//!   (outside `tests/fixtures`, whose builds must succeed).
//! - `transform_order`: imports, a triple-slash reference, a module augmentation, an ambient module import, a
//!   module file augmenting that ambient module, pattern ambient modules and an ambient module shadowing a file, whose files roblox-ts transforms in another order than the program's; [`transform_order`] also checks that order
//!   (`order.txt`, stock roblox-ts's `getChangedSourceFiles`).
//! - `write_transformed`: `writeTransformedFiles` with a TypeScript transformer that changes nothing (`identity.cjs`,
//!   claimed by the `transformer` test plugin): each compiled file printed again beside its Luau. Without the
//!   transformer nothing is printed and old `.transformed.ts` files are cleaned
//!   ([`write_transformed_without_transformers`]).
//!
//! A fixture may name the `rbxts_test_plugins` plugins its builds register in a `plugins.json` (a list of names).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use rbxts_project::{ProjectOptionLayer, STACK_SIZE};
use rbxts_plugin::erased::DynPlugin;
use rbxts_transform::{CompileOptions, CompileReport, PluginOptions, compile_project_with};
use tsrs_compiler::diagnosticwriter::flatten_diagnostic_message;
use tsrs_diagnostics::Category;

use rbxts_test_support::{copy_dir, link_dir, node_modules};

/// Skips a fixture test without a `node_modules` ([`rbxts_test_support::skip`]).
fn skip(test: &str, why: &str) {
    rbxts_test_support::skip(&format!("fixtures::{test}"), why);
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(base, &path, out);
        } else {
            out.push(path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
}

/// A fixture's temporary copy, removed on drop, panics included. Links inside it are removed as links first, never
/// followed into the shared `node_modules`.
struct TempProject {
    root: PathBuf,
    links: Vec<PathBuf>,
}

impl TempProject {
    /// A copy of `fixture` without its `expected*/` folders and `links.json` (and any `node_modules` or `out`).
    fn copy_of(fixture: &Path, label: &str) -> TempProject {
        let root = std::env::temp_dir().join(format!("rbxts-rs-fixture-{label}-{}", std::process::id()));
        // a copy left by a killed run of the same process id
        let _ = std::fs::remove_dir_all(&root);
        let project = TempProject { root, links: Vec::new() };
        std::fs::create_dir_all(&project.root).unwrap();
        for entry in std::fs::read_dir(fixture).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("expected")
                || name == "links.json"
                || name == "plugins.json"
                || name == "node_modules"
                || name == "out"
            {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &project.root.join(&name));
            } else {
                std::fs::copy(entry.path(), project.root.join(&name)).unwrap();
            }
        }
        project
    }

    /// Links `relative` (inside the copy) to `target`.
    fn link(&mut self, relative: &str, target: &Path) {
        let link = self.root.join(relative);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        link_dir(&link, target);
        self.links.push(link);
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        // innermost links first; removing a junction or a directory symlink removes the link, not its target
        for link in self.links.iter().rev() {
            if std::fs::remove_dir(link).is_err() {
                let _ = std::fs::remove_file(link);
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The test plugins a fixture names in its `plugins.json` (a list of `rbxts_test_plugins` names), registered for its
/// builds: a fixture whose tsconfig lists a transform needs the plugin that claims it.
fn fixture_plugins(fixture: &Path) -> Vec<String> {
    std::fs::read_to_string(fixture.join("plugins.json"))
        .ok()
        .map(|text| serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}/plugins.json: {e}", fixture.display())))
        .unwrap_or_default()
}

/// Builds the project at `root` (no writes, unless `write`) with the test plugins `plugins`, on a thread with the
/// compiler's stack size.
fn build(root: &Path, write: bool, plugins: &[String]) -> CompileReport {
    let cwd = root.to_string_lossy().into_owned();
    let plugins: Vec<String> = plugins.to_vec();
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || {
            let plugins: Vec<Box<dyn DynPlugin>> = plugins
                .iter()
                .map(|name| rbxts_test_plugins::by_name(name).unwrap_or_else(|| panic!("no test plugin {name}")))
                .collect();
            let plugin_options = PluginOptions { plugins: &plugins, ..PluginOptions::default() };
            compile_project_with(
                &CompileOptions {
                    tsconfig: "tsconfig.json".into(),
                    cwd,
                    project_options: ProjectOptionLayer::default(),
                    out_dir_override: None,
                    write_partial: false,
                    no_write: !write,
                    checkers: None,
                },
                &plugin_options,
            )
        })
        .unwrap()
        .join()
        .unwrap()
}

/// Everything the build reported, one line each: fatal errors, warnings, diagnostics, internal errors and
/// unsupported kinds.
fn reported(report: &CompileReport, root: &Path) -> Vec<String> {
    let relative = |file: &str| {
        Path::new(file).strip_prefix(root).map_or_else(|_| file.to_owned(), |p| p.to_string_lossy().replace('\\', "/"))
    };
    let mut lines: Vec<String> = report.fatal.iter().map(|m| format!("fatal: {m}")).collect();
    lines.extend(report.warnings.iter().map(|w| format!("warning: {w}")));
    let all_diagnostics = report.diagnostics.iter().chain(report.files.iter().flat_map(|f| f.diagnostics.iter()));
    for d in all_diagnostics {
        let category = match d.category() {
            Category::Error => "error",
            Category::Warning => "warning",
            Category::Suggestion => "suggestion",
            Category::Message => "message",
        };
        let file = d.file().map(|f| relative(f.file_name())).unwrap_or_default();
        lines.push(format!("{category} {file}: {}", flatten_diagnostic_message(*d, " ")));
    }
    for file in &report.files {
        let name = relative(&file.file_name);
        lines.extend(file.internal_errors.iter().map(|e| format!("internal error {name}: {e}")));
    }
    lines
}

/// The build of `root` against `expected`: every failure, one entry each.
fn compare(report: &CompileReport, root: &Path, expected: &Path) -> Vec<String> {
    let mut failures = reported(report, root);
    let mut produced = Vec::new();
    for file in &report.files {
        let relative = Path::new(&file.out_path)
            .strip_prefix(&report.out_dir)
            .unwrap_or(Path::new(&file.out_path))
            .to_string_lossy()
            .replace('\\', "/");
        produced.push(relative.clone());
        let Some(luau) = &file.luau else {
            failures.push(format!("{relative}: no output"));
            continue;
        };
        let expected_path = expected.join(&relative);
        let Ok(expected) = std::fs::read_to_string(&expected_path) else {
            failures.push(format!("{relative}: produced but not in expected/\n--- ours\n{luau}"));
            continue;
        };
        // exact, line ends included: the renderer writes `\n` as roblox-ts does, and `.gitattributes` checks every
        // `expected/` file out with LF whatever `core.autocrlf` says, so nothing needs normalising
        if *luau != expected {
            let line = luau
                .lines()
                .zip(expected.lines())
                .position(|(a, b)| a != b)
                .map_or_else(|| luau.lines().count().min(expected.lines().count()) + 1, |i| i + 1);
            failures.push(format!("{relative}: differs from expected at line {line}\n--- ours\n{luau}"));
        }
    }
    let mut expected_files = Vec::new();
    collect_files(expected, expected, &mut expected_files);
    for relative in expected_files {
        if !produced.contains(&relative) {
            failures.push(format!("{relative}: expected but not produced"));
        }
    }
    failures
}

/// Build infos, which the trees leave out: stock's `.rbxtsc.tsbuildinfo` and rbxts-rs's incremental state differ by
/// design.
fn is_build_info(path: &str) -> bool {
    path.ends_with(".tsbuildinfo") || path.ends_with(".rbxts-rs.buildinfo")
}

/// The build's report, then its written output trees against `expected` (both ways), byte for byte: an asset is
/// copied as it is (line ends included), as stock does. `roots`: the output directories compared, relative to the
/// project; `expected/` is the tree of the only root `out`, else holds one tree per root at the same relative path.
/// Build infos are left out.
fn compare_tree(report: &CompileReport, root: &Path, expected: &Path, roots: &[&str]) -> Vec<String> {
    let mut failures = reported(report, root);
    for &out_root in roots {
        let expected = if roots == ["out"] { expected.to_owned() } else { expected.join(out_root) };
        let prefix = if roots == ["out"] { String::new() } else { format!("{out_root}/") };
        failures.extend(compare_one_tree(&root.join(out_root), &expected, &prefix));
    }
    failures
}

/// One output tree against its expected tree; failures name files with `prefix`.
fn compare_one_tree(out: &Path, expected: &Path, prefix: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut written = Vec::new();
    if out.is_dir() {
        collect_files(out, out, &mut written);
    }
    written.retain(|path| !is_build_info(path));
    let mut expected_files = Vec::new();
    if expected.is_dir() {
        collect_files(expected, expected, &mut expected_files);
    }
    for relative in &written {
        let ours = std::fs::read(out.join(relative)).unwrap_or_default();
        let shown = String::from_utf8_lossy(&ours).replace('\r', "\\r");
        match std::fs::read(expected.join(relative)) {
            Err(_) => failures.push(format!("{prefix}{relative}: written but not in expected/\n--- ours\n{shown}")),
            Ok(theirs) if theirs != ours => {
                failures.push(format!("{prefix}{relative}: differs from expected (byte for byte)\n--- ours\n{shown}"))
            }
            Ok(_) => {}
        }
    }
    for relative in expected_files {
        if !written.contains(&relative) {
            failures.push(format!("{prefix}{relative}: expected but not written"));
        }
    }
    failures
}

/// Builds a fixture copy, in each layout of its `links.json` (one default layout without one), and checks it.
///
/// `links.json` is `{ "project"?: dir, "layouts": { name: [[link, target], …] } }`: the project directory inside the
/// copy (default: the copy itself), and per layout the links to make, in order, each a path inside the copy and a
/// target, `@shared/<package>` for the shared `node_modules`'s `@rbxts/<package>` or a path inside the copy. Without
/// `links.json`, `node_modules` links to the shared `node_modules`.
fn check(area: &'static str) {
    check_in(area, None);
}

/// [`check`], comparing the written output tree instead: every file under the project's `out` (Luau and assets) equals
/// its `expected/` file byte for byte, and every `expected/` file is written.
fn check_tree(area: &'static str) {
    check_in(area, Some(&["out"]));
}

/// [`check_tree`] over several output directories (a build with project references): `expected/<root>` per root.
fn check_trees(area: &'static str, roots: &'static [&'static str]) {
    check_in(area, Some(roots));
}

/// [`check`] (`trees` None) or [`check_trees`].
fn check_in(area: &'static str, trees: Option<&'static [&'static str]>) {
    let node_modules = match node_modules() {
        Ok(path) => path,
        Err(why) => return skip(area, &why),
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(area);
    let links: Option<serde_json::Value> = std::fs::read_to_string(fixture.join("links.json"))
        .ok()
        .map(|text| serde_json::from_str(&text).unwrap_or_else(|e| panic!("{area}/links.json: {e}")));
    let project_dir = links.as_ref().and_then(|l| l["project"].as_str()).unwrap_or(".").to_owned();
    let layouts: Vec<(String, Vec<(String, String)>)> = match &links {
        None => vec![(String::new(), vec![("node_modules".to_owned(), node_modules.to_string_lossy().into_owned())])],
        Some(links) => links["layouts"]
            .as_object()
            .unwrap_or_else(|| panic!("{area}/links.json: no layouts"))
            .iter()
            .map(|(name, pairs)| {
                let pairs = pairs.as_array().unwrap_or_else(|| panic!("{area}/links.json: {name} is not a list"));
                let pairs = pairs
                    .iter()
                    .map(|pair| match pair.as_array().map(Vec::as_slice) {
                        Some([link, target]) => (link.as_str().unwrap().to_owned(), target.as_str().unwrap().to_owned()),
                        _ => panic!("{area}/links.json: {name}: a link is [link, target]"),
                    })
                    .collect();
                (name.clone(), pairs)
            })
            .collect(),
    };

    let mut failures = Vec::new();
    for (layout, pairs) in layouts {
        let label = if layout.is_empty() { area.to_owned() } else { format!("{area}-{layout}") };
        let mut project = TempProject::copy_of(&fixture, &label);
        for (link, target) in pairs {
            let target = match target.strip_prefix("@shared/") {
                Some(package) => node_modules.join("@rbxts").join(package),
                None if Path::new(&target).is_absolute() => PathBuf::from(target),
                None => project.root.join(target),
            };
            project.link(&link, &target);
        }
        let root = project.root.join(&project_dir);
        let report = build(&root, trees.is_some(), &fixture_plugins(&fixture));
        let layout_failures = if let Some(roots) = trees {
            compare_tree(&report, &root, &fixture.join("expected"), roots)
        } else {
            compare(&report, &root, &fixture.join("expected"))
        };
        if !layout_failures.is_empty() {
            failures.push(format!("{label}:\n{}", layout_failures.join("\n")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn junction() {
    check("junction");
}

#[test]
fn junction_augmentation() {
    check("junction_augmentation");
}

#[test]
fn junction_interleaved_imports() {
    check("junction_interleaved_imports");
}

#[test]
fn junction_interleaved_type_refs() {
    check("junction_interleaved_type_refs");
}

#[test]
fn junction_workspace_core_first() {
    check("junction_workspace_core_first");
}

#[test]
fn junction_relative_dependency() {
    check("junction_relative_dependency");
}

#[test]
fn junction_two_links() {
    check("junction_two_links");
}

#[test]
fn junction_workspace() {
    check("junction_workspace");
}

#[test]
fn binding() {
    check("binding");
}

#[test]
fn classes() {
    check("classes");
}

#[test]
fn contextual_this() {
    check("contextual_this");
}

#[test]
fn exprs() {
    check("exprs");
}

#[test]
fn json() {
    check_tree("json");
}

#[test]
fn declarations() {
    check_tree("declarations");
}

#[test]
fn write_transformed() {
    check_tree("write_transformed");
}

#[test]
fn project_references() {
    check_trees("project_references", &["out", "projects/common/out", "projects/shared/out"]);
}

#[test]
fn jsx() {
    check("jsx");
}

#[test]
fn literals() {
    check("literals");
}

#[test]
fn loops() {
    check("loops");
}

#[test]
fn macros() {
    check("macros");
}

#[test]
fn stmts() {
    check("stmts");
}

/// An entity past U+10FFFF, which crashes stock roblox-ts: one error per entity, and the file is not written.
#[test]
fn stockfix_jsx_entity() {
    let node_modules = match node_modules() {
        Ok(path) => path,
        Err(why) => return skip("stockfix_jsx_entity", &why),
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/error_fixtures/jsx_entity");
    let mut project = TempProject::copy_of(&fixture, "stockfix_jsx_entity");
    project.link("node_modules", &node_modules);
    let report = build(&project.root, false, &[]);
    let message = "JSX character entity is not a valid code point (above U+10FFFF)!";
    assert_eq!(
        reported(&report, &project.root),
        vec![format!("error src/shared/entity.tsx: {message}"), format!("error src/shared/entity.tsx: {message}")]
    );
    let entity = report.files.iter().find(|f| f.file_name.ends_with("entity.tsx")).expect("entity.tsx is reported");
    assert!(!entity.is_clean() && !entity.written);
}

/// The outputs, then `runtime/check.luau` on them with Lune (`lune` on the PATH; skipped and said so without it).
#[test]
fn stockfix() {
    check("stockfix");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stockfix");
    match std::process::Command::new("lune").args(["run", "runtime/check.luau"]).current_dir(&fixture).output() {
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "fixtures::stockfix: the Lune runtime check skipped: lune: {e}");
        }
        Ok(output) => assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).starts_with("ok "),
            "runtime/check.luau:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

/// The files are transformed and reported in roblox-ts's order (`order.txt`, from stock roblox-ts), not program order.
#[test]
fn transform_order() {
    check("transform_order");
    let Ok(node_modules) = node_modules() else { return };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transform_order");
    let mut project = TempProject::copy_of(&fixture, "transform_order-files");
    project.link("node_modules", &node_modules);
    let report = build(&project.root, false, &[]);
    let root = format!("{}/", project.root.to_string_lossy().replace('\\', "/"));
    let ours: Vec<String> = report
        .files
        .iter()
        .map(|f| {
            let name = f.file_name.replace('\\', "/");
            name.strip_prefix(&root).unwrap_or(&name).to_owned()
        })
        .collect();
    let expected = std::fs::read_to_string(fixture.join("order.txt")).unwrap();
    let expected: Vec<&str> = expected.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(ours, expected);
}

/// `write_transformed` without a TypeScript transformer (its tsconfig's `plugins` entry removed): stock roblox-ts
/// writes no `.transformed.ts` then, and its clean removes old ones; the Luau is the same.
#[test]
fn write_transformed_without_transformers() {
    let Ok(node_modules) = node_modules() else { return };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/write_transformed");
    let mut project = TempProject::copy_of(&fixture, "write_transformed-none");
    project.link("node_modules", &node_modules);
    let tsconfig = project.root.join("tsconfig.json");
    let text = std::fs::read_to_string(&tsconfig).unwrap();
    let without = text.replace(",\n\t\t\"plugins\": [{ \"transform\": \"./identity.cjs\" }]", "");
    assert_ne!(text, without, "the fixture's tsconfig lists the transformer");
    std::fs::write(&tsconfig, without).unwrap();
    std::fs::create_dir_all(project.root.join("out")).unwrap();
    std::fs::write(project.root.join("out/stale.transformed.ts"), "export {};\n").unwrap();
    let report = build(&project.root, true, &[]);
    let mut failures = reported(&report, &project.root);
    let mut written = Vec::new();
    collect_files(&project.root.join("out"), &project.root.join("out"), &mut written);
    written.sort();
    let mut expected: Vec<String> = Vec::new();
    collect_files(&fixture.join("expected"), &fixture.join("expected"), &mut expected);
    expected.retain(|f| !f.contains(".transformed."));
    expected.sort();
    if written != expected {
        failures.push(format!("written {written:?}, expected {expected:?}"));
    }
    for file in &expected {
        if std::fs::read(project.root.join("out").join(file)).ok() != std::fs::read(fixture.join("expected").join(file)).ok()
        {
            failures.push(format!("{file}: differs from expected"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A solution config in the middle of the graph: the root references `projects/group` (`files: []`, only
/// references), which references `left` and `right`, both referencing `common`. `expected/` is stock's `out`.
#[test]
fn midsolution() {
    check_tree("midsolution");
}

/// `midsolution` with an error: the solution config in the middle passes the failure on, as roblox-ts's `blocked`
/// does. An error in `common` blocks every other project (stock reports that error alone and writes no output); one
/// in `left` blocks the group and the root, while `common` and `right` build (`expected-error-left/`, stock's `out`
/// but for its runtime include copy, which rbxts-rs leaves to a successful build).
#[test]
fn midsolution_errors() {
    let Ok(node_modules) = node_modules() else { return };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/midsolution");
    let cases: [(&str, &str, &str, &str, &str); 2] = [
        (
            "common",
            "projects/common/src/util/math.ts",
            "export const twice",
            "export const bad: number = \"x\";\nexport const twice",
            "error projects/common/src/util/math.ts: Type 'string' is not assignable to type 'number'.",
        ),
        (
            "left",
            "projects/left/src/index.ts",
            "make(twice(1))",
            "make(\"1\")",
            "error projects/left/src/index.ts: Argument of type 'string' is not assignable to parameter of type 'number'.",
        ),
    ];
    let mut failures = Vec::new();
    for (case, file, find, replace, expected_error) in cases {
        let mut project = TempProject::copy_of(&fixture, &format!("midsolution-error-{case}"));
        project.link("node_modules", &node_modules);
        let path = project.root.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(find), "{case}: {file} has no `{find}`");
        std::fs::write(&path, text.replacen(find, replace, 1)).unwrap();
        let report = build(&project.root, true, &[]);
        let lines = reported(&report, &project.root);
        if lines != [expected_error] {
            failures.push(format!("{case}: reported {lines:?}"));
        }
        let expected = fixture.join(format!("expected-error-{case}"));
        failures.extend(
            compare_one_tree(&project.root.join("out"), &expected, "out/").into_iter().map(|f| format!("{case}: {f}")),
        );
        if project.root.join("include").exists() {
            failures.push(format!("{case}: a failing build copied the runtime include"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
