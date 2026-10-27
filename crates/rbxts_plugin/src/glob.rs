//! Globs as build inputs (DESIGN §14 "Globs"): what a plugin hands the compiler with `record_glob`, so that the
//! compiler can match the glob again when files are added, deleted or renamed under its root.

use std::path::Path;

/// A plugin's glob, handed to the compiler by `record_glob` ([`crate::Cx::record_glob`],
/// [`crate::SetupCx::record_glob`], [`crate::FinishCx::record_glob`]).
///
/// The compiler cannot match a plugin's glob itself: the semantics are the plugin's (Flamework's are glob 9.3 /
/// minimatch 8, with its own rules for dot files, links, case and spelling), and they cover the walk, not only a
/// test on each path. So the plugin hands over the whole expansion, and the compiler calls it again in watch.
///
/// **Contract for an implementation.**
/// - `expand` is a function of the pattern it was made with and of the file system under `root`: the same tree
///   gives the same answer, in the same order. It reads nothing outside `root` that a match depends on (a change
///   there is not watched), except through links under `root` (a known limit: their targets are not watched).
/// - It returns each match once, in an order the plugin defines (Flamework: sorted by UTF-16 code units, as
///   JavaScript sorts). The compiler compares answers as they are, order included.
/// - It may run on any thread, at any time between builds, and many times: no side effects, no build state.
///
/// Closures `Fn(&Path) -> Vec<String>` implement it.
pub trait GlobMatcher: Send + Sync + 'static {
    /// Every match under `root`, each once, in the plugin's order. Reads the disk.
    fn expand(&self, root: &Path) -> Vec<String>;

    /// Whether adding, deleting or renaming `relative` (a file or directory under the root, relative to it, with `/`
    /// separators) can change what [`GlobMatcher::expand`] returns. The compiler may skip matching again for an
    /// event this rejects, so an implementation must never reject a path that can change the answer.
    ///
    /// Default: `true`, every event under the root may change it.
    fn affected_by(&self, relative: &str) -> bool {
        let _ = relative;
        true
    }
}

impl<F> GlobMatcher for F
where
    F: Fn(&Path) -> Vec<String> + Send + Sync + 'static,
{
    fn expand(&self, root: &Path) -> Vec<String> {
        self(root)
    }
}

/// What every backend's default `record_glob` does: match now, record nothing.
pub(crate) fn expand_unrecorded(root: &Path, matcher: &dyn GlobMatcher) -> Vec<String> {
    matcher.expand(root)
}

/// What every backend's default `env` does: the process environment, unrecorded. A name the environment cannot
/// hold (empty, or with `=` or NUL) is unset, rather than a panic in the standard library.
pub(crate) fn process_env(name: &str) -> Option<String> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return None;
    }
    std::env::var(name).ok()
}
