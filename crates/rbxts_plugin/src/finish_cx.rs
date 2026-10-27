//! `FinishCx`: what [`crate::Plugin::finish`] can ask of the compiler (DESIGN §11.3, §14).
//!
//! Nothing here touches the disk at once: artifacts, removals and state files are queued and written in the
//! compiler's single write phase, only when the build has no errors (DESIGN §14 "Commit"). (The default
//! [`FinishBackend::remove_empty_dir`] is the exception: a compiler that has no queue for it removes the directory at
//! the call.) The reads (`read_dir`, `exists`, `read_file`) happen at once and are recorded, so that watch runs
//! `finish` again when an answer changes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rbxts_project::{ProjectServices, nodepath};

use crate::{BuildKind, Fatal, FileIndex, GlobMatcher, Utf16Pos, escape_luau_string};

/// The compiler's side of [`FinishCx`]. One is made per plugin and build, on the finishing thread.
///
/// **Paths.** `write_artifact`, `write_state_file`, `remove_artifact`, `remove_empty_dir` and `watch_dir` take
/// absolute paths, or paths relative to the project directory (the tsconfig's directory,
/// `project().data.project_path`). Artifacts and state files must lie under the project directory, the `outDir` or
/// the include path (`--includePath`); the compiler fails the build, naming the plugin, for any other path, and for
/// a path that is also an output of the build.
pub trait FinishBackend {
    /// The project services.
    fn project(&self) -> &ProjectServices;
    /// The kind of build.
    fn build_kind(&self) -> BuildKind;
    /// Every source file was transformed this build (see [`crate::SetupBackend::transforms_all_files`]).
    fn transforms_all_files(&self) -> bool;
    /// Sets the value of a late string of this plugin. `escaped` is already Luau-escaped
    /// ([`escape_luau_string`]): the renderer puts it between quotes as it is. A later call for the same key
    /// replaces the value. After `finish` returns, the compiler fails the build, naming the plugin and the key,
    /// if a key used by a current file (transformed this build or cached) has no value; values for keys no file
    /// uses are ignored. A cached file whose values changed is re-rendered (contract on
    /// [`crate::CxBackend::late_string`]).
    fn resolve_late(&mut self, key: &str, escaped: String);
    /// Queues an artifact, written in commit step 2 (after the Luau outputs) only if its bytes changed. The
    /// compiler remembers the artifact set: an artifact written last build but not this one is deleted in step 3.
    fn write_artifact(&mut self, path: &Path, bytes: Vec<u8>);
    /// Queues the plugin's persistent-state file (Flamework's `flamework.build`), written in commit step 4,
    /// before the compiler's own incremental state, so a crash never leaves the compiler's state newer.
    fn write_state_file(&mut self, path: &Path, bytes: Vec<u8>);
    /// Queues the deletion of a file the plugin owns but did not write this build and may not have written in
    /// this process (a stale artifact from an earlier toolchain or project type); commit step 3.
    ///
    /// **Emptied directories.** In step 3 the compiler also removes the directories its artifact diff empties (an
    /// artifact written last build and not this one was the last thing in them): only directories inside the
    /// artifact roots that held earlier artifacts, never a parent above them, never a link. So a plugin's folder of
    /// artifacts goes once its last artifact goes; [`FinishBackend::remove_empty_dir`] is for the cases the diff
    /// does not cover (a folder left by an earlier process or toolchain).
    fn remove_artifact(&mut self, path: &Path);
    /// Watches a directory recursively from now on: an add, delete or rename under it starts a rebuild that re-runs
    /// `finish`, though no source file changed. The set is replaced every build.
    ///
    /// For an on-disk input `finish` reads in a way the recorded reads do not cover. A glob uses
    /// [`FinishBackend::record_glob`] instead, and a listing, an existence check or a file
    /// [`FinishBackend::read_dir`], [`FinishBackend::exists`] or [`FinishBackend::read_file`]: those re-run `finish`
    /// only when their answer changes, this one on every event under the directory.
    fn watch_dir(&mut self, path: &Path);
    /// A warning at a position of a file, printed with the build's diagnostics; does not fail the build.
    fn warn_at(&mut self, file: FileIndex, pos: Utf16Pos, message: &str, hint: Option<&str>);
    /// An informational message, shown with `--verbose`.
    fn info(&mut self, message: &str);

