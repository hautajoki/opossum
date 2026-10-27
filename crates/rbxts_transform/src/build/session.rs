//! [`BuildSession`]: the build lifecycle of DESIGN §14 over one project, for one build (`rbxtsc build`) or many
//! (watch mode, the parity harness).
//!
//! A build is `build(kind, changes)`:
//! 1. **Load the program** and the project setup, as roblox-ts does every build: module resolution, the Rojo
//!    resolvers and the package mapping are redone (a package installed or upgraded under `node_modules`, which no
//!    watcher sees, is picked up). Unchanged files come from the session's [`SourceFileCache`] (the same
//!    `SourceFile` objects), so only edited files are parsed again. A watch rebuild reuses the last load instead when
//!    every read it made (files, texts, listings, links, the project setup's included) reads the same now
//!    ([`ReadLog::recheck`](rbxts_project::ReadLog::recheck)): the program is made again from the last one with the
//!    edited files parsed again (tsrs's `reuse_program`), and the setup stands. Exact: a load is a function of
//!    what it read. (`reuse_program` alone, without the check, kept stale module resolutions: an import that
//!    failed to resolve stayed unresolved after its package was installed.)
//! 2. **Decide the files to transform** ([`super::graph`]): every project file in a clean build, after a change of
//!    the salt (compiler, options, configs, Rojo files, plugin list), after a plugin's `setup` re-ran or read a
//!    changed build input, after a plugin state file the last commit left went missing or changed (its plugin
//!    starts from a fresh state, without a kept `Shared`: as in a new process); otherwise roblox-ts's closure with TypeScript's global rule and the augmentation edges,
//!    plus the project files whose output is missing, the files without a cached part for every plugin (an
//!    incremental build brings the parts the plugins encoded, [`super::state`]), the files whose recorded
//!    `Cx::env` reads differ (a new process) or whose recorded `Cx::record_glob` answers differ (watch; a new
//!    process has no matcher and transforms them all) ([`super::plugin_inputs`]).
//! 3. **Check** those files (the others are unchanged since a build without errors).
//! 4. **Plugins:** per plugin, `rebuild_start` on a watch rebuild with a kept `Shared` and no changed build input, or
//!    `setup` on a working copy of the committed state (`default_state()` for clean and incremental builds).
//!    `rebuild_start` runs before the check, as the files to check depend on its answer.
//! 5. **Transform** the files; 6. plugin **`finish`** with the parts of all current files (cached for the files
//!    not transformed); 7. **render**, and re-render kept files whose late strings changed value;
//! 8. **commit** (no errors only): Luau and assets, artifacts, deletions (stale outputs, artifacts of the last build
//!    not written by this one, removals, the directories those stale artifacts emptied, `remove_empty_dir`s),
//!    plugin state files, then the incremental state (with each file's encoded parts and the state files' versions);
//!    9. the working states and `Shared`s become the committed ones, with every file's facts, parts, late values and
//!    recorded inputs, and the versions of the state files the commit left. A state file setup read as a build
//!    input is recorded with the committed bytes, unless the commit changed whether it reads as text (it was
//!    missing): then the read stays, the next build runs that setup again, and
//!    [`BuildSummary::setup_inputs_changed`] tells a watcher to start that build at once.
//!
//! A cached file whose late strings change value is rendered again from its kept Luau; one with no Luau kept (its
//! parts came from the incremental state) makes the build start over once, transforming it.
//!
//! A build that does not commit leaves the committed state as it was: the next build diffs against the last
//! committed one, re-runs `setup` where this one made a new `Shared`, and gets the changes of both.

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_luau::{LateStrings, Stmt};
use rbxts_plugin::erased::{ErasedPart, ErasedShared, ErasedState};
use rbxts_plugin::{BuildKind, ChangeSet, FileIndex, RebuildPlan, StateOrigin};
use rbxts_project::read_check::{ProgramStamp, Recheck, Settled};
use rbxts_project::{LoadOptions, Project, ProjectDiagnostic, ProjectServices, SourceFileCache, nodepath};
use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_compiler::{Context, Program};
use tsrs_core::P;
use tsrs_diagnostics::Category;

use super::checker_groups::{self, GroupPlugins};
use super::graph::{self, FileFacts, Version};
use super::outputs::{self, OutputInputs};
use super::references::GraphContext;
use super::plugin_inputs::{
    DirRules, DiskInput, EnvSnapshot, FileInputs, FinishRead, InputWatch, RecordedGlob, plain, remove_emptied_dirs,
    remove_queued_dirs,
};
use super::state::{self, DiskState, FileRecord, StateFile};
use super::transform_order::transform_order;
use crate::classes::diagnostic_service::drop_reported_earlier;
use crate::compile::{
    CompileOptions, CompileReport, FileOutcome, comment_directive_diagnostics, explain_failures, output_path,
    panic_message, quietly, record_timings,
};
use crate::plugins::host::{HookTime, resolve_module_from};
use crate::plugins::lifecycle::{
    ActivePlugin, BuildInput, CommitQueue, Finish, PluginOptions, Setup, activate, commit_artifacts, commit_removals,
    commit_state_files, output_key, plain_late_key, run_finish, run_setup,
};
use crate::plugins::spill::LoadCheck;
use crate::plugins::{HookKind, PluginHost};
use crate::project::copy_include::{IncludeCopy, prepare_copy_include};
use crate::project::emit_declarations::emit_declarations;
use crate::project::transformers::transform_paths::TransformPaths;
use crate::shared::diagnostics::{internal_error, text_diagnostic};

/// A program and its project setup, kept for the next build.
struct Loaded {
    project: Project,
    services: ProjectServices,
    setup_diagnostics: Vec<ProjectDiagnostic>,
    setup_fatal: Option<String>,
    /// The tsconfig and the configs it extends.
    config_paths: Vec<String>,
    /// The salt without the plugin list: compiler, options, config and Rojo texts.
    base_salt: String,
    /// Text versions by file object, across every program of the session.
    versions: FxHashMap<P<SourceFile>, Version>,
    /// What the checks of this load's reads found settled (`read_check`), kept across the rebuilds that reuse it.
    settled: Settled,
}

/// A committed file: its facts and, for project files, what the plugins and the late-string re-render need.
struct CommittedFile {
    facts: FileFacts,
    /// Per active plugin: the file's part (project files only).
    parts: Vec<Option<ErasedPart>>,
    /// (plugin, namespaced key, committed value) of each late string the file's Luau uses.
    late: Vec<(usize, String, String)>,
    /// The file's Luau, kept in watch mode when it uses late strings (for the re-render).
    ast: Option<Vec<Stmt>>,
    /// Its load-check mark, kept with `ast` when a plugin emitted into it: a late string's new value can decide whether
    /// Luau accepts the re-rendered file (a folded comparison, a dead branch).
    load_check: Option<LoadCheck>,
    /// What its hooks read through `Cx::env` and `Cx::record_glob`.
    inputs: FileInputs,
    /// Per active plugin, the part's encoded bytes (made for the incremental state, or read from it).
    encoded: Option<Vec<Option<Vec<u8>>>>,
    /// Its last transform's diagnostics (warnings: a file with an error never commits), reported again by a
    /// rebuild that does not transform it, as a clean build would report them (made again from the incremental
    /// state in a new process: [`super::replay`]). As the transform reported them, before the build dropped plugin
    /// duplicates (`compile::drop_plugin_duplicates`).
    diagnostics: Vec<P<Diagnostic>>,
    /// Which of `diagnostics` plugins reported.
    plugin_diagnostics: Vec<P<Diagnostic>>,
    /// Its diagnostics are known: false for a file of the incremental state whose diagnostics could not be kept
    /// there, which is transformed again.
    replayable: bool,
}

impl CommittedFile {
    fn new(facts: FileFacts) -> CommittedFile {
        CommittedFile {
            facts,
            parts: Vec::new(),
            late: Vec::new(),
            ast: None,
            load_check: None,
            inputs: FileInputs::default(),
            encoded: None,
            diagnostics: Vec::new(),
            plugin_diagnostics: Vec::new(),
            replayable: true,
        }
    }

