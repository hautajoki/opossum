//! The build inputs plugins record besides `setup`'s file reads (DESIGN §14): the environment (`Cx::env`), globs
//! (`Cx::record_glob`, `SetupCx::record_glob`, `FinishCx::record_glob`) and the recorded finish reads
//! (`FinishCx::read_dir` / `exists` / `read_file`); how a build or a watcher asks them again; and the directory rules
//! of `FinishCx::remove_empty_dir` with the removal of the directories the artifact diff empties.
//!
//! - **Environment:** one [`EnvSnapshot`] per build, read by `SetupCx::env` and `Cx::env` alike. A file's reads are
//!   recorded with its cached part ([`FileInputs`]); an incremental build in a new process transforms a file again
//!   when a recorded name reads another value now.
//! - **Globs:** recorded with their matcher and answer ([`RecordedGlob`]). In watch, an event under a glob's root that
//!   the matcher's `affected_by` accepts (or one creating or removing the root) matches the glob again; answers are
//!   compared as they are, order included. A file's glob that answers differently transforms that file again; a
//!   setup glob is a changed build input (setup again, every file); a finish glob, like the recorded finish reads
//!   ([`FinishRead`]), runs `finish` again. The incremental state keeps a file's globs without their matchers, so a
//!   new process transforms every file that recorded one again (the plugin's contract), and keeps setup's answers
//!   to compare with the ones setup records anew.
//! - **Directories:** [`DirRules`] are the write rules applied to a directory; [`remove_queued_dirs`] and
//!   [`remove_emptied_dirs`] run in commit step 3.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};

use rbxts_plugin::{DirEntry, EntryKind, GlobMatcher};
use rbxts_project::{ProjectServices, nodepath};
use tsrs_core::CompilerOptions;

use super::graph::Version;
use crate::plugins::lifecycle::{BuildInput, output_key};

// --- the environment ---

/// A name the process environment can hold: not empty, without `=` or NUL (on Windows `std::env::var` would
/// otherwise answer for the hidden `=C:` variables, or panic).
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['=', '\0'])
}

/// One snapshot of the process environment per build: `SetupCx::env` and `Cx::env` give the same value for a name
/// in one build (the first read of a name fixes its value for the build; only this process could change it). A name
/// the environment cannot hold ([`valid_env_name`]) is unset.
#[derive(Debug, Default)]
pub struct EnvSnapshot(Mutex<FxHashMap<String, Option<String>>>);

impl EnvSnapshot {
    /// The value of `name` in this build.
    pub fn get(&self, name: &str) -> Option<String> {
        if !valid_env_name(name) {
            return None;
        }
        let mut values = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        values.entry(name.to_owned()).or_insert_with(|| std::env::var(name).ok()).clone()
    }
}

// --- globs ---

/// A plugin's glob matcher. Two compare equal: the plugin guarantees that one `(root, pattern)` means one matcher.
#[derive(Clone)]
pub struct Matcher(pub Arc<dyn GlobMatcher>);

impl std::fmt::Debug for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GlobMatcher")
    }
}

impl PartialEq for Matcher {
    fn eq(&self, _: &Matcher) -> bool {
        true
    }
}

impl Eq for Matcher {}

/// `path` (normalized, `/` separators) is `root` or lies inside it, ASCII case ignored (an over-approximation on a
/// case-sensitive file system: it only makes a check run).
fn is_at_or_under(path: &str, root: &str) -> bool {
    let root = root.strip_suffix('/').unwrap_or(root);
    path.len() >= root.len()
        && path.is_char_boundary(root.len())
        && path[..root.len()].eq_ignore_ascii_case(root)
        && (path.len() == root.len() || path.as_bytes()[root.len()] == b'/')
}

/// A path as the checks compare it: normalized, `/` separators.
pub(crate) fn plain(path: &str) -> String {
    nodepath::normalize(path).replace('\\', "/")
}

/// A glob a plugin recorded, with the answer it got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedGlob {
    /// The plugin's index among the build's active plugins.
    pub plugin: usize,
    /// Absolute, normalized, `/` separators.
    pub root: String,
    /// The pattern, relative to the root.
    pub pattern: String,
    /// `None` when read from the incremental state (a new process has no matcher).
    pub matcher: Option<Matcher>,
    /// The matches, as the matcher gave them.
    pub answer: Vec<String>,
}

