//! `rbxtsc -w --events json` as a process on a copy of `rbxts_transform`'s watch fixture: the real watcher's
//! debounce (a burst of writes is one rebuild), a rename (the old output goes, the new one comes), an error (nothing
//! written) and its fix. The finer invariants are the in-process harness's (`rbxts_transform/tests/watch.rs`).

#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;

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

/// The watcher process and its copy, both gone on drop.
struct Watcher {
    child: Child,
    root: PathBuf,
    events: Receiver<Value>,
    /// Every stdout line, for failure messages.
    lines: Receiver<String>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let link = self.root.join("node_modules");
        let _ = Command::new("cmd")
            .args(["/C", "rmdir"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Watcher {
    /// The next `build-end` event (the `build-start` events are checked on the way: one per generation).
    fn build_end(&self, expected_gen: u64) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let event = match self.events.recv_timeout(left) {
                Ok(event) => event,
                Err(_) => {
                    let output: Vec<String> = self.lines.try_iter().collect();
                    panic!("no build-end for generation {expected_gen} within 60 s; output:\n{}", output.join("\n"));
                }
            };
            assert_eq!(event["gen"].as_u64(), Some(expected_gen), "unexpected event {event}");
            if event["event"] == "build-end" {
                return event;
            }
        }
    }

    /// No further build starts within `quiet`.
    fn assert_quiet(&self, quiet: Duration) {
        if let Ok(event) = self.events.recv_timeout(quiet) {
            panic!("unexpected event {event}");
        }
    }
}

#[test]
fn the_watcher_debounces_renames_and_recovers_from_errors() {
    let node_modules = rbxts_test_support::node_modules().unwrap_or_default();
    if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/basic");
    let root = std::env::temp_dir().join(format!("rbxts-rs-cli-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&fixture, &root);
    // the plain command line has no stamp plugin
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap();
    let tsconfig = tsconfig.replace(
        "\"plugins\": [{ \"transform\": \"rbxts-transform-stamp\", \"prefix\": \"watch \" }]",
        "\"plugins\": []",
    );
    std::fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("node_modules").to_string_lossy().replace('/', "\\"))
        .arg(node_modules.to_string_lossy().replace('/', "\\"))
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "could not link node_modules");

    let mut child = Command::new(env!("CARGO_BIN_EXE_rbxtsc"))
        .args(["-w", "--events", "json", "-p"])
        .arg(&root)
        .env("RBXTS_WATCH_MEMORY_LIMIT_MB", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (event_tx, events) = channel();
    let (line_tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with("{\"event\"")
                && let Ok(value) = serde_json::from_str::<Value>(&line)
            {
                let _ = event_tx.send(value);
            }
            let _ = line_tx.send(line);
        }
    });
    let watcher = Watcher { child, root: root.clone(), events, lines };
    let initial = watcher.build_end(0);
    assert_eq!(initial["ok"], true, "{initial}");
    assert!(root.join("out/shared/d.luau").is_file());

    // a burst of three writes inside the debounce: one rebuild
    let c = root.join("src/shared/c.ts");
    for text in ["Map<string, number>", "Map<string, boolean>", "Map<string, string>"] {
        std::fs::write(&c, format!("export interface Inner {{\n\ta: {text};\n}}\n\nexport const cValue = 1;\n"))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let end = watcher.build_end(1);
    assert_eq!(end["ok"], true, "{end}");
    let d = std::fs::read_to_string(root.join("out/shared/d.luau")).unwrap();
    assert!(d.contains("ReadonlyMap.size"), "D was rebuilt with Inner.a a map:\n{d}");
    watcher.assert_quiet(Duration::from_millis(500));

    // a rename: the old output goes, the new one comes
    std::fs::rename(root.join("src/shared/importer.ts"), root.join("src/shared/importer2.ts")).unwrap();
    let end = watcher.build_end(2);
    assert_eq!(end["ok"], true, "{end}");
    assert!(!root.join("out/shared/importer.luau").exists());
    assert!(root.join("out/shared/importer2.luau").is_file());

    // an error: nothing written; the fix
    let before = std::fs::read_to_string(root.join("out/shared/d.luau")).unwrap();
    std::fs::write(
        root.join("src/shared/d.ts"),
        "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.sizee();\n}\n",
    )
    .unwrap();
    let end = watcher.build_end(3);
    assert_eq!(end["ok"], false, "{end}");
    assert_eq!(std::fs::read_to_string(root.join("out/shared/d.luau")).unwrap(), before);
    std::fs::write(
        root.join("src/shared/d.ts"),
        "import { A } from \"./b\";\n\nexport function sizeOf(value: A) {\n\treturn value.x.a.size() + 1;\n}\n",
    )
    .unwrap();
    let end = watcher.build_end(4);
    assert_eq!(end["ok"], true, "{end}");
    assert!(std::fs::read_to_string(root.join("out/shared/d.luau")).unwrap().contains("+ 1"));
    drop(watcher);
}

/// Unlinks `link` (a junction) without touching its target.
fn unlink(link: &Path) {
    let _ = Command::new("cmd").args(["/C", "rmdir"]).arg(link.to_string_lossy().replace('/', "\\")).status();
}

/// Starts `rbxtsc -w --events json` on `root` and waits for its initial build.
fn start(root: &Path) -> Watcher {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rbxtsc"))
        .args(["-w", "--events", "json", "-p"])
        .arg(root)
        .env("RBXTS_WATCH_MEMORY_LIMIT_MB", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (event_tx, events) = channel();
    let (line_tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with("{\"event\"")
                && let Ok(value) = serde_json::from_str::<Value>(&line)
            {
                let _ = event_tx.send(value);
            }
            let _ = line_tx.send(line);
        }
    });
    Watcher { child, root: root.to_path_buf(), events, lines }
}

