//! A plugin state file in `rbxtsc -w`, with the `stamp` test plugin (its state file `.stamp-state` in the project
//! directory), in a process of its own: this test binary is the compiler when `RBXTS_TEST_CHILD_ARGS` is set (the
//! `child` test). The watcher's own write of the state file and the same bytes written again start nothing; the file
//! deleted, edited or put back to the bytes before the last commit starts a rebuild in which the plugin starts from
//! a fresh state and every file is transformed.
//!
//! With the `scan` test plugin too (`RBXTS_TEST_CHILD_SCAN`), whose `Shared` depends on whether its own state file
//! exists (as Flamework's candidate scan): the build whose commit creates that file is followed at once by one more,
//! and nothing after it.

#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;

const CHILD_ARGS: &str = "RBXTS_TEST_CHILD_ARGS";
const STATE: &str = ".stamp-state";
const SCAN_ENV: &str = "RBXTS_TEST_CHILD_SCAN";

/// The compiler with `stamp` (and `scan` with `RBXTS_TEST_CHILD_SCAN`), when this process was started as one (its
/// arguments in `RBXTS_TEST_CHILD_ARGS`, one per line); otherwise nothing.
#[test]
fn child() {
    let Ok(args) = std::env::var(CHILD_ARGS) else { return };
    let args: Vec<String> = args.lines().map(str::to_owned).collect();
    let mut cli = rbxts_cli::Cli::new().erased_plugin(rbxts_test_plugins::by_name("stamp").unwrap());
    if std::env::var_os(SCAN_ENV).is_some() {
        cli = cli.erased_plugin(rbxts_test_plugins::by_name("scan").unwrap());
    }
    let status = cli.run_with(args);
    std::process::exit(i32::from(status));
}

/// A copy of the basic watch fixture (its tsconfig turns `stamp` on) with a linked node_modules; removed on drop.
struct Project(PathBuf);

impl Project {
    fn new(label: &str) -> Option<Project> {
        let node_modules = rbxts_test_support::node_modules().unwrap_or_default();
        if !node_modules.join("@rbxts").join("compiler-types").is_dir() {
            rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
            return None;
        }
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/tests/watch_fixtures/basic");
        let root = std::env::temp_dir().join(format!("rbxts-rs-cli-state-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_dir(&fixture, &root);
        let project = Project(root);
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
        std::fs::write(self.0.join(relative), text).unwrap();
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(relative)).ok()
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
        Watcher::start_with(project, false)
    }

    fn start_with(project: &Project, scan: bool) -> Watcher {
        let mut command = Command::new(std::env::current_exe().unwrap());
        if scan {
            command.env(SCAN_ENV, "1");
        } else {
            command.env_remove(SCAN_ENV);
        }
        let mut child = command
            .args(["--exact", "child", "--nocapture", "--test-threads=1"])
            .env(CHILD_ARGS, ["-w", "--events", "json"].join("\n"))
            .env_remove("STAMP_PREFIX")
            .env("RBXTS_WATCH_MEMORY_LIMIT_MB", "0")
            .env("RBXTS_WATCH_CHILD", "1")
            .current_dir(&project.0)
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

    /// The next `build-start` event within 60 s.
    fn next_start(&self) -> Value {
        let event = self.events.recv_timeout(Duration::from_secs(60)).expect("a build-start within 60 s");
        assert_eq!(event["event"], "build-start", "{event}");
        event
    }

    /// The next `build-end` event within 60 s; it must have succeeded.
    fn next_end(&self) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(event) = self.events.recv_timeout(left) else {
                let output: Vec<String> = self.lines.try_iter().collect();
                panic!("no build-end within 60 s; output:\n{}", output.join("\n"));
            };
            if event["event"] == "build-end" {
                assert_eq!(event["ok"], true, "{event}");
                return event;
            }
        }
    }

    /// No build starts within `quiet`.
    fn assert_quiet(&self, quiet: Duration) {
        if let Ok(event) = self.events.recv_timeout(quiet) {
            panic!("unexpected event {event}");
        }
    }
}

