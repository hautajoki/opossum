//! Watch mode (`rbxtsc -w`, DESIGN §14 "Watcher"): one [`BuildSession`] rebuilt on file system events.
//!
//! - **Events.** A `notify` watcher (ReadDirectoryChangesW on Windows, or polling with `--usePolling`). Watched:
//!   - recursively: the root directories, the directories plugins asked to watch (`watch_dir`), and every package
//!     directory the build depends on (a package a program file is in or a read went into, every package of a
//!     type root, a package an unresolved import names, a package plugin `resolve_module` calls went through);
//!   - not recursively: the project directory, the directories of program files outside packages,
//!     `node_modules` and its `@scope` directories and the type roots (created, removed or renamed, and the packages
//!     in them), and, for every path the last program load read (tsrs's `trackingvfs` view: files, directories
//!     and the paths module resolution probed that did not exist, so `node_modules/<name>` in every ancestor,
//!     `@types`, `typeRoots` and `paths` locations; setup's own reads: the project's package.json lookup, the
//!     type-root packages' package.json), the nearest existing directory at or above it. A missing `node_modules`
//!     counts too (a fresh clone, then an install). Also watched recursively: the folders the Rojo resolvers search
//!     for nested project files (`$path` folders, in the output roots too) and the runtime include files' source
//!     (`node_modules/roblox-ts/include`).
//! - **What counts.** In the output roots, this watcher's own writes and deletions never do. A JSON file the load
//!   read whose text is the same again under the same name never does (a package manager rewriting package.json as
//!   it was; a case-only rename does count). Config files and build inputs always; under the output roots (never a
//!   root holding the project, a source root or the tsconfig) or the include folder only Rojo project files in a
//!   folder the resolvers search (as roblox-ts's `isRojoConfigDirectory`); then: anything in the include source; a
//!   path the load read, or a missing directory on the way to one; an entry added, removed or renamed in a module
//!   directory (not dot entries: `.package-lock.json`, `.cache`); a Rojo project file in the project directory; in a folder a Rojo
//!   resolver searches (type roots included), a project file, or a folder holding one or holding one it read;
//!   inside a package the build uses, `package.json` and other JSON, TypeScript and Luau files and directories
//!   (nothing else in a package that is only in a type root); under a source root (not in its `node_modules` or
//!   `.git`), anything but a TypeScript file no program takes in (an excluded one: roblox-ts neither compiles nor
//!   copies it; a `.d.ts` of a declaration build is an asset), under a plugin's `watch_dir`, anything; in the
//!   directory of a program file or one the include globs listed, a directory an include could reach into or a
//!   file a program takes in (the tsconfig's `files`, or its `include` and not its `exclude`, by a new walk of its
//!   globs once per burst) (lockfiles in the project directory never count). A directory's own modification never
//!   counts (its entries' events say more; a directory created or moved in does), nor do rbxts-rs's temporary files
//!   or this watcher's own artifacts as it wrote them, nor a directory holding nothing but this watcher's own
//!   outputs and build infos (the `out` or `cache` folder its build made: no second build at startup).
//! - **New watches.** The watched directories are brought up to date after each build, before it is reported. A
//!   directory watched for the first time is compared with the build: a file written or created, or a directory
//!   created, since the build started; for a module, package or root directory its own modification time (an entry
//!   added, removed or renamed whatever the entry's times: an archive extracted with old times), unless this watcher
//!   wrote into it; a path the load read that is there now and was not, or the other way round; a program file
//!   whose time or size is not the one it was parsed at. Any of those starts another rebuild. A build that loads no
//!   project keeps the last good watches, plus what its failed load read. A watch that cannot be made is reported
//!   once (with the listing's error: notify calls an access denial a missing path) and tried again quietly at each
//!   update; it is compared with the build when first wanted and when made at last, not at each failed retry. A path
//!   two updates in a row report is dropped the second time: a read the disk contradicts whatever the build does.
//! - **Plugin inputs** ([`inputs`]): the globs plugins recorded (files', `setup`'s, `finish`'s) and what `finish`
//!   read are watched too; an event only they may care about starts a rebuild when the session finds an answer
//!   changed.
//! - **Plugin state files** (the ones the last commit left): an event on one counts only when the file no longer
//!   holds what the commit left (deleted, edited), so this watcher's own write starts nothing. A build whose commit
//!   changed what a plugin's `setup` read of its own state file answered (missing before, there now:
//!   [`BuildSummary::setup_inputs_changed`]) is followed at once by one more build, which runs that setup again; a
//!   build started that way starts no other, unless it lost a state file itself
//!   ([`BuildSummary::lost_state_files`]: deleted or edited since the last commit, by someone else, so no loop).
//! - **Debounce.** Trailing, 100 ms as roblox-ts: each event restarts the wait, and the paths of one burst become
//!   one rebuild. A rename is its two paths (the session sees a deletion and an addition).
//! - **Errors.** A panic in a rebuild is reported and the session starts over with a clean build; the watcher
//!   keeps running.
//! - **`--events json`:** one JSON line per build on stdout: `{"event":"build-start","gen":N,"changes":[…]}` and
//!   `{"event":"build-end","gen":N,"ok":…,"errors":N,"written":[…],"deleted":[…]}`.
//! - **Memory.** Each rebuild frees the program it replaced, with its checkers and what they made
//!   ([`BuildSession`]), so the working set stays near one build's. As a safety net for what is still kept (each
//!   edited file's older parsed versions, a plugin that leaks), the watcher restarts its process past a working-set
//!   limit (`RBXTS_WATCH_MEMORY_LIMIT_MB`, default 2048; 0 turns it off): a small supervisor process runs the watcher
//!   as a child and starts a new one when it exits with [`RESTART_EXIT_CODE`]. The new watcher's first build is a
//!   clean one, or an incremental one for an `incremental` project.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, SystemTime};

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_project::{IncludeMatcher, ReadLog, Seen, nodepath, rojo, text_hash};
use rbxts_transform::build::graph::Version;
use rbxts_transform::build::state::StateFile;
use rbxts_transform::{BuildSession, BuildSummary, CompileOptions, CompileReport, PluginOptions};
use tsrs_diagnostics::Category;

use crate::{BuildArgs, print_report};

#[path = "watch_inputs.rs"]
mod inputs;

/// The exit code a watcher child uses to ask its supervisor for a restart.
pub const RESTART_EXIT_CODE: i32 = 75;
/// Set in a watcher child's environment by its supervisor.
const CHILD_ENV: &str = "RBXTS_WATCH_CHILD";
/// The generation the child's first rebuild continues from.
const GENERATION_ENV: &str = "RBXTS_WATCH_GENERATION";
/// Where a child that asks for a restart leaves its last generation.
const GENERATION_FILE_ENV: &str = "RBXTS_WATCH_GENERATION_FILE";
const DEBOUNCE: Duration = Duration::from_millis(100);
/// `RBXTS_WATCH_DEBUG` set: the watched directories and every raw event go to stderr.
fn debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var_os("RBXTS_WATCH_DEBUG").is_some())
}

/// The touched path standing for "events were lost".
const RESCAN: &str = "<rescan>";

/// The local time as TypeScript's watch status prints it, `new Date().toLocaleTimeString()`: Node's en-US form,
/// `h:mm:ss AM` (no leading zero). rbxts-rs: Node formats in the system's ICU locale (en-AU: `7:05:09 pm`); this is
/// always the en-US form, with a plain space before `AM`/`PM` where newer Node prints U+202F (as TypeScript's own
/// watch tests normalise it).
fn time_now() -> String {
    format_time(chrono::Local::now().time())
}

fn format_time(time: chrono::NaiveTime) -> String {
    use chrono::Timelike;
    let (pm, hour12) = time.hour12();
    format!("{hour12}:{:02}:{:02} {}", time.minute(), time.second(), if pm { "PM" } else { "AM" })
}

/// A watch status line, as TypeScript's pretty watch status reporter prints it.
fn status(message: &str) {
    println!("[\u{1b}[90m{}\u{1b}[0m] {message}", time_now());
}

fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

fn json_list(items: &[String]) -> String {
    serde_json::Value::Array(items.iter().map(|s| serde_json::Value::String(s.clone())).collect()).to_string()
}

/// The number of error diagnostics and fatal messages of a build.
pub fn error_count(report: &CompileReport) -> usize {
    let mut errors = report.diagnostics.iter().filter(|d| d.category() == Category::Error).count();
    errors += report.files.iter().flat_map(|f| &f.diagnostics).filter(|d| d.category() == Category::Error).count();
    errors + report.fatal.len()
}

/// Runs watch mode: as the supervisor (which restarts the watcher past the memory limit) unless this process is the
/// watcher child. Returns the exit status.
pub fn run(
    plugins: &[Box<dyn rbxts_plugin::erased::DynPlugin>],
    build: &BuildArgs,
    options: CompileOptions,
    cwd: &str,
) -> u8 {
    if std::env::var(CHILD_ENV).is_err() && memory_limit().is_some() {
        return supervise();
    }
    if std::env::var(CHILD_ENV).is_ok() {
        // the supervisor holds this process's stdin open: end-of-file means it is gone
        std::thread::spawn(|| {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            std::process::exit(0);
        });
    }
    let generation = std::env::var(GENERATION_ENV).ok().and_then(|g| g.parse().ok()).unwrap_or(0);
    watch(plugins, build, options, cwd, generation)
}

