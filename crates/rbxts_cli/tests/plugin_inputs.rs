//! The `probe-inputs` test plugin (`rbxts_test_plugins::inputs`) through the command line, in processes of its own:
//! this test binary is the compiler when `RBXTS_TEST_CHILD_ARGS` is set (the `child` test).
//! - an incremental build in a new process transforms again only the files whose `Cx::env` reads read another value
//!   now; a plugin diagnostic shows its source in TypeScript's code slot (`TS @probe/inputs`);
//! - the watcher: a file a finish glob matches, a listed directory's entries, a path finish tested or read, a file a
//!   file's glob matches, a glob root removed and created again; events that change no answer start nothing; a
//!   listed directory that cannot be watched and builds failing before `finish` start no rebuild loop.

#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;

const CHILD_ARGS: &str = "RBXTS_TEST_CHILD_ARGS";

/// The compiler with `probe-inputs`, when this process was started as one (its arguments in `RBXTS_TEST_CHILD_ARGS`,
/// one per line); otherwise nothing.
#[test]
fn child() {
    let Ok(args) = std::env::var(CHILD_ARGS) else { return };
    let args: Vec<String> = args.lines().map(str::to_owned).collect();
    let status =
        rbxts_cli::Cli::new().erased_plugin(rbxts_test_plugins::by_name("probe-inputs").unwrap()).run_with(args);
    std::process::exit(i32::from(status));
}

/// This test binary as the compiler, in `root`.
fn compiler(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env(CHILD_ARGS, args.join("\n"))
        .env_remove("PROBE_FILE")
        .env_remove("PROBE_SETUP")
        .current_dir(root);
    command
}

/// A copy of the basic watch fixture with a linked node_modules and the probe's directories; removed on drop.
struct Project(PathBuf);

impl Project {
    fn new(label: &str, incremental: bool) -> Option<Project> {
        let node_modules = rbxts_test_support::node_modules().unwrap_or_default();
        if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
            rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
            return None;
        }
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/basic");
        let root = std::env::temp_dir().join(format!("rbxts-rs-cli-inputs-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_dir(&fixture, &root);
        let project = Project(root);
        // the command line has no stamp plugin
        let mut tsconfig = project.read("tsconfig.json").replace(
            "\"plugins\": [{ \"transform\": \"rbxts-transform-stamp\", \"prefix\": \"watch \" }]",
            "\"plugins\": []",
        );
        if incremental {
            tsconfig =
                tsconfig.replacen("\"compilerOptions\": {", "\"compilerOptions\": {\n\t\t\"incremental\": true,", 1);
        }
        project.write("tsconfig.json", &tsconfig);
        for dir in ["setup-glob", "file-glob", "globs/finish", "finish-dir"] {
            std::fs::create_dir_all(project.0.join(dir)).unwrap();
        }
        project.write("src/shared/marked.ts", "// probe-inputs\nexport const marked = 1;\n");
        project.write("src/shared/globbed.ts", "// probe-inputs probe-glob\nexport const globbed = 1;\n");
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(project.0.join("node_modules").to_string_lossy().replace('/', "\\"))
            .arg(node_modules.to_string_lossy().replace('/', "\\"))
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "could not link node_modules");
        Some(project)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.0.join(relative)).unwrap_or_default()
    }

    fn exists(&self, relative: &str) -> bool {
        self.0.join(relative).exists()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = Command::new("cmd")
            .args(["/C", "rmdir"])
            .arg(self.0.join("node_modules").to_string_lossy().replace('/', "\\"))
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

/// One `rbxtsc --verbose` build: its output and the number of files it transformed.
fn build(project: &Project, env: &[(&str, &str)]) -> (String, usize) {
    let mut command = compiler(&project.0, &["--verbose"]);
    command.envs(env.iter().copied());
    let output = command.output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "the build failed:\n{text}");
    // `<n> files, <m> written, <e> errors` (after the test harness's own `test child ... `)
    let files = text
        .lines()
        .filter(|line| line.ends_with(" errors"))
        .find_map(|line| line.split_once(" files, ").and_then(|(n, _)| n.split_whitespace().last()))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no file count in:\n{text}"));
    (text, files)
}

#[test]
fn a_new_process_transforms_the_files_whose_environment_changed() {
    let Some(project) = Project::new("env", true) else { return };
    project.write("src/shared/warned.ts", "// probe-warn probe-own-source\nexport const warned = 1;\n");
    let (output, all) = build(&project, &[]);
    assert!(all > 3);
    // the plugin's source in the code slot, and a diagnostic's own
    assert!(output.contains(" TS @probe/inputs: \u{1b}[0mprobe-inputs warning"), "{output}");
    assert!(output.contains(" TS @probe/own: \u{1b}[0mprobe-inputs own source"), "{output}");
    // nothing changed: the file with a glob only (its matcher is not persisted); the other files' warnings are reported
    // again from the incremental state, with their sources, as a clean build reports them
    let (again, count) = build(&project, &[]);
    assert_eq!(count, 1);
    assert!(again.contains(" TS @probe/inputs: \u{1b}[0mprobe-inputs warning"), "{again}");
    assert!(again.contains(" TS @probe/own: \u{1b}[0mprobe-inputs own source"), "{again}");
    // PROBE_FILE set: the two files that read it (one has the glob too), and nothing else
    assert_eq!(build(&project, &[("PROBE_FILE", "one")]).1, 2);
    assert!(project.read("out/shared/marked.luau").contains("env Some(\"one\")"));
    assert_eq!(build(&project, &[("PROBE_FILE", "one")]).1, 1);
    // setup's environment is a build input: every file
    assert_eq!(build(&project, &[("PROBE_FILE", "one"), ("PROBE_SETUP", "s")]).1, all);
    assert!(project.read("probe-inputs.txt").contains("setup env Some(\"s\")"));
}