impl RecordedGlob {
    /// Matches the glob, recording the answer.
    pub fn record(plugin: usize, root: String, pattern: &str, matcher: Arc<dyn GlobMatcher>) -> RecordedGlob {
        let answer = matcher.expand(Path::new(&root));
        RecordedGlob { plugin, root, pattern: pattern.to_owned(), matcher: Some(Matcher(matcher)), answer }
    }

    /// The answer now differs from the recorded one (always, without a matcher).
    pub fn changed(&self) -> bool {
        match &self.matcher {
            Some(matcher) => matcher.0.expand(Path::new(&self.root)) != self.answer,
            None => true,
        }
    }

    /// An add, delete or rename at `path` (normalized, `/` separators) may change the answer: it is under the root
    /// and the matcher's `affected_by` accepts it, or it is the root or a directory on the way to it.
    pub fn concerned_by(&self, path: &str) -> bool {
        if is_at_or_under(&self.root, path) {
            return true;
        }
        if !is_at_or_under(path, &self.root) {
            return false;
        }
        let start = self.root.strip_suffix('/').unwrap_or(&self.root).len() + 1;
        let relative = path.get(start..).unwrap_or_default();
        self.matcher.as_ref().is_none_or(|m| m.0.affected_by(relative))
    }

    /// One plugin's records with an equal `(root, pattern)` are one record.
    pub fn same_record(&self, other: &RecordedGlob) -> bool {
        self.plugin == other.plugin && self.root.eq_ignore_ascii_case(&other.root) && self.pattern == other.pattern
    }
}

/// What a file recorded while it was transformed (`Cx::env`, `Cx::record_glob`), kept with its cached part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileInputs {
    /// (plugin, name, value), each name once per plugin.
    pub env: Vec<(usize, String, Option<String>)>,
    /// The globs it recorded, each `(plugin, root, pattern)` once.
    pub globs: Vec<RecordedGlob>,
}

impl FileInputs {
    /// Records an environment read.
    pub fn record_env(&mut self, plugin: usize, name: &str, value: &Option<String>) {
        if !self.env.iter().any(|(p, n, _)| *p == plugin && n == name) {
            self.env.push((plugin, name.to_owned(), value.clone()));
        }
    }

    /// Records a glob; one with an equal `(root, pattern)` from the same plugin is replaced.
    pub fn record_glob(&mut self, glob: RecordedGlob) {
        match self.globs.iter_mut().find(|g| g.same_record(&glob)) {
            Some(existing) => *existing = glob,
            None => self.globs.push(glob),
        }
    }

    /// A recorded environment variable reads another value in `env` (a build in a new process).
    pub fn env_changed(&self, env: &EnvSnapshot) -> bool {
        self.env.iter().any(|(_, name, value)| env.get(name) != *value)
    }
}

// --- the recorded finish reads ---

/// What `read_file` answered: the text's version, or the error.
pub type ReadAnswer = Result<Option<Version>, io::ErrorKind>;

/// A read `finish` made through the compiler, with its answer: in watch, a different answer runs `finish` again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishRead {
    /// A glob matched through `record_glob`.
    Glob(RecordedGlob),
    /// A directory listed through `read_dir`.
    ReadDir {
        /// The plugin's index among the build's active plugins.
        plugin: usize,
        /// The directory, absolute.
        path: String,
        /// Its entries, `None` when it could not be listed.
        answer: Option<Vec<DirEntry>>,
    },
    /// A path tested through `exists`.
    Exists {
        /// The plugin's index among the build's active plugins.
        plugin: usize,
        /// The path, absolute.
        path: String,
        /// Whether it existed.
        answer: bool,
    },
    /// A file read through `read_file`.
    ReadFile {
        /// The plugin's index among the build's active plugins.
        plugin: usize,
        /// The file, absolute.
        path: String,
        /// The version of its text, or the error.
        answer: ReadAnswer,
    },
}

/// What a watcher watches for a recorded input.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InputWatch {
    /// A glob root: everything under it, recursively (the nearest existing ancestor while it is missing).
    Tree(String),
    /// A listed directory: its entries, and the directory itself.
    Entries(String),
    /// A path: its own events (its directory's watch, or the nearest existing ancestor's).
    Path(String),
}

impl InputWatch {
    /// The watched path.
    pub fn path(&self) -> &str {
        match self {
            InputWatch::Tree(p) | InputWatch::Entries(p) | InputWatch::Path(p) => p,
        }
    }
}