#[test]
fn a_state_file_deleted_or_edited_while_watching_starts_its_plugin_fresh() {
    let Some(project) = Project::new("watch") else { return };
    let watcher = Watcher::start(&project);
    let quiet = Duration::from_millis(2000);
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    assert_eq!(project.read(STATE).as_deref(), Some("1"));
    // the watcher's own write of the state file (and of the artifact) starts nothing
    watcher.assert_quiet(quiet);

    // an edit: the closure; the state file written again (its own write) starts nothing
    let d = "src/shared/d.ts";
    let text = project.read(d).unwrap();
    project.write(d, &format!("{text}\nexport const sfile = 1;\n"));
    let end = watcher.next_end();
    assert_eq!(end["all"], false, "{end}");
    assert_eq!(project.read(STATE).as_deref(), Some("2"));
    watcher.assert_quiet(quiet);

    // the same bytes written again: nothing
    project.write(STATE, "2");
    watcher.assert_quiet(quiet);

    // deleted: a rebuild, the plugin fresh (its build count starts over), every file
    std::fs::remove_file(project.0.join(STATE)).unwrap();
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    assert_eq!(project.read(STATE).as_deref(), Some("1"));
    assert!(project.read("stamp-report.json").unwrap_or_default().ends_with(",\"builds\":1}\n"));
    watcher.assert_quiet(quiet);

    // edited
    project.write(STATE, "41");
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    assert_eq!(project.read(STATE).as_deref(), Some("1"));
    watcher.assert_quiet(quiet);

    // the bytes before the last commit
    project.write(STATE, "41");
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    assert_eq!(project.read(STATE).as_deref(), Some("1"));
    watcher.assert_quiet(quiet);
    drop(watcher);
}

/// `scan`: its setup reads its own state file as a build input and makes another `Shared` while it is missing. The
/// build whose commit creates it is followed at once by one more (named by the file), which runs setup again, so the
/// output is the one a build with the file there makes; that one starts nothing, nor does an edit. Deleting the file
/// does the same again.
#[test]
fn a_state_file_the_commit_creates_starts_one_more_build_at_once() {
    let Some(project) = Project::new("scan") else { return };
    project.write("src/shared/scan.d.ts", "/** @scan */\ndeclare function scanned(): string;\n");
    project.write("src/shared/scanned.ts", "export const answer = scanned();\n");
    project.write("scan-ancestor.txt", "first");
    let scanned = || project.read("out/shared/scanned.luau").unwrap_or_default();
    let is_state = |change: &Value| change.as_str().is_some_and(|c| c.replace('\\', "/").ends_with("/scan-state.txt"));
    let watcher = Watcher::start_with(&project, true);
    let quiet = Duration::from_millis(2000);
    // the first build reads the ancestor, the one after it the state file the first created
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    let start = watcher.next_start();
    let changes = start["changes"].as_array().unwrap();
    assert!(changes.len() == 1 && is_state(&changes[0]), "{start}");
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    assert!(scanned().contains("\"own\""), "{}", scanned());
    watcher.assert_quiet(quiet);

    // an edit: the closure, then nothing
    let d = "src/shared/d.ts";
    let text = project.read(d).unwrap();
    project.write(d, &format!("{text}\nexport const scan = 1;\n"));
    let end = watcher.next_end();
    assert_eq!(end["all"], false, "{end}");
    watcher.assert_quiet(quiet);

    // deleted: the plugin starts fresh and reads the ancestor; the commit creates the file again: one more build
    std::fs::remove_file(project.0.join("scan-state.txt")).unwrap();
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    let start = watcher.next_start();
    assert!(start["changes"].as_array().unwrap().iter().any(is_state), "{start}");
    let end = watcher.next_end();
    assert_eq!(end["all"], true, "{end}");
    assert!(scanned().contains("\"own\""), "{}", scanned());
    watcher.assert_quiet(quiet);
    drop(watcher);
}

/// `scan` (val-sfile2 `racedel`): the state file deleted, then deleted again as the follow-up build starts (its
/// `build-start` event; the build looks at the disk well after). The follow-up lost the file itself, so it starts one
/// more build (only an outside change does that: no loop), and the output is the one a build with the file there
/// makes, not the one of a setup that read it as missing.
#[test]
fn a_follow_up_that_lost_its_state_file_starts_one_more_build() {
    let Some(project) = Project::new("racedel") else { return };
    project.write("src/shared/scan.d.ts", "/** @scan */\ndeclare function scanned(): string;\n");
    project.write("src/shared/scanned.ts", "export const answer = scanned();\n");
    project.write("scan-ancestor.txt", "first");
    let scanned = || project.read("out/shared/scanned.luau").unwrap_or_default();
    let watcher = Watcher::start_with(&project, true);
    let quiet = Duration::from_millis(2000);
    watcher.next_end();
    watcher.next_start();
    watcher.next_end();
    assert!(scanned().contains("\"own\""), "{}", scanned());
    watcher.assert_quiet(quiet);

    std::fs::remove_file(project.0.join("scan-state.txt")).unwrap();
    watcher.next_start();
    watcher.next_end();
    // the follow-up: deleted again before it looks
    watcher.next_start();
    std::fs::remove_file(project.0.join("scan-state.txt")).unwrap();
    watcher.next_end();
    // and the one it starts, which reads the file its own commit created
    watcher.next_start();
    watcher.next_end();
    assert!(scanned().contains("\"own\""), "{}", scanned());
    assert!(project.read("scan-state.txt").is_some());
    watcher.assert_quiet(quiet);
    drop(watcher);
}