/// The working-set limit in bytes, `None` when turned off.
fn memory_limit() -> Option<usize> {
    let mb = std::env::var("RBXTS_WATCH_MEMORY_LIMIT_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(2048);
    (mb > 0).then_some(mb << 20)
}

/// The supervisor: runs the watcher as a child process with the same arguments and starts a new one whenever it
/// asks for a restart.
fn supervise() -> u8 {
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("rbxtsc: cannot find its own executable to run the watcher");
        return 1;
    };
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut generation: u64 = 0;
    let generation_file = std::env::temp_dir().join(format!("rbxtsc-watch-{}.generation", std::process::id()));
    loop {
        // the child's stdin is a pipe this process holds open: when the supervisor goes (killed, not only by
        // Ctrl+C, which reaches both), the child reads end-of-file and exits too
        let child = std::process::Command::new(&exe)
            .args(&args)
            .env(CHILD_ENV, "1")
            .env(GENERATION_ENV, generation.to_string())
            .env(GENERATION_FILE_ENV, &generation_file)
            .stdin(std::process::Stdio::piped())
            .spawn();
        let status = child.and_then(|mut child| {
            let stdin = child.stdin.take();
            let status = child.wait();
            drop(stdin);
            status
        });
        let last = std::fs::read_to_string(&generation_file).ok().and_then(|g| g.trim().parse().ok());
        let _ = std::fs::remove_file(&generation_file);
        match status {
            Ok(status) if status.code() == Some(RESTART_EXIT_CODE) => {
                generation = last.unwrap_or(generation);
                status_line_restart();
            }
            Ok(status) => return u8::try_from(status.code().unwrap_or(1)).unwrap_or(1),
            Err(e) => {
                eprintln!("rbxtsc: could not start the watcher: {e}");
                return 1;
            }
        }
    }
}

fn status_line_restart() {
    status("The watcher reached its memory limit; restarting it (RBXTS_WATCH_MEMORY_LIMIT_MB).");
}

/// What the watcher watches, and which events count.
#[derive(Clone, Default)]
struct Watched {
    /// Path keys of the tsconfig, the configs it extends, the Rojo project files and the plugins' input files: their
    /// events always count, whatever directory they are in (an outDir set to the project directory included).
    config_inputs: FxHashSet<String>,
    recursive: BTreeSet<String>,
    flat: BTreeSet<String>,
    /// The flat directories inside a recursive watch ([`Watched::dedupe`]): watched themselves while that recursive
    /// watch is not active (it failed: a folder that cannot be listed), so their events are not lost.
    covered: BTreeSet<String>,
    /// Path keys of the directories whose events are dropped (output roots, include folder).
    ignored_roots: Vec<String>,
    /// Path keys of the recursive roots (source roots, plugin `watch_dir`s): any event under them counts (but
    /// under a `node_modules` or `.git` in them).
    root_keys: Vec<String>,
    /// Path keys of the directories whose entries count when the program could read them (the directories of
    /// program files outside packages, the directories the tsconfig's include globs listed; the project directory
    /// before any project loaded): a directory, or a file the tsconfig's `files` and `include` could take in
    /// (every file while `include` is not known).
    open_dirs: FxHashSet<String>,
    /// The project directory's path key: its Rojo project files count (the project's Rojo config is looked for
    /// there), its other entries as any directory's (lockfiles and the like do not).
    project_dir: String,
    /// Path keys of the folders the Rojo resolvers search for nested project files (`$path` folders outside the
    /// ignored roots, the type roots): a project file, or a folder holding one, created or removed there counts.
    rojo_dirs: Vec<String>,
    /// Which files the programs could take in, for `open_dirs` and the source roots: the root project's and every
    /// referenced project's include matcher (none before a project loaded: any file).
    matchers: Vec<IncludeMatcher>,
    /// Per matcher, the root files its tsconfig finds on disk now (a new walk of its globs, made at most once per
    /// burst of events: [`Watched::forget_listings`]).
    listings: std::cell::RefCell<FxHashMap<usize, std::collections::HashSet<String>>>,
    /// Path keys of the source roots (not the plugins' `watch_dir`s): a TypeScript file in them counts only when a
    /// program takes it in; anything else in them (assets) does.
    source_roots: Vec<String>,
    /// A loaded project emits declarations: a `.d.ts` file under a source root is an asset of it.
    declaration_assets: bool,
    /// Path keys of the outputs the last committed builds left, with their size and modification time then: an
    /// event that leaves one otherwise (edited or deleted by hand) counts, and the build writes it again.
    committed_outputs: FxHashMap<String, rbxts_transform::build::outputs::Stamp>,
    /// Path keys of the JSON files the last load read, with their spelling and the hash of their text: an event
    /// leaving the same text under the same name does not count.
    texts: FxHashMap<String, (String, u64)>,
    /// Path keys of the runtime include files' source folder (watched recursively; anything in it counts).
    include_sources: Vec<String>,
    /// Path keys of `node_modules`, its scope directories and the type roots: the directory itself, and any entry
    /// added, removed or renamed in it (but dot entries) counts, whether it exists yet or not.
    module_dirs: FxHashSet<String>,
    /// Path keys of package directories, watched recursively: a `package.json` or other JSON file (Rojo project
    /// files), a TypeScript or Luau file or a directory anywhere in them counts when the package is used.
    package_dirs: FxHashSet<String>,
    /// Path keys of the packages the build uses (a program file or a read of the load in them, a plugin resolution
    /// through them, an unresolved import naming them). The other packages are in a type root only: setup reads
    /// their `package.json` (an anchor) and the package resolver searches them for project files (`rojo_dirs`);
    /// nothing else in them counts.
    used_packages: FxHashSet<String>,
    /// Path keys of the package directories plugin resolutions go through: any event counts.
    plugin_package_dirs: FxHashSet<String>,
    /// Path keys whose own events count: every path the last load read, the paths module resolution probed that did
    /// not exist included, and the missing directories on the way to them.
    anchors: FxHashSet<String>,
    /// What the last load read (path, what it learned), for comparing a directory watched for the first time.
    reads: Vec<(String, Seen)>,
    /// The program's files with the modification time and size they were parsed at.
    stamps: Vec<(String, Option<SystemTime>, i64)>,
    /// The plugins' recorded inputs.
    inputs: inputs::InputWatches,
    /// Path keys of the plugin state files the last committed build left: an event counts when the file no longer
    /// holds what that build left (deleted or edited; this watcher's own write never counts), wherever it is (an
    /// output root included), before every other rule.
    state_files: FxHashMap<String, StateFile>,
}

/// The package root of a path under `node_modules`: `…/node_modules/name` or `…/node_modules/@scope/name`.
fn package_root(path: &str) -> Option<String> {
    if !path.contains("node_modules") {
        return None;
    }
    let normalized = nodepath::normalize(path).replace('\\', "/");
    let at = normalized.rfind("/node_modules/")? + "/node_modules/".len();
    let rest = &normalized[at..];
    let mut parts = rest.split('/');
    let first = parts.next().filter(|f| !f.is_empty())?;
    let length = if first.starts_with('@') {
        first.len() + 1 + parts.next().filter(|p| !p.is_empty())?.len()
    } else {
        first.len()
    };
    Some(normalized[..at + length].to_owned())
}

fn key(path: &str) -> String {
    // TypeScript's spellings (most of the read log) are normalized already
    let plain = !path.contains('\\') && !path.contains("/.") && !path.contains("//") && !path.ends_with('/');
    if plain { path.to_lowercase() } else { nodepath::normalize(path).replace('\\', "/").to_lowercase() }
}

fn is_under(path_key: &str, root_key: &str) -> bool {
    path_key == root_key
        || (path_key.len() > root_key.len()
            && path_key.starts_with(root_key)
            && (root_key.ends_with('/') || path_key.as_bytes()[root_key.len()] == b'/'))
}

/// The parent of a path key or path, `None` at a root.
fn parent_of(path: &str) -> Option<&str> {
    let parent = nodepath::dirname(path);
    (parent != path && !parent.is_empty()).then_some(parent)
}

fn is_dir(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// Whether an entry of a package counts: package.json and other JSON (Rojo project files, JSON modules), TypeScript
/// and Luau files, directories (one created, moved in or removed: on Windows a directory moved in is one event).
fn package_entry_counts(path: &str, name: &str) -> bool {
    if name.starts_with('.') {
        return false;
    }
    const EXTENSIONS: [&str; 9] = [".json", ".ts", ".tsx", ".mts", ".cts", ".lua", ".luau", ".toml", ".d.ts"];
    if EXTENSIONS.iter().any(|e| name.ends_with(e)) {
        return true;
    }
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        // gone: a directory removed or renamed away (its name has no extension) counts
        Err(_) => !name.contains('.'),
    }
}

/// This watcher's writes and deletions, by path key: the version written, or None for a path it deleted.
type Own = FxHashMap<String, OwnWrite>;

/// What this watcher's last build left at a path.
#[derive(Clone, Copy)]
enum OwnWrite {
    /// An output with the stamp of the bytes the build wrote ([`BuildSession::committed_outputs`], made from those
    /// bytes): never read back after the commit, where a hand edit landing first would pass for the build's write.
    Output(rbxts_transform::build::outputs::Stamp),
    /// Another file the build wrote (an artifact, a state file, the incremental state), with its text as read after.
    Text(Version),
    /// Deleted.
    Gone,
}

