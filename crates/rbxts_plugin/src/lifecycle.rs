//! The build lifecycle a plugin sees (DESIGN §14).
//!
//! ```text
//! build(kind, changes)
//!   load/update program → check affected closure
//!   → [rebuild_start(changes): keep Shared or set up again]   (watch rebuilds with a kept Shared only)
//!   → setup(state)                                            (when there is no Shared to keep)
//!   → instance() per checker → transform the closure → end_file → FilePart
//!   → finish(parts of ALL current files, state)
//!   → render → commit (no errors only): outputs, artifacts, stale deletions, state files
//!   → the working state becomes the committed state, and the Shared is kept with it
//! ```
//!
//! A build that does not commit (errors or `Fatal`) drops the `Shared` it made and its working state; the next
//! build runs `setup` again on the committed state.

/// What kind of build is starting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildKind {
    /// A full build with no earlier state: every file is transformed, the plugin state starts from
    /// `State::default()`.
    Clean,
    /// A build in a new process that reuses the compiler's incremental state on disk. Only the changed closure is
    /// transformed; the other files' parts come from that state. The plugin state starts from `State::default()`
    /// ([`StateOrigin::Fresh`]): the plugin reloads what it persisted with
    /// [`crate::SetupCx::read_persisted`].
    Incremental,
    /// A rebuild in watch mode. The committed plugin state of the last successful build is handed in
    /// ([`StateOrigin::Memory`]).
    WatchRebuild {
        /// 1 for the first rebuild after the initial build, then counting up.
        generation: u64,
    },
}

impl BuildKind {
    /// A clean build: no earlier state, and every file is transformed.
    ///
    /// The kind alone does not say whether every file is transformed: a watch rebuild after
    /// [`RebuildPlan::RerunSetup`] or a changed build input transforms every file too. Ask
    /// [`crate::SetupCx::transforms_all_files`] / [`crate::FinishCx::transforms_all_files`] for that.
    pub fn is_clean(self) -> bool {
        matches!(self, BuildKind::Clean)
    }
}

/// Where the plugin state handed to `setup` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateOrigin {
    /// `State::default()`: a clean build, an incremental build in a new process, or the first build after a
    /// compiler or plugin version change.
    Fresh,
    /// The state the last successful build in this process committed.
    Memory,
}

/// What changed since the last successful build. An editor's rename-over shows up as delete + add. Paths are
/// absolute, in the program's spelling (TypeScript-normalized, forward slashes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Source files whose text changed.
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    /// Other files added, changed or deleted under the project roots or a watched directory
    /// ([`crate::FinishCx::watch_dir`]).
    pub non_ts: Vec<String>,
    /// Build inputs that changed: the tsconfig, the Rojo files, what plugins read through
    /// [`crate::SetupCx::read_input`] / [`crate::SetupCx::env`], and the root of a glob setup recorded
    /// ([`crate::SetupCx::record_glob`]) whose matches changed.
    pub config_inputs: Vec<String>,
}

impl ChangeSet {
    /// Nothing changed.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
            && self.added.is_empty()
            && self.deleted.is_empty()
            && self.non_ts.is_empty()
            && self.config_inputs.is_empty()
    }
}

/// A plugin's answer to [`crate::Plugin::rebuild_start`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RebuildPlan {
    /// Keep the `Shared` of the last build. Only the changed closure is transformed.
    #[default]
    KeepShared,
    /// Run `setup` again. The new `Shared` may change any file's output, so every file is transformed.
    RerunSetup,
}