    /// Matches a glob under `root` (an absolute directory) for `finish`: the artifacts may depend on the matches.
    /// Returns `matcher.expand(root)` as it is. A narrower [`FinishBackend::watch_dir`]: a rebuild only when the
    /// matches change.
    ///
    /// **Contract for an override.**
    /// - Returns `matcher.expand(root)` unchanged, and records `(plugin, root, pattern, matcher, answer)` for the
    ///   watcher. The set is replaced every build, as `watch_dir`'s is; one plugin's records with an equal
    ///   `(root, pattern)` are one record (an equal pattern means an equal matcher).
    /// - Unlike `watch_dir` and the write paths, `root` need not lie under the project directory, the `outDir` or
    ///   the include path: it is only read (Flamework's globs are rooted at the package root, which can be above
    ///   the project directory).
    /// - Watch: the compiler watches `root` recursively, files and directories (its nearest existing ancestor
    ///   while it does not exist). After an add, delete or rename under it that [`GlobMatcher::affected_by`]
    ///   accepts, it calls `expand` again. An answer that differs starts a rebuild that runs `finish` (with every
    ///   cached part) and commits, though no source file changed; an equal answer starts nothing. The compiler's own
    ///   writes under `root` may be ignored or matched again: the answers compare equal once the outputs settle, so
    ///   either way the rebuilds stop.
    /// - A build in a new process runs `finish`, which records the glob anew: nothing is persisted.
    ///
    /// **Default:** `matcher.expand(root)`, recording nothing.
    fn record_glob(
        &mut self,
        root: &Path,
        pattern: &str,
        matcher: Arc<dyn GlobMatcher>,
    ) -> Vec<String> {
        let _ = pattern;
        crate::glob::expand_unrecorded(root, &*matcher)
    }

    /// Removes a directory if it is empty: the plugin's own folder of artifacts once it writes none there
    /// (Flamework's `include/flamework`, which the TS transformer removes with `rmdirSync`), in the cases the
    /// compiler's own removal of emptied directories does not cover (see [`FinishBackend::remove_artifact`]).
    /// `Err` is a refusal or an I/O failure the backend could not record (the default's);
    /// [`FinishCx::remove_empty_dir`] turns it into a [`Fatal`].
    ///
    /// **Refused paths** (the write rules applied to a directory). Allowed only strictly inside an artifact root:
    /// the project directory, the `outDir` or the include path. Refused, with `Err`:
    /// - a path outside them, or a root itself, or a directory that holds one (the source root `rootDir` and
    ///   `rootDirs` included);
    /// - a path the build writes or copies to (Luau outputs, copied assets, include files, declaration outputs),
    ///   or a directory that holds one;
    /// - a directory inside a source root, unless it is inside the include path: sources are never a plugin's;
    /// - a path with a component ending in `.` or a space (Windows would open another file).
    ///
    /// Each test is made through the file system (a junction or link inside the project can lead outside it), at the
    /// call, so a refusal does not depend on what is on disk at commit.
    ///
    /// **Contract for an override.** Checks the path at the call and, as [`FinishBackend::write_artifact`] does,
    /// records a refusal as an error naming the plugin (the build fails) and returns `Ok`: the `Result` carries
    /// only I/O failures, and an override does no I/O at the call. It queues the removal for commit step 3, after
    /// the build's artifact removals and stale-output deletions, only when the build commits; a failure there is a
    /// commit error.
    /// Then: the directory is removed if it is an empty directory, and left alone if it is missing, not empty, not
    /// a directory, or a link (the link itself is never removed). Parents are not removed. An artifact a later
    /// build writes into it creates it again.
    ///
    /// **Default:** the same checks, and the removal at once (so before the removals this build queues: a folder
    /// whose last files this build removes goes on the next build). It cannot record an error, so a refusal is an
    /// `Err`. Not knowing the build's outputs, the default refuses every path inside the program's `outDir` that is
    /// not inside the include path. An I/O error other than a missing or non-empty directory is an `Err`.
    fn remove_empty_dir(&mut self, path: &Path) -> Result<(), String> {
        let services = self.project();
        let include = if services.data.options.include_path.is_empty() {
            nodepath::resolve(&[&services.data.project_path, "include"])
        } else {
            services.data.options.include_path.clone()
        };
        let roots = DirRoots {
            project: &services.data.project_path,
            out: &services.path_translator.out_dir,
            include: &include,
            source: &services.path_translator.root_dir,
            case_sensitive: services.use_case_sensitive_file_names,
        };
        remove_empty_dir_now(&roots, path)
    }