impl Watcher {
    /// Takes events until none comes for 1.5 s; the last build-end seen.
    fn settle(&self) -> Option<Value> {
        let mut last = None;
        while let Ok(event) = self.events.recv_timeout(Duration::from_millis(1500)) {
            if event["event"] == "build-end" {
                last = Some(event);
            }
        }
        last
    }

    /// Build ends until one that succeeded (a package written in steps can take more than one rebuild), within 60 s.
    fn until_ok(&self) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) if event["event"] == "build-end" && event["ok"] == true => return event,
                Ok(_) => {}
                Err(_) => {
                    let output: Vec<String> = self.lines.try_iter().collect();
                    panic!("no successful build within 60 s; output:\n{}", output.join("\n"));
                }
            }
        }
    }
}

/// Events from outside the sources: a directory moved into `src` from outside the watched roots (one event for the
/// directory on Windows), and a package installed, then changed, under `node_modules` with no source edit.
#[test]
fn the_watcher_sees_directories_moved_in_and_packages_installed() {
    let node_modules = rbxts_test_support::node_modules().unwrap_or_default();
    if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/basic");
    let base = std::env::temp_dir().join(format!("rbxts-rs-cli-watch-pkg-{}", std::process::id()));
    let root = base.join("project");
    let outside = base.join("outside");
    let _ = std::fs::remove_dir_all(&base);
    copy_dir(&fixture, &root);
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap();
    let tsconfig = tsconfig.replace(
        "\"plugins\": [{ \"transform\": \"rbxts-transform-stamp\", \"prefix\": \"watch \" }]",
        "\"plugins\": []",
    );
    std::fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
    // a real node_modules and @rbxts, one junction per package, so a package can be added in the copy alone
    let scope = root.join("node_modules/@rbxts");
    std::fs::create_dir_all(&scope).unwrap();
    let mut links = Vec::new();
    for entry in std::fs::read_dir(node_modules.join("@rbxts")).unwrap().flatten() {
        let link = scope.join(entry.file_name());
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(entry.path().to_string_lossy().replace('/', "\\"))
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        links.push(link);
    }
    std::fs::create_dir_all(outside.join("feat")).unwrap();
    std::fs::write(outside.join("feat/f.ts"), "export const f = 1;\n").unwrap();

    let watcher = start(&root);
    let initial = watcher.build_end(0);
    assert_eq!(initial["ok"], true, "{initial}");

    // a directory moved in from outside the watched roots
    std::fs::rename(outside.join("feat"), root.join("src/shared/feat")).unwrap();
    let end = watcher.build_end(1);
    assert_eq!(end["ok"], true, "{end}");
    assert!(root.join("out/shared/feat/f.luau").is_file(), "the moved-in directory was built");

    // an import of a package not installed yet
    std::fs::write(
        root.join("src/shared/usePkg.ts"),
        "import { v } from \"@rbxts/vpkg\";\n\nexport const w = v.size();\n",
    )
    .unwrap();
    let end = watcher.build_end(2);
    assert_eq!(end["ok"], false, "{end}");

    // the package appears (no source edit): a rebuild resolves it
    let staged = base.join("vpkg");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(
        staged.join("package.json"),
        "{ \"name\": \"@rbxts/vpkg\", \"main\": \"init.lua\", \"types\": \"index.d.ts\" }\n",
    )
    .unwrap();
    std::fs::write(staged.join("index.d.ts"), "export declare const v: Array<number>;\n").unwrap();
    std::fs::write(staged.join("init.lua"), "return { v = {} }\n").unwrap();
    std::fs::rename(&staged, scope.join("vpkg")).unwrap();
    watcher.until_ok();
    let use_pkg = std::fs::read_to_string(root.join("out/shared/usePkg.luau")).unwrap();
    assert!(!use_pkg.contains("ReadonlyMap.size"), "{use_pkg}");

    // the package's declaration changes (no source edit): a rebuild follows it
    std::fs::write(scope.join("vpkg/index.d.ts"), "export declare const v: Map<number, number>;\n").unwrap();
    watcher.until_ok();
    let use_pkg = std::fs::read_to_string(root.join("out/shared/usePkg.luau")).unwrap();
    assert!(use_pkg.contains("ReadonlyMap.size"), "the package's new types were used:\n{use_pkg}");

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// A copy of the watch fixture without the stamp plugin, with a real `node_modules` and `@rbxts` holding one
/// junction per package (so packages can be added in the copy alone). Returns (base, project, junctions).
fn linked_project(label: &str) -> Option<(PathBuf, PathBuf, Vec<PathBuf>)> {
    let node_modules = rbxts_test_support::node_modules().unwrap_or_default();
    if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return None;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/basic");
    let base = std::env::temp_dir().join(format!("rbxts-rs-cli-watch-{label}-{}", std::process::id()));
    let root = base.join("project");
    let _ = std::fs::remove_dir_all(&base);
    copy_dir(&fixture, &root);
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap().replace(
        "\"plugins\": [{ \"transform\": \"rbxts-transform-stamp\", \"prefix\": \"watch \" }]",
        "\"plugins\": []",
    );
    std::fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
    let links = link_packages(&root.join("node_modules/@rbxts"));
    Some((base, root, links))
}

/// The fixture's `node_modules`: `RBXTS_RS_FIXTURE_NODE_MODULES` or roblox-ts's tests project's.
fn fixture_node_modules() -> PathBuf {
    rbxts_test_support::node_modules().unwrap_or_default()
}

/// Makes `scope` (a real directory) with one junction per package of the fixture's `@rbxts`; the junctions.
fn link_packages(scope: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(scope).unwrap();
    let mut links = Vec::new();
    for entry in std::fs::read_dir(fixture_node_modules().join("@rbxts")).unwrap().flatten() {
        let link = scope.join(entry.file_name());
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(entry.path().to_string_lossy().replace('/', "\\"))
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        links.push(link);
    }
    links
}

/// A package folder created (one rebuild starts), its files written while that rebuild runs: the folder is
/// watched only after the build, so the writes raise no event of their own, yet the watcher ends on a successful
/// build. Then churn under node_modules (`.package-lock.json`, `.cache/`) starts no rebuild.
#[test]
fn files_written_into_a_new_package_folder_during_its_rebuild_are_seen() {
    let Some((base, root, links)) = linked_project("late-pkg") else { return };
    std::fs::write(
        root.join("src/shared/usePkg.ts"),
        "import { v } from \"@rbxts/latepkg\";\n\nexport const w = v.size();\n",
    )
    .unwrap();
    let watcher = start(&root);
    let initial = watcher.build_end(0);
    assert_eq!(initial["ok"], false, "the package is missing: {initial}");

    let package = root.join("node_modules/@rbxts/latepkg");
    std::fs::create_dir(&package).unwrap();
    // wait for the rebuild the folder starts, then fill the folder while it runs
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let event = watcher.events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).unwrap();
        if event["event"] == "build-start" {
            break;
        }
    }
    std::fs::write(
        package.join("package.json"),
        "{ \"name\": \"@rbxts/latepkg\", \"main\": \"init.lua\", \"types\": \"index.d.ts\" }\n",
    )
    .unwrap();
    std::fs::write(package.join("index.d.ts"), "export declare const v: Array<number>;\n").unwrap();
    std::fs::write(package.join("init.lua"), "return { v = {} }\n").unwrap();
    watcher.until_ok();
    // a write the build may not have seen starts one more rebuild (the folder is compared with the build's start)
    let last = watcher.settle();
    assert!(last.as_ref().is_none_or(|end| end["ok"] == true), "the last build failed: {last:?}");
    assert!(root.join("out/shared/usePkg.luau").is_file());

    // node_modules churn: dot entries are no packages
    std::fs::write(root.join("node_modules/.package-lock.json"), "{}\n").unwrap();
    std::fs::create_dir_all(root.join("node_modules/.cache/x")).unwrap();
    std::fs::write(root.join("node_modules/.cache/x/y"), "z").unwrap();
    std::fs::write(root.join("node_modules/@rbxts/.lock"), "z").unwrap();
    watcher.assert_quiet(Duration::from_millis(1500));

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// A tsconfig whose outDir is the project directory fails the build; the watcher must still see the tsconfig
/// restored (its events are configs, never ignored as outputs) and later source edits.
#[test]
fn the_watcher_hears_the_tsconfig_after_an_out_dir_holding_the_project() {
    let Some((base, root, links)) = linked_project("outdir") else { return };
    let watcher = start(&root);
    let initial = watcher.build_end(0);
    assert_eq!(initial["ok"], true, "{initial}");
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap();
    std::fs::write(root.join("tsconfig.json"), tsconfig.replace("\"outDir\": \"out\"", "\"outDir\": \".\"")).unwrap();
    let end = watcher.build_end(1);
    assert_eq!(end["ok"], false, "an outDir holding the sources fails: {end}");
    std::fs::write(root.join("tsconfig.json"), &tsconfig).unwrap();
    let end = watcher.build_end(2);
    assert_eq!(end["ok"], true, "the restored tsconfig rebuilt: {end}");
    std::fs::write(root.join("src/shared/later.ts"), "export const later = 1;\n").unwrap();
    let end = watcher.build_end(3);
    assert_eq!(end["ok"], true, "{end}");
    assert!(root.join("out/shared/later.luau").is_file());
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Sets a file's modification time to 1985, as an archive extraction or `cp -p` of an old tree leaves it.
fn set_old(path: &Path) {
    let old = std::time::UNIX_EPOCH + Duration::from_secs(499_162_500);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
}

/// Removes a directory tree, retrying while a watch handle holds a directory in it pending deletion.
fn remove_tree(path: &Path) {
    for _ in 0..50 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    panic!("could not remove {}", path.display());
}

impl Watcher {
    /// Waits for the next build to start (within 60 s).
    fn build_start(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) if event["event"] == "build-start" => return,
                Ok(_) => {}
                Err(_) => panic!("no build started within 60 s"),
            }
        }
    }

    /// After a change that starts a build: waits for it to start (however loaded the machine), then settles; the last
    /// build-end.
    fn changed(&self) -> Option<Value> {
        self.build_start();
        self.settle()
    }

    /// Settles (no build for 1.5 s) and checks that the last build succeeded.
    fn settled_ok(&self, label: &str) {
        let last = self.settle();
        assert!(last.as_ref().is_some_and(|end| end["ok"] == true), "{label}: the last build failed: {last:?}");
    }

    /// After a successful build: settles, and checks that any build since succeeded too.
    fn still_ok(&self, label: &str) {
        let last = self.settle();
        assert!(last.as_ref().is_none_or(|end| end["ok"] == true), "{label}: the last build failed: {last:?}");
    }
}