/// Whether an event leaves `path` as this watcher's last build left it: written with the same bytes, or deleted and
/// still gone.
fn is_own(path: &str, path_key: &str, own: &Own) -> bool {
    match own.get(path_key) {
        Some(OwnWrite::Output(stamp)) => {
            stamp.check(path, rbxts_transform::build::outputs::Found::at(path)).is_some()
        }
        Some(OwnWrite::Text(version)) => {
            std::fs::read(path).is_ok_and(|bytes| Version::of(&String::from_utf8_lossy(&bytes)) == *version)
        }
        Some(OwnWrite::Gone) => std::fs::symlink_metadata(path).is_err(),
        None => false,
    }
}

/// Whether a folder holds a Rojo project file anywhere below it that this watcher did not write as it is. A link
/// counts (the resolvers follow links; this does not).
fn holds_project_file(dir: &Path, own: &Own) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|entry| match entry.file_type() {
        Ok(t) if t.is_symlink() => true,
        Ok(t) if t.is_dir() => holds_project_file(&entry.path(), own),
        Ok(_) => {
            rojo::is_rojo_project_file_name(&entry.file_name().to_string_lossy()) && {
                let path = path_text(&entry.path());
                !is_own(&path, &key(&path), own)
            }
        }
        Err(_) => true,
    })
}

/// Whether a directory holds this watcher's own outputs and nothing else: every file below it written by this
/// watcher as it is now, or a build info (the incremental state and tsc's build info, which the build writes
/// beside each other), and at least one. A folder the build created for its outputs (`out`, a `cache` for the
/// build infos) is its own write: its creation starts nothing.
fn holds_only_own(dir: &Path, own: &Own) -> bool {
    fn walk(dir: &Path, own: &Own, files: &mut usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        entries.flatten().all(|entry| match entry.file_type() {
            Ok(t) if t.is_symlink() => false,
            Ok(t) if t.is_dir() => walk(&entry.path(), own, files),
            Ok(_) => {
                *files += 1;
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = path_text(&entry.path());
                name.ends_with(".tsbuildinfo") || name.ends_with(".rbxts-rs.buildinfo") || is_own(&path, &key(&path), own)
            }
            Err(_) => false,
        })
    }
    let mut files = 0;
    walk(dir, own, &mut files) && files > 0
}

/// Whether the file at `path` now is the one a load read as `spelled` (its name spelled the same on disk, so not a
/// case-only rename) with the text `hash` ([`text_hash`]).
fn same_text(path: &str, spelled: &str, hash: u64) -> bool {
    let name = nodepath::basename(spelled);
    let on_disk = std::fs::read_dir(nodepath::dirname(path)).ok().and_then(|entries| {
        entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.eq_ignore_ascii_case(name))
    });
    on_disk.as_deref() == Some(name)
        && std::fs::read(path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .is_some_and(|text| text_hash(&text) == hash)
}

impl Watched {
    /// Whether an entry of a folder a Rojo resolver searches changes what it finds: a project file, a folder holding
    /// one (created or moved in; a link counts), or a folder a project file the resolvers read was in (removed or
    /// moved away). This watcher's own writes and deletions do not count.
    fn rojo_entry_counts(&self, path: &str, path_key: &str, name: &str, own: &Own) -> bool {
        if is_own(path, path_key, own) {
            return false;
        }
        if rojo::is_rojo_project_file_name(name) {
            return true;
        }
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => true,
            Ok(m) if m.is_dir() => holds_project_file(Path::new(path), own),
            Ok(_) => false,
            Err(_) => self.config_inputs.iter().any(|input| is_under(input, path_key) && !own.contains_key(input)),
        }
    }

    /// Whether an entry of an open directory (not under a root) counts: a directory a file the programs take in
    /// could be in (watched from the next build on), or a file the programs take in now.
    fn open_entry_counts(&self, path: &str, name: &str) -> bool {
        let below = || self.matchers.is_empty() || self.matchers.iter().any(|m| m.possibly_includes_below(path));
        match std::fs::metadata(path) {
            Ok(m) if m.is_dir() => below(),
            Ok(_) => self.taken_in(path),
            // gone: a directory (no extension) or a file (one the program read is an anchor)
            Err(_) => (!name.contains('.') && below()) || self.taken_in(path),
        }
    }

    /// Whether an entry under a source root counts: anything but a TypeScript file (`.ts`, `.tsx`) no program takes
    /// in, which roblox-ts neither compiles nor copies (`copyItem`'s `isCompilableFile` filter: an excluded file).
    /// A `.d.ts` file of a declaration build is an asset, copied.
    fn root_entry_counts(&self, path: &str, name: &str) -> bool {
        if !(name.ends_with(".ts") || name.ends_with(".tsx")) || std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
            return true;
        }
        (self.declaration_assets && name.ends_with(".d.ts")) || self.taken_in(path)
    }

    /// Whether a program would take `path` in as a root file now: one of the tsconfigs' `files`, or matched by
    /// its `include` and not its `exclude` (any file before a project loaded). The include matcher answers first;
    /// a file it could take in is looked up in a new walk of that tsconfig's globs.
    fn taken_in(&self, path: &str) -> bool {
        self.matchers.is_empty()
            || self.matchers.iter().enumerate().any(|(i, matcher)| {
                matcher.possibly_includes(path)
                    && (self.listings.borrow_mut().entry(i).or_insert_with(|| matcher.current_root_files()))
                        .contains(&matcher.key(path))
            })
    }

    /// Drops the walks [`Watched::taken_in`] made: the next burst of events walks again.
    fn forget_listings(&self) {
        self.listings.borrow_mut().clear();
    }

    /// Watches `path`'s deepest existing directory (its own directory when it exists, else the nearest existing
    /// ancestor), and makes the path and every missing directory on the way to it count: a file or directory
    /// appearing there (or going) changes what a load reads.
    fn cover(&mut self, path: &str, dirs: &mut FxHashMap<String, bool>) {
        let mut exists = |p: &str| *dirs.entry(key(p)).or_insert_with(|| is_dir(p));
        self.anchors.insert(key(path));
        let mut dir = match parent_of(path) {
            Some(parent) => parent.to_owned(),
            None => return,
        };
        // the common case: the directory was watched already for another path
        if self.flat.contains(&dir) {
            return;
        }
        while !exists(&dir) {
            self.anchors.insert(key(&dir));
            match parent_of(&dir) {
                Some(parent) => dir = parent.to_owned(),
                None => return,
            }
        }
        self.flat.insert(dir);
    }

    /// Adds what a load read: each path counts and is covered; directories the tsconfig's globs or the program
    /// listed are watched, their entries counting; the packages read from are package directories, used unless
    /// only their `package.json` was read (setup's package mapping reads every type-root package's); the JSON
    /// texts read.
    fn add_log(&mut self, log: &ReadLog, dirs: &mut FxHashMap<String, bool>) {
        self.texts.extend(log.texts().into_iter().map(|(path, hash)| (key(&path), (path, hash))));
        let reads = log.entries();
        // the directories the load found (a directory, or a file's directory) need no look of their own; one gone
        // since is caught when its watch is made (`Subscriptions::update`)
        for (path, seen) in &reads {
            if seen.has(Seen::DIR) {
                dirs.insert(key(path), true);
            }
            if seen.has(Seen::FILE)
                && let Some(parent) = parent_of(path)
            {
                dirs.insert(key(parent), true);
            }
        }
        for (path, seen) in &reads {
            let k = key(path);
            if self.ignored_roots.iter().any(|root| is_under(&k, root)) {
                continue;
            }
            self.cover(path, dirs);
            if seen.has(Seen::LISTED) && *dirs.entry(k.clone()).or_insert_with(|| is_dir(path)) {
                self.flat.insert(path.clone());
                if k.split('/').any(|c| c == "node_modules") {
                    self.module_dirs.insert(k.clone());
                } else {
                    self.open_dirs.insert(k.clone());
                }
            }
            if let Some(root) = package_root(path) {
                let root_key = key(&root);
                let used = k != root_key && k != format!("{root_key}/package.json");
                self.add_package(&root, dirs, used);
            }
        }
        self.reads.extend(reads);
    }

    /// A package directory: watched recursively when it exists; it and its contents count (see `used_packages`).
    fn add_package(&mut self, dir: &str, dirs: &mut FxHashMap<String, bool>, used: bool) {
        let k = key(dir);
        if used {
            self.used_packages.insert(k.clone());
        }
        if self.package_dirs.insert(k.clone()) {
            let exists = *dirs.entry(k.clone()).or_insert_with(|| is_dir(dir));
            self.anchors.insert(k);
            if exists {
                self.recursive.insert(dir.to_owned());
            }
        }
    }

    /// The folders the Rojo resolvers search (`rojo_dirs`): watched recursively when they exist (unless a module
    /// directory, whose packages are watched already, or inside a recursive watch), and covered (a `$path` folder
    /// appearing or going).
    fn add_rojo_dirs(&mut self, partitions: Vec<String>, dirs: &mut FxHashMap<String, bool>) {
        let recursive: Vec<String> = self.recursive.iter().map(|d| key(d)).collect();
        for partition in partitions {
            let k = key(&partition);
            if self.rojo_dirs.contains(&k) {
                continue;
            }
            self.rojo_dirs.push(k.clone());
            // in an output root (`$path: out/shared`): only its project files count, and it is there to watch
            // once a build wrote it
            if self.ignored_roots.iter().any(|root| is_under(&k, root)) {
                if !recursive.iter().any(|r| is_under(&k, r)) && is_dir(&partition) {
                    self.recursive.insert(partition);
                }
                continue;
            }
            if self.module_dirs.contains(&k) {
                continue;
            }
            self.cover(&partition, dirs);
            if !recursive.iter().any(|r| is_under(&k, r)) && *dirs.entry(k).or_insert_with(|| is_dir(&partition)) {
                self.recursive.insert(partition);
            }
        }
    }

    /// The package directory a path key is in, if any.
    fn package_of(&self, path_key: &str) -> Option<&str> {
        let mut dir = path_key;
        while let Some(parent) = parent_of(dir) {
            if let Some(package) = self.package_dirs.get(parent) {
                return Some(package);
            }
            dir = parent;
        }
        None
    }

    /// A `node_modules` or scope directory: it and its entries count, watched when it exists, its parent watched
    /// (or the nearest existing ancestor) so it appearing or going is seen.
    fn add_module_dir(&mut self, dir: &str, dirs: &mut FxHashMap<String, bool>) {
        self.module_dirs.insert(key(dir));
        self.cover(dir, dirs);
        if is_dir(dir) {
            self.flat.insert(dir.to_owned());
        }
    }

    /// Whether a path key is in a package directory.
    fn in_package(&self, path_key: &str) -> bool {
        self.package_of(path_key).is_some()
    }

    /// One watch per directory, whatever its spelling; none for a directory inside a recursive watch, which goes to
    /// `covered` instead (watched itself while that recursive watch is not active).
    fn dedupe(&mut self) {
        let mut seen = FxHashSet::default();
        self.recursive.retain(|dir| seen.insert(key(dir)));
        let recursive_keys = seen;
        let mut seen = FxHashSet::default();
        let mut covered = Vec::new();
        self.flat.retain(|dir| {
            let k = key(dir);
            let mut ancestor = Some(k.as_str());
            while let Some(a) = ancestor {
                if recursive_keys.contains(a) {
                    covered.push(dir.clone());
                    return false;
                }
                ancestor = parent_of(a);
            }
            seen.insert(k)
        });
        self.covered = covered.into_iter().collect();
    }
}

