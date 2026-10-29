//! Local runs: a project's `src/**/*.local.spec.luau` (bench: `*.local.bench.luau`) in in-process VMs, one fresh VM
//! per file, so no file sees another's module state and a crash or a hang costs one file, not the run.
//!
//! Focus is global although files run apart, by one speculative pass: every file runs unfocused and reports how many
//! of its selected cases have `only`; when any has, those results are dropped and only the files with `only` cases
//! run again, focused. The common run, with no `.only`, costs one pass.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use luau::vm::Vm;
use serde_json::{Value, json};

use crate::{CHUNK, Host, Mode, Request, Suite, aggregate, bundle};

/// How a local run runs.
#[derive(Clone, Debug)]
pub struct LocalOptions {
    /// Spec or bench files.
    pub mode: Mode,
    /// Selects cases by label.
    pub filter: Option<String>,
    /// How long one file may run before its VM is interrupted.
    pub timeout: Option<Duration>,
    /// Native code for `--!native` modules and `@native` functions, as on Roblox; off measures interpreted.
    pub codegen: bool,
}

/// Runs the project's local suites: the aggregate report, and what each file printed (`(label, output)`, files that
/// printed nothing left out), in file order. Test files run on a thread per core; bench files one at a time with
/// nothing else running, so timings do not share cores.
pub fn run_local(root: &Path, opts: &LocalOptions) -> Result<(Value, Vec<(String, String)>), String> {
    let suffix = match opts.mode {
        Mode::Test => ".local.spec.luau",
        Mode::Bench => ".local.bench.luau",
    };
    let mut files = Vec::new();
    discover(root, "src", suffix, &mut files)?;
    files.sort();

    let mut results = run_files(root, &files, opts, false)?;
    let focused: Vec<String> = files
        .iter()
        .zip(&results)
        .filter(|(_, (report, _))| report["only"].as_u64().is_some_and(|only| only > 0))
        .map(|(file, _)| file.clone())
        .collect();
    let mut discovered = None;
    if !focused.is_empty() {
        discovered = Some(results.iter().filter_map(|(report, _)| report["discovered"].as_u64()).sum::<u64>());
        files = focused;
        results = run_files(root, &files, opts, true)?;
    }

    let reports: Vec<Value> = results.iter().map(|(report, _)| report.clone()).collect();
    let mut report = aggregate(&reports, opts.mode, opts.filter.as_deref());
    if let Some(discovered) = discovered {
        // a focused run discovered every file's cases, not only the focused files'
        report["discovered"] = discovered.into();
    }
    let outputs = files.into_iter().zip(results).filter(|(_, (_, out))| !out.is_empty()).map(|(f, (_, o))| (f, o));
    Ok((report, outputs.collect()))
}

/// Whether a trace line of a local run is a project frame: everything but C functions and the runtime (`[C]`,
/// [`CHUNK`]), since every other chunk is named by its path in the project.
pub fn project_frame(line: &str) -> bool {
    !line.starts_with('[')
}

/// Appends the files under `root/dir` whose names end in `suffix`, as paths relative to `root` with `/`. A missing
/// directory holds none; symlinks are not followed.
fn discover(root: &Path, dir: &str, suffix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = match std::fs::read_dir(root.join(dir)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{dir}: {e}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("{dir}: {e}"))?;
        let Ok(name) = entry.file_name().into_string() else { continue };
        let kind = entry.file_type().map_err(|e| format!("{dir}/{name}: {e}"))?;
        let path = format!("{dir}/{name}");
        if kind.is_dir() {
            discover(root, &path, suffix, out)?;
        } else if kind.is_file() && name.ends_with(suffix) {
            out.push(path);
        }
    }
    Ok(())
}

/// Each file's report and output, in `files` order, from worker threads pulling the next file. Workers get a deep
/// stack: Luau's parser and require chains recurse through C++ frames.
fn run_files(root: &Path, files: &[String], opts: &LocalOptions, focus: bool) -> Result<Vec<(Value, String)>, String> {
    let workers = match opts.mode {
        Mode::Bench => 1,
        Mode::Test => std::thread::available_parallelism().map_or(1, |n| n.get()),
    };
    let next = AtomicUsize::new(0);
    let results = Mutex::new(vec![None; files.len()]);
    let started = std::thread::scope(|scope| {
        let work = || {
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(file) = files.get(index) else { break };
                let result = run_file(root, file, opts, focus);
                results.lock().unwrap_or_else(|e| e.into_inner())[index] = Some(result);
            }
        };
        let builder = || std::thread::Builder::new().stack_size(64 << 20);
        (0..workers.min(files.len())).filter(|_| builder().spawn_scoped(scope, work).is_ok()).count()
    });
    if started == 0 && !files.is_empty() {
        return Err("could not start a thread to run specs on".to_owned());
    }
    Ok(results.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().flatten().collect())
}