fn package_json(name: &str) -> String {
    format!("{{ \"name\": \"@rbxts/{name}\", \"main\": \"out/init.lua\", \"types\": \"out/index.d.ts\" }}\n")
}

/// Package files in a subfolder of the package (`types: out/index.d.ts`): written after the watch started, or while
/// the rebuild their package folder started runs (several delays), or with a 1985 modification time (an extracted
/// archive) while it runs. Each is seen: the watcher ends on a successful build.
#[test]
fn package_files_in_subfolders_late_or_old_are_seen() {
    let Some((base, root, links)) = linked_project("nested") else { return };
    let scope = root.join("node_modules/@rbxts");
    // package.json and an empty out/ before the watch, the types file later
    write(&scope.join("neststatic/package.json"), &package_json("neststatic"));
    std::fs::create_dir_all(scope.join("neststatic/out")).unwrap();
    write(
        &root.join("src/shared/nestStatic.ts"),
        "import { v } from \"@rbxts/neststatic\";\n\nexport const w = v.size();\n",
    );
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], false, "the types are missing");
    write(&scope.join("neststatic/out/index.d.ts"), "export declare const v: Array<number>;\n");
    write(&scope.join("neststatic/out/init.lua"), "return { v = {} }\n");
    watcher.until_ok();
    watcher.still_ok("neststatic");
    assert!(root.join("out/shared/nestStatic.luau").is_file());

    // package.json and out/ start a rebuild; the types land while it runs
    for (i, delay) in [0u64, 15, 40, 100].into_iter().enumerate() {
        let name = format!("nestrace{i}");
        write(
            &root.join(format!("src/shared/{name}.ts")),
            &format!("import {{ v }} from \"@rbxts/{name}\";\n\nexport const w = v.size();\n"),
        );
        let end = watcher.changed();
        assert!(end.as_ref().is_some_and(|e| e["ok"] == false), "{name} is missing: {end:?}");
        write(&scope.join(format!("{name}/package.json")), &package_json(&name));
        std::fs::create_dir_all(scope.join(format!("{name}/out"))).unwrap();
        watcher.build_start();
        std::thread::sleep(Duration::from_millis(delay));
        write(&scope.join(format!("{name}/out/index.d.ts")), "export declare const v: Array<number>;\n");
        write(&scope.join(format!("{name}/out/init.lua")), "return { v = {} }\n");
        watcher.settled_ok(&format!("types {delay} ms into the rebuild"));
    }

    // the types re-export a file that lands, with an old modification time, while the rebuild runs
    for (i, delay) in [0u64, 30, 80].into_iter().enumerate() {
        let name = format!("oldm{i}");
        write(
            &root.join(format!("src/shared/{name}.ts")),
            &format!("import {{ extra }} from \"@rbxts/{name}\";\n\nexport const w = extra;\n"),
        );
        let end = watcher.changed();
        assert!(end.as_ref().is_some_and(|e| e["ok"] == false), "{name} is missing: {end:?}");
        write(&scope.join(format!("{name}/package.json")), &package_json(&name));
        write(&scope.join(format!("{name}/out/index.d.ts")), "export * from \"./extra\";\n");
        write(&scope.join(format!("{name}/out/init.lua")), "return {}\n");
        watcher.build_start();
        std::thread::sleep(Duration::from_millis(delay));
        let extra = scope.join(format!("{name}/out/extra.d.ts"));
        write(&extra, "export declare const extra: number;\n");
        set_old(&extra);
        watcher.settled_ok(&format!("an old file {delay} ms into the rebuild"));
    }

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// node_modules removed (`npm ci`, `rm -rf node_modules`), a source edited while it is gone, then node_modules back:
/// made again folder by folder, or renamed in whole. The watcher ends on a successful build with the edit.
#[test]
fn node_modules_removed_and_reinstalled_is_seen() {
    for mode in ["copy", "rename"] {
        let Some((base, root, links)) = linked_project(&format!("nmwipe-{mode}")) else { return };
        let watcher = start(&root);
        assert_eq!(watcher.build_end(0)["ok"], true);
        for link in &links {
            unlink(link);
        }
        remove_tree(&root.join("node_modules"));
        let end = watcher.changed();
        assert!(end.as_ref().is_some_and(|e| e["ok"] == false), "{mode}: no node_modules: {end:?}");
        let b = std::fs::read_to_string(root.join("src/shared/b.ts")).unwrap();
        std::fs::write(root.join("src/shared/b.ts"), b + "\nexport const whileGone = 1;\n").unwrap();
        let end = watcher.changed();
        assert!(end.as_ref().is_some_and(|e| e["ok"] == false), "{mode}: the edit while it is gone: {end:?}");
        let links = if mode == "copy" {
            link_packages(&root.join("node_modules/@rbxts"))
        } else {
            let staged = base.join("staged-node_modules");
            let links = link_packages(&staged.join("@rbxts"));
            std::fs::rename(&staged, root.join("node_modules")).unwrap();
            links.iter().map(|l| root.join("node_modules").join(l.strip_prefix(&staged).unwrap())).collect()
        };
        watcher.until_ok();
        watcher.still_ok(mode);
        let b = std::fs::read_to_string(root.join("out/shared/b.luau")).unwrap();
        assert!(b.contains("whileGone"), "{mode}: the edit made while node_modules was gone:\n{b}");
        for link in links {
            unlink(&link);
        }
        drop(watcher);
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// A watch started before node_modules exists (a fresh clone), then the install: rebuilt.
#[test]
fn a_watch_started_before_the_install_sees_it() {
    let Some((base, root, links)) = linked_project("nmfirst") else { return };
    for link in &links {
        unlink(link);
    }
    remove_tree(&root.join("node_modules"));
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], false, "no node_modules");
    let links = link_packages(&root.join("node_modules/@rbxts"));
    watcher.until_ok();
    watcher.still_ok("installed");
    assert!(root.join("out/shared/d.luau").is_file());
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// The `TS.import` line of an output, or "(none)".
fn import_line(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().find(|l| l.contains("TS.import")).map(str::to_owned))
        .unwrap_or_else(|| "(none)".to_owned())
}