/// The tsconfig's directory, watched before any project could be loaded (a broken tsconfig at the first build: any
/// entry counts), and the `node_modules` beside it, which counts even when it does not exist yet (a fresh clone, then
/// an install).
fn minimal_set(session: &BuildSession<'_>) -> Watched {
    let mut watched = Watched::default();
    let tsconfig = nodepath::resolve(&[&session.options.cwd, &session.options.tsconfig]);
    let project_dir = nodepath::dirname(&tsconfig).to_owned();
    watched.flat.insert(project_dir.clone());
    watched.open_dirs.insert(key(&project_dir));
    watched.project_dir = key(&project_dir);
    watched.config_inputs.insert(key(&tsconfig));
    let mut dirs = FxHashMap::default();
    watched.add_module_dir(&nodepath::join(&[&project_dir, "node_modules"]), &mut dirs);
    watched
}

/// What to watch for the last loaded project; `None` when the last build loaded none (then the last good set is
/// kept: a failing load says nothing about where the sources are).
fn watched_set(session: &BuildSession<'_>) -> Option<Watched> {
    let mut watched = minimal_set(session);
    let mut dirs: FxHashMap<String, bool> = FxHashMap::default();
    let (services, program_files, root_dirs) = session.watch_roots()?;
    let project_dir = services.data.project_path.clone();
    // a project loaded: the project directory's entries count as the build could read them
    watched.open_dirs.remove(&watched.project_dir);
    watched.project_dir = key(&project_dir);
    watched.matchers = session.include_matcher().into_iter().collect();
    let out_dir = session.options.out_dir_override.clone().unwrap_or_else(|| services.path_translator.out_dir.clone());
    // an output root holding the project, a source root or the tsconfig would drop the events that matter (the
    // build refuses such a root anyway): it is never ignored
    let protected: Vec<String> = root_dirs
        .iter()
        .map(|r| key(r))
        .chain([key(&project_dir), key(&nodepath::resolve(&[&session.options.cwd, &session.options.tsconfig]))])
        .collect();
    for root in
        [out_dir, services.path_translator.out_dir.clone(), nodepath::resolve(&[&services.data.options.include_path])]
    {
        let root = key(&root);
        if !protected.iter().any(|p| is_under(p, &root)) {
            watched.ignored_roots.push(root);
        }
    }
    for input in session.config_inputs() {
        watched.config_inputs.insert(key(&input));
        watched.cover(&input, &mut dirs);
    }
    // the plugins' state files: deleted or edited by hand, the next build starts their plugins fresh
    for file in session.state_files() {
        watched.state_files.insert(key(&file.path), file.clone());
        watched.cover(&file.path, &mut dirs);
    }
    for root in root_dirs.iter().map(|r| nodepath::resolve(&[r])).chain(session.plugin_watch_dirs()) {
        watched.root_keys.push(key(&root));
        watched.recursive.insert(root);
    }
    watched.source_roots.extend(root_dirs.iter().map(|r| key(&nodepath::resolve(&[r]))));
    watched.declaration_assets |= services.path_translator.declaration;
    watched.committed_outputs.extend(session.committed_outputs().into_iter().map(|(path, stamp)| (key(&path), stamp)));
    watched.flat.insert(project_dir.clone());
    // node_modules, its scopes and the type roots: packages appearing or going
    let node_modules = services.data.node_modules_path.clone();
    watched.add_module_dir(&node_modules, &mut dirs);
    if let Ok(entries) = std::fs::read_dir(&node_modules) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('@') && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
                watched.add_module_dir(&nodepath::join(&[&node_modules, &name]), &mut dirs);
            }
        }
    }
    // every package in a type root is read by the setup (its package.json for the path mapping, its folders for
    // nested Rojo projects) and, without `types`, loaded by TypeScript
    for type_root in &services.type_roots {
        watched.add_module_dir(type_root, &mut dirs);
        if let Ok(entries) = std::fs::read_dir(type_root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with('.') && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
                    watched.add_package(&nodepath::join(&[type_root, &name]), &mut dirs, false);
                }
            }
        }
    }
    // plugin module resolutions are build inputs: the package directories they go through count whole
    for (specifier, _from, resolved) in session.plugin_resolutions() {
        if let Some(resolved) = &resolved {
            let dir = nodepath::dirname(resolved).to_owned();
            watched.plugin_package_dirs.insert(key(&dir));
            watched.cover(resolved, &mut dirs);
            if let Some(root) = package_root(resolved) {
                watched.add_package(&root, &mut dirs, true);
            }
        }
        if !specifier.starts_with('.') && !nodepath::is_absolute(&specifier) {
            let mut package = node_modules.clone();
            for part in specifier.split('/').take(if specifier.starts_with('@') { 2 } else { 1 }) {
                package = nodepath::join(&[&package, part]);
                watched.plugin_package_dirs.insert(key(&package));
                watched.cover(&package, &mut dirs);
            }
            watched.add_package(&package, &mut dirs, true);
        }
    }
    // the packages an unresolved import names, where they would be installed
    for name in session.unresolved_packages() {
        let mut dir = node_modules.clone();
        for part in name.split('/').take(if name.starts_with('@') { 2 } else { 1 }) {
            dir = nodepath::join(&[&dir, part]);
        }
        watched.cover(&dir, &mut dirs);
        watched.add_package(&dir, &mut dirs, true);
    }
    for file in &program_files {
        if file.starts_with("bundled:") {
            continue;
        }
        if let Some(root) = package_root(file) {
            watched.add_package(&root, &mut dirs, true);
            continue;
        }
        let dir = nodepath::dirname(file).to_owned();
        watched.open_dirs.insert(key(&dir));
        watched.flat.insert(dir);
    }
    if let Some(log) = session.read_log() {
        watched.add_log(&log, &mut dirs);
    }
    watched.add_rojo_dirs(session.rojo_partitions(), &mut dirs);
    // the runtime include files: the package.json paths their source lookup looks at, and the source folder
    if let Some((probed, source)) = session.include_inputs() {
        for path in &probed {
            watched.cover(path, &mut dirs);
        }
        if let Some(source) = source {
            watched.include_sources.push(key(&source));
            watched.cover(&source, &mut dirs);
            if is_dir(&source) {
                watched.recursive.insert(source);
            }
        }
    }
    watched.stamps = session.loaded_stamps();
    watched.inputs = inputs::InputWatches::of(session);
    watched.recursive.extend(watched.inputs.recursive.iter().cloned());
    watched.flat.extend(watched.inputs.flat.iter().cloned());
    for root in session.output_roots() {
        let root = key(&root);
        if !protected.iter().any(|p| is_under(p, &root)) && !watched.ignored_roots.contains(&root) {
            watched.ignored_roots.push(root);
        }
    }
    for reference in session.reference_sessions() {
        add_referenced_project(&mut watched, reference, &mut dirs);
    }
    watched.dedupe();
    Some(watched)
}