    /// The entries of a directory, for `finish`, recorded: in watch, a change in the answer re-runs `finish`.
    /// `None` when `path` is not a readable directory. Entries are sorted by name (byte order), each with its kind
    /// with links followed ([`EntryKind`]).
    ///
    /// **Recorded reads** (`read_dir`, [`FinishBackend::exists`], [`FinishBackend::read_file`]). They let every
    /// warning and artifact of `finish` depend only on the file parts and recorded answers, so a watch rebuild's
    /// warnings equal a clean build's. Paths are absolute, or relative to the project directory; they need not lie
    /// under the artifact roots (they are only read).
    ///
    /// **Contract for an override.**
    /// - Returns what the default returns, and records `(plugin, kind of read, path, answer)` for the watcher. The
    ///   set is replaced every build; equal reads are one record.
    /// - Watch: the compiler watches what the answer depends on: for `read_dir`, the directory's entries (an add,
    ///   delete or rename of a child, a child's kind changing) or, while it is missing, its nearest existing
    ///   ancestor; for `exists` and `read_file`, the path and its nearest existing ancestor; for `read_file`, the
    ///   file's text too. After such an event it reads again; an answer that differs starts a rebuild that runs
    ///   `finish` (with every cached part) and commits, though no source file changed. An equal one starts nothing.
    ///   The compiler's own writes may be ignored or read again: the answers settle, so the rebuilds stop.
    /// - A build in a new process runs `finish`, which records anew: nothing is persisted.
    ///
    /// **Default:** the read, now, recording nothing.
    fn read_dir(&mut self, path: &Path) -> Option<Vec<DirEntry>> {
        let path = read_path(&*self, path);
        read_dir_now(&path)
    }

    /// Whether something is at `path` (links followed, as `Path::exists`), for `finish`, recorded as
    /// [`FinishBackend::read_dir`] is.
    ///
    /// **Default:** `Path::exists`, now, recording nothing.
    fn exists(&mut self, path: &Path) -> bool {
        read_path(&*self, path).exists()
    }

    /// A file's text (UTF-8, as stored), for `finish`, recorded as [`FinishBackend::read_dir`] is. `Ok(None)` when
    /// it does not exist. The finish counterpart of [`crate::SetupBackend::read_input`], without its rebuild of
    /// everything: a change re-runs `finish` only.
    ///
    /// **Default:** the read, now, recording nothing.
    fn read_file(&mut self, path: &Path) -> io::Result<Option<String>> {
        match std::fs::read_to_string(read_path(&*self, path)) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// What a directory entry is, links followed ([`FinishBackend::read_dir`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Directory,
    /// Anything else: a link that leads nowhere, a device, an entry that could not be read.
    Other,
}

/// One entry of [`FinishBackend::read_dir`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DirEntry {
    /// The entry's name (lossy for a name that is not Unicode).
    pub name: String,
    pub kind: EntryKind,
}

/// A path a plugin reads: absolute as it is, relative against the project directory.
fn read_path<B: FinishBackend + ?Sized>(backend: &B, path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if nodepath::is_absolute(&text) {
        path.to_owned()
    } else {
        PathBuf::from(nodepath::resolve(&[&backend.project().data.project_path, &text]))
    }
}

/// The default [`FinishBackend::read_dir`].
fn read_dir_now(path: &Path) -> Option<Vec<DirEntry>> {
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

/// The context of [`crate::Plugin::finish`]; forwards to [`FinishBackend`] and Luau-escapes late strings.
///
/// **Paths.** `write_artifact`, `write_state_file`, `remove_artifact`, `remove_empty_dir` and `watch_dir` take
/// absolute paths, or paths relative to the project directory (the tsconfig's directory,
/// `project().data.project_path`). Artifacts and state files must lie under the project directory, the `outDir` or
/// the include path (`--includePath`); the compiler fails the build, naming the plugin, for any other path, and for
/// a path that is also an output of the build.
pub struct FinishCx<'a> {
    backend: &'a mut dyn FinishBackend,
}