/// A folder outside the sources mounted by a Rojo `$path` (added while watching): project files created in it and in
/// its subfolders, a folder holding one moved in, then each removed. The Rojo resolver searches mounted folders for
/// project files, so each step changes the instance path the importer requires.
#[test]
fn rojo_project_files_in_a_mounted_folder_outside_the_sources_are_seen() {
    let Some((base, root, links)) = linked_project("rojopath") else { return };
    write(&root.join("vendor/a/b/x.lua"), "return { f = 1 }\n");
    write(&root.join("vendor/a/b/x.d.ts"), "export declare const f: number;\n");
    write(&root.join("src/shared/useVendor.ts"), "import { f } from \"../../vendor/a/b/x\";\n\nexport const g = f;\n");
    let out = root.join("out/shared/useVendor.luau");
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], false, "vendor is not mounted yet");

    let rojo = root.join("default.project.json");
    let text = std::fs::read_to_string(&rojo).unwrap();
    let mounted = text.replace("\"shared\": {", "\"vendor\": { \"$path\": \"vendor\" },\n\t\t\t\"shared\": {");
    assert_ne!(text, mounted);
    std::fs::write(&rojo, mounted).unwrap();
    let mut generation = 1;
    let mut step = |label: &str, change: &dyn Fn(), path: &[&str]| {
        change();
        let end = watcher.build_end(generation);
        generation += 1;
        assert_eq!(end["ok"], true, "{label}: {end}");
        let expected = path.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(", ");
        let line = import_line(&out);
        assert!(line.contains(&format!("\"ReplicatedStorage\"), {expected}).f")), "{label}: {line}");
        watcher.assert_quiet(Duration::from_millis(500));
    };
    step("$path vendor added", &|| {}, &["vendor", "a", "b", "x"]);
    let project = |name: &str, child: &str, path: &str| {
        format!(
            "{{ \"name\": \"{name}\", \"tree\": {{ \"$className\": \"Folder\", \"{child}\": {{ \"$path\": \"{path}\" }} }} }}\n"
        )
    };
    step(
        "vendor/default.project.json created",
        &|| write(&root.join("vendor/default.project.json"), &project("v", "renamed", "a")),
        &["vendor", "renamed", "b", "x"],
    );
    step(
        "vendor/a/default.project.json created",
        &|| write(&root.join("vendor/a/default.project.json"), &project("va", "bb", "b")),
        &["vendor", "renamed", "bb", "x"],
    );
    step(
        "vendor/default.project.json removed",
        &|| std::fs::remove_file(root.join("vendor/default.project.json")).unwrap(),
        &["vendor", "va", "bb", "x"],
    );
    step(
        "vendor/a/default.project.json removed",
        &|| std::fs::remove_file(root.join("vendor/a/default.project.json")).unwrap(),
        &["vendor", "a", "b", "x"],
    );
    // a folder holding a project file that mounts vendor/a/b again, moved in (one event for the folder), then removed
    let staged = base.join("m");
    write(&staged.join("m.project.json"), "{ \"name\": \"mm\", \"tree\": { \"$path\": \"../a/b\" } }\n");
    step(
        "a folder holding a project file moved in",
        &|| std::fs::rename(&staged, root.join("vendor/m")).unwrap(),
        &["vendor", "m", "mm", "x"],
    );
    step("that folder removed", &|| remove_tree(&root.join("vendor/m")), &["vendor", "a", "b", "x"]);
    // other files in the mounted folder change nothing the build reads
    write(&root.join("vendor/a/b/y.lua"), "return 2\n");
    write(&root.join("vendor/a/notes.txt"), "x\n");
    std::fs::create_dir_all(root.join("vendor/c/d")).unwrap();
    watcher.assert_quiet(Duration::from_millis(1500));

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// Writes that change nothing the build reads start no rebuild: lockfiles in the project directory, a used package's
/// package.json and the tsconfig rewritten with the same text, code in a package that is only in a type root. What
/// setup reads of such a package (its package.json, its Rojo project files) still counts, as do the used package's
/// files.
#[test]
fn writes_that_change_nothing_the_build_reads_start_no_rebuild() {
    let Some((base, root, links)) = linked_project("quiet") else { return };
    let scope = root.join("node_modules/@rbxts");
    for name in ["usedpkg", "typeonly"] {
        write(
            &scope.join(format!("{name}/package.json")),
            &format!("{{ \"name\": \"@rbxts/{name}\", \"main\": \"init.lua\", \"types\": \"index.d.ts\" }}\n"),
        );
        write(&scope.join(format!("{name}/index.d.ts")), "export declare const v: number;\n");
        write(&scope.join(format!("{name}/init.lua")), "return { v = 1 }\n");
    }
    write(&root.join("src/shared/useUsed.ts"), "import { v } from \"@rbxts/usedpkg\";\n\nexport const w = v;\n");
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    watcher.assert_quiet(Duration::from_millis(1000));

    for lockfile in ["bun.lockb", "bun.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"] {
        std::fs::write(root.join(lockfile), "lock\n").unwrap();
    }
    watcher.assert_quiet(Duration::from_millis(1500));
    for file in ["tsconfig.json", "package.json", "node_modules/@rbxts/usedpkg/package.json"] {
        let path = root.join(file);
        std::fs::write(&path, std::fs::read(&path).unwrap()).unwrap();
    }
    watcher.assert_quiet(Duration::from_millis(1500));
    let typeonly = scope.join("typeonly");
    std::fs::write(typeonly.join("index.d.ts"), "export declare const v: string;\n").unwrap();
    std::fs::write(typeonly.join("init.lua"), "return { v = 2 }\n").unwrap();
    write(&typeonly.join("sub/deep.lua"), "return 3\n");
    let package_json = std::fs::read(typeonly.join("package.json")).unwrap();
    std::fs::write(typeonly.join("package.json"), &package_json).unwrap();
    watcher.assert_quiet(Duration::from_millis(1500));

    // what setup reads of it: package.json (the package mapping), Rojo project files (the package resolver)
    std::fs::write(
        typeonly.join("package.json"),
        "{ \"name\": \"@rbxts/typeonly\", \"main\": \"sub/deep.lua\", \"types\": \"index.d.ts\" }\n",
    )
    .unwrap();
    assert_eq!(watcher.build_end(1)["ok"], true);
    write(
        &typeonly.join("sub/default.project.json"),
        "{ \"name\": \"t\", \"tree\": { \"$className\": \"Folder\" } }\n",
    );
    assert_eq!(watcher.build_end(2)["ok"], true);
    // the used package's declarations
    std::fs::write(scope.join("usedpkg/index.d.ts"), "export declare const v: string;\n").unwrap();
    assert_eq!(watcher.build_end(3)["ok"], true);
    std::fs::write(root.join("package.json"), "{ \"name\": \"watch-fixture\", \"version\": \"1.0.1\" }\n").unwrap();
    assert_eq!(watcher.build_end(4)["ok"], true, "the project's package.json changed");

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// The name of the output in `dir` whose name, lowercased, starts with `stem` (a JSON module's output).
fn output_named(dir: &Path, stem: &str) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.to_lowercase().starts_with(stem))
}

