//! The watch-parity harness's driver (DESIGN §14 "W1", census §6.4): a scripted edit sequence applied to a watched
//! copy of a project, where after every step the watcher's tree must equal a clean build of the same sources.
//!
//! - The watcher is an in-process [`BuildSession`] in watch mode over `work/` (a copy of the project: its sources,
//!   configs and include folder, with `node_modules` linked, never copied). Each step edits `work/` and hands the
//!   touched paths to the session (`Watcher::apply(ChangeSet)` of the design).
//! - After each step, the sources are copied to a fresh `clean-<n>/` and built there by a new session, **seeded with
//!   the watcher's committed plugin states from before the step** (the state the watcher's build started from; for
//!   incremental builds, those of the last build that committed, which the next process starts from).
//! - **A step that builds:** every file of the two trees must be equal byte for byte (outputs, assets, plugin
//!   artifacts and state files; the incremental state is left out, as it names absolute paths), and the clean build
//!   must succeed too.
//! - **A step that fails:** the watcher's tree must be unchanged (nothing written), and its error diagnostics must
//!   equal the clean build's. With project references, per project: one that built has the clean build's outputs,
//!   one that failed or was blocked wrote nothing.
//! - **Every step:** every diagnostic, warnings included, equals the clean build's, each as often as it is
//!   reported (a watch rebuild, or a new process of an incremental build, reports the warnings of the files it does
//!   not transform again too).
//!
//! Scripts are JSON: `{"prepare": [action, …], "steps": [{"name": …, "do": [action, …], "expect": "ok" | "error"}]}`,
//! where `prepare` edits the watched copy before the initial build. Actions: `{"write":
//! {"file", "text"}}`, `{"append": {"file", "text"}}`, `{"patch": {"file", "find", "replace"}}` (the text must occur), `{"delete": file}`, `{"rename":
//! {"from", "to"}}`, `{"mkdir": dir}`, `{"delete-output": file}` (relative to the project, a hand-deleted output),
//! `{"touch": file}`. Paths are relative to the project directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use rbxts_plugin::erased::DynPlugin;
use rbxts_project::ProjectOptionLayer;
use tsrs_compiler::diagnosticwriter::flatten_diagnostic_message;
use tsrs_diagnostics::Category;

use super::session::BuildSession;
use crate::compile::{CompileOptions, CompileReport};
use crate::plugins::lifecycle::PluginOptions;

/// One edit.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Writes `text` as the file (creating it and its directories).
    Write {
        /// The file, relative to the project.
        file: String,
        /// Its new text.
        text: String,
    },
    /// Adds `text` at the end of the file.
    Append {
        /// The file, relative to the project.
        file: String,
        /// The text added.
        text: String,
    },
    /// Replaces the first `find` in the file with `replace` (an error when it is not there).
    Patch {
        /// The file, relative to the project.
        file: String,
        /// The text replaced.
        find: String,
        /// Its replacement.
        replace: String,
    },
    /// Deletes a file or directory of the project.
    Delete(String),
    /// Renames a file or directory of the project.
    Rename {
        /// The old path, relative to the project.
        from: String,
        /// The new path, relative to the project.
        to: String,
    },
    /// Creates a directory (and its parents).
    Mkdir(String),
    /// Deletes an output by hand (relative to the project).
    DeleteOutput(String),
    /// Writes a file's bytes back unchanged (a save without a change).
    Touch(String),
}

/// One step: its edits and whether the build after them succeeds.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// What the step does, for reports.
    pub name: String,
    /// Its edits, in order.
    pub actions: Vec<Action>,
    /// The build after the edits succeeds (else it must fail as a clean build does, writing nothing).
    pub expect_ok: bool,
}

/// A script: edits made before the initial build (to make the project build), then the steps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    /// The edits made to the copy before its initial build.
    pub prepare: Vec<Action>,
    /// The steps, in order.
    pub steps: Vec<Step>,
}