impl<'a> FinishCx<'a> {
    /// Wraps the compiler's backend for one `finish` call.
    pub fn new(backend: &'a mut dyn FinishBackend) -> FinishCx<'a> {
        FinishCx { backend }
    }

    /// The project services ([`FinishBackend::project`]).
    pub fn project(&self) -> &ProjectServices {
        self.backend.project()
    }

    /// The kind of build ([`FinishBackend::build_kind`]).
    pub fn build_kind(&self) -> BuildKind {
        self.backend.build_kind()
    }

    /// Whether every file was transformed this build ([`FinishBackend::transforms_all_files`]).
    pub fn transforms_all_files(&self) -> bool {
        self.backend.transforms_all_files()
    }

    /// Gives the late string `key` (see [`crate::Cx::late_string`]) its value: `text` is plain text, which this
    /// method escapes for Luau. Every key used by a current file must be resolved ([`FinishBackend::resolve_late`]).
    pub fn resolve_late(&mut self, key: &str, text: &str) {
        self.backend.resolve_late(key, escape_luau_string(text));
    }

    /// Queues an artifact ([`FinishBackend::write_artifact`]).
    pub fn write_artifact(&mut self, path: impl AsRef<Path>, bytes: Vec<u8>) {
        self.backend.write_artifact(path.as_ref(), bytes);
    }

    /// Queues the persistent-state file ([`FinishBackend::write_state_file`]).
    pub fn write_state_file(&mut self, path: impl AsRef<Path>, bytes: Vec<u8>) {
        self.backend.write_state_file(path.as_ref(), bytes);
    }

    /// Queues a deletion ([`FinishBackend::remove_artifact`]).
    pub fn remove_artifact(&mut self, path: impl AsRef<Path>) {
        self.backend.remove_artifact(path.as_ref());
    }

    /// Removes the plugin's folder of artifacts if it is empty at commit ([`FinishBackend::remove_empty_dir`]). A
    /// refused path is a plugin bug: the [`Fatal`] stops the build (`cx.remove_empty_dir(dir)?`).
    pub fn remove_empty_dir(&mut self, path: impl AsRef<Path>) -> Result<(), Fatal> {
        self.backend
            .remove_empty_dir(path.as_ref())
            .map_err(Fatal::new)
    }

    /// Watches a glob root ([`FinishBackend::watch_dir`]).
    pub fn watch_dir(&mut self, path: impl AsRef<Path>) {
        self.backend.watch_dir(path.as_ref());
    }

    /// Matches a glob under the absolute directory `root` for the artifacts: in watch, a change in the matches runs
    /// `finish` again ([`FinishBackend::record_glob`]). `pattern` names the glob: one pattern, one matcher.
    pub fn record_glob(
        &mut self,
        root: impl AsRef<Path>,
        pattern: &str,
        matcher: impl GlobMatcher,
    ) -> Vec<String> {
        self.backend
            .record_glob(root.as_ref(), pattern, Arc::new(matcher))
    }

    /// A directory's entries, sorted by name, recorded for watch ([`FinishBackend::read_dir`]). `None` when it is
    /// not a readable directory.
    pub fn read_dir(&mut self, path: impl AsRef<Path>) -> Option<Vec<DirEntry>> {
        self.backend.read_dir(path.as_ref())
    }

    /// Whether something is at `path`, recorded for watch ([`FinishBackend::exists`]).
    pub fn exists(&mut self, path: impl AsRef<Path>) -> bool {
        self.backend.exists(path.as_ref())
    }

    /// A file's text, recorded for watch ([`FinishBackend::read_file`]). `Ok(None)` when it does not exist.
    pub fn read_file(&mut self, path: impl AsRef<Path>) -> io::Result<Option<String>> {
        self.backend.read_file(path.as_ref())
    }

    /// A positioned warning ([`FinishBackend::warn_at`]).
    pub fn warn_at(&mut self, file: FileIndex, pos: Utf16Pos, message: &str, hint: Option<&str>) {
        self.backend.warn_at(file, pos, message, hint);
    }

    /// A verbose message ([`FinishBackend::info`]).
    pub fn info(&mut self, message: &str) {
        self.backend.info(message);
    }
}

// --- the default `remove_empty_dir` ---

/// The directories the default [`FinishBackend::remove_empty_dir`] judges a path by (absolute; an empty one is
/// absent).
struct DirRoots<'a> {
    project: &'a str,
    out: &'a str,
    include: &'a str,
    /// The source root (`rootDir`, the path translator's).
    source: &'a str,
    case_sensitive: bool,
}