/// The default `FinishCx::read_dir`: the entries sorted by name (byte order), links followed; `None` when `path` is
/// not a readable directory.
pub fn read_dir_now(path: &Path) -> Option<Vec<DirEntry>> {
    let read = std::fs::read_dir(path).ok()?;
    let mut entries: Vec<DirEntry> = read
        .filter_map(Result::ok)
        .map(|entry| {
            let kind = match std::fs::metadata(entry.path()) {
                Ok(m) if m.is_dir() => EntryKind::Directory,
                Ok(m) if m.is_file() => EntryKind::File,
                _ => EntryKind::Other,
            };
            DirEntry { name: entry.file_name().to_string_lossy().into_owned(), kind }
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Some(entries)
}

/// The default `FinishCx::read_file`: the text, `Ok(None)` when missing.
pub fn read_file_now(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The recorded answer of a `read_file` result.
pub fn read_answer(result: &io::Result<Option<String>>) -> ReadAnswer {
    match result {
        Ok(text) => Ok(text.as_deref().map(Version::of)),
        Err(e) => Err(e.kind()),
    }
}

impl FinishRead {
    /// The answer now differs from the recorded one.
    pub fn changed(&self) -> bool {
        match self {
            FinishRead::Glob(glob) => glob.changed(),
            FinishRead::ReadDir { path, answer, .. } => read_dir_now(Path::new(path)) != *answer,
            FinishRead::Exists { path, answer, .. } => Path::new(path).exists() != *answer,
            FinishRead::ReadFile { path, answer, .. } => read_answer(&read_file_now(Path::new(path))) != *answer,
        }
    }

    /// An event at `path` (normalized, `/` separators) may change the answer.
    pub fn concerned_by(&self, path: &str) -> bool {
        match self {
            FinishRead::Glob(glob) => glob.concerned_by(path),
            // an entry added, removed, renamed or changing kind; the directory itself or one on the way to it
            FinishRead::ReadDir { path: dir, .. } => {
                is_at_or_under(dir, path) || parent_of(path).is_some_and(|parent| parent.eq_ignore_ascii_case(dir))
            }
            FinishRead::Exists { path: read, .. } | FinishRead::ReadFile { path: read, .. } => {
                is_at_or_under(read, path)
            }
        }
    }

    /// What a watcher watches for it.
    pub fn watch(&self) -> InputWatch {
        match self {
            FinishRead::Glob(glob) => InputWatch::Tree(glob.root.clone()),
            FinishRead::ReadDir { path, .. } => InputWatch::Entries(path.clone()),
            FinishRead::Exists { path, .. } | FinishRead::ReadFile { path, .. } => InputWatch::Path(path.clone()),
        }
    }
}

/// Records a finish read: equal records are one.
pub fn record_read(reads: &mut Vec<FinishRead>, read: FinishRead) {
    if !reads.contains(&read) {
        reads.push(read);
    }
}

fn parent_of(path: &str) -> Option<&str> {
    let parent = nodepath::dirname(path);
    (parent != path && !parent.is_empty()).then_some(parent)
}

// --- setup inputs in the incremental state ---

/// A setup input as the incremental state keeps it: [`BuildInput`] with the plugin's name, without a matcher.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiskInput {
    /// [`BuildInput::File`].
    File {
        /// The plugin's name.
        plugin: String,
        /// The file, absolute.
        path: String,
        /// The version of its text as hex ([`crate::build::graph::Version::to_hex`]), `None` when it was missing.
        version: Option<String>,
    },
    /// [`BuildInput::Env`].
    Env {
        /// The plugin's name.
        plugin: String,
        /// The variable.
        name: String,
        /// Its value, `None` when unset.
        value: Option<String>,
    },
    /// [`BuildInput::Resolution`].
    Resolution {
        /// The plugin's name.
        plugin: String,
        /// The specifier resolved.
        specifier: String,
        /// The file it was resolved from.
        from: String,
        /// The file it resolved to, `None` when it did not resolve.
        resolved: Option<String>,
    },
    /// [`BuildInput::Glob`].
    Glob {
        /// The plugin's name.
        plugin: String,
        /// The glob's root (absolute, normalized, `/` separators).
        root: String,
        /// The pattern, relative to the root.
        pattern: String,
        /// The matches it gave.
        answer: Vec<String>,
    },
}

impl DiskInput {
    /// The input as the state keeps it.
    pub fn of(input: &BuildInput) -> DiskInput {
        match input {
            BuildInput::File { plugin, path, version, .. } => DiskInput::File {
                plugin: (*plugin).to_owned(),
                path: path.clone(),
                version: version.map(Version::to_hex),
            },
            BuildInput::Env { plugin, name, value } => {
                DiskInput::Env { plugin: (*plugin).to_owned(), name: name.clone(), value: value.clone() }
            }
            BuildInput::Resolution { plugin, specifier, from, resolved } => DiskInput::Resolution {
                plugin: (*plugin).to_owned(),
                specifier: specifier.clone(),
                from: from.clone(),
                resolved: resolved.clone(),
            },
            BuildInput::Glob { plugin, glob } => DiskInput::Glob {
                plugin: (*plugin).to_owned(),
                root: glob.root.clone(),
                pattern: glob.pattern.clone(),
                answer: glob.answer.clone(),
            },
        }
    }

    /// The setup inputs of a build as the state keeps them: sorted, each once.
    pub fn all_of<'a>(inputs: impl IntoIterator<Item = &'a BuildInput>) -> Vec<DiskInput> {
        let mut all: Vec<DiskInput> = inputs.into_iter().map(DiskInput::of).collect();
        all.sort();
        all.dedup();
        all
    }

    /// The input as the incremental state writes it: `[kind, plugin, …]`.
    pub fn to_json(&self) -> Value {
        match self {
            DiskInput::File { plugin, path, version } => json!(["file", plugin, path, version]),
            DiskInput::Env { plugin, name, value } => json!(["env", plugin, name, value]),
            DiskInput::Resolution { plugin, specifier, from, resolved } => {
                json!(["resolution", plugin, specifier, from, resolved])
            }
            DiskInput::Glob { plugin, root, pattern, answer } => json!(["glob", plugin, root, pattern, answer]),
        }
    }

    /// An input [`DiskInput::to_json`] wrote; `None` for anything else.
    pub fn from_json(value: &Value) -> Option<DiskInput> {
        let items = value.as_array()?;
        let text = |i: usize| items.get(i)?.as_str().map(str::to_owned);
        let optional = |i: usize| -> Option<Option<String>> {
            match items.get(i)? {
                Value::Null => Some(None),
                Value::String(s) => Some(Some(s.clone())),
                _ => None,
            }
        };
        Some(match items.first()?.as_str()? {
            "file" => DiskInput::File { plugin: text(1)?, path: text(2)?, version: optional(3)? },
            "env" => DiskInput::Env { plugin: text(1)?, name: text(2)?, value: optional(3)? },
            "resolution" => {
                DiskInput::Resolution { plugin: text(1)?, specifier: text(2)?, from: text(3)?, resolved: optional(4)? }
            }
            "glob" => DiskInput::Glob {
                plugin: text(1)?,
                root: text(2)?,
                pattern: text(3)?,
                answer: items
                    .get(4)?
                    .as_array()?
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<_>>()?,
            },
            _ => return None,
        })
    }

    /// A file, environment or resolution input reads differently now (a glob is decided after setup ran again,
    /// which records it anew).
    pub fn changed_before_setup(&self, env: &EnvSnapshot, resolve: impl Fn(&str, &str) -> Option<String>) -> bool {
        match self {
            DiskInput::File { path, version, .. } => {
                std::fs::read_to_string(path).ok().map(|text| Version::of(&text).to_hex()) != *version
            }
            DiskInput::Env { name, value, .. } => env.get(name) != *value,
            DiskInput::Resolution { specifier, from, resolved, .. } => resolve(specifier, from) != *resolved,
            DiskInput::Glob { .. } => false,
        }
    }
}

// --- directories ---

/// `key` lies strictly inside `root` (both real keys).
fn strictly_inside(key: &str, root: &str) -> bool {
    key.len() > root.len() && key.starts_with(root) && key.as_bytes()[root.len()] == b'/'
}

/// A component Windows would rewrite when opening it (a trailing `.` or space).
fn has_rewritten_component(path: &str) -> bool {
    path.split(['/', '\\']).any(|c| c != "." && c != ".." && (c.ends_with('.') || c.ends_with(' ')))
}

/// The write rules applied to a directory (`FinishCx::remove_empty_dir`, and the directories the artifact diff
/// empties). Keys are real keys ([`output_key`]): through the file system, so a junction or link inside the project
/// cannot lead outside it.
pub(crate) struct DirRules {
    /// The artifact roots: the project directory, the outDir (and an override), the include path.
    artifact_roots: Vec<String>,
    /// The source roots (`rootDir`, `rootDirs`).
    source_roots: Vec<String>,
    include: String,
}

impl DirRules {
    /// The rules of a build whose outputs go to `out_dir` (the override when there is one).
    pub fn new(services: &ProjectServices, options: &CompilerOptions, out_dir: &str) -> DirRules {
        let data = &services.data;
        let include = if data.options.include_path.is_empty() {
            nodepath::resolve(&[&data.project_path, "include"])
        } else {
            data.options.include_path.clone()
        };
        let key = |path: &str| output_key(services, path);
        let mut artifact_roots: Vec<String> =
            [data.project_path.as_str(), services.path_translator.out_dir.as_str(), out_dir, include.as_str()]
                .into_iter()
                .filter(|root| !root.is_empty())
                .map(key)
                .collect();
        artifact_roots.dedup();
        let mut source_roots: Vec<String> = std::iter::once(services.path_translator.root_dir.clone())
            .chain(rbxts_project::setup::get_root_dirs(options))
            .filter(|root| !root.is_empty())
            .map(|root| key(&root))
            .collect();
        source_roots.sort();
        source_roots.dedup();
        DirRules { artifact_roots, source_roots, include: key(&include) }
    }

    /// The directory is a root, or holds one.
    fn is_or_holds_root(&self, key: &str) -> bool {
        self.artifact_roots.iter().chain(&self.source_roots).any(|root| root == key || strictly_inside(root, key))
    }

    /// Why `remove_empty_dir(absolute)` is refused (`outputs`: the real keys of this build's outputs), if it is.
    pub fn refusal(&self, services: &ProjectServices, absolute: &str, outputs: &FxHashSet<String>) -> Option<String> {
        if has_rewritten_component(absolute) {
            return Some("has a path component ending in `.` or a space".to_owned());
        }
        let key = output_key(services, absolute);
        if self.is_or_holds_root(&key) {
            return Some(
                "is the project directory, the outDir, the include path or a source root, or holds one".to_owned(),
            );
        }
        if outputs.contains(&key) || outputs.iter().any(|output| strictly_inside(output, &key)) {
            return Some("is an output of this build, or holds one".to_owned());
        }
        if !strictly_inside(&key, &self.include) {
            if self.source_roots.iter().any(|root| strictly_inside(&key, root)) {
                return Some("is inside a source root: sources are never a plugin's".to_owned());
            }
            if !self.artifact_roots.iter().any(|root| strictly_inside(&key, root)) {
                return Some("is outside the project directory, the outDir and the include path".to_owned());
            }
        }
        None
    }
}

/// What removing an empty directory found.
enum Removed {
    Yes,
    /// Missing, not empty, not a directory, or a link: left alone.
    No,
}

/// Removes `path` if it is an empty directory (never a link, never its target).
fn remove_if_empty(path: &Path) -> io::Result<Removed> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Removed::No),
        Err(e) => return Err(e),
    };
    // a link (a junction included) is left alone, and so is anything but a directory
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(Removed::No);
    }
    match std::fs::read_dir(path) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Ok(Removed::No);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Removed::No),
        Err(e) => return Err(e),
    }
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(Removed::Yes),
        // something was created or removed in between
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => Ok(Removed::No),
        Err(e) => Err(e),
    }
}