fn parse_actions(list: &Value, context: &str) -> Result<Vec<Action>, String> {
    let text_of = |v: &Value, field: &str| -> Result<String, String> {
        v.get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{context}: `{field}` must be a string"))
    };
    let path_of = |v: &Value, what: &str| -> Result<String, String> {
        v.as_str().map(str::to_owned).ok_or_else(|| format!("{context}: {what} takes a path"))
    };
    let mut actions = Vec::new();
    for action in list.as_array().ok_or_else(|| format!("{context}: actions are an array"))? {
        let (kind, body) = action
            .as_object()
            .and_then(|o| o.iter().next())
            .ok_or_else(|| format!("{context}: an action is an object with one key"))?;
        actions.push(match kind.as_str() {
            "write" => Action::Write { file: text_of(body, "file")?, text: text_of(body, "text")? },
            "append" => Action::Append { file: text_of(body, "file")?, text: text_of(body, "text")? },
            "patch" => Action::Patch {
                file: text_of(body, "file")?,
                find: text_of(body, "find")?,
                replace: text_of(body, "replace")?,
            },
            "delete" => Action::Delete(path_of(body, "delete")?),
            "rename" => Action::Rename { from: text_of(body, "from")?, to: text_of(body, "to")? },
            "mkdir" => Action::Mkdir(path_of(body, "mkdir")?),
            "delete-output" => Action::DeleteOutput(path_of(body, "delete-output")?),
            "touch" => Action::Touch(path_of(body, "touch")?),
            other => return Err(format!("{context}: unknown action `{other}`")),
        });
    }
    Ok(actions)
}

/// Parses a script.
pub fn parse_script(text: &str) -> Result<Script, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("invalid script JSON: {e}"))?;
    let prepare = match value.get("prepare") {
        Some(list) => parse_actions(list, "prepare")?,
        None => Vec::new(),
    };
    let mut steps = Vec::new();
    for step in value.get("steps").and_then(Value::as_array).ok_or("a script needs a `steps` array")? {
        let name = step.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        let expect_ok = match step.get("expect").and_then(Value::as_str).unwrap_or("ok") {
            "ok" => true,
            "error" => false,
            other => return Err(format!("step `{name}`: expect must be ok or error, not {other}")),
        };
        let actions = parse_actions(
            step.get("do").ok_or_else(|| format!("step `{name}` needs `do`"))?,
            &format!("step `{name}`"),
        )?;
        steps.push(Step { name, actions, expect_ok });
    }
    Ok(Script { prepare, steps })
}

/// The harness's setup.
pub struct ParityOptions<'a> {
    /// The project to copy (never written).
    pub source: PathBuf,
    /// The tsconfig, relative to the project.
    pub tsconfig: String,
    /// Entries of the project directory to copy (configs, sources, include folder); `node_modules` is linked.
    pub copy: Vec<String>,
    /// Where `work/` and the `clean-<n>/` copies go (emptied first).
    pub scratch: PathBuf,
    /// The command line's project options, for the watcher's builds and the clean ones.
    pub project_options: ProjectOptionLayer,
    /// The plugins both builds run.
    pub plugins: &'a [Box<dyn DynPlugin>],
    /// A tsconfig `plugins` entry no registered plugin claims is ignored.
    pub ignore_unknown_plugins: bool,
    /// The `node_modules` to link (default: the project's own).
    pub node_modules: Option<PathBuf>,
    /// How the copies get their `node_modules`.
    pub node_modules_mode: NodeModules,
    /// Incremental builds instead of a watcher: the copy's tsconfig gets `"incremental": true`, and each step is a
    /// new session (a new `rbxtsc build`) that reads the incremental state the last one wrote.
    pub incremental: bool,
    /// Keep each step's clean copy (for debugging).
    pub keep_clean_copies: bool,
    /// Build and compare the clean builds (off: only the watcher's builds run, for timings and memory).
    pub check_clean: bool,
    /// The process's working set in bytes, read after each step (the harness has no system calls of its own).
    pub memory: Option<fn() -> Option<usize>>,
    /// The checker count of the watcher's and the clean builds (`None`: the default).
    pub checkers: Option<usize>,
}