/// What a project a build with project references builds before its root adds to the watched set: its source roots
/// (any event in them counts), configs, state files, the directories of its program files, what its load read, its
/// Rojo folders and its plugins' recorded inputs; its output roots are ignored, as the root's are (the root's program
/// reads the declarations written there: their events are this watcher's own writes).
fn add_referenced_project(watched: &mut Watched, session: &BuildSession<'_>, dirs: &mut FxHashMap<String, bool>) {
    let Some((services, program_files, root_dirs)) = session.watch_roots() else { return };
    watched.source_roots.extend(root_dirs.iter().map(|r| key(&nodepath::resolve(&[r]))));
    watched.declaration_assets |= services.path_translator.declaration;
    watched.committed_outputs.extend(session.committed_outputs().into_iter().map(|(path, stamp)| (key(&path), stamp)));
    // a file its tsconfig could take in, in the directory of one of its program files, counts
    watched.matchers.extend(session.include_matcher());
    let protected: Vec<String> = root_dirs.iter().map(|r| key(r)).collect();
    for root in session.output_roots() {
        let root = key(&root);
        if !protected.iter().any(|p| is_under(p, &root)) && !watched.ignored_roots.contains(&root) {
            watched.ignored_roots.push(root);
        }
    }
    for input in session.config_inputs() {
        watched.config_inputs.insert(key(&input));
        watched.cover(&input, dirs);
    }
    for file in session.state_files() {
        watched.state_files.insert(key(&file.path), file.clone());
        watched.cover(&file.path, dirs);
    }
    for root in root_dirs.iter().map(|r| nodepath::resolve(&[r])).chain(session.plugin_watch_dirs()) {
        watched.root_keys.push(key(&root));
        watched.recursive.insert(root);
    }
    for file in &program_files {
        if file.starts_with("bundled:") {
            continue;
        }
        if let Some(root) = package_root(file) {
            watched.add_package(&root, dirs, true);
            continue;
        }
        let dir = nodepath::dirname(file).to_owned();
        watched.open_dirs.insert(key(&dir));
        watched.flat.insert(dir);
    }
    if let Some(log) = session.read_log() {
        watched.add_log(&log, dirs);
    }
    watched.add_rojo_dirs(session.rojo_partitions(), dirs);
    watched.stamps.extend(session.loaded_stamps());
    let inputs = inputs::InputWatches::of(session);
    watched.recursive.extend(inputs.recursive.iter().cloned());
    watched.flat.extend(inputs.flat.iter().cloned());
}

/// Whether an event path matters. `folder_modify`: the event is only a modification of an existing directory (its
/// entries changed), which counts only for a plugin package directory (the events of the entries say more).
fn relevant(path: &str, folder_modify: bool, watched: &Watched, own: &Own) -> bool {
    let k = key(path);
    let parent = nodepath::dirname(&k);
    let name = nodepath::basename(&k);
    // a plugin state file counts only when it no longer holds what the last commit left (deleted, edited): this
    // watcher's own write, or a tool rewriting the same bytes, starts nothing (also when a plugin's setup read it as
    // a build input: Flamework's candidate scan reads its own `flamework.build`)
    if let Some(file) = watched.state_files.get(&k) {
        return !folder_modify && !file.intact();
    }
    let output = watched.ignored_roots.iter().any(|root| is_under(&k, root));
    // in the output roots, this watcher's own writes and deletions as it made them come first (an asset-copied
    // Rojo project file is a config input the next load reads, yet the build's own)
    if output && is_own(path, &k, own) {
        return false;
    }
    // a JSON file the load read, with the same text again under the same name (a package manager rewriting
    // package.json as it was; a case-only rename is a change)
    if let Some((spelled, hash)) = watched.texts.get(&k)
        && same_text(path, spelled, *hash)
    {
        return false;
    }
    // configs and build inputs first: roblox-ts's watcher checks isConfigPath before its output-path ignore
    if watched.config_inputs.contains(&k) {
        return true;
    }
    if output {
        // an output the last commit left, no longer as it left it (edited or deleted by hand): written again
        if let Some(stamp) = watched.committed_outputs.get(&k) {
            return !folder_modify && stamp.check(path, rbxts_transform::build::outputs::Found::at(path)).is_none();
        }
        // Rojo project files in the folders the resolvers search count there too, as in roblox-ts
        // (`isRojoConfigDirectory`): a hand-written `out/…/default.project.json` changes the requires
        return !folder_modify
            && watched.rojo_dirs.iter().any(|dir| is_under(parent, dir))
            && watched.rojo_entry_counts(path, &k, name, own);
    }
    if folder_modify {
        return watched.plugin_package_dirs.contains(&k);
    }
    if name.starts_with('.') && (name.contains(".rbxts-tmp-") || name.contains(".rbxts-old-")) {
        return false;
    }
    // this watcher's own artifacts and state files, as it wrote them
    if is_own(path, &k, own) {
        return false;
    }
    // the runtime include files' source: every file in it is copied
    if watched.include_sources.iter().any(|source| is_under(&k, source)) {
        return true;
    }
    // package names never start with a dot: `node_modules/.package-lock.json`, `.cache`, `.bin` are churn
    let in_module_dir = watched.module_dirs.contains(parent);
    if in_module_dir {
        return !name.starts_with('.');
    }
    if watched.anchors.contains(&k) || watched.module_dirs.contains(&k) {
        return true;
    }
    if watched.plugin_package_dirs.contains(parent) || watched.plugin_package_dirs.contains(&k) {
        return true;
    }
    // the project's Rojo config is looked for in the project directory
    if parent == watched.project_dir && (rojo::is_rojo_project_file_name(name) || name == "roblox-project.json") {
        return true;
    }
    if watched.rojo_dirs.iter().any(|dir| is_under(parent, dir)) && watched.rojo_entry_counts(path, &k, name, own) {
        return true;
    }
    if let Some(package) = watched.package_of(&k) {
        return watched.used_packages.contains(package) && package_entry_counts(path, name);
    }
    if k.split('/').any(|c| c == "node_modules" || c == ".git") {
        return false;
    }
    if watched.root_keys.iter().any(|root| is_under(&k, root)) {
        // a TypeScript file in a source root (not in a plugin's `watch_dir` too) counts when a program takes it in
        let source = |root: &String| watched.source_roots.contains(root);
        let plugin_dir = watched.root_keys.iter().any(|root| is_under(&k, root) && !source(root));
        let in_source_root = watched.source_roots.iter().any(|root| is_under(&k, root));
        return plugin_dir || !in_source_root || watched.root_entry_counts(path, name);
    }
    watched.open_dirs.contains(parent) && watched.open_entry_counts(path, name)
}

/// The watches of the last build that loaded a project, and those in use after a build that loaded none.
struct Watches {
    good: Watched,
    failed: Option<Watched>,
}

impl Watches {
    fn current(&self) -> &Watched {
        self.failed.as_ref().unwrap_or(&self.good)
    }
}

/// A watched directory and whether its watch is recursive.
type Watch = (String, bool);

/// An event path of a burst, flagged when all its events were modifications of an existing directory.
type BurstPath = (String, bool);

struct Subscriptions {
    watcher: Box<dyn Watcher>,
    /// The watches made.
    active: FxHashSet<(String, bool)>,
    /// The wanted directories that were not there at the last update.
    missing: FxHashSet<String>,
    /// The watches that failed, reported once; tried again at each update until one succeeds.
    failed: FxHashSet<(String, bool)>,
    /// The paths the last update reported as changed before their directory was watched.
    last_stale: FxHashSet<String>,
}