/// A component Windows would rewrite when opening it (a trailing `.` or space), so that two spellings name one
/// directory.
fn has_rewritten_component(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|c| c != "." && c != ".." && (c.ends_with('.') || c.ends_with(' ')))
}

/// The path as the file system resolves it, as a comparison key: its deepest existing ancestor canonicalised
/// (junctions, links, case), the rest appended; `/` separators, no trailing one, lower case unless the file system
/// is case-sensitive.
fn real_key(path: &str, case_sensitive: bool) -> String {
    let mut existing = PathBuf::from(nodepath::normalize(path));
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                rest.push(name.to_owned());
                existing.pop();
            }
            None => break,
        }
    }
    let mut real = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.iter().rev() {
        real.push(name);
    }
    let text = real.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let mut key = nodepath::normalize(text).replace('\\', "/");
    while key.len() > 1 && key.ends_with('/') && !key.ends_with(":/") {
        key.pop();
    }
    if case_sensitive {
        key
    } else {
        key.to_lowercase()
    }
}

/// `key` lies strictly inside `root` (both [`real_key`]s).
fn is_inside(key: &str, root: &str) -> bool {
    key.len() > root.len() && key.starts_with(root) && key.as_bytes()[root.len()] == b'/'
}

/// The default [`FinishBackend::remove_empty_dir`]: the checks, then the removal at once.
fn remove_empty_dir_now(roots: &DirRoots<'_>, path: &Path) -> Result<(), String> {
    let text = path.to_string_lossy();
    let absolute = if nodepath::is_absolute(&text) {
        nodepath::normalize(&text)
    } else {
        nodepath::resolve(&[roots.project, &text])
    };
    let refused = |why: &str| Err(format!("remove_empty_dir: {absolute} {why}"));
    if has_rewritten_component(&absolute) {
        return refused("has a path component ending in `.` or a space");
    }
    let key = real_key(&absolute, roots.case_sensitive);
    let root_key = |root: &str| (!root.is_empty()).then(|| real_key(root, roots.case_sensitive));
    let (project, out, include, source) = (
        root_key(roots.project),
        root_key(roots.out),
        root_key(roots.include),
        root_key(roots.source),
    );
    for root in [&project, &out, &include, &source].into_iter().flatten() {
        if key == *root || is_inside(root, &key) {
            return refused(
                "is the project directory, the outDir, the include path or the source root, or holds one",
            );
        }
    }
    let inside = |root: &Option<String>| root.as_ref().is_some_and(|root| is_inside(&key, root));
    if !inside(&include) {
        if inside(&out) {
            return refused(
                "is inside the outDir: the build writes there (only the include path is open to plugins there)",
            );
        }
        if inside(&source) {
            return refused("is inside the source root: sources are never a plugin's");
        }
        if !inside(&project) {
            return refused("is outside the project directory, the outDir and the include path");
        }
    }

    let target = Path::new(&absolute);
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("remove_empty_dir: could not read {absolute}: {e}")),
    };
    // A link (a junction included) is left alone, and so is anything but a directory.
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }
    match std::fs::read_dir(target) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Ok(());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("remove_empty_dir: could not list {absolute}: {e}")),
    }
    match std::fs::remove_dir(target) {
        Ok(()) => Ok(()),
        // Something was created or removed in between.
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(format!("remove_empty_dir: could not remove {absolute}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh project tree in the temporary directory: `src/`, `out/`, `include/`.
    struct Tree {
        dir: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Tree {
            let dir = std::env::temp_dir().join(format!(
                "rbxts_plugin-remove-empty-dir-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            for sub in ["src", "out", "include"] {
                std::fs::create_dir_all(dir.join(sub)).unwrap();
            }
            Tree { dir }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.dir.join(relative)
        }

        fn text(&self, relative: &str) -> String {
            self.path(relative).to_string_lossy().replace('\\', "/")
        }

        fn remove(&self, path: impl AsRef<Path>) -> Result<(), String> {
            let (project, out, include, source) = (
                self.text(""),
                self.text("out"),
                self.text("include"),
                self.text("src"),
            );
            let roots = DirRoots {
                project: project.trim_end_matches('/'),
                out: &out,
                include: &include,
                source: &source,
                case_sensitive: false,
            };
            remove_empty_dir_now(&roots, path.as_ref())
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn removes_an_empty_directory_in_the_include_path() {
        let tree = Tree::new("include");
        std::fs::create_dir_all(tree.path("include/flamework")).unwrap();
        assert_eq!(tree.remove(tree.path("include/flamework")), Ok(()));
        assert!(!tree.path("include/flamework").exists());
        assert!(tree.path("include").is_dir(), "the root stays");
    }

    #[test]
    fn a_relative_path_is_relative_to_the_project_directory() {
        let tree = Tree::new("relative");
        std::fs::create_dir_all(tree.path("include/flamework")).unwrap();
        assert_eq!(tree.remove("include/flamework"), Ok(()));
        assert!(!tree.path("include/flamework").exists());
    }

    #[test]
    fn leaves_a_directory_that_is_not_empty_missing_or_a_file() {
        let tree = Tree::new("noop");
        std::fs::create_dir_all(tree.path("include/flamework")).unwrap();
        std::fs::write(tree.path("include/flamework/globs.json"), "{}").unwrap();
        assert_eq!(tree.remove(tree.path("include/flamework")), Ok(()));
        assert!(tree.path("include/flamework/globs.json").is_file());

        assert_eq!(tree.remove(tree.path("include/missing")), Ok(()));

        std::fs::write(tree.path("include/file"), "x").unwrap();
        assert_eq!(tree.remove(tree.path("include/file")), Ok(()));
        assert!(tree.path("include/file").is_file());
    }

    #[test]
    fn an_empty_directory_elsewhere_in_the_project_is_allowed() {
        let tree = Tree::new("project");
        std::fs::create_dir_all(tree.path("artifacts/empty")).unwrap();
        assert_eq!(tree.remove(tree.path("artifacts/empty")), Ok(()));
        assert!(!tree.path("artifacts/empty").exists());
        assert!(tree.path("artifacts").is_dir(), "parents stay");
    }

    #[test]
    fn refuses_roots_their_holders_outputs_sources_and_outside_paths() {
        let tree = Tree::new("refused");
        std::fs::create_dir_all(tree.path("out/server")).unwrap();
        std::fs::create_dir_all(tree.path("src/empty")).unwrap();
        let refused = |relative: &str| {
            let result = tree.remove(tree.path(relative));
            assert!(result.is_err(), "{relative} was not refused");
            result.unwrap_err()
        };
        assert!(refused("include").contains("or holds one"));
        assert!(refused("out").contains("or holds one"));
        assert!(refused("src").contains("or holds one"));
        assert!(refused("out/server").contains("inside the outDir"));
        assert!(refused("src/empty").contains("inside the source root"));
        assert!(refused("../elsewhere").contains("outside the project directory"));
        assert!(refused("include/trailing.").contains("ending in `.` or a space"));
        assert!(refused("include/trailing ").contains("ending in `.` or a space"));
        // the project directory itself
        let project = tree.text("");
        assert!(tree.remove(project.trim_end_matches('/')).is_err());
        assert!(tree.path("src/empty").is_dir() && tree.path("out/server").is_dir());
    }

    #[test]
    fn the_include_path_inside_out_is_open() {
        let tree = Tree::new("include-in-out");
        std::fs::create_dir_all(tree.path("out/include/flamework")).unwrap();
        let (project, out, include, source) = (
            tree.text(""),
            tree.text("out"),
            tree.text("out/include"),
            tree.text("src"),
        );
        let roots = DirRoots {
            project: project.trim_end_matches('/'),
            out: &out,
            include: &include,
            source: &source,
            case_sensitive: false,
        };
        assert_eq!(
            remove_empty_dir_now(&roots, &tree.path("out/include/flamework")),
            Ok(())
        );
        assert!(!tree.path("out/include/flamework").exists());
        assert!(remove_empty_dir_now(&roots, &tree.path("out/include")).is_err());
    }

    #[test]
    fn rewritten_components() {
        assert!(has_rewritten_component("C:/a/b./c"));
        assert!(has_rewritten_component("/a/b "));
        assert!(!has_rewritten_component("/a/./b/../c"));
        assert!(!has_rewritten_component("/a/.b/c.d"));
    }
}