/// The watcher process; killed on drop.
struct Watcher {
    child: Child,
    events: Receiver<Value>,
    lines: Receiver<String>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Watcher {
    fn start(project: &Project) -> Watcher {
        Watcher::start_with(project, &[])
    }

    fn start_with(project: &Project, env: &[(&str, &str)]) -> Watcher {
        let mut child = compiler(&project.0, &["-w", "--events", "json"])
            .envs(env.iter().copied())
            .env("RBXTS_WATCH_MEMORY_LIMIT_MB", "0")
            .env("RBXTS_WATCH_CHILD", "1")
            .stdin(Stdio::piped())
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
        Watcher { child, events, lines }
    }

    /// The next `build-end` event, of any generation, within 60 s; it must have succeeded.
    fn next_end(&self) -> Value {
        let end = self.next_end_any();
        assert_eq!(end["ok"], true, "{end}");
        end
    }

    /// The next `build-end` event, of any generation, within 60 s, failed or not.
    fn next_end_any(&self) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(event) = self.events.recv_timeout(left) else {
                let output: Vec<String> = self.lines.try_iter().collect();
                panic!("no build-end within 60 s; output:\n{}", output.join("\n"));
            };
            if event["event"] == "build-end" {
                return event;
            }
        }
    }

    /// The `build-end` events until none comes for 2 s, for at most 8 s (a rebuild loop goes on).
    fn ends_until_quiet(&self) -> Vec<Value> {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        let mut ends = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now())
            && let Ok(event) = self.events.recv_timeout(left.min(Duration::from_secs(2)))
        {
            if event["event"] == "build-end" {
                ends.push(event);
            }
        }
        ends
    }

    /// Takes events until none comes for 1.5 s (a rebuild for a directory this watcher's own artifacts created or
    /// removed in the project directory may follow a build).
    fn settle(&self) {
        while self.events.recv_timeout(Duration::from_millis(1500)).is_ok() {}
    }

    /// No build starts within `quiet`.
    fn assert_quiet(&self, quiet: Duration) {
        if let Ok(event) = self.events.recv_timeout(quiet) {
            panic!("unexpected event {event}");
        }
    }
}