/// One file in a fresh VM: its report and what it printed. A VM that cannot start, a run that dies outside a case and
/// a run past the timeout each become one failing case of the file, so the run goes on.
fn run_file(root: &Path, file: &str, opts: &LocalOptions, focus: bool) -> (Value, String) {
    let mut vm = match Vm::new(root, opts.codegen) {
        Ok(vm) => vm,
        Err(e) => return (failed(file, "runs suite", &e, None, opts), String::new()),
    };
    let suites = [Suite { label: file.to_owned(), path: Vec::new() }];
    let req = Request { suites: &suites, mode: opts.mode, filter: opts.filter.as_deref(), focus: Some(focus) };
    let source = bundle(Host::Vm, &req);

    let (done, finished) = mpsc::channel::<()>();
    let watchdog = opts.timeout.map(|timeout| {
        let interrupter = vm.interrupter();
        std::thread::spawn(move || {
            let late = finished.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout);
            if late {
                interrupter.interrupt();
            }
            late
        })
    });
    let result = vm.eval(CHUNK, &source);
    drop(done);
    let timed_out = watchdog.is_some_and(|watchdog| watchdog.join().unwrap_or(false));
    let output = vm.take_output();

    let report = match result {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            failed(file, "runs suite", &format!("the runtime returned no report: {e}"), None, opts)
        }),
        Err(_) if timed_out => {
            let seconds = opts.timeout.unwrap_or_default().as_secs_f64();
            let message = format!("timed out after {seconds} s");
            failed(file, &message, &message, Some("test.timeout"), opts)
        }
        Err(e) => failed(file, "runs suite", &e, None, opts),
    };
    (report, output)
}

/// A file's report holding one failing test, `<file> / <case>`, with its diagnostic `code` when it has one.
fn failed(file: &str, case: &str, message: &str, code: Option<&str>, opts: &LocalOptions) -> Value {
    let mut failure = json!({
        "kind": "test", "labels": [file, case], "message": message, "source": file, "line": 0, "trace": "",
    });
    if let Some(code) = code {
        failure["code"] = code.into();
    }
    let mut report = aggregate(&[], opts.mode, opts.filter.as_deref());
    for key in ["discovered", "selected", "test_total", "test_fail"] {
        report[key] = 1.into();
    }
    report["ok"] = false.into();
    report["failures"] = json!([failure]);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(mode: Mode, filter: Option<&str>, timeout: Option<Duration>) -> LocalOptions {
        LocalOptions { mode, filter: filter.map(str::to_owned), timeout, codegen: true }
    }

    /// Fails with every failure of `report` printed.
    fn assert_passed(report: &Value) {
        assert!(report["ok"] == true, "{}", crate::print(report, "local", &project_frame, false));
    }

    /// A project in a fresh temporary directory, from `(path, contents)` pairs.
    fn project(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("opossum-spec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    fn labels(report: &Value, list: &str) -> Vec<String> {
        let lists = report[list].as_array().unwrap().iter();
        lists
            .map(|case| {
                case["labels"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect::<Vec<_>>().join(" / ")
            })
            .collect()
    }

    #[test]
    fn files_share_no_module_state() {
        let spec = "local state = require('./state')\nreturn function(t)\n\tt.test('first', function()\n\t\tassert(not state.seen, 'module state leaked')\n\t\tstate.seen = true\n\tend)\nend";
        let root = project(
            "isolation",
            &[("src/state.luau", "return {}"), ("src/a.local.spec.luau", spec), ("src/b.local.spec.luau", spec)],
        );
        let (report, _) = run_local(&root, &options(Mode::Test, None, None)).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_passed(&report);
        assert_eq!(report["test_pass"], 2);
    }

    #[test]
    fn focus_holds_across_files() {
        let root = project(
            "focus",
            &[
                (
                    "src/a.local.spec.luau",
                    "print('a ran')\nreturn function(t) t.test('one', function() end, { only = true }) t.test('two', function() end) end",
                ),
                ("src/b.local.spec.luau", "print('b ran')\nreturn function(t) t.test('three', function() end) end"),
            ],
        );
        let (focused, output) = run_local(&root, &options(Mode::Test, None, None)).unwrap();
        // an `only` in a file the filter drops focuses nothing
        let (filtered, _) = run_local(&root, &options(Mode::Test, Some("b.local"), None)).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            (labels(&focused, "tests"), &focused["focused"], &focused["discovered"]),
            (vec!["src/a.local.spec.luau / one".to_owned()], &true.into(), &3.into())
        );
        // only the focused pass's output is shown
        assert_eq!(output, [("src/a.local.spec.luau".to_owned(), "a ran\n".to_owned())]);
        assert_eq!(
            (labels(&filtered, "tests"), &filtered["focused"]),
            (vec!["src/b.local.spec.luau / three".to_owned()], &false.into())
        );
    }

    #[test]
    fn a_hanging_file_times_out_alone() {
        let root = project(
            "timeout",
            &[
                (
                    "src/a.local.spec.luau",
                    "return function(t) t.test('hangs', function() while true do pcall(function() while true do end end) end end) end",
                ),
                ("src/b.local.spec.luau", "return function(t) t.test('passes', function() end) end"),
            ],
        );
        let (report, _) = run_local(&root, &options(Mode::Test, None, Some(Duration::from_millis(200)))).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(labels(&report, "failures"), ["src/a.local.spec.luau / timed out after 0.2 s"]);
        assert_eq!(report["failures"][0]["code"], "test.timeout");
        assert_eq!(labels(&report, "tests"), ["src/b.local.spec.luau / passes"]);
    }
}