impl Subscriptions {
    /// Watches what `watched` lists. Returns the paths in the directories it starts watching that changed since
    /// `since` (the build's start) and matter: a write between the program load and the new watch (a package
    /// folder filled during the build that its creation started, a `watch_dir` root) raised no event here, so
    /// those paths start another rebuild.
    fn update(&mut self, watched: &Watched, since: SystemTime, own: &Own) -> Vec<String> {
        if debug() {
            eprintln!("[rbxtsc watch] recursive {:?}\n[rbxtsc watch] flat {:?}", watched.recursive, watched.flat);
        }
        let (mut wanted, missing): (FxHashSet<Watch>, FxHashSet<Watch>) = watched
            .recursive
            .iter()
            .map(|d| (d.clone(), true))
            .chain(watched.flat.iter().map(|d| (d.clone(), false)))
            .partition(|(d, _)| Path::new(d).is_dir());
        // a directory the build found that is gone already (removed in between: its removal raised no event here)
        // starts a rebuild, once
        let missing: FxHashSet<String> = missing.into_iter().map(|(d, _)| d).collect();
        let mut stale: Vec<String> = missing.iter().filter(|d| !self.missing.contains(*d)).cloned().collect();
        self.missing = missing;
        // the covered flat directories (inside a recursive watch) that exist: watched themselves while that recursive
        // watch is not active (it failed), so their events are not lost
        let covered: Vec<Watch> =
            watched.covered.iter().filter(|d| Path::new(d).is_dir()).map(|d| (d.clone(), false)).collect();
        // every watch no longer wanted at all goes first: notify keeps one watch per path, so an unwatch after a new
        // watch of the same path would remove that one
        for watch @ (dir, _) in &self.active {
            if !wanted.contains(watch) && !covered.contains(watch) {
                let _ = self.watcher.unwatch(Path::new(dir));
            }
        }
        let mut active: FxHashSet<Watch> = FxHashSet::default();
        let mut scan = None;
        // the recursive watches, then the flat ones with the covered ones whose recursive watch is not active
        for recursive in [true, false] {
            if !recursive {
                let roots: Vec<String> = active.iter().filter(|(_, r)| *r).map(|(d, _)| key(d)).collect();
                let under_root = |dir: &str| {
                    let k = key(dir);
                    roots.iter().any(|root| k == *root || is_under(&k, root))
                };
                for watch @ (dir, _) in &covered {
                    if !under_root(dir) {
                        wanted.insert(watch.clone());
                    } else if self.active.contains(watch) && !roots.contains(&key(dir)) {
                        // covered again by a recursive watch above it (one of the same path replaced it already)
                        let _ = self.watcher.unwatch(Path::new(dir));
                    }
                }
            }
            active.extend(self.active.iter().filter(|w| w.1 == recursive && wanted.contains(w)).cloned());
            let new: Vec<&(String, bool)> =
                wanted.iter().filter(|w| w.1 == recursive && !self.active.contains(w)).collect();
            if new.is_empty() {
                continue;
            }
            let scan = scan.get_or_insert_with(|| Scan::new(watched, since, own));
            for watch @ (dir, recursive) in new {
                let mode = if *recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
                // a directory is compared with the build once: when first watched, whether the watch is made or
                // not, and again when a failed one is made at last (not at every failed retry: a read the disk
                // contradicts for good, such as a folder that cannot be listed, would start a rebuild each time)
                let retry = self.failed.contains(watch);
                let made = match self.watcher.watch(Path::new(dir), mode) {
                    Ok(()) => {
                        self.failed.remove(watch);
                        active.insert(watch.clone());
                        true
                    }
                    // not active: tried again at the next update (a failure is reported once)
                    Err(e) => {
                        if self.failed.insert(watch.clone()) {
                            // notify reports an access denial as a missing path: the listing says why
                            let cause = std::fs::read_dir(dir).err().map_or_else(|| e.to_string(), |io| io.to_string());
                            eprintln!("rbxtsc: cannot watch {dir}: {cause}");
                        }
                        false
                    }
                };
                if !retry || made {
                    let whole = *recursive || watched.module_dirs.contains(&key(dir));
                    scan.dir(Path::new(dir), *recursive, whole, &mut stale);
                }
            }
        }
        self.failed.retain(|w| wanted.contains(w));
        self.active = active;
        stale.sort();
        stale.dedup();
        // a path the last update reported, reported again with no event in between, is a read the disk contradicts
        // whatever the build does (the rebuild it started read it again): not another change
        let last = std::mem::take(&mut self.last_stale);
        stale.retain(|path| !last.contains(path));
        self.last_stale = stale.iter().cloned().collect();
        if debug() && !stale.is_empty() {
            eprintln!("[rbxtsc watch] changed before their directory was watched: {stale:?}");
        }
        stale
    }
}

/// A program file, with the modification time and size it was parsed at.
type Stamp<'a> = (&'a str, Option<SystemTime>, i64);

/// Compares a directory watched for the first time with what the build read: what changed there since the build
/// started raised no event (the watch did not exist yet).
struct Scan<'a> {
    watched: &'a Watched,
    since: SystemTime,
    own: &'a Own,
    /// What the load read, by the path key of the directory holding it.
    reads: FxHashMap<String, Vec<(&'a str, Seen)>>,
    /// The program's files as parsed, by the path key of their directory.
    stamps: FxHashMap<String, Vec<Stamp<'a>>>,
    /// Path keys of the directories this watcher wrote into (their modification time is its own).
    own_dirs: FxHashSet<String>,
}

impl<'a> Scan<'a> {
    fn new(watched: &'a Watched, since: SystemTime, own: &'a Own) -> Scan<'a> {
        watched.forget_listings();
        let mut reads: FxHashMap<String, Vec<(&str, Seen)>> = FxHashMap::default();
        for (path, seen) in &watched.reads {
            if let Some(parent) = parent_of(path) {
                reads.entry(key(parent)).or_default().push((path, *seen));
            }
        }
        let mut stamps: FxHashMap<String, Vec<Stamp<'_>>> = FxHashMap::default();
        for (path, time, size) in &watched.stamps {
            if let Some(parent) = parent_of(path) {
                stamps.entry(key(parent)).or_default().push((path, *time, *size));
            }
        }
        let own_dirs = own.keys().filter_map(|k| parent_of(k).map(str::to_owned)).collect();
        Scan { watched, since, own, reads, stamps, own_dirs }
    }

    /// A file written or created since the build started, or a directory created since (a directory's modification
    /// time is its entries': those are judged on their own, or by the directory's own scan).
    fn recent(&self, metadata: &std::fs::Metadata) -> bool {
        metadata.created().is_ok_and(|t| t >= self.since)
            || (!metadata.is_dir() && metadata.modified().is_ok_and(|t| t >= self.since))
    }

    /// Adds to `stale` what in `dir` (and below it when `recursive`) differs from what the build read or changed
    /// since it started. `whole`: any entry of the directory counts (a module, package or root directory), so its
    /// own modification time counts too (an entry added, removed or renamed, whatever the entry's times).
    /// The Rojo project files below an output folder written or created since the build started, and not by this
    /// watcher.
    fn output_project_files(&self, dir: &Path, stale: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else { continue };
            let path = path_text(&entry.path());
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                self.output_project_files(&entry.path(), stale);
            } else if rojo::is_rojo_project_file_name(&entry.file_name().to_string_lossy())
                && self.recent(&metadata)
                && !is_own(&path, &key(&path), self.own)
            {
                stale.push(path);
            }
        }
    }

    fn dir(&self, dir: &Path, recursive: bool, whole: bool, stale: &mut Vec<String>) {
        let text = path_text(dir);
        let k = key(&text);
        if self.watched.ignored_roots.iter().any(|root| is_under(&k, root)) {
            // an output folder a Rojo resolver searches: only project files count there
            if self.watched.rojo_dirs.iter().any(|d| is_under(&k, d)) {
                self.output_project_files(dir, stale);
            }
            return;
        }
        let Ok(metadata) = std::fs::metadata(dir) else { return };
        // the directory itself is new since the build started (not one the build made for its outputs)
        if metadata.created().is_ok_and(|t| t >= self.since) {
            if !holds_only_own(dir, self.own) {
                stale.push(text);
            }
            return;
        }
        if whole && metadata.modified().is_ok_and(|t| t >= self.since) && !self.own_dirs.contains(&k) {
            stale.push(text.clone());
        }
        // what the load read here: a file or directory there or not, as it was (a directory the build made for its
        // own outputs since, such as the include folder the load's Rojo setup found missing, is its own write)
        for (path, seen) in self.reads.get(&k).into_iter().flatten() {
            let metadata = std::fs::metadata(path);
            let (is_file, is_dir) = metadata.map_or((false, false), |m| (m.is_file(), m.is_dir()));
            if seen.contradicted(is_file, is_dir) && !(is_dir && holds_only_own(Path::new(path), self.own)) {
                stale.push((*path).to_owned());
            }
        }
        // the program's files: the time and size they were parsed at (a file replaced with an old time)
        for (path, time, size) in self.stamps.get(&k).into_iter().flatten() {
            let now = std::fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len() as i64));
            if now != Some((*time, *size)) {
                stale.push((*path).to_owned());
            }
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else { continue };
            let path = entry.path();
            let path_string = path_text(&path);
            if self.recent(&metadata)
                && relevant(&path_string, false, self.watched, self.own)
                && !(metadata.is_dir() && holds_only_own(&path, self.own))
            {
                stale.push(path_string.clone());
            }
            let name = entry.file_name();
            // links are not followed into; a source root's node_modules is not a package (packages are scanned
            // as their own watches)
            if recursive
                && metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && name != ".git"
                && (name != "node_modules" || self.watched.in_package(&key(&path_string)))
            {
                self.dir(&path, true, whole, stale);
            }
        }
    }
}

