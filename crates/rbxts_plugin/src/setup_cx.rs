//! `SetupCx`: what [`crate::Plugin::setup`] can ask of the compiler (DESIGN §11.3, §14).

use std::io;
use std::path::Path;
use std::sync::Arc;

use rbxts_project::{ProjectServices, ResolvedModuleInfo};
use serde_json::{Map, Value};
use tsrs_compiler::Program;
use tsrs_core::{CompilerOptions, P};

use crate::{BuildKind, ChangeSet, GlobMatcher, StateOrigin};

/// The compiler's side of [`SetupCx`]. One is made per plugin and build, on one thread, after the check.
pub trait SetupBackend {
    /// The program.
    fn program(&self) -> &'static Program;
    /// The program's effective compiler options.
    fn options(&self) -> P<CompilerOptions>;
    /// The project services. Plugins take the Rojo resolver, include path, project directory and options only
    /// from here; they never re-derive them.
    fn project(&self) -> &ProjectServices;
    /// This plugin's entry in the tsconfig's `compilerOptions.plugins` (the one whose `transform` is
    /// [`crate::PluginInfo::tsconfig_transform`]); `None` for an always-on plugin without an entry.
    fn tsconfig_entry(&self) -> Option<&Map<String, Value>>;
    /// Where `specifier` resolves when imported from a file at `from_file` (an absolute path; the file need not
    /// exist), with the program's options: the program's own resolution when a program file imports it from
    /// there, else a resolver with the program's options. Flamework decides its guard library this way.
    fn resolve_module(&self, specifier: &str, from_file: &str) -> Option<ResolvedModuleInfo>;
    /// Reads a file as UTF-8 and records it as a build input: watched, and hashed into every file's version, so
    /// a change (creation and deletion included) re-runs setup and re-transforms everything. `Ok(None)` when it
    /// does not exist (the absence is recorded too).
    ///
    /// The plugin's own state file may be read here (Flamework's candidate scan stops at its own
    /// `flamework.build`), but `Shared` may then depend only on whether it exists, never on its contents
    /// ([`crate::Plugin::Shared`]): the plugin's own write of it is not a changed input.
    fn read_input(&mut self, path: &Path) -> io::Result<Option<String>>;
    /// An environment variable, recorded as a build input (hashed for incremental builds; the process
    /// environment does not change during watch). [`crate::CxBackend::env`] reads the same snapshot during the
    /// transform, recorded against a file instead.
    fn env(&mut self, name: &str) -> Option<String>;
    /// The kind of build.
    fn build_kind(&self) -> BuildKind;
    /// Every source file is transformed this build: a clean build, a changed build input, or a watch rebuild
    /// after [`crate::RebuildPlan::RerunSetup`]. Otherwise only the changed closure is, and the rest bring
    /// their cached parts.
    fn transforms_all_files(&self) -> bool;
    /// What changed since the last successful build; `None` for a clean build.
    fn changes(&self) -> Option<&ChangeSet>;
    /// Where the state handed to setup came from.
    fn state_origin(&self) -> StateOrigin;
    /// Reads a file this plugin persists itself ([`crate::FinishCx::write_state_file`]), to reload its state
    /// in a new process. Not a build input: the plugin writes it. `Ok(None)` when it does not exist.
    ///
    /// What it holds goes into `State`, never into `Shared` ([`crate::Plugin::Shared`]), and is read when the
    /// state is [`StateOrigin::Fresh`]. A watch rebuild hands the committed state on in memory
    /// ([`StateOrigin::Memory`]): the file is not re-read from disk between rebuilds (DESIGN §14).
    fn read_persisted(&mut self, path: &Path) -> io::Result<Option<Vec<u8>>>;
    /// An informational message, shown with `--verbose`.
    fn info(&mut self, message: &str);
    /// A warning, printed with the build's diagnostics; does not fail the build.
    fn warn(&mut self, message: &str, hint: Option<&str>);