/// How a copy gets its `node_modules`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeModules {
    /// A junction to the original (enough for types-only imports).
    #[default]
    Link,
    /// A copy, links followed (needed when the project imports packages: roblox-ts requires package files inside
    /// the project's own `node_modules`, and a link resolves outside it).
    Copy,
    /// Real directories for `node_modules` and its scopes, with a junction per package, so a script can add or
    /// change packages in the watched copy (`write` under `node_modules/`), and the clean copies get them too.
    LinkPackages,
}

/// Makes `to` (a new `node_modules`) from `from`: real directories for it and its `@scope` directories; each
/// package a junction to where `from`'s entry leads when that is a link, else a copy (a package a script wrote).
pub fn link_packages(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let path = entry.path();
        let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
        let target = to.join(&name);
        if name.to_string_lossy().starts_with('@') && std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            link_packages(&path, &target)?;
        } else if is_link {
            junction(&target, &path)?;
        } else if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            copy_tree_all(&path, &target).map_err(|e| format!("copying {}: {e}", path.display()))?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| format!("copying {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// One step's outcome.
#[derive(Debug, Default)]
pub struct StepResult {
    /// The step's name.
    pub name: String,
    /// The watcher's build succeeded.
    pub ok: bool,
    /// Project files the watcher transformed, and whether that was all of them.
    pub transformed: usize,
    /// Every project file was transformed.
    pub all: bool,
    /// Files the watcher parsed (source cache misses).
    pub parsed: usize,
    /// The watcher's build reused the last load (`BuildSummary::reused_load`).
    pub reused: bool,
    /// A build with project references: the projects the watcher's build skipped (not dirty).
    pub skipped: usize,
    /// The kind of the watcher's build.
    pub kind: Option<rbxts_plugin::BuildKind>,
    /// The watcher's phase times.
    pub timings: crate::compile::Timings,
    /// The working set after the step, when [`ParityOptions::memory`] is set.
    pub memory: Option<usize>,
    /// The watcher's build time and the clean build's, in milliseconds.
    pub watch_ms: f64,
    /// The clean build's time, in milliseconds.
    pub clean_ms: f64,
    /// Mismatches (empty when the step passes).
    pub problems: Vec<String>,
}

/// Copies a directory tree, skipping `node_modules` (a junction or a directory) and the named entries.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "node_modules" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Links `link` to `target` as a directory junction (Windows `mklink /J`).
pub fn junction(link: &Path, target: &Path) -> Result<(), String> {
    let target = std::fs::canonicalize(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let target = target.to_string_lossy();
    let target = target.strip_prefix(r"\\?\").unwrap_or(&target).to_owned();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(target.replace('/', "\\"))
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("mklink /J {} failed", link.display())) }
}

/// Removes a directory made by the harness, unlinking junctions first (never following them).
pub fn remove_tree(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
            if entry.file_name() == "node_modules" || is_link {
                // a link goes on its own (`rmdir` without /S never enters it); a real node_modules (linked
                // packages) is not empty, so it stays and is walked below
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "rmdir"])
                    .arg(path.to_string_lossy().replace('/', "\\"))
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir() && !m.file_type().is_symlink()) {
                remove_tree(&path);
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Every file under `dir` (relative path → bytes), skipping `node_modules` and the incremental state.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "node_modules" || name.ends_with(".rbxts-rs.buildinfo") {
                continue;
            }
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                walk(root, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.insert(relative, bytes);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// The differences between two snapshots, as messages (first lines of differing text files).
fn tree_diff(watch: &BTreeMap<String, Vec<u8>>, clean: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, bytes) in watch {
        match clean.get(path) {
            None => problems.push(format!("only in the watcher's tree: {path}")),
            Some(other) if other != bytes => {
                let a = String::from_utf8_lossy(bytes);
                let b = String::from_utf8_lossy(other);
                let line = a
                    .lines()
                    .zip(b.lines())
                    .position(|(x, y)| x != y)
                    .unwrap_or(a.lines().count().min(b.lines().count()));
                problems.push(format!(
                    "differs: {path} (line {}: watch {:?} / clean {:?})",
                    line + 1,
                    a.lines().nth(line).unwrap_or(""),
                    b.lines().nth(line).unwrap_or("")
                ));
            }
            Some(_) => {}
        }
    }
    for path in clean.keys() {
        if !watch.contains_key(path) {
            problems.push(format!("only in the clean build's tree: {path}"));
        }
    }
    problems
}

/// A build's error diagnostics as text, with paths relative to `root` (to compare two copies).
pub fn error_texts(report: &CompileReport, root: &Path) -> Vec<String> {
    let root = root.to_string_lossy().replace('\\', "/").to_lowercase();
    let mut out: Vec<String> = report
        .diagnostics
        .iter()
        .chain(report.files.iter().flat_map(|f| &f.diagnostics))
        .filter(|d| d.category() == Category::Error)
        .map(|&d| {
            let file = d.file().map(|f| f.get().file_name().to_lowercase()).unwrap_or_default();
            let file = file.strip_prefix(&root).unwrap_or(&file).to_owned();
            format!("{file}:{} TS{}: {}", d.pos().unwrap_or(0), d.code(), flatten_diagnostic_message(d, "\n"))
        })
        .chain(report.fatal.iter().map(|f| f.replace('\\', "/").to_lowercase().replace(&root, "")))
        .collect();
    out.sort();
    out
}

/// Every diagnostic of a report (each category) and its warnings, as [`error_texts`] names them, sorted, each as
/// often as it is reported.
pub fn diagnostic_texts(report: &CompileReport, root: &Path) -> Vec<String> {
    let root = root.to_string_lossy().replace('\\', "/").to_lowercase();
    let mut out: Vec<String> = report
        .diagnostics
        .iter()
        .chain(report.files.iter().flat_map(|f| &f.diagnostics))
        .map(|&d| {
            let file = d.file().map(|f| f.get().file_name().to_lowercase()).unwrap_or_default();
            let file = file.strip_prefix(&root).unwrap_or(&file).to_owned();
            let category = format!("{:?}", d.category());
            // with its related information: which copy of a plugin diagnostic a build keeps shows
            let related: String = d
                .related_information()
                .iter()
                .map(|r| format!(" [related {}: {}]", r.pos().unwrap_or(0), flatten_diagnostic_message(*r, "\n")))
                .collect();
            let message = flatten_diagnostic_message(d, "\n");
            format!("{category} {file}:{} TS{}: {message}{related}", d.pos().unwrap_or(0), d.code())
        })
        .chain(report.warnings.iter().map(|w| format!("warning {}", w.replace('\\', "/").to_lowercase().replace(&root, ""))))
        .collect();
    out.sort();
    out
}


/// `path` made absolute (against the working directory), with its `.` and `..` components resolved.
fn normalized_absolute(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// Runs the script; the results of the initial build (step 0) and of every step.
///
/// The clean build of a step runs in a copy of `work/` (`clean-<n>/`) when plugins are registered, since their
/// artifacts and state files live in the project, and for a project with project references, whose referenced
/// projects build into their own folders (the copied entries are the sources: list a referenced project's sources
/// and tsconfig, not the project folder with its outputs); without either it runs in `work/` itself with its outDir moved
/// to `clean-<n>/out` (no copy, and packages resolve the same), and the two output trees are compared.
pub fn run(options: &ParityOptions<'_>, script: &Script) -> Result<Vec<StepResult>, String> {
    // absolute and normalized, as the builds' file names must be (a relative `--scratch` would reach tsrs as it is)
    let scratch = normalized_absolute(&options.scratch)?;
    remove_tree(&scratch);
    let work = scratch.join("work");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let node_modules = options.node_modules.clone().unwrap_or_else(|| options.source.join("node_modules"));
    let copy_sources = |from: &Path, to: &Path| -> Result<(), String> {
        std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
        for name in &options.copy {
            let source = from.join(name);
            if source.is_dir() {
                copy_tree(&source, &to.join(name)).map_err(|e| format!("copying {}: {e}", source.display()))?;
            } else if source.is_file() {
                // an entry may be a nested path (`projects/lib/tsconfig.json`)
                if let Some(parent) = to.join(name).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::copy(&source, to.join(name)).map_err(|e| format!("copying {}: {e}", source.display()))?;
            }
        }
        Ok(())
    };
    copy_sources(&options.source, &work)?;
    match options.node_modules_mode {
        NodeModules::Copy => copy_tree_all(&node_modules, &work.join("node_modules"))
            .map_err(|e| format!("copying {}: {e}", node_modules.display()))?,
        NodeModules::Link => junction(&work.join("node_modules"), &node_modules)?,
        NodeModules::LinkPackages => link_original_packages(&node_modules, &work.join("node_modules"))?,
    }
    apply(&work, &script.prepare)?;
    if options.incremental {
        let tsconfig = work.join(&options.tsconfig);
        let text = std::fs::read_to_string(&tsconfig).map_err(|e| e.to_string())?;
        if !text.contains("\"compilerOptions\": {") {
            return Err("incremental: the tsconfig has no `\"compilerOptions\": {`".into());
        }
        let text = text.replacen("\"compilerOptions\": {", "\"compilerOptions\": {\n\t\t\"incremental\": true,", 1);
        std::fs::write(&tsconfig, text).map_err(|e| e.to_string())?;
    }
    let plugin_options = PluginOptions {
        plugins: options.plugins,
        ignore_unknown_tsconfig_plugins: options.ignore_unknown_plugins,
        ..PluginOptions::default()
    };
    let compile_options = |dir: &Path, out_dir: Option<&Path>| CompileOptions {
        tsconfig: options.tsconfig.clone(),
        cwd: dir.to_string_lossy().into_owned(),
        project_options: options.project_options.clone(),
        out_dir_override: out_dir.map(|d| d.to_string_lossy().replace('\\', "/")),
        write_partial: false,
        no_write: false,
        checkers: options.checkers,
    };
    // with project references too: the clean build must build the referenced projects, whose outputs are compared
    let in_copy =
        !options.plugins.is_empty() || rbxts_project::graph::has_references(&work.join(&options.tsconfig).to_string_lossy());
    let mut session = BuildSession::new(compile_options(&work, None), &plugin_options, !options.incremental);
    let mut results = Vec::new();
    let all_steps: Vec<Step> =
        std::iter::once(Step { name: "initial build".into(), actions: Vec::new(), expect_ok: true })
            .chain(script.steps.iter().cloned())
            .collect();
    // the files a build may write: everything but the sources and node_modules
    let outputs_of = |tree: BTreeMap<String, Vec<u8>>| -> BTreeMap<String, Vec<u8>> {
        tree.into_iter().filter(|(path, _)| !path_is_source(path, &options.copy)).collect()
    };
    let mut watch_out: Option<PathBuf> = None;
    // incremental: the plugin states of the last build that committed. A new process starts from what that build
    // persisted; one that failed committed nothing, and its session (a new one, which read no plugin state) does not
    // stand for what the next process starts from
    let mut last_committed = session.committed_states();
    for (index, step) in all_steps.iter().enumerate() {
        let seed = if options.incremental { last_committed.clone() } else { session.committed_states() };
        let touched = match apply(&work, &step.actions) {
            Ok(touched) => touched,
            Err(e) => {
                // the script no longer fits the tree (an earlier step went wrong): stop here
                results.push(StepResult { name: step.name.clone(), problems: vec![e], ..StepResult::default() });
                return Ok(results);
            }
        };
        // the outputs as the step's edits left them (a hand-deleted or hand-edited output included): what a
        // failing build must leave unchanged
        let before = outputs_of(snapshot(&work));
        let start = std::time::Instant::now();
        if options.incremental {
            session = BuildSession::new(compile_options(&work, None), &plugin_options, false);
        }
        let (report, summary) = session.build(&touched);
        if options.incremental && !report.has_errors() {
            last_committed = session.committed_states();
        }
        let watch_ms = start.elapsed().as_secs_f64() * 1e3;
        if !report.out_dir.is_empty() {
            watch_out = Some(PathBuf::from(&report.out_dir));
        }
        let mut result = StepResult {
            name: step.name.clone(),
            ok: !report.has_errors(),
            transformed: report.files.len(),
            all: summary.all,
            parsed: summary.parsed,
            reused: summary.reused_load,
            skipped: report.projects.iter().filter(|p| p.skipped).count(),
            kind: summary.kind,
            timings: report.timings,
            watch_ms,
            ..StepResult::default()
        };
        if result.ok != step.expect_ok {
            result.problems.push(format!(
                "expected {}, the watcher's build {}: {:?}",
                if step.expect_ok { "ok" } else { "an error" },
                if result.ok { "succeeded" } else { "failed" },
                error_texts(&report, &work)
            ));
        }
        result.memory = options.memory.and_then(|probe| probe());
        if !options.check_clean {
            results.push(result);
            continue;
        }
        // the clean build of the same sources, seeded with the state the watcher's build started from
        let clean = scratch.join(format!("clean-{index}"));
        let start = std::time::Instant::now();
        let (clean_report, clean_root, clean_tree) = if in_copy {
            copy_sources(&work, &clean)?;
            match options.node_modules_mode {
                // the packages the script wrote into the watched copy, the others linked
                NodeModules::LinkPackages => link_packages(&work.join("node_modules"), &clean.join("node_modules"))?,
                // copied as the watched copy's was: a linked `node_modules` resolves its packages outside the copy,
                // which changes what the checker sees (the roblox-ts tests project's types; stock agrees)
                NodeModules::Copy => copy_tree_all(&node_modules, &clean.join("node_modules"))
                    .map_err(|e| format!("copying {}: {e}", node_modules.display()))?,
                NodeModules::Link => junction(&clean.join("node_modules"), &node_modules)?,
            }
            let (report, _) =
                BuildSession::new(compile_options(&clean, None), &plugin_options, false).with_seed(seed).build(&[]);
            (report, clean.clone(), outputs_of(snapshot(&clean)))
        } else {
            let out = clean.join("out");
            let (report, _) = BuildSession::new(compile_options(&work, Some(&out)), &plugin_options, false)
                .with_seed(seed)
                .build(&[]);
            (report, work.clone(), snapshot(&out))
        };
        result.clean_ms = start.elapsed().as_secs_f64() * 1e3;
        let watch_tree = || match (&watch_out, in_copy) {
            (Some(out), false) => snapshot(out),
            _ => outputs_of(snapshot(&work)),
        };
        if result.ok {
            if clean_report.has_errors() {
                result.problems.push(format!("the clean build failed: {:?}", error_texts(&clean_report, &clean_root)));
            }
            result.problems.extend(tree_diff(&watch_tree(), &clean_tree));
        } else if !report.projects.is_empty() {
            // project references: a project that built has the clean build's outputs, one that failed or was blocked
            // wrote nothing, as did everything outside the projects' output roots
            let after = outputs_of(snapshot(&work));
            let prefix = |root: &str| -> String {
                let relative = Path::new(root).strip_prefix(&work).map(|p| p.to_string_lossy().replace('\\', "/"));
                format!("{}/", relative.unwrap_or_default())
            };
            let under = |tree: &BTreeMap<String, Vec<u8>>, roots: &[String]| -> BTreeMap<String, Vec<u8>> {
                tree.iter().filter(|(p, _)| roots.iter().any(|r| p.starts_with(r))).map(|(p, b)| (p.clone(), b.clone())).collect()
            };
            let mut all_roots = Vec::new();
            for project in &report.projects {
                let roots: Vec<String> = project.output_roots.iter().map(|r| prefix(r)).collect();
                all_roots.extend(roots.iter().cloned());
                if project.ok {
                    result.problems.extend(
                        tree_diff(&under(&after, &roots), &under(&clean_tree, &roots))
                            .into_iter()
                            .map(|p| format!("{}, which built: {p}", project.tsconfig)),
                    );
                } else {
                    result.problems.extend(
                        tree_diff(&under(&after, &roots), &under(&before, &roots))
                            .into_iter()
                            .map(|p| format!("{}, which failed, wrote: {p}", project.tsconfig)),
                    );
                }
            }
            let outside = |tree: &BTreeMap<String, Vec<u8>>| -> BTreeMap<String, Vec<u8>> {
                tree.iter().filter(|(p, _)| !all_roots.iter().any(|r| p.starts_with(r))).map(|(p, b)| (p.clone(), b.clone())).collect()
            };
            result
                .problems
                .extend(tree_diff(&outside(&after), &outside(&before)).into_iter().map(|p| format!("a failing build wrote: {p}")));
            let watch_errors = error_texts(&report, &work);
            let clean_errors = error_texts(&clean_report, &clean_root);
            if watch_errors != clean_errors {
                result.problems.push(format!("diagnostics differ: watch {watch_errors:?} / clean {clean_errors:?}"));
            }
        } else {
            // nothing written
            let after = outputs_of(snapshot(&work));
            result
                .problems
                .extend(tree_diff(&after, &before).into_iter().map(|p| format!("a failing build wrote: {p}")));
            let watch_errors = error_texts(&report, &work);
            // a clean build in the copy itself names its moved outDir where the watcher names `out`
            let moved_out = clean.join("out").to_string_lossy().replace('\\', "/").to_lowercase();
            let clean_errors: Vec<String> =
                error_texts(&clean_report, &clean_root).iter().map(|e| e.replace(&moved_out, "/out")).collect();
            if watch_errors != clean_errors {
                result.problems.push(format!("diagnostics differ: watch {watch_errors:?} / clean {clean_errors:?}"));
            }
        }
        // every diagnostic, warnings included and each as often as it is reported, equals the clean build's (a watch
        // rebuild, or a new process of an incremental build, reports the warnings of the files it does not transform
        // again)
        {
            let moved_out = clean.join("out").to_string_lossy().replace('\\', "/").to_lowercase();
            let watch_all = diagnostic_texts(&report, &work);
            let clean_all: Vec<String> =
                diagnostic_texts(&clean_report, &clean_root).iter().map(|e| e.replace(&moved_out, "/out")).collect();
            if watch_all != clean_all {
                result.problems.push(format!("all diagnostics differ: watch {watch_all:?} / clean {clean_all:?}"));
            }
        }
        if !options.keep_clean_copies {
            remove_tree(&clean);
        }
        results.push(result);
    }
    Ok(results)
}

/// Copies a directory tree, following links (a dereferenced copy).
fn copy_tree_all(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if std::fs::metadata(entry.path())?.is_dir() {
            copy_tree_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Whether a snapshot path is one the script edits (a source or config), not an output.
fn path_is_source(path: &str, copied: &[String]) -> bool {
    copied.iter().any(|c| path == c || path.starts_with(&format!("{c}/")))
}

/// Applies a step's actions in `dir`; the absolute paths touched.
fn apply(dir: &Path, actions: &[Action]) -> Result<Vec<String>, String> {
    let mut touched = Touched::default();
    let root = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // every edit stays inside the copy: a path through a linked node_modules would edit the original packages
    let abs = |p: &str| -> Result<PathBuf, String> {
        let path = dir.join(p);
        let mut existing = path.clone();
        while !existing.exists() {
            if !existing.pop() {
                break;
            }
        }
        let real = std::fs::canonicalize(&existing).map_err(|e| format!("{p}: {e}"))?;
        if real.starts_with(&root) { Ok(path) } else { Err(format!("{p} leads outside the copy (through a link)")) }
    };
    let text = |p: &PathBuf| p.to_string_lossy().replace('\\', "/");
    for action in actions {
        match action {
            Action::Write { file, text: body } => {
                let path = abs(file)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, body).map_err(|e| format!("{file}: {e}"))?;
                touched.add(text(&path));
            }
            Action::Append { file, text: body } => {
                let path = abs(file)?;
                let mut current = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
                current.push_str(body);
                std::fs::write(&path, current).map_err(|e| format!("{file}: {e}"))?;
                touched.add(text(&path));
            }
            Action::Patch { file, find, replace } => {
                let path = abs(file)?;
                let current = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
                if !current.contains(find.as_str()) {
                    return Err(format!("patch: {find:?} is not in {file}"));
                }
                std::fs::write(&path, current.replacen(find.as_str(), replace, 1))
                    .map_err(|e| format!("{file}: {e}"))?;
                touched.add(text(&path));
            }
            Action::Delete(file) => {
                let path = abs(file)?;
                if path.is_dir() {
                    std::fs::remove_dir_all(&path).map_err(|e| format!("{file}: {e}"))?;
                } else {
                    std::fs::remove_file(&path).map_err(|e| format!("{file}: {e}"))?;
                }
                touched.add(text(&path));
            }
            Action::Rename { from, to } => {
                let (a, b) = (abs(from)?, abs(to)?);
                if let Some(parent) = b.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::rename(&a, &b).map_err(|e| format!("{from} → {to}: {e}"))?;
                touched.add(text(&a));
                touched.add(text(&b));
            }
            Action::Mkdir(d) => {
                let path = abs(d)?;
                std::fs::create_dir_all(&path).map_err(|e| format!("{d}: {e}"))?;
                touched.add(text(&path));
            }
            Action::DeleteOutput(file) => {
                // a watcher counts no event in the output roots: the path is passed as an event that starts nothing
                // of its own (the next edit's build), so the build must find the output missing on its own, also in a
                // referenced project that nothing else made dirty
                let path = abs(file)?;
                std::fs::remove_file(&path).map_err(|e| format!("{file}: {e}"))?;
                touched.add(text(&path));
            }
            Action::Touch(file) => {
                let path = abs(file)?;
                let bytes = std::fs::read(&path).map_err(|e| format!("{file}: {e}"))?;
                std::fs::write(&path, bytes).map_err(|e| format!("{file}: {e}"))?;
                touched.add(text(&path));
            }
        }
    }
    Ok(touched.0)
}

/// The paths a step touched, as a watcher reports them: none under `node_modules`, which the watcher does not
/// watch (a package installed or changed is only picked up by the next rebuild).
#[derive(Default)]
struct Touched(Vec<String>);

impl Touched {
    fn add(&mut self, path: String) {
        if !path.contains("/node_modules/") {
            self.0.push(path);
        }
    }
}

/// [`NodeModules::LinkPackages`] from the original `node_modules`: every package is linked (none was written yet).
fn link_original_packages(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let path = entry.path();
        let target = to.join(&name);
        if name.to_string_lossy().starts_with('@') && std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            link_original_packages(&path, &target)?;
        } else if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            junction(&target, &path)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| format!("copying {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_scratch_becomes_absolute_and_normalized() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(normalized_absolute(Path::new("a/./b/../c")).unwrap(), cwd.join("a").join("c"));
        assert_eq!(normalized_absolute(Path::new("../x")).unwrap(), cwd.parent().unwrap().join("x"));
    }
}