fn watch(
    plugins: &[Box<dyn rbxts_plugin::erased::DynPlugin>],
    build: &BuildArgs,
    options: CompileOptions,
    cwd: &str,
    first_generation: u64,
) -> u8 {
    let plugin_options = PluginOptions { plugins, ..PluginOptions::default() };
    let mut session = BuildSession::new(options, &plugin_options, true);
    let (sender, receiver) = channel::<notify::Result<Event>>();
    let watcher: Box<dyn Watcher> = if build.use_polling {
        match notify::PollWatcher::new(sender, notify::Config::default().with_poll_interval(Duration::from_millis(250)))
        {
            Ok(w) => Box::new(w),
            Err(e) => {
                eprintln!("rbxtsc: cannot start the file watcher: {e}");
                return 1;
            }
        }
    } else {
        match notify::recommended_watcher(sender) {
            Ok(w) => Box::new(w),
            Err(e) => {
                eprintln!("rbxtsc: cannot start the file watcher: {e}");
                return 1;
            }
        }
    };
    let mut subscriptions = Subscriptions {
        watcher,
        active: FxHashSet::default(),
        missing: FxHashSet::default(),
        failed: FxHashSet::default(),
        last_stale: FxHashSet::default(),
    };
    let mut own: Own = FxHashMap::default();
    // a restarted watcher goes on from the last generation of the one before
    let mut generation = if first_generation > 0 { first_generation + 1 } else { 0 };

    let mut watched = Watches { good: minimal_set(&session), failed: None };
    status("Starting compilation in watch mode...");
    let (mut stale, again, _) =
        rebuild(&mut session, &mut subscriptions, &mut watched, build, cwd, generation, &[], &mut own);
    // the last build's commit changed what a plugin's setup read (its own state file created): one more build at
    // once, which runs that setup again; a build started that way starts no other (no loop, whatever a plugin
    // writes), unless it lost a state file itself: deleted or edited again by someone else since the last commit
    // (only an outside change does that, so it can't loop either)
    let mut follow_up = !again.is_empty();
    stale.extend(again);
    loop {
        // paths that changed before their directory was watched (already judged) start a rebuild of their own
        let mut paths: Vec<String> = if stale.is_empty() {
            let Some(events) = collect(&receiver) else { return 0 };
            // a directory watched from now on is compared with the disk from here (the answers below read it)
            let since = SystemTime::now();
            watched.current().forget_listings();
            // a folder the last build made for its outputs (`out`, `cache`) is its own write
            let (relevant, dropped): (Vec<BurstPath>, Vec<BurstPath>) = events.into_iter().partition(|(p, folder_modify)| {
                p == RESCAN
                    || (relevant(p, *folder_modify, watched.current(), &own)
                        && !(!*folder_modify && Path::new(p).is_dir() && holds_only_own(Path::new(p), &own)))
            });
            let mut paths: Vec<String> = relevant.into_iter().map(|(p, _)| p).collect();
            // what only a plugin's recorded input may care about: with a rebuild anyway, or when an answer changed
            let candidates = inputs::candidates(dropped, &watched.current().inputs, &own);
            if !candidates.is_empty() {
                if !paths.is_empty() || session.recorded_inputs_changed(Some(&candidates)) {
                    paths.extend(candidates);
                } else {
                    stale = inputs::refresh(&session, &mut subscriptions, &mut watched, &own, since);
                }
            }
            paths
        } else {
            std::mem::take(&mut stale)
        };
        paths.sort();
        paths.dedup();
        // a directory as a touched path makes the session load the project again, with every file's facts
        if let Some(rescan) = paths.iter_mut().find(|p| *p == RESCAN) {
            *rescan = session.options.tsconfig.clone(); // a config input: a rebuild with every file looked at
        }
        if paths.is_empty() {
            continue;
        }
        generation += 1;
        status("File change detected. Starting incremental compilation...");
        let (again, lost);
        (stale, again, lost) =
            rebuild(&mut session, &mut subscriptions, &mut watched, build, cwd, generation, &paths, &mut own);
        follow_up = (!follow_up || lost) && !again.is_empty();
        if follow_up {
            stale.extend(again);
        }
        if let Some(limit) = memory_limit()
            && std::env::var(CHILD_ENV).is_ok()
            && memory_stats::memory_stats().is_some_and(|m| m.physical_mem > limit)
        {
            if let Ok(file) = std::env::var(GENERATION_FILE_ENV) {
                let _ = std::fs::write(file, generation.to_string());
            }
            std::process::exit(RESTART_EXIT_CODE);
        }
    }
}

/// Waits for an event, then for the burst to end (each event restarts the wait); the paths of the burst, sorted,
/// each flagged when all its events were modifications of an existing directory (see [`relevant`]). A directory
/// created, moved in or renamed is a plain event (on Windows a directory moved in is one event for the directory).
/// Whether a path is a directory is looked at once the burst ended, not while it runs: opening a directory being
/// deleted leaves it pending deletion while the handle is open, and the deleting tool (`npm`, `rm -rf`) failing
/// on it with EPERM. `None` when the watcher stopped.
fn collect(receiver: &Receiver<notify::Result<Event>>) -> Option<Vec<(String, bool)>> {
    let mut paths: BTreeMap<String, bool> = BTreeMap::new();
    let add = |event: notify::Result<Event>, paths: &mut BTreeMap<String, bool>| match event {
        _ if debug() && {
            eprintln!("[rbxtsc watch] event {event:?}");
            false
        } => {}
        // events were lost (an overflowing buffer): everything may have changed
        Err(_) => {
            paths.insert(RESCAN.to_owned(), false);
        }
        Ok(event) if event.need_rescan() => {
            paths.insert(RESCAN.to_owned(), false);
        }
        Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
            let modify = matches!(
                event.kind,
                EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Other)
            );
            // a path's flag: every event of it so far was a modification
            for path in event.paths {
                let entry = paths.entry(path_text(&path)).or_insert(modify);
                *entry &= modify;
            }
        }
        Ok(_) => {}
    };
    add(receiver.recv().ok()?, &mut paths);
    loop {
        match receiver.recv_timeout(DEBOUNCE) {
            Ok(event) => add(event, &mut paths),
            Err(RecvTimeoutError::Timeout) => {
                return Some(
                    paths.into_iter().map(|(p, modify)| (p.clone(), modify && Path::new(&p).is_dir())).collect(),
                );
            }
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

/// One build of the watch loop, with its report and events. A panic is reported, and the session starts over.
/// After the build, the watched directories are brought up to date before anything is printed, so a change made
/// as soon as the build is reported finished is seen. Returns the paths that changed before their directory was
/// watched, the state files whose commit changed what a plugin's setup read
/// ([`BuildSummary::setup_inputs_changed`]), and whether the build lost a state file
/// ([`BuildSummary::lost_state_files`]).
#[allow(clippy::too_many_arguments)]
fn rebuild(
    session: &mut BuildSession<'_>,
    subscriptions: &mut Subscriptions,
    watched: &mut Watches,
    build: &BuildArgs,
    cwd: &str,
    generation: u64,
    paths: &[String],
    own: &mut Own,
) -> (Vec<String>, Vec<String>, bool) {
    if build.events_json {
        println!("{{\"event\":\"build-start\",\"gen\":{generation},\"changes\":{}}}", json_list(paths));
    }
    let since = SystemTime::now();
    let result = catch_unwind(AssertUnwindSafe(|| session.build(paths)));
    if result.is_err() {
        session.reset();
    }
    // this build's own writes are known before the new watches look for changes
    if let Ok((_, summary)) = &result {
        let stamps: FxHashMap<String, rbxts_transform::build::outputs::Stamp> =
            session.committed_outputs().into_iter().map(|(path, stamp)| (key(&path), stamp)).collect();
        for path in &summary.written {
            let k = key(path);
            if let Some(stamp) = stamps.get(&k) {
                own.insert(k, OwnWrite::Output(*stamp));
            } else if let Ok(bytes) = std::fs::read(path) {
                own.insert(k, OwnWrite::Text(Version::of(&String::from_utf8_lossy(&bytes))));
            }
        }
        for path in &summary.deleted {
            own.insert(key(path), OwnWrite::Gone);
        }
    }
    let start = std::time::Instant::now();
    match watched_set(session) {
        Some(set) => {
            watched.good = set;
            watched.failed = None;
        }
        // no project loaded: the last good set, and what the failed load read (a tsconfig extending a package not
        // installed yet)
        None => {
            let mut set = watched.good.clone();
            if let Some(log) = session.read_log() {
                set.add_log(&log, &mut FxHashMap::default());
                set.dedupe();
            }
            watched.failed = Some(set);
        }
    }
    let computed = start.elapsed();
    let before = subscriptions.active.clone();
    let mut stale = subscriptions.update(watched.current(), since, own);
    // a recorded input read before its directory was watched: asked again now that it is (a watch made, not one
    // failing again)
    if result.is_ok() {
        let new_input_watches = watched.current().inputs.has_new_watches(&before, &subscriptions.active);
        stale.extend(inputs::recheck(session, &watched.current().inputs, new_input_watches));
    }
    if debug() {
        let current = watched.current();
        eprintln!(
            "[rbxtsc watch] watch set {:.1} ms, update and scan {:.1} ms: {} reads, {} anchors, {} packages, {} recursive, {} flat",
            computed.as_secs_f64() * 1000.0,
            (start.elapsed() - computed).as_secs_f64() * 1000.0,
            current.reads.len(),
            current.anchors.len(),
            current.package_dirs.len(),
            current.recursive.len(),
            current.flat.len()
        );
    }
    let (report, summary) = match result {
        Ok(result) => result,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "panic".to_owned());
            eprintln!("\u{1b}[31mrbxtsc: the build crashed:\u{1b}[0m {message}\nThe next build starts clean.");
            if build.events_json {
                println!(
                    "{{\"event\":\"build-end\",\"gen\":{generation},\"ok\":false,\"errors\":1,\"crash\":{},\"written\":[],\"deleted\":[]}}",
                    json_string(&message)
                );
            }
            status("Found 1 error. Watching for file changes.");
            return (stale, Vec::new(), false);
        }
    };
    print_report(&report, cwd, build);
    let errors = error_count(&report);
    if build.events_json {
        print_end(generation, &report, &summary, errors);
    }
    status(&format!("Found {errors} error{}. Watching for file changes.", if errors == 1 { "" } else { "s" }));
    (stale, summary.setup_inputs_changed, !summary.lost_state_files.is_empty())
}