    /// A file of the incremental state. It keeps its parts when every plugin's decodes, its `Cx::env` reads read
    /// the same and it recorded no glob (a glob's matcher is not persisted); otherwise it has none, so it is
    /// transformed again.
    /// `file` is the program's file of that path, if any, in which its kept diagnostics are made again.
    fn from_disk(
        facts: FileFacts,
        record: FileRecord,
        (active, env): (&[ActivePlugin<'_>], &EnvSnapshot),
        (file, program): (Option<P<SourceFile>>, &Program),
    ) -> CommittedFile {
        let inputs = FileInputs {
            env: record.env,
            globs: record
                .globs
                .into_iter()
                .map(|(plugin, root, pattern, answer)| RecordedGlob { plugin, root, pattern, matcher: None, answer })
                .collect(),
        };
        let reusable = !active.is_empty()
            && record.parts.len() == active.len()
            && inputs.globs.is_empty()
            && !inputs.env_changed(env);
        let parts: Vec<Option<ErasedPart>> = if reusable {
            let decoded: Option<Vec<ErasedPart>> = active
                .iter()
                .zip(&record.parts)
                .map(|(plugin, bytes)| plugin.plugin.decode_file_part(bytes.as_deref()?))
                .collect();
            decoded.map(|parts| parts.into_iter().map(Some).collect()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let encoded = (!parts.is_empty()).then_some(record.parts);
        let mut replayable = !record.no_replay && (file.is_some() || record.diagnostics.is_empty());
        let mut diagnostics = Vec::new();
        let mut plugin_diagnostics = Vec::new();
        if let Some(file) = file {
            for kept in &record.diagnostics {
                // a diagnostic at a file the program no longer has: the file is transformed again
                let Some(d) = super::replay::from_disk(kept, file, program) else {
                    replayable = false;
                    break;
                };
                if kept.plugin {
                    plugin_diagnostics.push(d);
                }
                diagnostics.push(d);
            }
        }
        CommittedFile {
            facts,
            parts,
            late: record.late,
            ast: None,
            load_check: None,
            inputs,
            encoded,
            diagnostics,
            plugin_diagnostics,
            replayable,
        }
    }
}

/// A plugin's committed state, and the `Shared` made from it with the inputs its `setup` read.
struct PluginRecord {
    name: &'static str,
    state: ErasedState,
    shared: Option<ErasedShared>,
    inputs: Vec<BuildInput>,
}

/// What the last committed build left: in memory (watch), or read from the incremental state.
struct Committed {
    salt: Version,
    plugin_ids: Vec<(String, String)>,
    files: FxHashMap<String, CommittedFile>,
    plugins: Vec<PluginRecord>,
    artifacts: Vec<String>,
    /// The directories plugins asked to watch (`watch_dir`).
    watch_dirs: Vec<String>,
    /// Read from the incremental state: no plugin states, parts only where the plugins encoded them.
    from_disk: bool,
    /// Read from the incremental state: what the plugins' `setup` read.
    setup_inputs: Option<Vec<DiskInput>>,
    /// The plugin state files the build committed, with the version of the bytes it left on disk.
    state_files: Vec<StateFile>,
    /// Every output the build left (Luau, declarations and their maps, transformed files, assets): one gone since
    /// makes the project dirty in a build with project references ([`BuildSession::needs_build`]).
    outputs: Vec<String>,
    /// By path key: each output's size and content hash as the build left it. An output changed or deleted
    /// since (by hand) has its source transformed again ([`outputs::Listing::intact`]), as a clean build would
    /// write it; a watcher counts an event on one that leaves it otherwise ([`BuildSession::committed_outputs`]).
    output_stamps: FxHashMap<String, outputs::Stamp>,
}

/// One project's builds: the committed state, the parsed files and the last program, across builds.
pub struct BuildSession<'p> {
    /// What every build of the session compiles (tsconfig, directory, options).
    pub options: CompileOptions,
    plugins: &'p PluginOptions<'p>,
    /// Keep what watch rebuilds need in memory (the Luau of files using late strings).
    watch: bool,
    cache: Arc<SourceFileCache>,
    last: Option<Loaded>,
    committed: Option<Committed>,
    /// Paths touched since the last committed build.
    pending: BTreeSet<String>,
    /// Watch rebuilds started (the generation of the next one is this plus one).
    rebuilds: u64,
    /// Plugin states for a clean build (the harness's seeded clean build).
    seed: Option<Vec<(&'static str, ErasedState)>>,
    /// The incremental state was looked at (only the first build reads it).
    disk_checked: bool,
    /// What the last load read (recorded in watch sessions only), kept when the load failed.
    reads: Option<Arc<rbxts_project::ReadLog>>,
    /// What the last build that ran `finish` read through `FinishCx` (globs, listings, existence, files).
    finish_reads: Vec<FinishRead>,
    /// The last build ran `finish` (`finish_reads` are its own).
    finish_ran: bool,
    /// Cached files to transform in the build's second run (late strings changed value, no Luau kept).
    retransform: FxHashSet<String>,
    /// The invalid `RBXTS_CHECKERS` warning was given: a session gives it once, not on every watch rebuild.
    checkers_warned: bool,
    /// What the project graph tells this project's builds, in a build with project references (set before each).
    pub(super) graph: Option<GraphContext>,
    /// The sessions of the projects the root of a graph references, in build order, by graph key.
    pub(super) references: Vec<(String, BuildSession<'p>)>,
    /// A referenced project's session (its graph's root builds it).
    pub(super) is_reference: bool,
    /// The config paths of the last project graph made (watched).
    pub(super) graph_config_paths: Vec<String>,
    /// In a build with project references: what this project's last build reported, replayed while it is not
    /// dirty (roblox-ts keeps a clean project's last diagnostics); None until a build succeeded.
    pub(super) replay: Option<Replay>,
    /// In a build with project references: the next build must build this project (its last build failed, or it
    /// was blocked by a reference that did).
    pub(super) must_build: bool,
}

/// A project's last report, as a build that skips it reports it again.
#[derive(Clone, Default)]
pub(super) struct Replay {
    /// Its diagnostics, the transform's included.
    pub diagnostics: Vec<P<Diagnostic>>,
    pub warnings: Vec<String>,
}

impl Replay {
    /// The report's diagnostics, copied out of the program's checker regions (`detach`): a project's program is freed
    /// once a later build of it replaces it, and a skipped project replays them after that.
    pub(super) fn of(report: &CompileReport) -> Replay {
        let mut diagnostics = report.diagnostics.clone();
        for file in &report.files {
            diagnostics.extend(file.diagnostics.iter().copied());
        }
        let (diagnostics, _) = detach(diagnostics, Vec::new());
        Replay { diagnostics, warnings: report.warnings.clone() }
    }

    pub(super) fn report(&self) -> CompileReport {
        CompileReport { diagnostics: self.diagnostics.clone(), warnings: self.warnings.clone(), ..CompileReport::default() }
    }
}

/// What one build decided, for reports and events.
#[derive(Clone, Debug, Default)]
pub struct BuildSummary {
    /// The kind of build (clean, incremental, watch rebuild), once decided.
    pub kind: Option<BuildKind>,
    /// Every project file was transformed.
    pub all: bool,
    /// Files parsed for this build (source cache misses).
    pub parsed: usize,
    /// The build reused the last load (`reuse`): nothing it read read differently, and the program was made again
    /// from the last one with the edited files parsed again.
    pub reused_load: bool,
    /// Project files re-rendered because their late strings changed value.
    pub rerendered: Vec<String>,
    /// Outputs written (Luau, assets, artifacts, state files) and deleted.
    pub written: Vec<String>,
    /// Outputs and artifacts deleted.
    pub deleted: Vec<String>,
    /// The changes since the last committed build.
    pub changes: ChangeSet,
    /// State files the last committed build left that were missing or changed before this one: their plugins
    /// started from a fresh state (no kept `Shared`), and every file was transformed.
    pub lost_state_files: Vec<String>,
    /// Plugin state files whose commit changed what a plugin's `setup` read as a build input: setup read one as
    /// missing (or not as text) and this build left text in it, or the other way round. The plugin's `Shared` may
    /// depend on that (Flamework's candidate scan loads an ancestor `flamework.build` while its own is missing), so
    /// the read is kept as it was and the next build runs that setup again; a watcher starts that build at once.
    /// New bytes alone are not one: a `Shared` never depends on its own state file's bytes (`Plugin::Shared`). For
    /// another plugin's state file, they are.
    pub setup_inputs_changed: Vec<String>,
}

impl<'p> BuildSession<'p> {
    /// A session over one project. `watch` keeps the Luau of files with late strings for later re-renders.
    pub fn new(options: CompileOptions, plugins: &'p PluginOptions<'p>, watch: bool) -> BuildSession<'p> {
        BuildSession {
            options,
            plugins,
            watch,
            cache: Arc::new(SourceFileCache::new()),
            last: None,
            committed: None,
            pending: BTreeSet::new(),
            rebuilds: 0,
            seed: None,
            disk_checked: false,
            reads: None,
            finish_reads: Vec::new(),
            finish_ran: false,
            retransform: FxHashSet::default(),
            checkers_warned: false,
            graph: None,
            references: Vec::new(),
            is_reference: false,
            graph_config_paths: Vec::new(),
            replay: None,
            must_build: true,
        }
    }

    /// A session for another project of this one's graph, with the same plugins and mode.
    pub(super) fn child(&self, options: CompileOptions) -> BuildSession<'p> {
        BuildSession::new(options, self.plugins, self.watch)
    }

    /// Starts the first (clean) build from these plugin states instead of `default_state()`: the harness's clean
    /// build seeded with a watcher's persistent state.
    pub fn with_seed(mut self, states: Vec<(&'static str, ErasedState)>) -> BuildSession<'p> {
        self.seed = Some(states);
        self.disk_checked = true;
        self
    }

    /// The committed plugin states, by plugin name (to seed a clean build with).
    pub fn committed_states(&self) -> Vec<(&'static str, ErasedState)> {
        self.committed.as_ref().map_or_else(Vec::new, |c| c.plugins.iter().map(|p| (p.name, p.state.clone())).collect())
    }

    /// A build has committed in this session (or the incremental state was read).
    pub fn has_committed(&self) -> bool {
        self.committed.is_some()
    }

    /// Forgets the committed state: the next build is a clean one.
    pub fn reset(&mut self) {
        self.committed = None;
        self.last = None;
        self.pending.clear();
        self.disk_checked = true;
    }

    /// The sessions of the projects this one references, in build order (a build with project references): a
    /// watcher watches their sources and inputs too.
    pub fn reference_sessions(&self) -> impl Iterator<Item = &BuildSession<'p>> {
        self.references.iter().map(|(_, session)| session)
    }

    /// Every output the last committed build left (Luau, declarations, transformed files, assets), with its size and
    /// modification time then: a watcher counts an event that leaves one otherwise (changed or deleted by hand), and
    /// the next build writes it again.
    pub fn committed_outputs(&self) -> Vec<(String, outputs::Stamp)> {
        let (Some(committed), Some(last)) = (&self.committed, &self.last) else { return Vec::new() };
        committed
            .outputs
            .iter()
            .filter_map(|path| {
                let stamp = committed.output_stamps.get(&path_key_of(&last.services, path))?;
                Some((path.clone(), *stamp))
            })
            .collect()
    }

    /// The output roots of the last loaded project: its outDir (or the override) and its `declarationDir`.
    pub fn output_roots(&self) -> Vec<String> {
        let Some(last) = &self.last else { return Vec::new() };
        outputs::output_roots(&last.services, &last.project.program.options(), self.options.out_dir_override.as_deref())
    }

    /// The tsconfig and `extends` files, the Rojo project files (the project's and the package resolvers') and the
    /// plugin build inputs of the last loaded build, and every config of its project graph: what a watcher watches
    /// besides the sources.
    pub fn config_inputs(&self) -> Vec<String> {
        let mut inputs = self.graph_config_paths.clone();
        if let Some(last) = &self.last {
            inputs.extend(last.config_paths.iter().cloned());
            inputs.extend(last.services.rojo_resolver.config_files().iter().cloned());
            for resolver in &last.services.pkg_rojo_resolvers {
                inputs.extend(resolver.config_files().iter().cloned());
            }
        }
        if let Some(committed) = &self.committed {
            for plugin in &committed.plugins {
                for input in &plugin.inputs {
                    if let BuildInput::File { path, .. } = input {
                        inputs.push(path.clone());
                    }
                }
            }
        }
        inputs
    }

    /// The module resolutions plugins asked for in `setup` (`SetupCx::resolve_module`): (specifier, from file,
    /// resolved file). They are build inputs: a watcher watches the package directories they depend on, and a
    /// changed result runs `setup` again.
    pub fn plugin_resolutions(&self) -> Vec<(String, String, Option<String>)> {
        let mut out = Vec::new();
        if let Some(committed) = &self.committed {
            for plugin in &committed.plugins {
                for input in &plugin.inputs {
                    if let BuildInput::Resolution { specifier, from, resolved, .. } = input {
                        out.push((specifier.clone(), from.clone(), resolved.clone()));
                    }
                }
            }
        }
        out
    }

    /// The bare module specifiers the last loaded program could not resolve (packages not installed yet), for a
    /// watcher to watch where they would appear.
    pub fn unresolved_packages(&self) -> Vec<String> {
        let Some(last) = &self.last else { return Vec::new() };
        let mut names: Vec<String> = last
            .project
            .program
            .get_resolved_modules()
            .values()
            .flat_map(|cache| cache.iter())
            .filter(|(key, resolved)| {
                !resolved.is_resolved() && !key.name.starts_with('.') && !key.name.starts_with('/')
            })
            .map(|(key, _)| key.name.to_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// What the last program load read from the file system (the tsconfig parse and the program: files, directories
    /// and the paths module resolution probed that did not exist), for a watcher; also when that load failed (a
    /// tsconfig extending a package not installed yet).
    pub fn read_log(&self) -> Option<Arc<rbxts_project::ReadLog>> {
        self.reads.clone()
    }

    /// The last program's files on disk, each with the modification time and size it had when parsed (a watcher
    /// compares them with the disk for directories it starts watching).
    pub fn loaded_stamps(&self) -> Vec<(String, Option<std::time::SystemTime>, i64)> {
        let Some(last) = &self.last else { return Vec::new() };
        last.project
            .program
            .source_files()
            .iter()
            .filter_map(|f| {
                let file = f.get();
                let (time, size) = self.cache.stamp(file.path())?;
                Some((file.file_name().to_owned(), time, size))
            })
            .collect()
    }

    /// What the plugins' recorded inputs ask a watcher to watch: the roots of the globs the files recorded and
    /// `setup` recorded (the last committed build's), and what the last `finish` read (its globs' roots, the
    /// directories it listed, the paths it tested or read).
    pub fn recorded_input_watches(&self) -> Vec<InputWatch> {
        let mut watches = BTreeSet::new();
        for glob in self.recorded_globs() {
            watches.insert(InputWatch::Tree(glob.root.clone()));
        }
        for read in &self.finish_reads {
            watches.insert(read.watch());
        }
        watches.into_iter().collect()
    }

    /// Whether a recorded input answers differently now: a glob of a file (that file is transformed again) or of
    /// `setup` (a changed build input), or a read of the last `finish` (it runs again). With `paths` (events a
    /// watcher saw), only the inputs one of them may concern are asked (a glob: under its root and accepted by its
    /// matcher's `affected_by`, or on the way to its root); with `None`, every input.
    pub fn recorded_inputs_changed(&self, paths: Option<&[String]>) -> bool {
        let paths: Option<Vec<String>> = paths.map(|paths| paths.iter().map(|p| plain(p)).collect());
        let concerned =
            |test: &dyn Fn(&str) -> bool| paths.as_ref().is_none_or(|paths| paths.iter().any(|p| test(p)));
        self.recorded_globs().any(|glob| concerned(&|p| glob.concerned_by(p)) && glob.changed())
            || self.finish_reads.iter().any(|read| concerned(&|p| read.concerned_by(p)) && read.changed())
    }

    /// Every recorded input asked again after a build (for a watcher's new watches: what changed before them raised
    /// no event): the globs, and the reads of `finish` when that build ran it. A build that stopped before `finish`
    /// (a type error) kept the last finish's reads, which a change made since answers differently at every build
    /// until one runs `finish` again; the build that gets that far runs it anyway, and only a change of its own
    /// (an event) lets a build get there.
    pub fn recorded_inputs_changed_since_build(&self) -> bool {
        if self.finish_ran {
            self.recorded_inputs_changed(None)
        } else {
            self.recorded_globs().any(RecordedGlob::changed)
        }
    }

    /// The globs of the last committed build's files and setup that have their matcher.
    fn recorded_globs(&self) -> impl Iterator<Item = &RecordedGlob> {
        let committed = self.committed.as_ref();
        let files = committed.into_iter().flat_map(|c| c.files.values()).flat_map(|f| &f.inputs.globs);
        let setup = committed.into_iter().flat_map(|c| &c.plugins).flat_map(|p| &p.inputs).filter_map(|input| match input {
            BuildInput::Glob { glob, .. } => Some(glob),
            _ => None,
        });
        files.chain(setup).filter(|glob| glob.matcher.is_some())
    }

    /// The places the last loaded project's Rojo resolvers mount (`$path` folders, missing ones included; the type
    /// roots for the package resolvers): they search these folders for project files, so one created or removed
    /// there changes them.
    pub fn rojo_partitions(&self) -> Vec<String> {
        let Some(last) = &self.last else { return Vec::new() };
        std::iter::once(&last.services.rojo_resolver)
            .chain(&last.services.pkg_rojo_resolvers)
            .flat_map(|resolver| resolver.partitions().iter().map(|p| p.fs_path.clone()))
            .collect()
    }

    /// The runtime include copy's inputs, for a watcher: the package.json paths the include source lookup looks at
    /// (`node_modules/roblox-ts` from the project folder upwards), and the include source folder when it is a
    /// package's (every file in it is copied). None when the last loaded project gets no include copy.
    pub fn include_inputs(&self) -> Option<(Vec<String>, Option<String>)> {
        use crate::project::copy_include::{IncludeSource, include_source_lookup, wants_include};
        let data = &self.last.as_ref()?.services.data;
        if !wants_include(&data.options, data.is_package) {
            return None;
        }
        let (probed, source) = include_source_lookup(&data.project_path);
        let source = match source {
            IncludeSource::Package(dir) => Some(dir),
            IncludeSource::Embedded => None,
        };
        Some((probed, source))
    }

    /// Whether a file would be one of the last loaded project's root files (its tsconfig's `files` and `include`).
    pub fn include_matcher(&self) -> Option<rbxts_project::IncludeMatcher> {
        self.last.as_ref().map(|last| last.project.include_matcher())
    }

    /// The directories plugins asked the last committed build to watch (`FinishCx::watch_dir`).
    pub fn plugin_watch_dirs(&self) -> Vec<String> {
        self.committed.as_ref().map_or_else(Vec::new, |c| c.watch_dirs.clone())
    }

    /// The plugin state files the last committed build left, with their bytes' versions: a watcher counts an event
    /// on one only when the file no longer holds them (its own write never does), and the next build checks them
    /// ([`BuildSummary::lost_state_files`]).
    pub fn state_files(&self) -> &[StateFile] {
        self.committed.as_ref().map_or(&[], |c| c.state_files.as_slice())
    }

    /// The last loaded project's services, program file names and root directories (for a watcher's directory
    /// set).
    pub fn watch_roots(&self) -> Option<(&ProjectServices, Vec<String>, Vec<String>)> {
        let last = self.last.as_ref()?;
        let files = last.project.program.source_files().iter().map(|f| f.get().file_name().to_owned()).collect();
        let options = last.project.program.options();
        let mut roots = rbxts_project::setup::get_root_dirs(&options);
        if roots.is_empty() {
            roots.push(last.services.data.project_path.clone());
        }
        Some((&last.services, files, roots.iter().map(|r| nodepath::resolve(&[r])).collect()))
    }

    /// One build. `touched`: the paths a watcher saw change since the last build (empty for the first build). A
    /// tsconfig with project references builds its graph: each referenced project in build order, then this one
    /// ([`super::references`]).
    ///
    /// The report's diagnostics live until the session's next build: a watch session frees the program they were
    /// made in once a later build replaces it.
    pub fn build(&mut self, touched: &[String]) -> (CompileReport, BuildSummary) {
        if !self.is_reference && rbxts_project::graph::has_references(&self.tsconfig_path()) {
            return super::references::build_graph(self, touched);
        }
        if !self.is_reference {
            self.graph = None;
            self.references.clear();
            self.graph_config_paths.clear();
        }
        self.build_project(touched)
    }

    /// One build of this project alone (with the graph's facts when it is part of one).
    pub(super) fn build_project(&mut self, touched: &[String]) -> (CompileReport, BuildSummary) {
        let rebuilds = self.rebuilds;
        self.finish_ran = false;
        let committed = self.committed.take();
        let last = self.last.take();
        // the programs this build replaces: the last build's, and the first run's when the build starts over
        let mut replaced: Vec<Project> = Vec::new();
        let (mut report, mut summary, mut committed, mut last) = self.run(touched, committed, last, &mut replaced);
        if !self.retransform.is_empty() {
            // cached files whose late strings changed value with no Luau kept to render again (their parts came from
            // the incremental state): the build starts over once, transforming them
            self.rebuilds = rebuilds;
            (report, summary, committed, last) = self.run(&[], committed, last, &mut replaced);
            self.retransform.clear();
        }
        self.committed = committed;
        self.last = last;
        self.free_replaced(replaced);
        (report, summary)
    }

    /// Frees the programs a watch build replaced (the memory of a long watch stays flat). A session that does not
    /// watch keeps them (its programs have no regions to give back: one build, or the harness's clean builds).
    fn free_replaced(&self, replaced: Vec<Project>) {
        if !self.watch {
            return;
        }
        for project in replaced {
            // SAFETY: nothing reads the replaced program or what was made under its checkers again. The session
            // keeps no handle into it: the committed state holds copies of the diagnostics it keeps (`detach`), the
            // project setup and file facts are owned data, and the parsed files are the source cache's (bound before
            // any checker existed: `LoadOptions::regions`). The plugins' `Shared`, `State` and file parts hold no
            // program handles (the `rbxts_plugin` contract), and their instances lived for one build. Its checkers are
            // released: the build that used them is over. The caller's report of the build before this one is no
            // longer used (a report lives until the session's next build, `BuildSession::build`).
            unsafe { project.free() };
        }
    }

    /// Whether this project of a build with project references is dirty (roblox-ts's `ProjectBuild.build`):
    /// a first build (no paths touched), its last build failed or was blocked, an output its last build left is gone,
    /// or one of `touched` concerns it: under its root dirs, one of its program files, configs (Rojo files and plugin
    /// inputs included) or plugin state files, under a plugin's watched directory, a recorded plugin input that
    /// answers differently, or a path its last program load read or probed (or in a directory it listed, or above
    /// one: a file new in a folder its tsconfig includes, as the config parse listed it). rbxts-rs: also any path under a
    /// `node_modules` and any Rojo project file, which roblox-ts leaves to its watcher. A project its references
    /// make dirty is dirty too (the caller's).
    pub(super) fn needs_build(&self, touched: &[String]) -> bool {
        if touched.is_empty() || self.must_build || self.replay.is_none() {
            return true;
        }
        let Some(committed) = &self.committed else { return true };
        let Some(last) = &self.last else { return true };
        let services = &last.services;
        // an output the last build left, gone or changed since (by hand)
        let changed = |output: &String| match committed.output_stamps.get(&path_key_of(services, output)) {
            Some(stamp) => stamp.check(output, outputs::Found::at(output)).is_none(),
            None => std::fs::symlink_metadata(output).is_err(),
        };
        if committed.outputs.iter().any(changed) {
            return true;
        }
        let key = |path: &str| path_key_of(services, path);
        let Some((_, files, roots)) = self.watch_roots() else { return true };
        if self.recorded_inputs_changed(Some(touched)) {
            return true;
        }
        let under = |path: &str, root: &str| outputs::is_descendant_key(path, root);
        let roots: Vec<String> =
            roots.iter().map(|r| key(r)).chain(self.plugin_watch_dirs().iter().map(|d| key(d))).collect();
        let mut exact: FxHashSet<String> = files.iter().map(|f| key(f)).collect();
        exact.extend(self.config_inputs().iter().map(|c| key(c)));
        exact.extend(self.state_files().iter().map(|s| key(&s.path)));
        let reads: Vec<String> = self
            .reads
            .as_ref()
            .map(|log| log.entries().into_iter().map(|(path, _)| key(&path)).collect())
            .unwrap_or_default();
        touched.iter().any(|path| {
            let k = key(path);
            k.contains("/node_modules/")
                || k.ends_with(".project.json")
                || exact.contains(&k)
                || roots.iter().any(|root| under(&k, root))
                || reads.iter().any(|read| under(read, &k) || nodepath::dirname(&k) == read)
        })
    }

    pub(super) fn tsconfig_path(&self) -> String {
        if nodepath::is_absolute(&self.options.tsconfig) {
            self.options.tsconfig.clone()
        } else {
            nodepath::join(&[&self.options.cwd, &self.options.tsconfig])
        }
    }

    /// Loads the program and the project setup.
    fn load(&mut self, report: &mut CompileReport, versions: FxHashMap<P<SourceFile>, Version>) -> Option<Loaded> {
        self.reads = self.watch.then(|| Arc::new(rbxts_project::ReadLog::default()));
        let project = match rbxts_project::load_cached(
            &LoadOptions {
                tsconfig: self.options.tsconfig.clone(),
                cwd: self.options.cwd.clone(),
                checkers: self.options.checker_count(),
                lazy_dts: false,
                regions: self.watch,
            },
            Some(&self.cache),
            self.reads.clone(),
        ) {
            Ok(project) => project,
            Err(diagnostics) => {
                report.diagnostics = diagnostics;
                report.fatal.push("the tsconfig could not be used".to_owned());
                return None;
            }
        };
        report.timings.config = project.config_time;
        report.timings.program = project.program_time;
        let start = Instant::now();
        let tsconfig_path = self.tsconfig_path();
        let setup = match &self.graph {
            Some(graph) => project.setup_with_data(graph.data.clone()),
            None => project.setup(&tsconfig_path, &self.options.project_options),
        };
        let setup = match setup.map(|mut setup| {
            if let Some(graph) = &self.graph {
                setup.services.project_reference_paths = graph.reference_paths.clone();
            }
            setup
        }) {
            Ok(setup) => setup,
            Err(error) => {
                report.fatal = error.messages;
                report.timings.setup = start.elapsed();
                return None;
            }
        };
        report.timings.setup = start.elapsed();
        let mut config_paths = vec![nodepath::normalize(&tsconfig_path)];
        config_paths.extend(project.config.extended_source_files().iter().map(|p| nodepath::normalize(p)));
        let base_salt = self.base_salt(&setup.services, &config_paths);
        Some(Loaded {
            project,
            services: setup.services,
            setup_diagnostics: setup.diagnostics,
            setup_fatal: setup.fatal,
            config_paths,
            base_salt,
            versions,
            settled: Settled::default(),
        })
    }

    /// A watch rebuild's load when nothing the last load read reads differently now ([`ReadLog::recheck`]): its
    /// tsconfig parse, module resolution and project setup stand, and the program is made again from the last one
    /// with the edited files parsed again (`Project::reuse`, tsrs's `reuse_program`). `Err` gives `last` back when the
    /// check or the reuse fails (a file's imports changed): the project is loaded again then.
    ///
    /// Exact: a load is a function of what it read (the log has every answer it got, the project setup's reads
    /// included), so a load with every answer the same, and the same files but the edited ones, is this one. Not in a
    /// build with project references (a project's setup there depends on the graph), and not outside watch mode (one
    /// build per session).
    fn reuse(&mut self, report: &mut CompileReport, mut last: Box<Loaded>) -> Result<(Loaded, Project), Box<Loaded>> {
        let Some(reads) = self.reads.clone().filter(|_| self.watch && self.graph.is_none()) else { return Err(last) };
        let start = Instant::now();
        let program: Vec<ProgramStamp> = last
            .project
            .program
            .source_files()
            .iter()
            .filter(|f| !f.get().file_name().starts_with("bundled:"))
            .map(|f| (f.get().file_name().to_owned(), self.cache.stamp(f.path())))
            .collect();
        let case_sensitive = last.services.use_case_sensitive_file_names;
        let changed = match reads.recheck(&program, case_sensitive, &mut last.settled) {
            Recheck::Same { changed } => changed,
            Recheck::Different(why) => {
                if std::env::var_os("RBXTS_WATCH_DEBUG").is_some() {
                    eprintln!("[rbxtsc watch] loading again: {why}");
                }
                return Err(last);
            }
        };
        let checked = start.elapsed();
        let Some(project) = last.project.reuse(&changed) else { return Err(last) };
        report.timings.config = checked;
        report.timings.program = project.program_time;
        report.timings.setup = std::time::Duration::ZERO;
        let Loaded { project: replaced, mut services, setup_diagnostics, setup_fatal, config_paths, base_salt, versions, settled } =
            *last;
        // a fresh package.json cache: the transform's lookups read the disk again, as a new load's do
        services.packages = Default::default();
        let loaded = Loaded {
            project,
            services,
            setup_diagnostics,
            setup_fatal,
            config_paths,
            base_salt,
            versions,
            settled,
        };
        Ok((loaded, replaced))
    }

    /// The salt's text before the plugin list: what roblox-ts salts every file version with (compiler version,
    /// options, Rojo texts), with the config texts for the compiler options.
    fn base_salt(&self, services: &ProjectServices, config_paths: &[String]) -> String {
        let mut salt = String::new();
        salt.push_str(&state::compiler_identity());
        salt.push('\n');
        salt.push_str(&format!("{:?}\n{:?}\n", services.data.options, self.options.out_dir_override));
        salt.push_str(&format!("{:?}\n", services.project_type));
        // createNodeModulesPathMapping reads every type-root package's package.json: a changed main or types entry
        // changes the requires of the files importing it, with no change to any program file
        let mut mapping: Vec<_> = services.node_modules_path_mapping.iter().collect();
        mapping.sort();
        salt.push_str(&format!("{mapping:?}\n"));
        // createProgramFactory salts with the project reference paths
        let mut references: Vec<_> = services.project_reference_paths.iter().collect();
        references.sort();
        salt.push_str(&format!("{references:?}\n"));
        let mut texts = |paths: &[String]| {
            for path in paths {
                salt.push_str(path);
                salt.push('\0');
                let version = std::fs::read(path).map(|b| Version::of(&String::from_utf8_lossy(&b)));
                salt.push_str(&version.map_or_else(|_| "-".to_owned(), Version::to_hex));
                salt.push('\n');
            }
        };
        texts(config_paths);
        texts(services.rojo_resolver.config_files());
        for resolver in &services.pkg_rojo_resolvers {
            texts(resolver.config_files());
        }
        salt
    }

    #[allow(clippy::type_complexity)]
    fn run(
        &mut self,
        touched: &[String],
        mut committed: Option<Committed>,
        last: Option<Loaded>,
        replaced: &mut Vec<Project>,
    ) -> (CompileReport, BuildSummary, Option<Committed>, Option<Loaded>) {
        let mut report = CompileReport::default();
        let mut summary = BuildSummary::default();
        self.pending.extend(touched.iter().cloned());
        // the second run of a build (`build`): these files are transformed whatever else decides
        let retrying = !self.retransform.is_empty();
        // one snapshot of the process environment for the build: setup and the hooks read the same values
        let env = Arc::new(EnvSnapshot::default());
        let parses_before = self.cache.parses();

        // 1. load the program, over the source cache
        // a watch rebuild reuses the last load when everything it read reads the same (`reuse`)
        let last = match last.map(|last| self.reuse(&mut report, Box::new(last))) {
            Some(Ok((loaded, project))) => {
                replaced.push(project);
                summary.reused_load = true;
                Ok(loaded)
            }
            Some(Err(last)) => Err(Some(*last)),
            None => Err(None),
        };
        let loaded = match last {
            Ok(loaded) => Some(loaded),
            Err(last) => {
                let versions = match last {
                    Some(Loaded { project, versions, .. }) => {
                        replaced.push(project);
                        versions
                    }
                    None => FxHashMap::default(),
                };
                self.load(&mut report, versions)
            }
        };
        let Some(loaded) = loaded else {
            summary.parsed = self.cache.parses() - parses_before;
            return (report, summary, committed, None);
        };
        summary.parsed = self.cache.parses() - parses_before;
        let mut loaded = loaded;
        let project = &loaded.project;
        let services = &loaded.services;
        report.program_files = project.program.source_files().len();
        report.warnings = services.warnings.clone();
        if !self.checkers_warned {
            let warning = self.options.checkers_warning();
            self.checkers_warned = warning.is_some();
            report.warnings.extend(warning);
        }
        for d in &loaded.setup_diagnostics {
            let category =
                if d.severity == rbxts_project::Severity::Error { Category::Error } else { Category::Warning };
            report.diagnostics.push(text_diagnostic(&d.message, category));
        }
        if let Some(fatal) = &loaded.setup_fatal {
            report.fatal.push(fatal.clone());
            return (report, summary, committed, Some(loaded));
        }
        if project.program.options().assume_changes_only_affect_direct_dependencies.is_true() {
            report.fatal.push(
                "assumeChangesOnlyAffectDirectDependencies is not supported: it leaves the outputs of indirect \
                 importers stale (rbxts-rs re-transforms every file that reaches a changed file)"
                    .to_owned(),
            );
            return (report, summary, committed, Some(loaded));
        }
        report.out_dir =
            self.options.out_dir_override.clone().unwrap_or_else(|| services.path_translator.out_dir.clone());
        let tsconfig_path = self.tsconfig_path();
        let active = match activate(self.plugins, &tsconfig_path) {
            Ok((active, warnings)) => {
                report.diagnostics.extend(warnings.iter().map(|w| text_diagnostic(w, Category::Warning)));
                active
            }
            Err(errors) => {
                report.fatal = errors;
                return (report, summary, committed, Some(loaded));
            }
        };
        report.plugins = active.iter().map(|p| p.info.name).collect();
        let plugin_ids: Vec<(String, String)> =
            active.iter().map(|p| (p.info.name.to_owned(), p.info.version.to_owned())).collect();
        let salt = {
            let mut text = loaded.base_salt.clone();
            for (plugin, (name, version)) in active.iter().zip(&plugin_ids) {
                text.push_str(&format!("plugin {name} {version} {:?}\n", plugin.entry));
            }
            Version::of(&text)
        };

        // the incremental state, for the first build of the session
        let state_path = self.state_path(services);
        if committed.is_none() && !self.disk_checked {
            self.disk_checked = true;
            if let Some(path) = &state_path
                && let Some(disk) = DiskState::read(path)
                && disk.compiler == state::compiler_identity()
                && disk.plugins == plugin_ids
            {
                let mut records = disk.records;
                let program = loaded.project.program;
                let by_path: FxHashMap<String, P<SourceFile>> =
                    program.source_files().iter().map(|&f| (f.path().to_string(), f)).collect();
                committed = Some(Committed {
                    salt: disk.salt,
                    plugin_ids: disk.plugins,
                    files: disk
                        .files
                        .into_iter()
                        .map(|(k, facts)| {
                            let record = records.remove(&k).unwrap_or_default();
                            let file = by_path.get(&k).copied();
                            let file = CommittedFile::from_disk(facts, record, (&active, &env), (file, program));
                            (k, file)
                        })
                        .collect(),
                    plugins: Vec::new(),
                    artifacts: disk.artifacts,
                    watch_dirs: Vec::new(),
                    from_disk: true,
                    setup_inputs: Some(disk.setup_inputs),
                    state_files: disk.state_files,
                    outputs: Vec::new(),
                    output_stamps: disk.outputs.into_iter().collect(),
                });
            }
        }
        let kind = match &committed {
            None => BuildKind::Clean,
            Some(c) if c.from_disk => BuildKind::Incremental,
            Some(_) => {
                self.rebuilds += 1;
                BuildKind::WatchRebuild { generation: self.rebuilds }
            }
        };
        summary.kind = Some(kind);

        // 2. the files to transform
        let invalidate_start = Instant::now();
        // the outputs on disk before anything is written: the missing-output check and the clean
        let listing = outputs::list(
            services,
            &outputs::output_roots(services, &project.program.options(), self.options.out_dir_override.as_deref()),
            self.options.out_dir_override.as_deref(),
        );
        let facts = graph::program_facts(project.program, &mut loaded.versions);
        let project = &loaded.project;
        let services = &loaded.services;
        let mut all = match &committed {
            None => true,
            Some(c) => c.salt != salt || c.plugin_ids != plugin_ids,
        };
        // a state file the last commit left that is missing or changed now (deleted or edited by hand or a tool):
        // its plugin starts over as in a new process (no kept `Shared`, a fresh state), and every file is
        // transformed, as the cached parts were made with the state it held
        let lost: Vec<StateFile> = committed
            .as_ref()
            .map_or_else(Vec::new, |c| c.state_files.iter().filter(|f| !f.intact()).cloned().collect());
        let fresh: FxHashSet<String> = lost.iter().map(|f| f.plugin.clone()).collect();
        if !lost.is_empty() {
            all = true;
            summary.lost_state_files = lost.iter().map(|f| f.path.clone()).collect();
            for file in &lost {
                report.plugin_messages.push(format!(
                    "{}: its state file {} is missing or changed since the last build: the plugin starts from a \
                     fresh state, and every file is transformed",
                    file.plugin, file.path
                ));
            }
        }
        let mut changes = ChangeSet::default();
        let mut affected = FxHashSet::default();
        if let Some(c) = &committed {
            let invalidation = graph::invalidate(&c.files, |f| &f.facts, &facts);
            all |= invalidation.all;
            affected = invalidation.affected;
            changes.changed = invalidation.changed;
            changes.added = invalidation.added;
            changes.deleted = invalidation.deleted;
        }
        let program_keys: FxHashSet<String> = facts.iter().map(|(_, f)| path_key_of(services, &f.file_name)).collect();
        let config_keys: FxHashSet<String> = self.config_inputs_of(&loaded, committed.as_ref());
        for path in &self.pending {
            let key = path_key_of(services, path);
            if config_keys.contains(&key) {
                changes.config_inputs.push(path.clone());
            } else if !program_keys.contains(&key) {
                changes.non_ts.push(path.clone());
            }
        }

        let resolver = OnceCell::new();
        // an incremental build: what the last process's setup read, read again (a glob's answer is compared once setup
        // recorded it anew, below)
        if let Some(inputs) = committed.as_ref().and_then(|c| c.setup_inputs.as_ref())
            && inputs.iter().any(|input| {
                input.changed_before_setup(&env, |specifier, from| {
                    resolve_module_from(project.program, &resolver, specifier, from).map(|r| r.file_name)
                })
            })
        {
            all = true;
        }
        // watch: the globs recorded by files and by setup, matched again after events under their roots
        if let Some(c) = &committed
            && !c.from_disk
            && !self.pending.is_empty()
        {
            let pending: Vec<String> = self.pending.iter().map(|p| plain(p)).collect();
            for (key, file) in &c.files {
                let changed = file.inputs.globs.iter().any(|glob| {
                    glob.matcher.is_some() && pending.iter().any(|p| glob.concerned_by(p)) && glob.changed()
                });
                if changed {
                    affected.insert(key.clone());
                }
            }
            // a setup glob that answers differently is a changed build input, named by its root
            for record in &c.plugins {
                for input in &record.inputs {
                    if let BuildInput::Glob { glob, .. } = input
                        && glob.changed()
                    {
                        changes.config_inputs.push(glob.root.clone());
                    }
                }
            }
        }

        // plugins: keep the Shared (rebuild_start) or set up again
        let mut plans: Vec<Option<(ErasedShared, ErasedState, Vec<BuildInput>)>> = Vec::with_capacity(active.len());
        for plugin in &active {
            let record = committed.as_ref().and_then(|c| c.plugins.iter().find(|r| r.name == plugin.info.name));
            // a changed salt (options, configs, Rojo files, the plugin's tsconfig entry) is a changed build input
            let salt_changed = committed.as_ref().is_none_or(|c| c.salt != salt);
            let kept = match (kind, record) {
                (BuildKind::WatchRebuild { .. }, Some(record))
                    if !salt_changed && !fresh.contains(plugin.info.name) =>
                {
                    record.shared.as_ref().and_then(|shared| {
                        let changed = record.inputs.iter().any(|i| input_changed(i, project.program, &resolver));
                        if changed {
                            return None;
                        }
                        let plan = catch_unwind(AssertUnwindSafe(|| {
                            plugin.plugin.rebuild_start(shared, &record.state, &changes)
                        }));
                        match plan {
                            Ok(RebuildPlan::KeepShared) => {
                                Some((Arc::clone(shared), record.state.clone(), record.inputs.clone()))
                            }
                            Ok(RebuildPlan::RerunSetup) => None,
                            Err(panic) => {
                                report.fatal.push(format!(
                                    "plugin `{}` panicked in rebuild_start: {}",
                                    plugin.info.name,
                                    panic_message(&*panic)
                                ));
                                None
                            }
                        }
                    })
                }
                _ => None,
            };
            if kept.is_none() && matches!(kind, BuildKind::WatchRebuild { .. }) {
                all = true;
            }
            plans.push(kept);
        }
        if !report.fatal.is_empty() {
            return (report, summary, committed, Some(loaded));
        }
        summary.all = all;

        // roblox-ts's transform order (`getChangedFilePaths` of a clean build): the order of the files transformed,
        // of what is reported, and of the parts `finish` gets (`FileIndex`)
        let order = transform_order(project.program);
        let file_indices: FxHashMap<P<SourceFile>, FileIndex> =
            order.iter().enumerate().map(|(i, &f)| (f, FileIndex(i as u32))).collect();
        let mut files = project.project_files();
        files.sort_by_key(|f| file_indices[f]);
        report.project_files = files.len();
        // the output set and its errors before anything is compiled or written, as roblox-ts's getProjectOutputs
        // runs before compileFiles: two inputs emitting to one output, or an output root holding sources, write nothing
        let mut project_outputs = match self.project_outputs(&loaded, &files) {
            Ok(outputs) => outputs,
            Err(message) => {
                report.fatal.push(message);
                return (report, summary, committed, Some(loaded));
            }
        };
        // `--writeTransformedFiles` with a plugin a tsconfig `plugins` entry turns on (`write_transformed_files`)
        let writes_transformed =
            services.data.options.write_transformed_files && active.iter().any(|plugin| plugin.entry.is_some());
        let mut to_transform: Vec<P<SourceFile>> = if all {
            files.clone()
        } else {
            let committed = committed.as_ref().expect("a committed build");
            // the Luau outputs' checks first, in parallel (one written by the last build has its bytes read)
            let out_dir_override = self.options.out_dir_override.as_deref();
            let keys: Vec<String> = files
                .iter()
                .filter(|f| !affected.contains(f.path().as_str()))
                .map(|f| path_key_of(services, &output_path(services, f.get().file_name(), out_dir_override)))
                .collect();
            listing.check_all(&keys, &committed.output_stamps);
            files
                .iter()
                .copied()
                .filter(|&file| {
                    let key = file.path().to_string();
                    if affected.contains(&key) || self.retransform.contains(&key) {
                        return true;
                    }
                    // a file with no cached part for every plugin (it never transformed with them)
                    let cached = committed.files.get(&key);
                    if cached.is_none_or(|c| c.parts.len() != active.len() || c.parts.iter().any(Option::is_none)) {
                        return true;
                    }
                    // a file of the incremental state whose diagnostics it could not keep
                    if cached.is_some_and(|c| !c.replayable) {
                        return true;
                    }
                    // roblox-ts: any source whose output is missing (its Luau, or a declaration output); rbxts-rs: or
                    // not as the last commit left it (changed by hand), which a clean build would write again
                    let name = file.get().file_name();
                    let out_dir_override = self.options.out_dir_override.as_deref();
                    let out = output_path(services, name, out_dir_override);
                    let intact = |path: &str| listing.intact(&path_key_of(services, path), &committed.output_stamps);
                    !intact(&out)
                        || project.declaration_outputs(name).iter().any(|declaration| {
                            !intact(&outputs::overridden(services, declaration, out_dir_override))
                        })
                        || (writes_transformed && {
                            let transformed = services.path_translator.get_output_transformed_path(name);
                            !intact(&outputs::overridden(services, &transformed, out_dir_override))
                        })
                })
                .collect()
        };

        if !all && !files.is_empty() && to_transform.len() == files.len() {
            all = true;
            summary.all = true;
        }
        report.timings.invalidate = invalidate_start.elapsed();

        // 3. check the files to transform
        let start = Instant::now();
        let ctx = Context::background();
        // roblox-ts's `compileFiles` makes its MacroManager before checking anything: compiler types it cannot find
        // (`"types": []`) stop the build there, with that error alone
        if !to_transform.is_empty() {
            let mut checker = project.program.get_type_checker_for_file_exclusive(&ctx, to_transform[0]);
            if let Err(error) = crate::classes::macro_manager::MacroManager::new(&mut checker) {
                report.fatal.extend(error.messages);
                return (report, summary, committed, Some(loaded));
            }
        }
        // the checker diagnostics are `checked_from..checked_to` (a plugin error drops them: `first_error_cut`)
        let checked_from = report.diagnostics.len();
        report.diagnostics.extend(project.program_diagnostics());
        report.diagnostics.extend(project.check_files(&ctx, &to_transform));
        if !services.data.options.allow_comment_directives {
            for &file in &to_transform {
                report.diagnostics.extend(comment_directive_diagnostics(file));
            }
        }
        report.timings.check = start.elapsed();
        let mut check_failed = report.diagnostics.iter().any(|d| d.category() == Category::Error);

        // 4. plugin setup, also when the check failed (explain_checker_diagnostic needs the instances)
        let start = Instant::now();
        let mut run = None;
        if !active.is_empty() {
            let mut shared = Vec::with_capacity(active.len());
            let mut states = Vec::with_capacity(active.len());
            let mut inputs = Vec::with_capacity(active.len());
            let mut times = vec![[HookTime::default(); HookKind::ALL.len()]; active.len()];
            let change_set = (kind != BuildKind::Clean).then_some(&changes);
            for (index, (plugin, plan)) in active.iter().zip(plans).enumerate() {
                if let Some((kept, state, kept_inputs)) = plan {
                    shared.push(kept);
                    states.push(state);
                    inputs.push(kept_inputs);
                    continue;
                }
                // a plugin whose state file was lost starts fresh, as in a new process
                let lost_state = fresh.contains(plugin.info.name);
                let origin = if matches!(kind, BuildKind::WatchRebuild { .. }) && !lost_state {
                    StateOrigin::Memory
                } else {
                    StateOrigin::Fresh
                };
                let state = if lost_state {
                    plugin.plugin.default_state()
                } else {
                    self.initial_state(plugin, kind, committed.as_ref())
                };
                let mut backend = Setup {
                    program: project.program,
                    services,
                    plugin: plugin.info.name,
                    entry: plugin.entry.as_ref(),
                    resolver: &resolver,
                    env: &env,
                    inputs: Default::default(),
                    messages: &mut report.plugin_messages,
                    warnings: &mut report.warnings,
                    kind,
                    all,
                    changes: change_set,
                    origin,
                };
                let result = catch_unwind(AssertUnwindSafe(|| run_setup(plugin, &mut backend, state)));
                let read = backend.inputs.into_inner();
                let (result, state, time) = match result {
                    Ok(result) => result,
                    Err(panic) => {
                        report.fatal.push(format!(
                            "plugin `{}` panicked in setup: {}",
                            plugin.info.name,
                            panic_message(&*panic)
                        ));
                        continue;
                    }
                };
                PluginHost::add_time(&mut times, index, HookKind::Setup, time);
                report.build_inputs.extend(read.iter().cloned());
                match result {
                    Ok(made) => {
                        shared.push(made);
                        states.push(state);
                        inputs.push(read);
                    }
                    Err(fatal) => report.fatal.push(format!("plugin `{}`: {fatal}", plugin.info.name)),
                }
            }
            run = Some(PluginRun { active, shared, states, inputs, times });
        }
        report.timings.plugin_setup = start.elapsed();
        if !report.fatal.is_empty() {
            return (report, summary, committed, Some(loaded));
        }
        // an incremental build whose setup read something else than the last process's (a glob answering
        // differently): a changed build input, so every file is transformed (and the ones not checked yet checked)
        if !all
            && let (Some(run), Some(before)) = (&run, committed.as_ref().and_then(|c| c.setup_inputs.as_ref()))
            && DiskInput::all_of(run.inputs.iter().flatten()) != *before
        {
            all = true;
            summary.all = true;
            let transforming: FxHashSet<P<SourceFile>> = to_transform.iter().copied().collect();
            let unchecked: Vec<P<SourceFile>> = files.iter().copied().filter(|f| !transforming.contains(f)).collect();
            report.diagnostics.extend(project.check_files(&ctx, &unchecked));
            if !services.data.options.allow_comment_directives {
                for &file in &unchecked {
                    report.diagnostics.extend(comment_directive_diagnostics(file));
                }
            }
            check_failed = report.diagnostics.iter().any(|d| d.category() == Category::Error);
            to_transform = files.clone();
        }
        // roblox-ts's `compileFiles` checks and transforms file by file, in transform order, and stops transforming at
        // the first error: the files before the first one the check failed are transformed (their diagnostics are
        // reported, up to the first transform error), nothing is written (`first_error_cut`). A rebuild transforms
        // those of them it would transform anyway and reports the cached lists of the others; the transform order of
        // the first failing file is `checked_prefix`.
        let checked_to = report.diagnostics.len();
        let mut checked_prefix: Option<u32> = None;
        if check_failed {
            if let Some(run) = &run {
                explain_failures(&mut report, project, services, &run.active, &run.shared, &run.times, &ctx, &env);
            }
            let failing: FxHashSet<P<SourceFile>> = report
                .diagnostics
                .iter()
                .filter(|d| d.category() == Category::Error)
                .map(|d| d.file())
                .collect::<Option<_>>()
                .unwrap_or_default();
            let first = files.iter().filter(|f| failing.contains(*f)).map(|f| file_indices[f].0).min();
            let Some(first) = first.filter(|&first| files.iter().any(|f| file_indices[f].0 < first)) else {
                return (report, summary, committed, Some(loaded));
            };
            if self.options.write_partial {
                return (report, summary, committed, Some(loaded));
            }
            to_transform.retain(|f| file_indices[f].0 < first);
            checked_prefix = Some(first);
        }

        // 5. transform
        let claims_all = self.plugins.claims_all || std::env::var("RBXTS_PLUGIN_CLAIMS").is_ok_and(|v| v == "all");
        let start = Instant::now();
        let mut asts: Vec<Option<Vec<Stmt>>> = Vec::with_capacity(to_transform.len());
        let mut new_parts: Vec<Vec<Option<ErasedPart>>> = Vec::with_capacity(to_transform.len());
        let mut file_late_keys: Vec<Vec<(usize, String)>> = Vec::with_capacity(to_transform.len());
        let mut load_checks = Vec::with_capacity(to_transform.len());
        let mut file_inputs: Vec<FileInputs> = Vec::with_capacity(to_transform.len());
        // per transformed file, what plugins reported into its diagnostics
        let mut plugin_reported: Vec<Vec<P<Diagnostic>>> = Vec::with_capacity(to_transform.len());
        // with the internal errors of plugins' misuse of the seam: the plugins' side of `first_error_cut`
        let mut plugin_side: Vec<Vec<P<Diagnostic>>> = Vec::with_capacity(to_transform.len());
        let mut cut = crate::compile::ErrorCut::None;
        let hook_times = {
            // every file on the checker that checked it, one thread per checker (`checker_groups`)
            let file_diagnostics: FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>> = match &run {
                Some(_) => {
                    let mut by_file: FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>> = FxHashMap::default();
                    for &d in &report.diagnostics {
                        if let Some(file) = d.file() {
                            by_file.entry(file).or_default().push(d);
                        }
                    }
                    by_file
                }
                None => FxHashMap::default(),
            };
            let plugins = run.as_ref().map(|run| GroupPlugins {
                active: &run.active,
                shared: &run.shared,
                env: &env,
                file_diagnostics: &file_diagnostics,
                file_indices: &file_indices,
                claims_all,
            });
            let transformed = checker_groups::transform_files(project, services, &ctx, plugins.as_ref(), &to_transform);
            if let Some(fatal) = transformed.setup_fatal {
                report.fatal.extend(fatal);
                return (report, summary, committed, Some(loaded));
            }
            // the files transformed, in transform order (an internal error leaves every later file out; the build then
            // fails)
            let mut done = Vec::with_capacity(to_transform.len());
            let mut once = Vec::with_capacity(to_transform.len());
            let mut classify = Vec::new();
            for (&file, transformed) in to_transform.iter().zip(transformed.files) {
                let Some(transformed) = transformed else { continue };
                let file_name = file.get().file_name().to_owned();
                let out_path = output_path(services, &file_name, self.options.out_dir_override.as_deref());
                new_parts.push(transformed.parts);
                file_late_keys.push(transformed.late_keys);
                load_checks.push(transformed.load_check);
                file_inputs.push(transformed.inputs);
                once.push(transformed.once);
                classify.extend(transformed.classify);
                plugin_side.push([&transformed.plugin_diagnostics[..], &transformed.plugin_failures].concat());
                plugin_reported.push(transformed.plugin_diagnostics);
                report.files.push(FileOutcome {
                    file_name,
                    out_path,
                    diagnostics: transformed.diagnostics,
                    internal_errors: transformed.internal_errors,
                    luau: None,
                    written: false,
                });
                asts.push(transformed.ast);
                done.push(file);
            }
            if !self.options.write_partial {
                let orders: Vec<u32> = done.iter().map(|f| file_indices[f].0).collect();
                let is_error = |d: &P<Diagnostic>| d.category() == Category::Error;
                // what plugins reported while explaining checker diagnostics, and classify_symbol's refusals
                let elsewhere = report.diagnostics[checked_to..].iter().any(is_error) || classify.iter().any(is_error);
                cut = crate::compile::first_error_cut(&mut report.files, &orders, &plugin_side, elsewhere);
                if cut == crate::compile::ErrorCut::Plugin {
                    report.diagnostics.drain(checked_from..checked_to);
                }
            }
            drop_reported_earlier(report.files.iter_mut().map(|f| &mut f.diagnostics).zip(once.iter().map(Vec::as_slice)));
            report.diagnostics.extend(crate::compile::classify_diagnostics(classify));
            report.fatal.extend(transformed.stopped);
            to_transform = done;
            transformed.hook_times
        };
        report.timings.transform = start.elapsed();
        // the transform diagnostics of the project files not transformed again: their last transform's, so a rebuild
        // reports what a clean build would (a single project's, or a dirty project of a graph whose files all cached)
        // (transform order, diagnostics, plugin diagnostics)
        let mut cached: Vec<(u32, Vec<P<Diagnostic>>, &[P<Diagnostic>])> = Vec::new();
        if let Some(c) = &committed {
            let transformed: FxHashSet<P<SourceFile>> = to_transform.iter().copied().collect();
            for &file in &files {
                if !transformed.contains(&file)
                    && checked_prefix.is_none_or(|first| file_indices[&file].0 < first)
                    && let Some(old) = c.files.get(&file.path().to_string())
                {
                    // cut as the files transformed now (`first_error_cut`)
                    let order = file_indices[&file].0;
                    let mut diagnostics = old.diagnostics.clone();
                    if !cut.keeps(order) {
                        diagnostics.retain(|d| old.plugin_diagnostics.contains(d));
                    }
                    cached.push((order, diagnostics, &old.plugin_diagnostics));
                }
            }
        }
        // what each transformed file reported, committed as it is (a rebuild drops duplicates again, over the cached
        // files' lists and its own, so it keeps the copy a clean build keeps)
        let reported: Vec<Vec<P<Diagnostic>>> = report.files.iter().map(|f| f.diagnostics.clone()).collect();
        // one of each plugin diagnostic in the build
        {
            let mut owners: Vec<crate::compile::DiagnosticOwner<'_>> = report
                .files
                .iter_mut()
                .zip(&plugin_reported)
                .zip(&to_transform)
                .map(|((f, plugin), file)| crate::compile::DiagnosticOwner {
                    order: file_indices[file].0,
                    diagnostics: &mut f.diagnostics,
                    plugin,
                })
                .collect();
            owners.extend(cached.iter_mut().map(|(order, diagnostics, plugin)| crate::compile::DiagnosticOwner {
                order: *order,
                diagnostics,
                plugin,
            }));
            crate::compile::drop_plugin_duplicates(&mut owners);
        }
        report.diagnostics.extend(cached.into_iter().flat_map(|(_, diagnostics, _)| diagnostics));

        let any_errors = report.has_errors();
        if (any_errors && !self.options.write_partial) || checked_prefix.is_some() {
            if let Some(run) = &run {
                record_timings(&mut report, &run.active, &run.times, hook_times.as_ref());
            }
            return (report, summary, committed, Some(loaded));
        }

        // 6. plugin finish: the parts of every current project file, in transform order
        let start = Instant::now();
        let transformed_at: FxHashMap<P<SourceFile>, usize> =
            to_transform.iter().enumerate().map(|(i, &f)| (f, i)).collect();
        let mut late_strings = LateStrings::new();
        let mut queue = CommitQueue::default();
        let rules = DirRules::new(services, &project.program.options(), &report.out_dir);
        let mut finished = run.is_none();
        if !any_errors && let Some(run) = run.as_mut() {
            let mut luau_outputs = FxHashSet::default();
            for &file in &files {
                let name = file.get().file_name();
                luau_outputs.insert(output_key(
                    services,
                    &output_path(services, name, self.options.out_dir_override.as_deref()),
                ));
                luau_outputs.insert(output_key(services, &services.path_translator.get_output_path(name)));
            }
            // the rest of the output set too (assets, declaration and transformed outputs): an artifact never
            // overwrites a copied asset
            for path in &project_outputs.files {
                luau_outputs.insert(output_key(services, path));
            }
            let mut resolved = FxHashSet::default();
            let mut errors = Vec::new();
            for (index, plugin) in run.active.iter().enumerate() {
                let mut plugin_parts: Vec<(FileIndex, &ErasedPart)> = Vec::with_capacity(files.len());
                for &file in &files {
                    let part = match transformed_at.get(&file) {
                        Some(&t) => new_parts[t].get(index).and_then(Option::as_ref),
                        None => committed
                            .as_ref()
                            .and_then(|c| c.files.get(file.path().as_str()))
                            .and_then(|c| c.parts.get(index))
                            .and_then(Option::as_ref),
                    };
                    if let Some(part) = part {
                        plugin_parts.push((file_indices[&file], part));
                    }
                }
                let mut backend = Finish {
                    order: &order,
                    services,
                    plugin: plugin.info.name,
                    index,
                    rules: &rules,
                    out_dir: &report.out_dir,
                    luau_outputs: &luau_outputs,
                    late: &mut late_strings,
                    resolved: &mut resolved,
                    queue: &mut queue,
                    errors: &mut errors,
                    warnings: &mut report.warnings,
                    messages: &mut report.plugin_messages,
                    kind,
                    all,
                };
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run_finish(plugin, &run.shared[index], &mut run.states[index], &plugin_parts, &mut backend)
                }));
                let (result, time) = match result {
                    Ok(result) => result,
                    Err(panic) => {
                        errors.push(format!(
                            "plugin `{}` panicked in finish: {}",
                            plugin.info.name,
                            panic_message(&*panic)
                        ));
                        continue;
                    }
                };
                PluginHost::add_time(&mut run.times, index, HookKind::Finish, time);
                if let Err(fatal) = result {
                    errors.push(format!("plugin `{}`: {fatal}", plugin.info.name));
                }
            }
            // every late key a current file uses must have a value, cached files included
            let mut reported = FxHashSet::default();
            for &file in &files {
                let keys: Vec<(usize, &str)> = match transformed_at.get(&file) {
                    Some(&t) => file_late_keys[t].iter().map(|(p, k)| (*p, k.as_str())).collect(),
                    None => committed
                        .as_ref()
                        .and_then(|c| c.files.get(file.path().as_str()))
                        .map(|c| c.late.iter().map(|(p, k, _)| (*p, k.as_str())).collect())
                        .unwrap_or_default(),
                };
                for (plugin, key) in keys {
                    if !resolved.contains(key) && reported.insert(key.to_owned()) {
                        let relative =
                            nodepath::relative(&services.data.project_path, file.get().file_name()).replace('\\', "/");
                        errors.push(format!(
                            "plugin `{}` left the late string `{}` unresolved (used in {relative})",
                            run.active[plugin].info.name,
                            plain_late_key(key)
                        ));
                    }
                }
            }
            report.fatal.extend(errors);
            finished = report.fatal.is_empty();
            self.finish_reads = std::mem::take(&mut queue.reads);
            self.finish_ran = true;
        }
        report.timings.plugin_finish = start.elapsed();
        if let Some(run) = &run {
            record_timings(&mut report, &run.active, &run.times, hook_times.as_ref());
        }
        if !report.fatal.is_empty() && !self.options.write_partial {
            return (report, summary, committed, Some(loaded));
        }

        // 7. render, and re-render kept files whose late strings changed value
        let start = Instant::now();
        let mut statement_lines: Vec<Option<Vec<u32>>> = Vec::with_capacity(report.files.len());
        for ((outcome, ast), (keys, check)) in
            report.files.iter_mut().zip(&asts).zip(file_late_keys.iter().zip(&load_checks))
        {
            statement_lines.push(None);
            if let Some(ast) = ast
                && outcome.is_clean()
                && (keys.is_empty() || finished)
            {
                // rbxts-rs: a panic while rendering is this file's internal error, not the build's crash; a file
                // marked for the load check keeps where each statement starts, to place Luau's messages
                let rendered = quietly(|| match check {
                    Some(_) => {
                        let (luau, lines) = rbxts_luau::render_with_lines(ast, &late_strings);
                        (luau, Some(lines))
                    }
                    None => (rbxts_luau::render(ast, &late_strings), None),
                });
                match rendered {
                    Ok((luau, lines)) => {
                        outcome.luau = Some(luau);
                        *statement_lines.last_mut().expect("pushed above") = lines;
                    }
                    Err(panic) => {
                        let message = format!("rendering the Luau panicked: {}", panic_message(&*panic));
                        match project.program.get_source_file(&outcome.file_name) {
                            Some(file) => outcome.diagnostics.push(internal_error(file.as_node(), &message)),
                            None => {
                                report.fatal.push(format!("{}: rbxts-rs internal error: {message}", outcome.file_name))
                            }
                        }
                        outcome.internal_errors.push(message);
                    }
                }
            }
        }
        let mut rerendered: Vec<(String, String)> = Vec::new();
        // per re-rendered file with a load-check mark: (index in `rerendered`, its file, statement lines, mark)
        let mut rerendered_checks: Vec<(usize, P<SourceFile>, Vec<u32>, &LoadCheck)> = Vec::new();
        let mut retransform = FxHashSet::default();
        if finished && let Some(c) = &committed {
            for &file in &files {
                if transformed_at.contains_key(&file) {
                    continue;
                }
                let Some(cached) = c.files.get(file.path().as_str()) else { continue };
                if cached.late.iter().all(|(_, key, value)| late_strings.get(key) == Some(value.as_str())) {
                    continue;
                }
                let name = file.get().file_name();
                let Some(ast) = &cached.ast else {
                    if retrying {
                        report.fatal.push(format!(
                            "internal error: the late strings of {name} changed value, but its Luau was not kept"
                        ));
                    } else {
                        retransform.insert(file.path().to_string());
                    }
                    continue;
                };
                // rbxts-rs: rendered as a transformed file is, under the panic guard and with statement lines when a
                // plugin emitted into it (the load check runs on it below)
                match quietly(|| rbxts_luau::render_with_lines(ast, &late_strings)) {
                    Ok((luau, lines)) => {
                        if let Some(check) = &cached.load_check {
                            rerendered_checks.push((rerendered.len(), file, lines, check));
                        }
                        rerendered.push((output_path(services, name, self.options.out_dir_override.as_deref()), luau));
                    }
                    Err(panic) => {
                        let message = format!("rendering the Luau panicked: {}", panic_message(&*panic));
                        report.diagnostics.push(internal_error(file.as_node(), &message));
                    }
                }
            }
        }
        if !retransform.is_empty() {
            self.retransform = retransform;
            return (report, summary, committed, Some(loaded));
        }
        report.timings.render = start.elapsed();
        // Luau's compiler on every file a plugin emitted into, in parallel: an error keeps the file from being written
        let start = Instant::now();
        let checked: Vec<usize> = (0..report.files.len())
            .filter(|&i| load_checks[i].is_some() && statement_lines[i].is_some() && report.files[i].luau.is_some())
            .collect();
        // the transformed files, then the cached files re-rendered with new late-string values
        let jobs: Vec<(&str, bool)> = checked
            .iter()
            .map(|&i| {
                let luau = report.files[i].luau.as_deref().expect("filtered above");
                (luau, load_checks[i].as_ref().expect("filtered above").has_directive())
            })
            .chain(
                rerendered_checks.iter().map(|(at, _, _, check)| (rerendered[*at].1.as_str(), check.has_directive())),
            )
            .collect();
        let verdicts = crate::plugins::spill::decide_all(&jobs);
        let (transformed_verdicts, rerendered_verdicts) = verdicts.split_at(checked.len());
        for (&i, verdict) in checked.iter().zip(transformed_verdicts) {
            let (Some(check), Some(lines)) = (&load_checks[i], &statement_lines[i]) else { continue };
            if let Some(diagnostic) =
                crate::plugins::spill::load_check_diagnostic(check, verdict, lines, to_transform[i])
            {
                let outcome = &mut report.files[i];
                if diagnostic.category() == Category::Error {
                    outcome.luau = None;
                }
                outcome.diagnostics.push(diagnostic);
            }
        }
        // a re-rendered file has no outcome of its own: its diagnostics are the build's (an error stops the commit)
        for ((_, file, lines, check), verdict) in rerendered_checks.iter().zip(rerendered_verdicts) {
            if let Some(diagnostic) = crate::plugins::spill::load_check_diagnostic(check, verdict, lines, *file) {
                report.diagnostics.push(diagnostic);
            }
        }
        report.timings.load_checked = (checked.len() + rerendered_checks.len()) as u32;
        report.timings.load_check = start.elapsed();
        // a file that failed to render or Luau refuses fails the build as a transform error does
        if (!report.fatal.is_empty() || report.has_errors()) && !self.options.write_partial {
            return (report, summary, committed, Some(loaded));
        }

        // the declarations of the files transformed (compileFiles: emitted before anything is written, so a
        // declaration error writes nothing)
        let options = project.program.options();
        let transform_paths = TransformPaths::new(options, &self.options.cwd);
        let mut extra_outputs: Vec<(String, String)> = Vec::new();
        if options.get_emit_declarations() {
            let emitted: Vec<P<SourceFile>> = to_transform
                .iter()
                .zip(&report.files)
                .filter(|(_, outcome)| outcome.luau.is_some() && outcome.is_clean())
                .map(|(&file, _)| file)
                .collect();
            let (outputs, diagnostics) = emit_declarations(project.program, &ctx, &emitted, &transform_paths);
            report.diagnostics.extend(diagnostics);
            let out_dir_override = self.options.out_dir_override.as_deref();
            extra_outputs = outputs
                .into_iter()
                .map(|(path, text)| (outputs::overridden(services, &path, out_dir_override), text))
                .collect();
            if report.has_errors() && !self.options.write_partial {
                return (report, summary, committed, Some(loaded));
            }
        }
        // `--writeTransformedFiles` (`write_transformed_files`): the TypeScript of the files compiled, printed again
        if run.as_ref().is_some_and(|run| run.active.iter().any(|plugin| plugin.entry.is_some()))
            && services.data.options.write_transformed_files
        {
            let out_dir_override = self.options.out_dir_override.as_deref();
            for (&file, outcome) in to_transform.iter().zip(&report.files) {
                if outcome.luau.is_some() && outcome.is_clean() {
                    let path = services.path_translator.get_output_transformed_path(file.get().file_name());
                    let path = outputs::overridden(services, &path, out_dir_override);
                    extra_outputs.push((path, crate::project::write_transformed_files::print_source_file(file)));
                }
            }
        }

        // 8. commit
        let start = Instant::now();
        let success = !report.has_errors() && finished;
        let mut records: FxHashMap<String, FileRecord> = FxHashMap::default();
        // the state files the commit leaves (each is written unless it holds these bytes already): checked before
        // the next build
        let state_files: Vec<StateFile> = if self.options.no_write {
            Vec::new()
        } else {
            queue
                .state_files
                .iter()
                .map(|(plugin, path, bytes)| StateFile {
                    plugin: (*plugin).to_owned(),
                    path: path.clone(),
                    version: Version::of_bytes(bytes),
                })
                .collect()
        };
        // the assets' bytes are read before the first write, so a vanished asset fails the build with nothing written
        let asset_bytes = if success && !self.options.no_write {
            match outputs::read_assets(&project_outputs, &transform_paths) {
                Ok(bytes) => bytes,
                Err(message) => {
                    report.fatal.push(message);
                    return (report, summary, committed, Some(loaded));
                }
            }
        } else {
            Vec::new()
        };
        // the runtime files (`copyInclude`), read before the first write too
        let include = match (success && !self.options.no_write)
            .then(|| prepare_copy_include(services, self.options.out_dir_override.as_deref()))
        {
            Some(Err(message)) => {
                report.fatal.push(message);
                return (report, summary, committed, Some(loaded));
            }
            prepared => prepared.and_then(Result::ok).flatten(),
        };
        // each output's stamp as this build leaves it (none without writes)
        let mut output_stamps: FxHashMap<String, outputs::Stamp> = FxHashMap::default();
        if !self.options.no_write {
            // rbxts-rs: in parallel, all or nothing (`write_outputs`), errors in the order of `report.files`; a
            // case-only rename renames the output too (roblox-ts keeps the old name: its bug). The re-rendered files
            // (a successful build's only) go in the same batch, without the case check, and so do a successful
            // build's assets (`asset_bytes`: the assets, then the declaration assets).
            let luau: Vec<(usize, &str, &[u8])> = report
                .files
                .iter()
                .enumerate()
                .filter_map(|(i, o)| o.luau.as_ref().map(|luau| (i, o.out_path.as_str(), luau.as_bytes())))
                .collect();
            let rerender = success && report.fatal.is_empty();
            let mut to_write: Vec<(&str, &[u8], bool)> =
                luau.iter().map(|&(_, path, bytes)| (path, bytes, true)).collect();
            if rerender {
                to_write.extend(rerendered.iter().map(|(p, s)| (p.as_str(), s.as_bytes(), false)));
            }
            let rerendered_count = to_write.len() - luau.len();
            to_write.extend(extra_outputs.iter().map(|(p, s)| (p.as_str(), s.as_bytes(), true)));
            let asset_paths = project_outputs.assets.iter().chain(&project_outputs.declaration_assets);
            to_write.extend(
                asset_paths.zip(&asset_bytes).map(|((_, output), bytes)| (output.as_str(), bytes.as_slice(), true)),
            );
            let mut results = outputs::write_outputs(services, Some(&listing), &to_write).into_iter();
            // the stamps of the bytes this build left, made from those bytes (never read back after the commit)
            let mut fresh: FxHashMap<String, outputs::Stamp> = FxHashMap::default();
            let mut note = |path: &str, result: Result<outputs::Written, String>, report: &mut CompileReport| match result {
                Ok(done) => {
                    if let Some(stamp) = done.stamp {
                        fresh.insert(path_key_of(services, path), stamp);
                    }
                    done.written
                }
                Err(message) => {
                    report.fatal.push(message);
                    false
                }
            };
            let indices: Vec<usize> = luau.iter().map(|&(i, _, _)| i).collect();
            for (i, result) in indices.into_iter().zip(results.by_ref()) {
                let path = report.files[i].out_path.clone();
                let written = note(&path, result, &mut report);
                report.files[i].written = written;
                if written {
                    summary.written.push(path);
                }
            }
            if rerender {
                for ((path, _), result) in rerendered.iter().zip(results.by_ref().take(rerendered_count)) {
                    if note(path, result, &mut report) {
                        summary.written.push(path.clone());
                    }
                }
                summary.rerendered = rerendered.iter().map(|(p, _)| p.clone()).collect();
            }
            for ((path, _), result) in extra_outputs.iter().zip(results.by_ref()) {
                if note(path, result, &mut report) {
                    summary.written.push(path.clone());
                }
            }
            let asset_paths = project_outputs.assets.iter().chain(&project_outputs.declaration_assets);
            for ((_, path), result) in asset_paths.zip(results) {
                if note(path, result, &mut report) {
                    summary.written.push(path.clone());
                }
            }
            // what the outputs are now: the stamp of the bytes this build left, else the committed stamp of one it left
            // as it was (or, for one no stamp is known of, what is there)
            output_stamps = project_outputs
                .files
                .iter()
                .filter_map(|path| {
                    let key = path_key_of(services, path);
                    if let Some(stamp) = fresh.get(&key) {
                        return Some((key, *stamp));
                    }
                    let kept = committed.as_ref().and_then(|c| c.output_stamps.get(&key));
                    let stamp = kept.and_then(|stamp| listing.checked(&key, stamp));
                    stamp.or_else(|| outputs::Stamp::read(path)).map(|stamp| (key, stamp))
                })
                .collect();
            // the runtime include files are outputs too, stamped from the bytes copied: one edited or deleted by hand
            // is written again (the commit adds the times it wrote them with)
            if let Some(include) = &include {
                for file in &include.files {
                    let stamp = outputs::Stamp {
                        size: file.bytes.len() as u64,
                        hash: outputs::content_hash(&file.bytes),
                        modified: 0,
                    };
                    output_stamps.insert(path_key_of(services, &file.output), stamp);
                }
            }
            if success && report.fatal.is_empty() {
                let artifacts: Vec<String> = queue.artifacts.iter().map(|(_, p, _)| p.clone()).collect();
                let stale: Vec<String> = committed
                    .as_ref()
                    .map(|c| c.artifacts.iter().filter(|a| !artifacts.contains(a)).cloned().collect())
                    .unwrap_or_default();
                let disk = state_path.as_ref().map(|path| {
                    let (records, setup_inputs) = match &run {
                        Some(run) => (
                            disk_records(
                                &run.active,
                                &files,
                                &transformed_at,
                                (&new_parts, &file_late_keys, &file_inputs),
                                &late_strings,
                                committed.as_ref(),
                            ),
                            DiskInput::all_of(run.inputs.iter().flatten()),
                        ),
                        None => Default::default(),
                    };
                    let mut records = records;
                    // every project file's last transform diagnostics, to be reported again by a new process: as
                    // the transform reported them, before the plugin duplicates were dropped (the new process drops
                    // them again over its own build), and which of them plugins reported
                    for &file in &files {
                        let key = file.path().to_string();
                        let diagnostics: Option<(Vec<P<Diagnostic>>, &[P<Diagnostic>])> = match transformed_at.get(&file) {
                            Some(&t) => {
                                let known: FxHashSet<P<Diagnostic>> = reported[t].iter().copied().collect();
                                let mut diagnostics = reported[t].clone();
                                diagnostics.extend(report.files[t].diagnostics.iter().copied().filter(|d| !known.contains(d)));
                                Some((diagnostics, &plugin_reported[t]))
                            }
                            None => committed
                                .as_ref()
                                .and_then(|c| c.files.get(&key))
                                .map(|c| (c.diagnostics.clone(), c.plugin_diagnostics.as_slice())),
                        };
                        let unknown = !transformed_at.contains_key(&file)
                            && committed.as_ref().and_then(|c| c.files.get(&key)).is_some_and(|c| !c.replayable);
                        let (kept, no_replay) = diagnostics
                            .map_or_else(Default::default, |(d, plugin)| super::replay::to_disk(&d, plugin, file));
                        if !kept.is_empty() || no_replay || unknown {
                            let record = records.entry(key).or_default();
                            record.diagnostics = kept;
                            record.no_replay = no_replay || unknown;
                        }
                    }
                    let state = DiskState {
                        compiler: state::compiler_identity(),
                        plugins: plugin_ids.clone(),
                        salt,
                        files: facts.iter().map(|(k, f)| (k.clone(), f.clone())).collect(),
                        artifacts: artifacts.clone(),
                        records,
                        setup_inputs,
                        state_files: state_files.clone(),
                        outputs: output_stamps.iter().map(|(k, s)| (k.clone(), *s)).collect(),
                    };
                    (path.as_str(), state)
                });
                match self.commit_rest(
                    &mut report,
                    &mut summary,
                    &loaded,
                    &mut project_outputs,
                    include.as_ref(),
                    &queue,
                    &stale,
                    &listing,
                    &rules,
                    disk.as_ref().map(|(path, state)| (*path, state)),
                ) {
                    Ok(include_stamps) => {
                        for (path, stamp) in include_stamps {
                            output_stamps.insert(path_key_of(services, &path), stamp);
                        }
                    }
                    Err(message) => report.fatal.push(message),
                }
                if let Some((_, state)) = disk {
                    records = state.records;
                }
            }
        }
        report.timings.write = start.elapsed();
        if !success || !report.fatal.is_empty() {
            return (report, summary, committed, Some(loaded));
        }

        // 9. the new committed state
        let run_parts = run.map(|run| {
            (run.active.iter().map(|p| p.info.name).collect::<Vec<_>>(), run.shared, run.states, run.inputs)
        });
        let mut old_files = committed.map(|c| c.files).unwrap_or_default();
        let mut new_files = FxHashMap::default();
        let mut parts_by_file: FxHashMap<P<SourceFile>, Vec<Option<ErasedPart>>> = FxHashMap::default();
        for (&file, parts) in to_transform.iter().zip(new_parts) {
            parts_by_file.insert(file, parts);
        }
        let mut asts_by_file: FxHashMap<P<SourceFile>, Option<Vec<Stmt>>> =
            to_transform.iter().copied().zip(asts).collect();
        let late_by_file: FxHashMap<P<SourceFile>, Vec<(usize, String)>> =
            to_transform.iter().copied().zip(file_late_keys).collect();
        let mut checks_by_file: FxHashMap<P<SourceFile>, Option<LoadCheck>> =
            to_transform.iter().copied().zip(load_checks).collect();
        let mut inputs_by_file: FxHashMap<P<SourceFile>, FileInputs> =
            to_transform.iter().copied().zip(file_inputs).collect();
        // as the transform reported them (`reported`, before the plugin duplicates were dropped), then what the build
        // added since (internal errors)
        let mut diagnostics_by_file: FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>> = to_transform
            .iter()
            .copied()
            .zip(report.files.iter().zip(&reported).map(|(f, reported)| {
                let known: FxHashSet<P<Diagnostic>> = reported.iter().copied().collect();
                let mut diagnostics = reported.clone();
                diagnostics.extend(f.diagnostics.iter().copied().filter(|d| !known.contains(d)));
                diagnostics
            }))
            .collect();
        let mut plugin_by_file: FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>> =
            to_transform.iter().copied().zip(plugin_reported).collect();
        let program_files: FxHashMap<String, P<SourceFile>> =
            project.program.source_files().iter().map(|&f| (f.path().to_string(), f)).collect();
        for (key, facts) in facts {
            let file = program_files.get(&key).copied();
            let entry = match file.and_then(|f| parts_by_file.remove(&f).map(|p| (f, p))) {
                Some((file, parts)) => {
                    let late: Vec<(usize, String, String)> = late_by_file[&file]
                        .iter()
                        .map(|(p, k)| (*p, k.clone(), late_strings.get(k).unwrap_or_default().to_owned()))
                        .collect();
                    let ast = if self.watch && !late.is_empty() { asts_by_file.remove(&file).flatten() } else { None };
                    let load_check = if ast.is_some() { checks_by_file.remove(&file).flatten() } else { None };
                    let inputs = inputs_by_file.remove(&file).unwrap_or_default();
                    let encoded = records.remove(&key).map(|record| record.parts);
                    let diagnostics = diagnostics_by_file.remove(&file).unwrap_or_default();
                    let plugin_diagnostics = plugin_by_file.remove(&file).unwrap_or_default();
                    // a watch session frees this program once a later build replaces it
                    let (diagnostics, plugin_diagnostics) = if self.watch {
                        detach(diagnostics, plugin_diagnostics)
                    } else {
                        (diagnostics, plugin_diagnostics)
                    };
                    CommittedFile {
                        facts,
                        parts,
                        late,
                        ast,
                        load_check,
                        inputs,
                        encoded,
                        diagnostics,
                        plugin_diagnostics,
                        replayable: true,
                    }
                }
                None => match old_files.remove(&key) {
                    Some(mut old) => {
                        old.facts = facts;
                        if let Some(record) = records.remove(&key) {
                            old.encoded = Some(record.parts);
                        }
                        for (_, k, value) in &mut old.late {
                            if let Some(v) = late_strings.get(k) {
                                *value = v.to_owned();
                            }
                        }
                        old
                    }
                    None => CommittedFile::new(facts),
                },
            };
            new_files.insert(key, entry);
        }
        // a state file setup read as a build input (Flamework's candidate scan reads its own `flamework.build`):
        // - read as text and left as text by this commit: recorded with what the commit left, so the next watch
        //   rebuild does not take the plugin's own write for a changed input (a `Shared` never depends on its own
        //   state file's bytes, the `Plugin::Shared` contract); a change by anyone else is a lost state file (above);
        // - read as missing (or not as text) and left as text, or the other way round: the read is kept as it was,
        //   so the next build runs that plugin's setup again, as its `Shared` may depend on it (Flamework's scan
        //   loads an ancestor `flamework.build` while its own is missing), and the summary says so, for a watcher
        //   to start that build at once.
        // Another plugin's state file is an input like any other file, whose bytes that plugin's `Shared` may depend
        // on: when this commit changed what setup read (new bytes too), the read is kept as it was and the summary
        // says so, as in the second case.
        // The incremental state keeps what setup read: a new process runs setup anyway, and Flamework relies on the
        // change there (its salt)
        let committed_texts: FxHashMap<String, (&str, Option<Version>)> = queue
            .state_files
            .iter()
            .map(|(owner, path, bytes)| {
                (path_key_of(services, path), (*owner, std::str::from_utf8(bytes).ok().map(Version::of)))
            })
            .collect();
        let plugins = match run_parts {
            Some((names, shared, states, mut inputs)) => {
                if !self.options.no_write {
                    for input in inputs.iter_mut().flatten() {
                        if let BuildInput::File { plugin, path, exists, version } = input
                            && let Some(&(owner, text)) = committed_texts.get(&path_key_of(services, path))
                        {
                            let own = *plugin == owner;
                            if own && *exists == text.is_some() {
                                *version = text;
                            } else if (own || *exists != text.is_some() || *version != text)
                                && !summary.setup_inputs_changed.contains(path)
                            {
                                report.plugin_messages.push(if own {
                                    format!(
                                        "{plugin}: its setup read its state file {path} {}, and this build left {}: \
                                         its setup runs again at the next build",
                                        if *exists { "as text" } else { "as missing" },
                                        if *exists { "bytes that are not text" } else { "it as text" },
                                    )
                                } else {
                                    format!(
                                        "{plugin}: its setup read {path}, the state file of {owner}, and this build \
                                         changed it: its setup runs again at the next build"
                                    )
                                });
                                summary.setup_inputs_changed.push(path.clone());
                            }
                        }
                    }
                }
                names
                    .into_iter()
                    .zip(shared)
                    .zip(states)
                    .zip(inputs)
                    .map(|(((name, shared), state), inputs)| PluginRecord { name, state, shared: Some(shared), inputs })
                    .collect()
            }
            None => Vec::new(),
        };
        let committed = Committed {
            salt,
            plugin_ids,
            files: new_files,
            plugins,
            artifacts: queue.artifacts.iter().map(|(_, p, _)| p.clone()).collect(),
            watch_dirs: queue.watch_dirs.iter().map(|(_, p)| p.clone()).collect(),
            from_disk: false,
            setup_inputs: None,
            state_files,
            outputs: project_outputs
                .files
                .iter()
                .cloned()
                .chain(include.iter().flat_map(|include| include.files.iter().map(|f| f.output.clone())))
                .collect(),
            output_stamps,
        };
        summary.changes = changes;
        self.pending.clear();
        (report, summary, Some(committed), Some(loaded))
    }

    /// Commit steps 1 (assets, the runtime include files), 2, 3 and 4 after the Luau outputs. Returns the stamps of
    /// the runtime include files as the commit left them.
    #[allow(clippy::too_many_arguments)]
    fn commit_rest(
        &self,
        report: &mut CompileReport,
        summary: &mut BuildSummary,
        loaded: &Loaded,
        outputs: &mut outputs::ProjectOutputs,
        include: Option<&IncludeCopy>,
        queue: &CommitQueue,
        stale: &[String],
        listing: &outputs::Listing,
        rules: &DirRules,
        disk: Option<(&str, &DiskState)>,
    ) -> Result<Vec<(String, outputs::Stamp)>, String> {
        let services = &loaded.services;
        // this build's artifacts, state files and incremental state are kept too
        for path in queue
            .artifacts
            .iter()
            .map(|(_, p, _)| p.as_str())
            .chain(queue.state_files.iter().map(|(_, p, _)| p.as_str()))
            .chain(disk.as_ref().map(|(path, _)| *path))
        {
            outputs.keep.insert(outputs::path_key(services, path));
        }
        // 1. the runtime include (the Luau and the assets are written)
        let mut include_stamps = Vec::new();
        if let Some(include) = include {
            for (path, written, stamp) in include.commit_stamped()? {
                if written {
                    summary.written.push(path.clone());
                }
                include_stamps.push((path, stamp));
            }
        }
        // 2. artifacts
        let written = commit_artifacts(queue)?;
        summary.written.extend(written.iter().cloned());
        report.artifacts_written.extend(written);
        // 3. stale outputs, stale artifacts, removals
        let deleted = outputs::clean(services, outputs, listing)?;
        summary.deleted.extend(deleted);
        let deleted = commit_removals(queue, stale)?;
        summary.deleted.extend(deleted.iter().cloned());
        report.artifacts_deleted.extend(deleted);
        // the directories the stale artifacts leave empty, then the plugins' `remove_empty_dir`s
        report.artifacts_deleted.extend(remove_emptied_dirs(services, rules, stale)?);
        report.artifacts_deleted.extend(remove_queued_dirs(&queue.empty_dirs)?);
        // 4. plugin state files, then the incremental state
        let written = commit_state_files(queue)?;
        summary.written.extend(written.iter().cloned());
        report.artifacts_written.extend(written);
        if let Some((path, state)) = disk
            && state.write(path).map_err(|e| format!("could not write the incremental state {path}: {e}"))?
        {
            summary.written.push(path.to_owned());
        }
        Ok(include_stamps)
    }

    /// roblox-ts's `getProjectOutputs` for this build: the output roots, the keep set (without this build's plugin
    /// files) and the assets, or the error that stops the build before anything is written.
    fn project_outputs(&self, loaded: &Loaded, files: &[P<SourceFile>]) -> Result<outputs::ProjectOutputs, String> {
        let services = &loaded.services;
        let out_dir_override = self.options.out_dir_override.as_deref();
        let emitted: Vec<(String, String, Vec<String>)> = files
            .iter()
            .map(|f| {
                let name = f.get().file_name();
                let declarations = loaded
                    .project
                    .declaration_outputs(name)
                    .iter()
                    .map(|d| outputs::overridden(services, d, out_dir_override))
                    .collect();
                (name.to_owned(), output_path(services, name, out_dir_override), declarations)
            })
            .collect();
        let options = loaded.project.program.options();
        let ts_build_info = rbxts_project::setup::ts5_build_info_path(&options);
        outputs::get_project_outputs(&OutputInputs {
            services,
            options: &options,
            emitted: &emitted,
            transformed_outputs: services.data.options.write_transformed_files
                && options.plugins.as_ref().is_some_and(|p| !p.is_empty()),
            out_dir_override,
            config_paths: &loaded.config_paths,
            ts_build_info: ts_build_info.as_deref(),
            graph: self.graph.as_ref().map(|g| &g.outputs),
        })
    }

    /// Where the incremental state goes: beside roblox-ts's build info for an `incremental` or `composite`
    /// project, and never for parity runs (an outDir override) or builds that write nothing.
    fn state_path(&self, services: &ProjectServices) -> Option<String> {
        if self.options.out_dir_override.is_some() || self.options.no_write || self.options.write_partial {
            return None;
        }
        services.path_translator.build_info_output_path.as_deref().map(state::state_path)
    }

    /// The working state a plugin's `setup` starts from.
    fn initial_state(&self, plugin: &ActivePlugin<'_>, kind: BuildKind, committed: Option<&Committed>) -> ErasedState {
        if let Some(seed) = &self.seed
            && let Some((_, state)) = seed.iter().find(|(name, _)| *name == plugin.info.name)
        {
            return state.clone();
        }
        if matches!(kind, BuildKind::WatchRebuild { .. })
            && let Some(record) = committed.and_then(|c| c.plugins.iter().find(|r| r.name == plugin.info.name))
        {
            return record.state.clone();
        }
        plugin.plugin.default_state()
    }

    /// The config inputs' path keys: tsconfig and `extends`, Rojo project files, plugin input files.
    fn config_inputs_of(&self, loaded: &Loaded, committed: Option<&Committed>) -> FxHashSet<String> {
        let services = &loaded.services;
        let mut keys: FxHashSet<String> = loaded.config_paths.iter().map(|p| path_key_of(services, p)).collect();
        keys.extend(services.rojo_resolver.config_files().iter().map(|p| path_key_of(services, p)));
        let committed = committed.or(self.committed.as_ref());
        if let Some(c) = committed {
            for plugin in &c.plugins {
                for input in &plugin.inputs {
                    if let BuildInput::File { path, .. } = input {
                        keys.insert(path_key_of(services, path));
                    }
                }
            }
        }
        keys
    }
}

/// Copies diagnostics out of the program that made them, into the thread's arena (which outlives every program),
/// `plugin` mapped onto the same copies (`plugin` is a sublist of `diagnostics`, compared by identity). What a
/// committed file keeps outlives its program, which a watch session frees once a later build replaces it: the
/// transform's diagnostics were made while a checker was held, in that checker's region.
fn detach(diagnostics: Vec<P<Diagnostic>>, plugin: Vec<P<Diagnostic>>) -> (Vec<P<Diagnostic>>, Vec<P<Diagnostic>>) {
    use tsrs_ast::DiagnosticExt;
    let mut copies: FxHashMap<P<Diagnostic>, P<Diagnostic>> = FxHashMap::default();
    let mut copy = |d: P<Diagnostic>| *copies.entry(d).or_insert_with(|| d.copy_deep());
    let diagnostics = diagnostics.into_iter().map(&mut copy).collect();
    let plugin = plugin.into_iter().map(&mut copy).collect();
    (diagnostics, plugin)
}

/// A path's comparison key.
fn path_key_of(services: &ProjectServices, path: &str) -> String {
    outputs::path_key(services, path)
}

/// The plugins of a build after setup.
struct PluginRun<'p> {
    active: Vec<ActivePlugin<'p>>,
    shared: Vec<ErasedShared>,
    states: Vec<ErasedState>,
    inputs: Vec<Vec<BuildInput>>,
    times: Vec<[HookTime; HookKind::ALL.len()]>,
}

/// A build input changed since `setup` read it.
fn input_changed(
    input: &BuildInput,
    program: &'static tsrs_compiler::Program,
    resolver: &OnceCell<rbxts_project::ModuleResolver>,
) -> bool {
    match input {
        BuildInput::File { path, version, .. } => {
            std::fs::read_to_string(path).ok().map(|text| Version::of(&text)) != *version
        }
        BuildInput::Env { name, value, .. } => std::env::var(name).ok() != *value,
        BuildInput::Resolution { specifier, from, resolved, .. } => {
            resolve_module_from(program, resolver, specifier, from).map(|r| r.file_name) != *resolved
        }
        BuildInput::Glob { glob, .. } => glob.changed(),
    }
}

/// What the incremental state keeps of each project file with plugins ([`FileRecord`]): the parts encoded (a cached
/// file's bytes as read or made before), the late strings with this build's values, the recorded inputs.
#[allow(clippy::type_complexity)]
fn disk_records(
    active: &[ActivePlugin<'_>],
    files: &[P<SourceFile>],
    transformed_at: &FxHashMap<P<SourceFile>, usize>,
    (parts, late_keys, inputs): (&[Vec<Option<ErasedPart>>], &[Vec<(usize, String)>], &[FileInputs]),
    late: &LateStrings,
    committed: Option<&Committed>,
) -> FxHashMap<String, FileRecord> {
    let encode = |parts: &[Option<ErasedPart>]| -> Vec<Option<Vec<u8>>> {
        if parts.len() != active.len() {
            return Vec::new();
        }
        // a plugin's encode never panics by contract; one that does stores the file without a part
        active
            .iter()
            .zip(parts)
            .map(|(plugin, part)| {
                let part = part.as_ref()?;
                catch_unwind(AssertUnwindSafe(|| plugin.plugin.encode_file_part(part))).ok().flatten()
            })
            .collect()
    };
    let value = |key: &str| late.get(key).unwrap_or_default().to_owned();
    let globs = |inputs: &FileInputs| -> Vec<(usize, String, String, Vec<String>)> {
        inputs.globs.iter().map(|g| (g.plugin, g.root.clone(), g.pattern.clone(), g.answer.clone())).collect()
    };
    let mut records = FxHashMap::default();
    for &file in files {
        let key = file.path().to_string();
        let record = match transformed_at.get(&file) {
            Some(&t) => FileRecord {
                parts: encode(&parts[t]),
                late: late_keys[t].iter().map(|(p, k)| (*p, k.clone(), value(k))).collect(),
                env: inputs[t].env.clone(),
                globs: globs(&inputs[t]),
                ..FileRecord::default()
            },
            None => {
                let Some(cached) = committed.and_then(|c| c.files.get(&key)) else { continue };
                FileRecord {
                    parts: cached.encoded.clone().unwrap_or_else(|| encode(&cached.parts)),
                    late: cached
                        .late
                        .iter()
                        .map(|(p, k, v)| (*p, k.clone(), late.get(k).map_or_else(|| v.clone(), str::to_owned)))
                        .collect(),
                    env: cached.inputs.env.clone(),
                    globs: globs(&cached.inputs),
                    ..FileRecord::default()
                }
            }
        };
        records.insert(key, record);
    }
    records
}