#[test]
fn the_watcher_follows_the_recorded_inputs() {
    let Some(project) = Project::new("watch", false) else { return };
    let watcher = Watcher::start(&project);
    watcher.next_end();
    watcher.settle();
    let quiet = Duration::from_millis(1500);

    // a finish glob: a file it does not match is nothing, one it matches runs finish only
    project.write("globs/finish/notes.md", "x");
    watcher.assert_quiet(quiet);
    project.write("globs/finish/a.json", "{}");
    let end = watcher.next_end();
    assert_eq!(end["transformed"], 0, "{end}");
    assert!(project.read("probe-inputs.txt").contains("finish glob [\"a.json\"]"));
    watcher.assert_quiet(quiet);

    // a listed directory: an entry writes an artifact, its removal removes it and the directories it emptied
    project.write("finish-dir/x.txt", "x");
    watcher.next_end();
    assert!(project.exists("probe-out/deep/x.txt.out"));
    watcher.settle();
    std::fs::remove_file(project.0.join("finish-dir/x.txt")).unwrap();
    watcher.next_end();
    assert!(!project.exists("probe-out"));
    watcher.settle();

    // `exists` and `read_file` in the project directory (Flamework's place source and build info): a path appearing
    // or a text changing runs finish only; the same text written again is nothing
    project.write("finish-flag", "");
    let end = watcher.next_end();
    assert_eq!(end["transformed"], 0, "{end}");
    assert!(project.read("probe-inputs.txt").contains("flag true"));
    watcher.settle();
    project.write("finish-file.txt", "first\n");
    let end = watcher.next_end();
    assert_eq!(end["transformed"], 0, "{end}");
    assert!(project.read("probe-inputs.txt").contains("file Some(\"first\\n\")"));
    watcher.settle();
    project.write("finish-file.txt", "first\n");
    watcher.assert_quiet(quiet);
    std::fs::remove_file(project.0.join("finish-flag")).unwrap();
    watcher.next_end();
    assert!(project.read("probe-inputs.txt").contains("flag false"));
    watcher.settle();

    // a file's glob: that file only; a directory its matcher does not accept is nothing
    std::fs::create_dir_all(project.0.join("file-glob/sub")).unwrap();
    watcher.assert_quiet(quiet);
    project.write("file-glob/a.txt", "x");
    let end = watcher.next_end();
    assert_eq!(end["transformed"], 1, "{end}");
    assert!(project.read("out/shared/globbed.luau").contains("glob [\"a.txt\"]"));
    watcher.assert_quiet(quiet);

    // a glob root removed (its matches go), created again empty (nothing changes, but it is watched again), then
    // given a match
    std::fs::remove_dir_all(project.0.join("globs/finish")).unwrap();
    watcher.next_end();
    assert!(project.read("probe-inputs.txt").contains("finish glob []"));
    watcher.assert_quiet(quiet);
    std::fs::create_dir_all(project.0.join("globs/finish")).unwrap();
    watcher.assert_quiet(quiet);
    project.write("globs/finish/c.json", "{}");
    watcher.next_end();
    assert!(project.read("probe-inputs.txt").contains("finish glob [\"c.json\"]"));
    watcher.assert_quiet(quiet);
    drop(watcher);
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

/// A directory `finish` lists that cannot be listed (its watch fails at every update) while builds stop before
/// `finish` (a type error) and a file `finish` read changes: one rebuild, not one after another. Then a new entry in
/// a directory setup globs, still broken: one rebuild too (a failed watch is not a new one at every update; the
/// recheck's finish-only rule doesn't cover setup's reads, so this one catches that rule alone). Fixing the error
/// rebuilds once, with the changed file; once the directory can be listed again, its watch is made and its entries
/// are seen.
#[test]
fn a_failed_input_watch_and_builds_failing_before_finish_start_no_rebuild_loop() {
    if std::env::var_os("USERNAME").is_none() {
        eprintln!("skipped: no USERNAME for icacls");
        return;
    }
    let Some(project) = Project::new("deniedloop", false) else { return };
    let denied = Denied::new(&project.0.join("finish-dir"));
    assert!(std::fs::read_dir(project.0.join("finish-dir")).is_err(), "the folder cannot be listed");
    let watcher = Watcher::start(&project);
    watcher.next_end();
    watcher.settle();
    let quiet = Duration::from_millis(2000);

    let good = project.read("src/shared/d.ts");
    project.write("src/shared/d.ts", &format!("{good}\nexport const broken: number = \"x\";\n"));
    let end = watcher.next_end_any();
    assert_eq!(end["ok"], false, "{end}");
    watcher.assert_quiet(quiet);
    project.write("finish-file.txt", "changed while broken\n");
    let ends = watcher.ends_until_quiet();
    assert!(ends.len() <= 1, "{} rebuilds after one change: {:?}", ends.len(), ends.first());
    assert!(ends.iter().all(|end| end["ok"] == false), "{ends:?}");
    project.write("setup-glob/x.txt", "x");
    let ends = watcher.ends_until_quiet();
    assert!(ends.len() <= 1, "{} rebuilds after a new setup-glob entry: {:?}", ends.len(), ends.first());
    assert!(ends.iter().all(|end| end["ok"] == false), "{ends:?}");

    project.write("src/shared/d.ts", &good);
    watcher.next_end();
    assert!(project.read("probe-inputs.txt").contains("file Some(\"changed while broken\\n\")"));
    assert!(project.read("probe-inputs.txt").contains("x.txt"), "{}", project.read("probe-inputs.txt"));
    watcher.assert_quiet(quiet);

    // listable again: the next build lists it and its watch is made, then an entry is seen
    drop(denied);
    project.write("src/shared/d.ts", &format!("{good}// after the grant\n"));
    watcher.next_end();
    assert!(project.read("probe-inputs.txt").contains("finish dir Some([])"));
    watcher.settle();
    project.write("finish-dir/new.txt", "n");
    watcher.next_end();
    assert!(project.exists("probe-out/deep/new.txt.out"));
    // (and one for the artifact folder it created)
    watcher.settle();
    watcher.assert_quiet(quiet);
    drop(watcher);
}

/// An invalid `RBXTS_CHECKERS` is warned about once per watch session, not at every rebuild; an empty one counts as
/// unset (no warning).
#[test]
fn the_checkers_warning_is_given_once_per_watch_session() {
    let Some(project) = Project::new("checkers", false) else { return };
    let watcher = Watcher::start_with(&project, &[("RBXTS_CHECKERS", "many")]);
    watcher.next_end();
    watcher.settle();
    let good = project.read("src/shared/d.ts");
    project.write("src/shared/d.ts", &format!("{good}\n// again\n"));
    watcher.next_end();
    watcher.settle();
    let warnings: Vec<String> = watcher.lines.try_iter().filter(|l| l.contains("RBXTS_CHECKERS=")).collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    drop(watcher);

    let (output, _) = build(&project, &[("RBXTS_CHECKERS", "")]);
    assert!(!output.contains("RBXTS_CHECKERS"), "{output}");
    let (output, _) = build(&project, &[("RBXTS_CHECKERS", " ")]);
    assert!(!output.contains("RBXTS_CHECKERS"), "{output}");
}