/// A JSON module the build reads: rewritten with the same bytes, nothing; renamed in case only (its text the same),
/// a rebuild that gives the output the new spelling, as a clean build does.
#[test]
fn a_case_only_rename_of_a_json_module_is_seen() {
    let Some((base, root, links)) = linked_project("jsonh") else { return };
    write(&root.join("src/shared/data.json"), "{\"v\": 1}\n");
    write(&root.join("src/shared/useData.ts"), "import data from \"./data.json\";\n\nexport const v = data.v;\n");
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    let out = root.join("out/shared");
    assert_eq!(output_named(&out, "data.").as_deref().map(|n| n.starts_with("data")), Some(true));
    std::fs::write(root.join("src/shared/data.json"), "{\"v\": 1}\n").unwrap();
    watcher.assert_quiet(Duration::from_millis(1500));
    std::fs::rename(root.join("src/shared/data.json"), root.join("src/shared/Data.json")).unwrap();
    let end = watcher.build_end(1);
    assert_eq!(end["ok"], true, "{end}");
    let name = output_named(&out, "data.").unwrap_or_default();
    assert!(name.starts_with("Data"), "the output takes the new spelling: {name}");
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// Takes the deny entry `icacls` put on a folder off again when dropped (also when a test fails).
struct Denied(PathBuf);

impl Denied {
    fn new(dir: &Path) -> Denied {
        let user = std::env::var("USERNAME").unwrap();
        let status = Command::new("icacls")
            .arg(dir)
            .args(["/deny", &format!("{user}:(OI)(CI)(RD)")])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls /deny");
        Denied(dir.to_path_buf())
    }
}

impl Drop for Denied {
    fn drop(&mut self) {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let _ = Command::new("icacls").arg(&self.0).args(["/remove:d", &user]).stdout(Stdio::null()).status();
    }
}

/// A package folder the load reads by path but that cannot be listed (so its watch fails): no rebuild loop, the
/// watch retried quietly at later updates and made once the folder can be listed again, its files then seen.
#[test]
fn a_folder_that_cannot_be_watched_starts_no_rebuild_loop() {
    if std::env::var_os("USERNAME").is_none() {
        eprintln!("skipped: no USERNAME for icacls");
        return;
    }
    let Some((base, root, links)) = linked_project("denied") else { return };
    let package = root.join("node_modules/denp");
    write(&package.join("package.json"), "{ \"name\": \"denp\", \"version\": \"1.0.0\", \"types\": \"index.d.ts\" }\n");
    write(&package.join("index.d.ts"), "export type V = number;\n");
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    let denied = Denied::new(&package);
    assert!(std::fs::read_dir(&package).is_err(), "the folder cannot be listed");
    write(&root.join("src/shared/useDen.ts"), "import type { V } from \"denp\";\n\nexport const n: V = 1;\n");
    watcher.build_end(1);
    // at most one more rebuild for what the new watches found, then quiet
    let _ = watcher.settle();
    watcher.assert_quiet(Duration::from_millis(5000));
    let b = root.join("src/shared/b.ts");
    let edit = |text: &str| {
        let current = std::fs::read_to_string(&b).unwrap();
        std::fs::write(&b, current + text).unwrap();
    };
    edit("export const e1 = 1;\n");
    watcher.build_start();
    let _ = watcher.settle();
    watcher.assert_quiet(Duration::from_millis(2000));
    drop(denied);
    edit("export const e2 = 1;\n");
    watcher.build_start();
    watcher.still_ok("the folder can be listed again");
    std::fs::write(package.join("index.d.ts"), "export type V = string;\n").unwrap();
    let end = watcher.changed();
    assert!(end.is_some_and(|e| e["ok"] == false), "the package's watch was made: V is a string now");
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// Rojo project files in the output folder the Rojo project mounts (`$path: out/shared`), as roblox-ts watches them:
/// one written there by hand, edited, removed, each changes the requires. The build's own copies of a project file
/// (an asset) start nothing.
#[test]
fn rojo_project_files_in_the_output_folder_are_seen() {
    let Some((base, root, links)) = linked_project("outrojo") else { return };
    write(&root.join("src/shared/sub/lib/x.ts"), "export const x = 1;\n");
    write(&root.join("src/shared/useX.ts"), "import { x } from \"./sub/lib/x\";\n\nexport const y = x;\n");
    let out = root.join("out/shared/useX.luau");
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    assert!(import_line(&out).contains("\"shared\", \"sub\", \"lib\", \"x\""), "{}", import_line(&out));
    let project = |child: &str| {
        format!(
            "{{ \"name\": \"sub\", \"tree\": {{ \"$className\": \"Folder\", \"{child}\": {{ \"$path\": \"lib\" }} }} }}\n"
        )
    };
    let hand = root.join("out/shared/sub/default.project.json");
    let mut generation = 1;
    for (label, text, segment) in
        [("written", Some("hand"), "hand"), ("edited", Some("hand2"), "hand2"), ("removed", None, "lib")]
    {
        match text {
            Some(child) => std::fs::write(&hand, project(child)).unwrap(),
            None => std::fs::remove_file(&hand).unwrap(),
        }
        let end = watcher.build_end(generation);
        generation += 1;
        assert_eq!(end["ok"], true, "{label}: {end}");
        let line = import_line(&out);
        assert!(line.contains(&format!("\"shared\", \"sub\", \"{segment}\", \"x\"")), "{label}: {line}");
        watcher.assert_quiet(Duration::from_millis(1000));
    }
    // a project file among the sources is copied into out as an asset: the build's own write starts nothing
    write(
        &root.join("src/shared/other/default.project.json"),
        "{ \"name\": \"other\", \"tree\": { \"$className\": \"Folder\" } }\n",
    );
    assert_eq!(watcher.build_end(generation)["ok"], true);
    assert!(root.join("out/shared/other/default.project.json").is_file(), "copied as an asset");
    watcher.assert_quiet(Duration::from_millis(1500));
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// The runtime include files come from the project's `node_modules/roblox-ts/include`: the package installed, a
/// file in it edited, a file added there, each copied into the project's include folder without a source edit.
#[test]
fn the_runtime_include_source_is_watched() {
    let Some((base, root, links)) = linked_project("rbinc") else { return };
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    let package = root.join("node_modules/roblox-ts");
    let staged = base.join("roblox-ts");
    write(&staged.join("package.json"), "{ \"name\": \"roblox-ts\", \"version\": \"3.0.0\" }\n");
    write(&staged.join("include/RuntimeLib.lua"), "-- runtime v1\nreturn {}\n");
    write(&staged.join("include/Promise.lua"), "-- promise v1\nreturn {}\n");
    std::fs::rename(&staged, &package).unwrap();
    watcher.until_ok();
    watcher.still_ok("installed");
    let runtime = root.join("include/RuntimeLib.lua");
    assert!(std::fs::read_to_string(&runtime).unwrap_or_default().starts_with("-- runtime v1"));
    std::fs::write(package.join("include/RuntimeLib.lua"), "-- runtime v2\nreturn {}\n").unwrap();
    watcher.until_ok();
    assert!(std::fs::read_to_string(&runtime).unwrap().starts_with("-- runtime v2"), "edited in place");
    std::fs::write(package.join("include/Extra.lua"), "-- extra\nreturn {}\n").unwrap();
    watcher.until_ok();
    assert!(root.join("include/Extra.lua").is_file(), "a new file in the include source");
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// Project references (`rbxts_transform`'s `references` watch fixture): an edit in the deepest referenced project
/// rebuilds it, its declarations and the root (a type change there turns the root's `+` into `..`), and the
/// declarations the build writes into the referenced projects' output folders start no rebuild of their own.
#[test]
fn edits_in_a_referenced_project_are_seen() {
    let node_modules = fixture_node_modules();
    if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/references");
    let base = std::env::temp_dir().join(format!("rbxts-rs-cli-watch-refs-{}", std::process::id()));
    let root = base.join("project");
    let _ = std::fs::remove_dir_all(&base);
    copy_dir(&fixture, &root);
    let links = link_packages(&root.join("node_modules/@rbxts"));
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    watcher.assert_quiet(Duration::from_secs(2));
    let main = root.join("out/main.server.luau");
    let common = root.join("projects/common/src/index.ts");
    let declaration = root.join("projects/common/out/types/index.d.ts");
    assert!(std::fs::read_to_string(&main).unwrap().contains("common.label + common.label"));

    let text = std::fs::read_to_string(&common).unwrap();
    std::fs::write(&common, text.replace("export let label: number = 0;", "export let label: string = \"\";")).unwrap();
    watcher.until_ok();
    watcher.still_ok("label to string");
    assert!(std::fs::read_to_string(&main).unwrap().contains("common.label .. common.label"), "the root rebuilt");
    assert!(std::fs::read_to_string(&declaration).unwrap().contains("label: string"), "the declaration rewritten");

    let text = std::fs::read_to_string(&common).unwrap();
    std::fs::write(&common, text.replace("total += value;", "total += value * 7;")).unwrap();
    watcher.until_ok();
    watcher.still_ok("body edit");
    let output = std::fs::read_to_string(root.join("projects/common/out/init.luau")).unwrap();
    assert!(output.contains("value * 7"), "the referenced project rebuilt: {output}");

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// One build at startup: the folders the first build makes for its outputs (`out` above the outDir `out/game`, a
/// `cache` for the build infos; `rbxts_transform`'s `midsolution` watch fixture) are its own writes, not changes made
/// before their directory was watched.
#[test]
fn the_first_build_starts_no_second_one() {
    let node_modules = fixture_node_modules();
    if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/midsolution");
    let base = std::env::temp_dir().join(format!("rbxts-rs-cli-watch-first-{}", std::process::id()));
    let root = base.join("project");
    let _ = std::fs::remove_dir_all(&base);
    copy_dir(&fixture, &root);
    let links = link_packages(&root.join("node_modules/@rbxts"));
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    assert!(root.join("cache").is_dir() && root.join("out/game").is_dir());
    watcher.assert_quiet(Duration::from_secs(3));
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// A new file in a folder a referenced project's tsconfig includes (`projects/common` includes `types`, outside its
/// root dir; `rbxts_transform`'s `midsolution` watch fixture) is matched with that project's include patterns, not
/// the root's: a `.d.ts` redeclaring a global there starts a build, which fails, and its removal one that succeeds.
#[test]
fn a_new_file_a_referenced_project_includes_is_seen() {
    let node_modules = fixture_node_modules();
    if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/midsolution");
    let base = std::env::temp_dir().join(format!("rbxts-rs-cli-watch-include-{}", std::process::id()));
    let root = base.join("project");
    let _ = std::fs::remove_dir_all(&base);
    copy_dir(&fixture, &root);
    let links = link_packages(&root.join("node_modules/@rbxts"));
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    let dup = root.join("projects/common/types/dup.d.ts");
    std::fs::write(&dup, "declare const SEP: boolean;\n").unwrap();
    let end = watcher.build_end(1);
    assert_eq!(end["ok"], false, "the conflicting declaration is part of common's program: {end}");
    std::fs::remove_file(&dup).unwrap();
    assert_eq!(watcher.build_end(2)["ok"], true);
    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// Files no program takes in start no rebuild: a TypeScript file in a folder the tsconfig excludes (in a source root),
/// a folder made where no include could reach, and a declaration file in it. What a program does take in, or an asset
/// in the excluded folder (roblox-ts copies everything under a root dir but compilable files), does.
#[test]
fn files_no_program_takes_in_start_no_rebuild() {
    let Some((base, root, links)) = linked_project("noprogram") else { return };
    let tsconfig = std::fs::read_to_string(root.join("tsconfig.json")).unwrap();
    let tsconfig = tsconfig.replacen("\"include\": [\"src\"],", "\"include\": [\"src\"],\n\t\"exclude\": [\"src/excluded\"],", 1);
    assert!(tsconfig.contains("\"exclude\""), "{tsconfig}");
    std::fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
    std::fs::create_dir_all(root.join("src/excluded")).unwrap();
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    watcher.assert_quiet(Duration::from_millis(1000));

    write(&root.join("src/excluded/skipped.ts"), "export const skipped: number = \"not checked\";\n");
    write(&root.join("src/excluded/types.d.ts"), "declare const excludedGlobal: number;\n");
    watcher.assert_quiet(Duration::from_millis(1500));
    std::fs::create_dir_all(root.join("elsewhere")).unwrap();
    watcher.assert_quiet(Duration::from_millis(1500));
    write(&root.join("elsewhere/extra.d.ts"), "declare const elsewhereGlobal: number;\n");
    write(&root.join("elsewhere/extra.ts"), "export const extra = 1;\n");
    watcher.assert_quiet(Duration::from_millis(1500));
    std::fs::remove_file(root.join("src/excluded/skipped.ts")).unwrap();
    watcher.assert_quiet(Duration::from_millis(1500));

    // an asset in the excluded folder is copied
    write(&root.join("src/excluded/asset.lua"), "return 1\n");
    assert_eq!(watcher.build_end(1)["ok"], true);
    assert!(root.join("out/excluded/asset.lua").is_file());
    // a file the program takes in
    write(&root.join("src/shared/fresh.ts"), "export const fresh = 1;\n");
    assert_eq!(watcher.build_end(2)["ok"], true);
    assert!(root.join("out/shared/fresh.luau").is_file());

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}

/// An output edited or deleted by hand is written again at once: the watcher counts the event (an output the last
/// commit left, no longer as it left it), and the build transforms its source again. Its own writes start nothing.
#[test]
fn outputs_changed_by_hand_are_written_again() {
    let Some((base, root, links)) = linked_project("handedit") else { return };
    let watcher = start(&root);
    assert_eq!(watcher.build_end(0)["ok"], true);
    watcher.assert_quiet(Duration::from_millis(1000));
    let d = root.join("out/shared/d.luau");
    let before = std::fs::read_to_string(&d).unwrap();

    std::fs::write(&d, "-- edited by hand\nreturn nil\n").unwrap();
    assert_eq!(watcher.build_end(1)["ok"], true);
    assert_eq!(std::fs::read_to_string(&d).unwrap(), before);
    watcher.assert_quiet(Duration::from_millis(1500));

    std::fs::remove_file(&d).unwrap();
    assert_eq!(watcher.build_end(2)["ok"], true);
    assert_eq!(std::fs::read_to_string(&d).unwrap(), before);
    watcher.assert_quiet(Duration::from_millis(1500));

    // an asset's copy
    let asset = root.join("out/shared/asset.lua");
    let asset_before = std::fs::read(&asset).unwrap();
    std::fs::write(&asset, "-- edited by hand\n").unwrap();
    assert_eq!(watcher.build_end(3)["ok"], true);
    assert_eq!(std::fs::read(&asset).unwrap(), asset_before);
    watcher.assert_quiet(Duration::from_millis(1500));

    // a runtime include file
    let runtime = ["include/RuntimeLib.luau", "include/RuntimeLib.lua"]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file())
        .expect("the include copy");
    let runtime_before = std::fs::read(&runtime).unwrap();
    std::fs::write(&runtime, "-- edited by hand\n").unwrap();
    assert_eq!(watcher.build_end(4)["ok"], true);
    assert_eq!(std::fs::read(&runtime).unwrap(), runtime_before);
    watcher.assert_quiet(Duration::from_millis(1500));
    std::fs::remove_file(&runtime).unwrap();
    assert_eq!(watcher.build_end(5)["ok"], true);
    assert_eq!(std::fs::read(&runtime).unwrap(), runtime_before);
    watcher.assert_quiet(Duration::from_millis(1500));

    for link in links {
        unlink(&link);
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&base);
}