/// Commit step 3, after the artifact removals: the directories the artifact diff emptied (`deleted`: the stale
/// artifacts this commit deleted). Each one's directories up to, never including, the deepest artifact root holding
/// it go while they are empty real directories that are no root and hold none. Returns the directories removed.
pub(crate) fn remove_emptied_dirs(
    services: &ProjectServices,
    rules: &DirRules,
    deleted: &[String],
) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    for artifact in deleted {
        let key = output_key(services, artifact);
        let Some(root) =
            rules.artifact_roots.iter().filter(|root| strictly_inside(&key, root)).max_by_key(|root| root.len())
        else {
            continue;
        };
        let mut dir = nodepath::dirname(artifact).to_owned();
        loop {
            let dir_key = output_key(services, &dir);
            if !strictly_inside(&dir_key, root) || rules.is_or_holds_root(&dir_key) {
                break;
            }
            match remove_if_empty(Path::new(&dir)) {
                Ok(Removed::Yes) => removed.push(dir.clone()),
                Ok(Removed::No) => break,
                Err(e) => return Err(format!("could not remove the emptied directory {dir}: {e}")),
            }
            match parent_of(&dir) {
                Some(parent) => dir = parent.to_owned(),
                None => break,
            }
        }
    }
    Ok(removed)
}