    /// Matches a glob under `root` (an absolute directory) and records it as a build input of setup, like a file
    /// read with [`SetupBackend::read_input`]: `Shared` may depend on the matches. Returns `matcher.expand(root)` as
    /// it is.
    ///
    /// **Contract for an override.**
    /// - Returns `matcher.expand(root)` unchanged, and records `(plugin, root, pattern, matcher, answer)` with the
    ///   build inputs; the answer is hashed into every file's version as the other inputs are.
    /// - Watch: the compiler watches `root` recursively, files and directories (its nearest existing ancestor
    ///   while it does not exist). After an add, delete or rename under it that [`GlobMatcher::affected_by`]
    ///   accepts, it calls `expand` again. An answer that differs is a changed build input: the kept `Shared` is
    ///   dropped, `setup` runs again and every file is transformed ([`crate::ChangeSet::config_inputs`] names the
    ///   root).
    /// - A build in a new process runs `setup`, which records the glob anew; an answer that differs from the one
    ///   in the incremental state is a changed build input.
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
}

/// The context of [`crate::Plugin::setup`]; forwards to [`SetupBackend`].
pub struct SetupCx<'a> {
    backend: &'a mut dyn SetupBackend,
}

impl<'a> SetupCx<'a> {
    /// Wraps the compiler's backend for one `setup` call.
    pub fn new(backend: &'a mut dyn SetupBackend) -> SetupCx<'a> {
        SetupCx { backend }
    }

    /// The program ([`SetupBackend::program`]).
    pub fn program(&self) -> &'static Program {
        self.backend.program()
    }

    /// The compiler options ([`SetupBackend::options`]).
    pub fn options(&self) -> P<CompilerOptions> {
        self.backend.options()
    }

    /// The project services ([`SetupBackend::project`]).
    pub fn project(&self) -> &ProjectServices {
        self.backend.project()
    }

    /// This plugin's tsconfig `plugins` entry ([`SetupBackend::tsconfig_entry`]).
    pub fn tsconfig_entry(&self) -> Option<&Map<String, Value>> {
        self.backend.tsconfig_entry()
    }

    /// Module resolution from a path ([`SetupBackend::resolve_module`]).
    pub fn resolve_module(&self, specifier: &str, from_file: &str) -> Option<ResolvedModuleInfo> {
        self.backend.resolve_module(specifier, from_file)
    }

    /// Reads a build input ([`SetupBackend::read_input`]).
    pub fn read_input(&mut self, path: impl AsRef<Path>) -> io::Result<Option<String>> {
        self.backend.read_input(path.as_ref())
    }

    /// Reads an environment variable as a build input ([`SetupBackend::env`]).
    pub fn env(&mut self, name: &str) -> Option<String> {
        self.backend.env(name)
    }

    /// The kind of build ([`SetupBackend::build_kind`]).
    pub fn build_kind(&self) -> BuildKind {
        self.backend.build_kind()
    }

    /// Whether every file is transformed this build ([`SetupBackend::transforms_all_files`]).
    pub fn transforms_all_files(&self) -> bool {
        self.backend.transforms_all_files()
    }

    /// The changes since the last build ([`SetupBackend::changes`]).
    pub fn changes(&self) -> Option<&ChangeSet> {
        self.backend.changes()
    }

    /// Where the handed-in state came from ([`SetupBackend::state_origin`]).
    pub fn state_origin(&self) -> StateOrigin {
        self.backend.state_origin()
    }

    /// Reads a file the plugin persisted itself ([`SetupBackend::read_persisted`]).
    pub fn read_persisted(&mut self, path: impl AsRef<Path>) -> io::Result<Option<Vec<u8>>> {
        self.backend.read_persisted(path.as_ref())
    }

    /// A verbose message ([`SetupBackend::info`]).
    pub fn info(&mut self, message: &str) {
        self.backend.info(message);
    }

    /// A warning ([`SetupBackend::warn`]).
    pub fn warn(&mut self, message: &str, hint: Option<&str>) {
        self.backend.warn(message, hint);
    }

    /// Matches a glob under the absolute directory `root`, recorded as a build input: in watch, a change in the
    /// matches runs setup again ([`SetupBackend::record_glob`]). `pattern` names the glob: one pattern, one matcher.
    pub fn record_glob(
        &mut self,
        root: impl AsRef<Path>,
        pattern: &str,
        matcher: impl GlobMatcher,
    ) -> Vec<String> {
        self.backend
            .record_glob(root.as_ref(), pattern, Arc::new(matcher))
    }
}