fn print_end(generation: u64, report: &CompileReport, summary: &BuildSummary, errors: usize) {
    let ok = !report.has_errors();
    println!(
        "{{\"event\":\"build-end\",\"gen\":{generation},\"ok\":{ok},\"errors\":{errors},\"all\":{},\"transformed\":{},\"written\":{},\"deleted\":{}}}",
        summary.all,
        report.files.len(),
        json_list(&summary.written),
        json_list(&summary.deleted)
    );
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use notify::RecursiveMode;
    use rustc_hash::{FxHashMap, FxHashSet};

    use super::{
        Subscriptions, Watched, format_time, is_under, package_entry_counts, package_root, path_text, relevant,
    };

    /// Node's en-US `toLocaleTimeString()`: no leading zero, 12 for noon and midnight.
    #[test]
    fn status_time() {
        let at = |h, m, s| format_time(chrono::NaiveTime::from_hms_opt(h, m, s).unwrap());
        assert_eq!(at(0, 5, 9), "12:05:09 AM");
        assert_eq!(at(7, 18, 41), "7:18:41 AM");
        assert_eq!(at(12, 0, 0), "12:00:00 PM");
        assert_eq!(at(19, 18, 41), "7:18:41 PM");
    }

    /// A watcher whose first watch of each path fails.
    struct FailFirst(Arc<Mutex<Vec<PathBuf>>>);

    impl notify::Watcher for FailFirst {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(FailFirst(Arc::default()))
        }

        fn watch(&mut self, path: &Path, _: RecursiveMode) -> notify::Result<()> {
            let mut tried = self.0.lock().unwrap();
            let first = !tried.iter().any(|p| p == path);
            tried.push(path.to_path_buf());
            if first { Err(notify::Error::generic("refused")) } else { Ok(()) }
        }

        fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    #[test]
    fn a_failed_watch_is_tried_again_at_the_next_update() {
        let dir = std::env::temp_dir().join(format!("rbxts-rs-watch-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tried = Arc::new(Mutex::new(Vec::new()));
        let mut subscriptions = Subscriptions {
            watcher: Box::new(FailFirst(Arc::clone(&tried))),
            active: FxHashSet::default(),
            missing: FxHashSet::default(),
            failed: FxHashSet::default(),
            last_stale: FxHashSet::default(),
        };
        let mut watched = Watched::default();
        let watch = (path_text(&dir), false);
        watched.flat.insert(watch.0.clone());
        let own = FxHashMap::default();
        subscriptions.update(&watched, SystemTime::now(), &own);
        assert!(!subscriptions.active.contains(&watch), "a failed watch is not active");
        assert!(subscriptions.failed.contains(&watch));
        subscriptions.update(&watched, SystemTime::now(), &own);
        assert!(subscriptions.active.contains(&watch), "tried again");
        assert!(subscriptions.failed.is_empty());
        subscriptions.update(&watched, SystemTime::now(), &own);
        assert_eq!(tried.lock().unwrap().len(), 2, "an active watch is not made again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A watcher that refuses recursive watches while its flag is set.
    struct RefuseRecursive(Arc<Mutex<bool>>);

    impl notify::Watcher for RefuseRecursive {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(RefuseRecursive(Arc::default()))
        }

        fn watch(&mut self, _: &Path, mode: RecursiveMode) -> notify::Result<()> {
            if mode == RecursiveMode::Recursive && *self.0.lock().unwrap() {
                Err(notify::Error::generic("refused"))
            } else {
                Ok(())
            }
        }

        fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    /// A flat directory inside a wanted recursive watch is watched itself while that recursive watch is not active
    /// (it failed), and no longer once it is.
    #[test]
    fn a_flat_directory_under_a_failed_recursive_watch_is_watched_itself() {
        let root = std::env::temp_dir().join(format!("rbxts-rs-watch-covered-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let refuse = Arc::new(Mutex::new(true));
        let mut subscriptions = Subscriptions {
            watcher: Box::new(RefuseRecursive(Arc::clone(&refuse))),
            active: FxHashSet::default(),
            missing: FxHashSet::default(),
            failed: FxHashSet::default(),
            last_stale: FxHashSet::default(),
        };
        let mut watched = Watched::default();
        let tree = (path_text(&root), true);
        let flat = (path_text(&sub), false);
        watched.recursive.insert(tree.0.clone());
        watched.flat.insert(flat.0.clone());
        watched.dedupe();
        assert!(watched.flat.is_empty(), "inside the recursive watch");
        let own = FxHashMap::default();
        subscriptions.update(&watched, SystemTime::now(), &own);
        assert!(!subscriptions.active.contains(&tree));
        assert!(subscriptions.active.contains(&flat), "watched itself: {:?}", subscriptions.active);
        *refuse.lock().unwrap() = false;
        subscriptions.update(&watched, SystemTime::now(), &own);
        assert!(subscriptions.active.contains(&tree));
        assert!(!subscriptions.active.contains(&flat), "covered again: {:?}", subscriptions.active);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn package_roots() {
        assert_eq!(
            package_root("E:/p/node_modules/@rbxts/types/include/a.d.ts").as_deref(),
            Some("E:/p/node_modules/@rbxts/types")
        );
        assert_eq!(package_root("E:/p/node_modules/pkg/lib/x.d.ts").as_deref(), Some("E:/p/node_modules/pkg"));
        assert_eq!(package_root("E:/p/src/x.ts"), None);
        // module directories themselves are no packages
        assert_eq!(package_root("E:/p/node_modules/@rbxts"), None);
        assert_eq!(package_root("E:/p/node_modules/@rbxts/"), None);
        assert_eq!(package_root("E:/p/node_modules/a/node_modules"), Some("E:/p/node_modules/a".to_owned()));
    }

    #[test]
    fn under() {
        assert!(is_under("e:/p/src/a.ts", "e:/p/src"));
        assert!(!is_under("e:/p/srcx/a.ts", "e:/p/src"));
        assert!(is_under("e:/p", "e:/"));
    }

    #[test]
    fn package_entries() {
        for name in ["package.json", "default.project.json", "index.d.ts", "init.lua", "init.luau", "x.tsx"] {
            assert!(package_entry_counts(&format!("Z:/none/{name}"), name), "{name}");
        }
        for name in ["README.md", ".npmignore", "LICENSE.txt", "x.js"] {
            assert!(!package_entry_counts(&format!("Z:/none/{name}"), name), "{name}");
        }
        // gone, no extension: a directory removed or renamed away
        assert!(package_entry_counts("Z:/none/out", "out"));
    }

    #[test]
    fn relevance() {
        let mut w = Watched::default();
        w.module_dirs.insert("e:/p/node_modules".into());
        w.module_dirs.insert("e:/p/node_modules/@rbxts".into());
        w.package_dirs.insert("e:/p/node_modules/@rbxts/pkg".into());
        w.used_packages.insert("e:/p/node_modules/@rbxts/pkg".into());
        w.package_dirs.insert("e:/p/node_modules/@rbxts/typeonly".into());
        w.anchors.insert("e:/p/node_modules/@rbxts/typeonly/package.json".into());
        w.rojo_dirs.push("e:/p/node_modules/@rbxts".into());
        w.rojo_dirs.push("e:/p/vendor".into());
        w.config_inputs.insert("e:/p/vendor/a/default.project.json".into());
        w.project_dir = "e:/p".into();
        w.anchors.insert("e:/node_modules".into());
        w.root_keys.push("e:/p/src".into());
        w.open_dirs.insert("e:/p".into());
        w.ignored_roots.push("e:/p/out".into());
        let own = Default::default();
        let yes = |p: &str| relevant(p, false, &w, &own);
        // the module directories themselves, and packages appearing or going in them
        assert!(yes("E:/p/node_modules"));
        assert!(yes("E:/p/node_modules/@rbxts/newpkg"));
        assert!(yes("E:/p/node_modules/@new"));
        assert!(!yes("E:/p/node_modules/.package-lock.json"));
        // inside a package, by kind; elsewhere under node_modules nothing
        assert!(yes("E:/p/node_modules/@rbxts/pkg/out/index.d.ts"));
        assert!(!yes("E:/p/node_modules/@rbxts/pkg/README.md"));
        assert!(!yes("E:/p/node_modules/other/index.d.ts"));
        // a package in a type root only: its package.json and Rojo project files, not its code
        assert!(yes("E:/p/node_modules/@rbxts/typeonly/package.json"));
        assert!(yes("E:/p/node_modules/@rbxts/typeonly/src/default.project.json"));
        assert!(!yes("E:/p/node_modules/@rbxts/typeonly/src/index.d.ts"));
        assert!(!yes("E:/p/node_modules/@rbxts/typeonly/src/init.lua"));
        assert!(!yes("E:/p/node_modules/@rbxts/typeonly/src/gone"));
        // a Rojo-mounted folder outside the sources: project files, and a folder that held a project file read
        assert!(yes("E:/p/vendor/x.project.json"));
        assert!(yes("E:/p/vendor/a"));
        assert!(!yes("E:/p/vendor/b"));
        assert!(!yes("E:/p/vendor/a/x.lua"));
        // a probed location outside the project, and its neighbours
        assert!(yes("E:/node_modules"));
        assert!(!yes("E:/elsewhere.txt"));
        // roots, open directories, outputs
        assert!(yes("E:/p/src/a/b.ts"));
        assert!(yes("E:/p/package.json"));
        assert!(!yes("E:/p/out/a.luau"));
        // the project directory: its Rojo project files; not lockfiles (not open, nothing read them)
        w.open_dirs.remove("e:/p");
        let yes = |p: &str| relevant(p, false, &w, &own);
        assert!(yes("E:/p/other.project.json"));
        assert!(!yes("E:/p/bun.lock"));
        assert!(!yes("E:/p/package-lock.json"));
        assert!(!yes("E:/p/sub/x.ts"));
        // a directory's own modification never counts here
        assert!(!relevant("E:/p/node_modules/@rbxts/pkg", true, &w, &own));
    }
}