/// Commit step 3, last: the `remove_empty_dir` calls of this build's `finish` ((plugin, absolute path), checked at
/// the call). Returns the directories removed.
pub(crate) fn remove_queued_dirs(queued: &[(&'static str, String)]) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    for (plugin, dir) in queued {
        match remove_if_empty(Path::new(dir)) {
            Ok(Removed::Yes) => removed.push(dir.clone()),
            Ok(Removed::No) => {}
            Err(e) => return Err(format!("plugin `{plugin}`: could not remove the empty directory {dir}: {e}")),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_or_under() {
        assert!(is_at_or_under("E:/p/src/a.ts", "e:/p/src"));
        assert!(is_at_or_under("E:/p/src", "e:/p/src"));
        assert!(!is_at_or_under("E:/p/srcx", "e:/p/src"));
        assert!(is_at_or_under("E:/p", "E:/"));
        assert!(!is_at_or_under("E:/p", "E:/p/src"));
    }

    #[test]
    fn a_glob_is_concerned_by_its_tree_and_the_way_to_its_root() {
        struct Txt;
        impl GlobMatcher for Txt {
            fn expand(&self, _: &Path) -> Vec<String> {
                Vec::new()
            }
            fn affected_by(&self, relative: &str) -> bool {
                relative.ends_with(".txt") || !relative.contains('.')
            }
        }
        let glob = RecordedGlob {
            plugin: 0,
            root: "E:/p/assets".into(),
            pattern: "**/*.txt".into(),
            matcher: Some(Matcher(Arc::new(Txt))),
            answer: Vec::new(),
        };
        assert!(glob.concerned_by("E:/p/assets/a/b.txt"));
        assert!(glob.concerned_by("E:/p/assets/dir"));
        assert!(!glob.concerned_by("E:/p/assets/a/b.md"));
        assert!(glob.concerned_by("E:/p/assets"));
        assert!(glob.concerned_by("E:/p"));
        assert!(!glob.concerned_by("E:/p/other/b.txt"));
        // without a matcher every event under the root counts
        let unmatched = RecordedGlob { matcher: None, ..glob };
        assert!(unmatched.concerned_by("E:/p/assets/a/b.md"));
        assert!(unmatched.changed());
    }

    #[test]
    fn reads_are_concerned_by_their_path_and_entries() {
        let dir = FinishRead::ReadDir { plugin: 0, path: "E:/p/dir".into(), answer: None };
        assert!(dir.concerned_by("E:/p/dir/x"));
        assert!(dir.concerned_by("E:/p/dir"));
        assert!(dir.concerned_by("E:/p"));
        assert!(!dir.concerned_by("E:/p/dir/x/y"));
        let file = FinishRead::ReadFile { plugin: 0, path: "E:/p/a.json".into(), answer: Ok(None) };
        assert!(file.concerned_by("e:/p/A.json"));
        assert!(!file.concerned_by("E:/p/b.json"));
    }

    #[test]
    fn disk_inputs_round_trip() {
        for input in [
            DiskInput::File { plugin: "p".into(), path: "E:/a".into(), version: Some("00".into()) },
            DiskInput::File { plugin: "p".into(), path: "E:/a".into(), version: None },
            DiskInput::Env { plugin: "p".into(), name: "X".into(), value: None },
            DiskInput::Resolution {
                plugin: "p".into(),
                specifier: "s".into(),
                from: "f".into(),
                resolved: Some("r".into()),
            },
            DiskInput::Glob { plugin: "p".into(), root: "E:/".into(), pattern: "*".into(), answer: vec!["a".into()] },
        ] {
            assert_eq!(DiskInput::from_json(&input.to_json()), Some(input));
        }
    }

    #[test]
    fn env_names_the_environment_cannot_hold_are_unset() {
        let env = EnvSnapshot::default();
        assert_eq!(env.get(""), None);
        assert_eq!(env.get("=C:"), None);
        assert_eq!(env.get("A\0B"), None);
        assert_eq!(env.get("PATH").is_some(), std::env::var("PATH").is_ok());
    }
}
