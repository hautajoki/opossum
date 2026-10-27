//! The build pipeline (`Project/functions/compileFiles.ts`, DESIGN §4): load, check every project file, set up the
//! project and the plugins, transform each file in roblox-ts's transform order ([`crate::build::transform_order`]),
//! let the plugins finish, render, and commit. The lifecycle itself, with incremental and watch builds, is
//! [`crate::build::BuildSession`]; this module has the one-build entry points and the per-file pieces.
//!
//! Any error stops the build before anything is written: checker and setup errors before the transform, transform
//! errors before rendering (roblox-ts's rule), plugin `finish` errors and unresolved late strings before
//! rendering. [`CompileOptions::write_partial`] relaxes the second for parity runs: files that transformed cleanly
//! are written even when other files failed (plugin artifacts never are).
//!
//! **Commit order** (DESIGN §14), only when the build has no errors: Luau outputs, assets and the runtime include
//! files ([`crate::project::copy_include`]); plugin artifacts (only if changed); deletions (stale outputs, stale
//! artifacts, removals); plugin state files, then the incremental state. Every file is written to a temporary file
//! and renamed into place, with retries while another process holds the target
//! ([`crate::plugins::lifecycle::write_atomic`]).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_luau::Stmt;
use rbxts_plugin::erased::{ErasedPart, ErasedShared};
use rbxts_plugin::{CheckerInfo, FileIndex, FileInfo};
use rbxts_project::{Project, ProjectOptionLayer, ProjectServices, nodepath};
use tsrs_ast::{Diagnostic, SourceFile, new_diagnostic};
use tsrs_compiler::Context;
use tsrs_core::P;
use tsrs_diagnostics::Category;

use crate::classes::diagnostic_service::ReportOnce;
use crate::classes::macro_manager::MacroManager;
use crate::classes::multi_transform_state::MultiTransformState;
use crate::classes::prereqs::reset_temp_ids;
use crate::classes::transform_state::TransformState;
use crate::nodes::transform_source_file::transform_source_file;
use crate::plugins::backend::{run_hook, to_diagnostic};
use crate::plugins::file_state::PluginFileState;
use crate::plugins::host::HookTime;
use crate::plugins::lifecycle::{ActivePlugin, BuildInput, PluginOptions, PluginTimings};
use crate::plugins::{HookKind, PluginHost};
use crate::shared::diagnostics::internal_error;

/// What to build and how.
#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    /// The tsconfig, absolute or relative to `cwd`.
    pub tsconfig: String,
    /// The directory relative paths resolve against.
    pub cwd: String,
    /// Command-line project options (`--optimizedLoops`, `--allowCommentDirectives`, …) over the tsconfig's
    /// `rbxts` section.
    pub project_options: ProjectOptionLayer,
    /// Write outputs here instead of `outDir`, keeping Rojo resolution against the configured `outDir` (so parity
    /// runs do not touch a project's real output).
    pub out_dir_override: Option<String>,
    /// Write the files that transformed without errors even when others failed (parity runs). roblox-ts writes
    /// nothing when any file has an error.
    pub write_partial: bool,
    /// Transform and render, but write nothing.
    pub no_write: bool,
    /// Type checkers, each checking and transforming its share of the files on a thread of its own (`--checkers`).
    /// `None`: [`default_checkers`]. The output is the same for any count, but for tsrs's known rare
    /// history-dependent case ([`DEFAULT_CHECKERS`]).
    pub checkers: Option<usize>,
}

/// The checker count when none is given: `RBXTS_CHECKERS` when it is a whole number from 1 to [`MAX_CHECKERS`]
/// (tests and harnesses run a whole suite at another count with it), else [`DEFAULT_CHECKERS`]. Not tied to the
/// machine, so every machine builds with the same count unless told otherwise.
pub fn default_checkers() -> usize {
    env_checkers().ok().flatten().unwrap_or(DEFAULT_CHECKERS)
}

/// `RBXTS_CHECKERS`: `Ok(None)` when unset or empty (blanks only), `Ok(Some(n))` for a whole number from 1 to [`MAX_CHECKERS`], else the
/// warning to give (the build then uses [`DEFAULT_CHECKERS`]).
pub fn env_checkers() -> Result<Option<usize>, String> {
    let Ok(value) = std::env::var("RBXTS_CHECKERS") else { return Ok(None) };
    if value.trim().is_empty() {
        return Ok(None);
    }
    match value.trim().parse::<usize>() {
        Ok(n) if (1..=MAX_CHECKERS).contains(&n) => Ok(Some(n)),
        _ => Err(format!(
            "RBXTS_CHECKERS={value:?} is not a whole number from 1 to {MAX_CHECKERS}; using {DEFAULT_CHECKERS} checkers"
        )),
    }
}

/// The default checker count (S3-par measurements: past it, more checkers add memory for little time). Fixed, not
/// capped by the machine's parallelism: with several checkers tsrs (like TypeScript 7) has a rare known case whose
/// diagnostics depend on which checker checked what (a base constraint cut short by the depth limit and cached:
/// tsrs `notes/open-history-dependence.md` §1), so a fixed count gives every machine the same output.
pub const DEFAULT_CHECKERS: usize = 4;

/// tsrs's limit.
pub const MAX_CHECKERS: usize = 256;

impl CompileOptions {
    /// The checker count this build asks for.
    pub fn checker_count(&self) -> usize {
        self.checkers.map_or_else(default_checkers, |n| n.clamp(1, MAX_CHECKERS))
    }

    /// The warning about an invalid `RBXTS_CHECKERS` this build would ignore, if any. A `BuildSession`
    /// gives it with its first build only.
    pub fn checkers_warning(&self) -> Option<String> {
        if self.checkers.is_some() { None } else { env_checkers().err() }
    }
}

/// Wall time per phase.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    /// Reading and parsing the tsconfig.
    pub config: Duration,
    /// Loading the program (reading, parsing, binding, module resolution).
    pub program: Duration,
    /// The project setup (Rojo resolvers, path translator, roblox-ts's checks).
    pub setup: Duration,
    /// File facts and the invalidation rules (incremental and watch builds), with the missing-output checks.
    pub invalidate: Duration,
    /// Type checking (the program's pre-emit diagnostics).
    pub check: Duration,
    /// Plugin `setup` (all plugins).
    pub plugin_setup: Duration,
    /// Transforming the files to Luau trees (plugin hooks included).
    pub transform: Duration,
    /// Plugin `finish` (all plugins) and the late-string key check.
    pub plugin_finish: Duration,
    /// Rendering the Luau trees to text.
    pub render: Duration,
    /// Luau's compiler on the files plugins emitted into (`plugins::spill`), and how many were checked.
    pub load_check: Duration,
    /// The number of files the load check compiled.
    pub load_checked: u32,
    /// Writing the outputs, assets and state, and removing stale outputs.
    pub write: Duration,
}

/// One project file's outcome.
pub struct FileOutcome {
    /// The source file, as the program spells it.
    pub file_name: String,
    /// Where the output goes (after `out_dir_override`).
    pub out_path: String,
    /// The transform's diagnostics (roblox-ts's, rbxts-rs's own, and plugins').
    pub diagnostics: Vec<P<Diagnostic>>,
    /// Internal errors (panics caught while transforming).
    pub internal_errors: Vec<String>,
    /// The rendered Luau, when the file transformed without errors.
    pub luau: Option<String>,
    /// The output was written (it was missing or different).
    pub written: bool,
}

impl FileOutcome {
    /// The file transformed without error diagnostics.
    pub fn is_clean(&self) -> bool {
        !self.diagnostics.iter().any(|d| d.category() == Category::Error)
    }
}

/// What a build did.
#[derive(Default)]
pub struct CompileReport {
    /// The time each phase took.
    pub timings: Timings,
    /// Config, program, setup and checker diagnostics (the transform's are per file).
    pub diagnostics: Vec<P<Diagnostic>>,
    /// Setup warnings (Rojo config search, resolver warnings) and plugin warnings.
    pub warnings: Vec<String>,
    /// A failure that stopped the build, with its messages (roblox-ts's `ProjectError` / text diagnostics, plugin
    /// `Fatal`s, unresolved late strings, commit failures).
    pub fatal: Vec<String>,
    /// One per transformed file, in transform order ([`crate::build::transform_order`]).
    pub files: Vec<FileOutcome>,
    /// Where the outputs go: `outDir`, or the override.
    pub out_dir: String,
    /// Files in the program, and project files transformed.
    pub program_files: usize,
    /// Project files transformed.
    pub project_files: usize,
    /// The plugins that were on, in registration order.
    pub plugins: Vec<&'static str>,
    /// Plugin hook times (`--timings`).
    pub plugin_timings: Vec<PluginTimings>,
    /// Plugin `info` messages (`--verbose`).
    pub plugin_messages: Vec<String>,
    /// What plugins read in `setup` (rbxts-rs: S2 — watched and hashed by incremental and watch builds).
    pub build_inputs: Vec<BuildInput>,
    /// Plugin artifacts and state files written, and files plugins removed.
    pub artifacts_written: Vec<String>,
    /// Artifacts of the last build this one did not write, and files plugins removed.
    pub artifacts_deleted: Vec<String>,
    /// A build with project references: each project that builds, in build order, with its output roots and whether
    /// its build succeeded (a project whose references failed is not built: blocked). Empty otherwise.
    pub projects: Vec<ProjectOutcome>,
    /// The diagnostics were thrown before any project was built (the project graph's: TS6306, TS6310, TS6202):
    /// roblox-ts's `build` command prints them as its `DiagnosticError` does, as one formatted block in the order
    /// found, then a line feed.
    pub thrown: bool,
}

/// One project of a build with project references.
#[derive(Clone, Debug)]
pub struct ProjectOutcome {
    /// The project's tsconfig.
    pub tsconfig: String,
    /// Its outDir and `declarationDir`.
    pub output_roots: Vec<String>,
    /// Built without errors (and committed).
    pub ok: bool,
    /// Not built: a project it references failed or was blocked.
    pub blocked: bool,
    /// Not built: not dirty (its last report repeated).
    pub skipped: bool,
}

impl CompileReport {
    /// The build failed (nothing written unless `write_partial`).
    pub fn has_errors(&self) -> bool {
        !self.fatal.is_empty()
            || self.diagnostics.iter().any(|d| d.category() == Category::Error)
            || self.files.iter().any(|f| !f.is_clean())
    }
}

/// `fileUsesCommentDirectives`: `@ts-ignore`, `@ts-expect-error` and `@ts-nocheck` need `allowCommentDirectives`. One
/// diagnostic per `@ts-nocheck` pragma (stock's `sourceFile.pragmas.get("ts-nocheck")`), also one a later `@ts-check`
/// overrides (TypeScript's check directive is the last of the two).
///
/// rbxts-rs: each diagnostic spans its comment. Stock's `createDiagnosticWithLocation` gives a range's diagnostic the
/// length `range.end` (not `end - pos`), so a directive or pragma after the file's first character underlines from
/// the comment on, past it (to the end of the file when the comment is far enough down): a stock bug, not copied.
pub(crate) fn comment_directive_diagnostics(file: P<SourceFile>) -> Vec<P<Diagnostic>> {
    let message = || tsrs_diagnostics::new_ad_hoc_message(&errors_text_no_comment_directives());
    let mut diagnostics = Vec::new();
    for directive in file.get().comment_directives() {
        diagnostics.push(new_diagnostic(Some(file), directive.loc, message(), &[]));
    }
    for pragma in file.get().pragmas().iter().filter(|p| p.name == "ts-nocheck") {
        diagnostics.push(new_diagnostic(Some(file), pragma.comment_range.text_range, message(), &[]));
    }
    diagnostics
}

fn errors_text_no_comment_directives() -> String {
    // the text of `errors.noCommentDirectives`, which takes a node there
    [
        "Usage of `@ts-ignore`, `@ts-expect-error`, and `@ts-nocheck` are not supported!",
        "roblox-ts needs type and symbol info to compile correctly.",
        "Suggestion: Consider using type assertions or `declare` statements.",
    ]
    .join("\n")
}

/// The output path, moved under `out_dir_override` when there is one.
pub(crate) fn output_path(services: &ProjectServices, file_name: &str, out_dir_override: Option<&str>) -> String {
    let out_path = services.path_translator.get_output_path(file_name);
    match out_dir_override {
        Some(dir) => {
            let relative = nodepath::relative(&services.path_translator.out_dir, &out_path);
            nodepath::join(&[dir, &relative])
        }
        None => out_path,
    }
}

/// The plugin facts of a file.
fn file_info(services: &ProjectServices, host: &PluginHost, file: P<SourceFile>) -> FileInfo {
    let path = file.get().file_name().to_owned();
    let relative_path = nodepath::relative(&services.data.project_path, &path).replace('\\', "/");
    FileInfo {
        source: file,
        index: host.file_indices.get(&file).copied().unwrap_or(FileIndex(u32::MAX)),
        rojo: services.file_info(&path),
        path,
        relative_path,
    }
}

thread_local! {
    /// The running code reports its panics itself (`quietly`): the panic hook prints nothing for them.
    static QUIET_PANICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f`, catching a panic without the panic hook printing it (the caller reports it as a diagnostic). The
/// hook stays as it was for every other panic and thread.
pub(crate) fn quietly<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !QUIET_PANICS.with(std::cell::Cell::get) {
                previous(info);
            }
        }));
    });
    QUIET_PANICS.with(|quiet| quiet.set(true));
    let result = catch_unwind(AssertUnwindSafe(f));
    QUIET_PANICS.with(|quiet| quiet.set(false));
    result
}

/// The text of a caught panic.
pub(crate) fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "panic".to_owned())
}

/// One file's transform: its Luau (unless it panicked) and what it reported.
pub(crate) struct FileTransform {
    pub ast: Option<Vec<Stmt>>,
    pub diagnostics: Vec<P<Diagnostic>>,
    pub internal_errors: Vec<String>,
    /// A panic was caught: the checker may be inconsistent, so no further file is transformed.
    pub panicked: bool,
    /// Per plugin: the file's part.
    pub parts: Vec<Option<ErasedPart>>,
    /// The late keys its Luau uses: (plugin, namespaced key).
    pub late_keys: Vec<(usize, String)>,
    /// A plugin emitted into it: Luau's compiler checks it once rendered.
    pub load_check: Option<crate::plugins::spill::LoadCheck>,
    /// What its hooks read through `Cx::env` and `Cx::record_glob`.
    pub inputs: crate::build::plugin_inputs::FileInputs,
    /// Its once-per-compilation diagnostics, with their keys (`DiagnosticService::once`).
    pub once: Vec<(ReportOnce, P<Diagnostic>)>,
    /// What plugins' `classify_symbol` reported while it was transformed (not in `diagnostics`): the build's, once
    /// each ([`classify_diagnostics`]).
    pub classify: Vec<P<Diagnostic>>,
    /// What plugins reported into `diagnostics` ([`drop_plugin_duplicates`]).
    pub plugin_diagnostics: Vec<P<Diagnostic>>,
    /// The internal errors of plugins' misuse of the seam in `diagnostics` ([`first_error_cut`]).
    pub plugin_failures: Vec<P<Diagnostic>>,
}

/// Where the first error of a build cuts its transform diagnostics ([`first_error_cut`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorCut {
    /// No error: every diagnostic stays.
    None,
    /// A plugin reported an error: only the plugins' side stays, of every file, and no checker diagnostic.
    Plugin,
    /// The file at this transform order is the first with an error: the later files keep only the plugins' side.
    After(u32),
}

impl ErrorCut {
    /// Whether the compiler diagnostics of the file at transform order `order` stay.
    pub(crate) fn keeps(self, order: u32) -> bool {
        match self {
            ErrorCut::None => true,
            ErrorCut::Plugin => false,
            ErrorCut::After(first) => order <= first,
        }
    }
}

/// roblox-ts's `compileFiles` stops at the first error: its TypeScript transformers (the plugins here) run over every
/// file first, and when one reports an error, only their diagnostics are reported (no checker diagnostic either: the
/// check comes after them); otherwise each file in transform order is checked, then transformed while no error has
/// been reported, so the transform diagnostics of the files after the first with an error are not. `files` are the
/// files transformed now, `orders` their transform orders, `plugin` their plugins' side (what plugins reported and the
/// internal errors of their misuse of the seam, a failing transformer's errors); `plugin_error` is a plugin error
/// reported elsewhere. Cuts `files` and returns the cut, which the caller applies to the cached lists of the files not
/// transformed again (their last transform's, which hold no error) and, for [`ErrorCut::Plugin`], to the checker
/// diagnostics. The plugins' side stays, as the transformers saw every file.
pub(crate) fn first_error_cut(
    files: &mut [FileOutcome],
    orders: &[u32],
    plugin: &[Vec<P<Diagnostic>>],
    plugin_error: bool,
) -> ErrorCut {
    let is_error = |d: &P<Diagnostic>| d.category() == Category::Error;
    let plugin_sets: Vec<FxHashSet<P<Diagnostic>>> = plugin.iter().map(|p| p.iter().copied().collect()).collect();
    let cut = if plugin_error || plugin.iter().flatten().any(is_error) {
        ErrorCut::Plugin
    } else {
        let first = files
            .iter()
            .zip(&plugin_sets)
            .zip(orders)
            .filter(|((f, p), _)| f.diagnostics.iter().any(|d| is_error(d) && !p.contains(d)))
            .map(|(_, &order)| order)
            .min();
        match first {
            Some(first) => ErrorCut::After(first),
            None => return ErrorCut::None,
        }
    };
    for ((file, plugin), &order) in files.iter_mut().zip(&plugin_sets).zip(orders) {
        if !cut.keeps(order) {
            file.diagnostics.retain(|d| plugin.contains(d));
        }
    }
    cut
}

/// One file's transform diagnostics, for [`drop_plugin_duplicates`].
pub(crate) struct DiagnosticOwner<'a> {
    /// The file's place in roblox-ts's transform order, in which TypeScript transformers add their diagnostics: of
    /// copies equal in every way, the first file's is kept, whichever files a rebuild transforms.
    pub order: u32,
    pub diagnostics: &'a mut Vec<P<Diagnostic>>,
    /// Which of `diagnostics` plugins reported.
    pub plugin: &'a [P<Diagnostic>],
}

/// Keeps one of each plugin diagnostic in the build, as TypeScript's `sortAndDeduplicateDiagnostics` does: of the
/// diagnostics equal to one a plugin reported (the same file, range, code (a plugin diagnostic's source) and message
/// text, the hint included; the category and related information do not count), the one first in TypeScript 5.9's
/// `compareDiagnostics` order is kept, which puts the one with more related information first; then a plugin's before
/// a compiler diagnostic (stock adds the transformers' diagnostics before roblox-ts's own and sorts stably), then the
/// owning file's place in the transform order and the position in its list. Compiler diagnostics
/// equal only to each other are left alone, as stock roblox-ts reports them.
///
/// **Watch and incremental builds.** `owners` holds every project file's transform diagnostics, the files
/// transformed now and the cached ones of files a rebuild does not transform again, each as its own last transform
/// reported them (a rebuild commits a file's list before this drop). The choice depends only on that set, so a
/// rebuild reports what a clean build would: when the file holding the kept copy stops reporting it, or is deleted,
/// a copy another file holds is reported in its place.
pub(crate) fn drop_plugin_duplicates(owners: &mut [DiagnosticOwner<'_>]) {
    type Key = (Option<String>, Option<u32>, Option<u32>, Option<&'static str>, i32, String);
    if owners.iter().all(|owner| owner.plugin.is_empty()) {
        return;
    }
    let key = |d: P<Diagnostic>| -> Key {
        let source = crate::plugins::diagnostic_source(d);
        (d.file().map(|f| f.file_name().to_owned()), d.pos(), d.end(), source, d.code(), d.localize())
    };
    // (owner, position in its list, diagnostic, a plugin's)
    type Candidate = (usize, usize, P<Diagnostic>, bool);
    let mut groups: FxHashMap<Key, Vec<Candidate>> = FxHashMap::default();
    for (o, owner) in owners.iter().enumerate() {
        let plugin: FxHashSet<P<Diagnostic>> = owner.plugin.iter().copied().collect();
        for (i, &d) in owner.diagnostics.iter().enumerate() {
            groups.entry(key(d)).or_default().push((o, i, d, plugin.contains(&d)));
        }
    }
    let mut dropped: Vec<FxHashSet<P<Diagnostic>>> = vec![FxHashSet::default(); owners.len()];
    for group in groups.values() {
        if group.len() < 2 || !group.iter().any(|&(_, _, _, plugin)| plugin) {
            continue;
        }
        let kept = group
            .iter()
            .min_by(|a, b| {
                ts_compare_diagnostics(a.2, b.2)
                    .then(b.3.cmp(&a.3))
                    .then(owners[a.0].order.cmp(&owners[b.0].order))
                    .then(a.1.cmp(&b.1))
            })
            .expect("a group is not empty");
        for &(o, _, d, _) in group {
            if d != kept.2 {
                dropped[o].insert(d);
            }
        }
    }
    for (owner, dropped) in owners.iter_mut().zip(dropped) {
        if !dropped.is_empty() {
            owner.diagnostics.retain(|d| !dropped.contains(d));
        }
    }
}

/// TypeScript 5.9's `compareDiagnostics`: file (its path, lowercased on Windows, as TypeScript compares it), start,
/// length, code (a plugin diagnostic's source first), message text, then related information (one with more first,
/// one with none last).
fn ts_compare_diagnostics(d1: P<Diagnostic>, d2: P<Diagnostic>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let file = |d: P<Diagnostic>| d.file().map(|f| f.path().to_string());
    let source = crate::plugins::diagnostic_source;
    let skip_related = file(d1)
        .cmp(&file(d2))
        .then(d1.pos().cmp(&d2.pos()))
        .then(d1.len().cmp(&d2.len()))
        .then(source(d1).cmp(&source(d2)))
        .then(d1.code().cmp(&d2.code()))
        .then_with(|| d1.localize().cmp(&d2.localize()));
    skip_related.then_with(|| {
        let (r1, r2) = (d1.related_information(), d2.related_information());
        match (r1.is_empty(), r2.is_empty()) {
            (true, true) => Ordering::Equal,
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (false, false) => r2.len().cmp(&r1.len()).then_with(|| {
                r1.iter()
                    .zip(r2)
                    .map(|(&a, &b)| ts_compare_diagnostics(a, b))
                    .find(|c| c.is_ne())
                    .unwrap_or(Ordering::Equal)
            }),
        }
    })
}

/// The diagnostics `classify_symbol` reported in all files, once each and sorted (file, position, message): which
/// file a checker first asks about a symbol from, and how many checkers ask, depend on the checker count and the
/// file order, so the build reports them on its own, not in a file's list.
pub(crate) fn classify_diagnostics(reported: impl IntoIterator<Item = P<Diagnostic>>) -> Vec<P<Diagnostic>> {
    // (file name, start, end, message)
    type Key = (String, u32, u32, String);
    let mut keyed: Vec<(Key, P<Diagnostic>)> = reported
        .into_iter()
        .map(|d| {
            let file = d.file().map(|f| f.file_name().to_owned()).unwrap_or_default();
            ((file, d.pos().unwrap_or(0), d.end().unwrap_or(0), d.localize()), d)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed.dedup_by(|a, b| a.0 == b.0);
    keyed.into_iter().map(|(_, d)| d).collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transform_file(
    project: &Project,
    checker: &mut tsrs_checker::Checker,
    services: &ProjectServices,
    macro_manager: &MacroManager,
    multi: &mut MultiTransformState,
    host: Option<&PluginHost>,
    file: P<SourceFile>,
) -> FileTransform {
    reset_temp_ids();
    // rbxts-rs: these two are filled by whichever file reaches a symbol first; per file, a file's output depends only on
    // the program (not on the files its checker transformed before it), for any checker count and in rebuilds
    multi.function_effects.clear();
    multi.binding_keys.clear();
    let mut state = TransformState::new(project, checker, services, macro_manager, multi, file);
    if let Some(host) = host {
        state.plugins = Some(host);
        state.plugin_file = PluginFileState::new(file_info(services, host, file), host.len());
    }
    let result = catch_unwind(AssertUnwindSafe(|| transform_source_file(&mut state, file)));
    let mut internal_errors = Vec::new();
    let mut panicked = false;
    let ast = match result {
        Ok(ast) => Some(ast),
        Err(panic) => {
            panicked = true;
            let message = panic_message(&*panic);
            // a panic inside a hook is the plugin's; inside compiler code (a hook's compile_* included) the compiler's
            let message = match host.and_then(PluginHost::take_panic_frame) {
                Some((plugin, hook)) => {
                    format!("plugin `{}` panicked in {}: {message}", host.expect("a host").name(plugin), hook.label())
                }
                None => message,
            };
            state.add_diagnostic(internal_error(file.as_node(), &message));
            internal_errors.push(message);
            None
        }
    };
    let diagnostics = std::mem::take(&mut state.diagnostics.diagnostics);
    let parts = std::mem::take(&mut state.plugin_file.parts);
    let late_keys = std::mem::take(&mut state.plugin_file.late_keys);
    let load_check = state.plugin_file.load_check.take();
    let inputs = std::mem::take(&mut state.plugin_file.inputs);
    let once = std::mem::take(&mut state.diagnostics.once);
    let classify = std::mem::take(&mut state.plugin_file.classify_diagnostics);
    let plugin_diagnostics = std::mem::take(&mut state.plugin_file.plugin_diagnostics);
    let plugin_failures = std::mem::take(&mut state.plugin_file.plugin_failures);
    FileTransform {
        ast,
        diagnostics,
        internal_errors,
        panicked,
        parts,
        late_keys,
        load_check,
        inputs,
        once,
        classify,
        plugin_diagnostics,
        plugin_failures,
    }
}

/// The check failed: each plugin may explain the checker diagnostics of the failing files
/// (`explain_checker_diagnostic`); an explanation replaces the diagnostic. `env` is the build's environment snapshot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn explain_failures(
    report: &mut CompileReport,
    project: &Project,
    services: &ProjectServices,
    active: &[ActivePlugin<'_>],
    shared: &[ErasedShared],
    times: &[[HookTime; HookKind::ALL.len()]],
    ctx: &Context,
    env: &std::sync::Arc<crate::build::plugin_inputs::EnvSnapshot>,
) {
    let failing: Vec<P<SourceFile>> = {
        let mut seen = FxHashSet::default();
        report
            .diagnostics
            .iter()
            .filter(|d| d.category() == Category::Error)
            .filter_map(|d| d.file())
            .filter(|f| seen.insert(*f))
            .collect()
    };
    if failing.is_empty() {
        return;
    }
    // each file on the checker that checked it, one checker at a time on this thread (a failing build is not timed);
    // what each file adds is kept in the failing files' order, so the report does not depend on the checker count
    let program = project.program;
    let count = program.checker_count().max(1);
    let mut by_owner: std::collections::BTreeMap<usize, Vec<(usize, P<SourceFile>)>> = Default::default();
    for (i, &file) in failing.iter().enumerate() {
        let owner = if count == 1 { 0 } else { program.checker_index_of_file(file).unwrap_or(0) };
        by_owner.entry(owner).or_default().push((i, file));
    }
    let mut replacements: FxHashMap<P<Diagnostic>, P<Diagnostic>> = FxHashMap::default();
    let mut extra: Vec<Vec<P<Diagnostic>>> = vec![Vec::new(); failing.len()];
    let mut panics: Vec<Vec<String>> = vec![Vec::new(); failing.len()];
    let mut hook_times: Vec<[HookTime; HookKind::ALL.len()]> =
        vec![[HookTime::default(); HookKind::ALL.len()]; active.len()];
    let file_indices: FxHashMap<P<SourceFile>, FileIndex> = crate::build::transform_order::transform_order(program)
        .into_iter()
        .enumerate()
        .map(|(i, f)| (f, FileIndex(i as u32)))
        .collect();
    for (owner, files) in by_owner {
        let mut checker = program.get_type_checker_for_file_exclusive(ctx, files[0].1);
        let Ok(macro_manager) = MacroManager::new(&mut checker) else { continue };
        let mut multi = MultiTransformState::new();
        let mut host = PluginHost::new(
            program,
            active,
            shared,
            CheckerInfo { index: owner, count },
            FxHashMap::default(),
            file_indices.clone(),
            false,
        );
        host.env = std::sync::Arc::clone(env);
        for (i, file) in files {
            let diagnostics: Vec<P<Diagnostic>> =
                report.diagnostics.iter().copied().filter(|d| d.file() == Some(file)).collect();
            let mut state = TransformState::new(project, &mut checker, services, &macro_manager, &mut multi, file);
            state.plugins = Some(&host);
            state.plugin_file = PluginFileState::new(file_info(services, &host, file), host.len());
            let root = file.as_node();
            for diagnostic in diagnostics {
                for plugin in 0..host.len() {
                    let explained = catch_unwind(AssertUnwindSafe(|| {
                        run_hook(&mut state, None, None, plugin, HookKind::Explain, root, |instance, cx| {
                            Ok(instance.explain_checker_diagnostic(cx, diagnostic))
                        })
                    }));
                    let explained = match explained {
                        Ok(explained) => explained,
                        Err(panic) => {
                            host.take_panic_frame();
                            panics[i].push(format!(
                                "plugin `{}` panicked in explain_checker_diagnostic: {}",
                                host.name(plugin),
                                panic_message(&*panic)
                            ));
                            continue;
                        }
                    };
                    if let Ok(Some(explanation)) = explained {
                        replacements.insert(diagnostic, to_diagnostic(&explanation, host.sources[plugin]));
                        break;
                    }
                }
            }
            extra[i] = std::mem::take(&mut state.diagnostics.diagnostics);
        }
        for (sum, times) in hook_times.iter_mut().zip(host.times.borrow().iter()) {
            for (sum, time) in sum.iter_mut().zip(times) {
                sum.calls += time.calls;
                sum.time += time.time;
            }
        }
    }
    report.fatal.extend(panics.into_iter().flatten());
    for diagnostic in &mut report.diagnostics {
        if let Some(&replacement) = replacements.get(diagnostic) {
            *diagnostic = replacement;
        }
    }
    report.diagnostics.extend(extra.into_iter().flatten());
    record_timings(report, active, times, Some(&hook_times));
}

pub(crate) fn record_timings(
    report: &mut CompileReport,
    active: &[ActivePlugin<'_>],
    outside: &[[HookTime; HookKind::ALL.len()]],
    hooks: Option<&Vec<[HookTime; HookKind::ALL.len()]>>,
) {
    report.plugin_timings = active
        .iter()
        .enumerate()
        .map(|(i, plugin)| PluginTimings {
            name: plugin.info.name,
            hooks: HookKind::ALL
                .iter()
                .map(|&kind| {
                    let mut time = outside[i][kind.index()];
                    if let Some(hooks) = hooks {
                        time.calls += hooks[i][kind.index()].calls;
                        time.time += hooks[i][kind.index()].time;
                    }
                    (kind, time)
                })
                .collect(),
        })
        .collect();
}

/// Builds a project without plugins: check → transform → render → write (DESIGN §4). Runs on the calling thread,
/// which needs a large stack ([`rbxts_project::STACK_SIZE`]).
pub fn compile_project(options: &CompileOptions) -> CompileReport {
    compile_project_with(options, &PluginOptions::default())
}

/// Builds a project with plugins: check → plugin setup → transform (with the plugins' hooks) → plugin finish →
/// render → commit (DESIGN §4, §14), as one build of a [`crate::build::BuildSession`]: a clean build, or an
/// incremental one when the project is `incremental` and its incremental state is current. Runs on the calling
/// thread, which needs a large stack ([`rbxts_project::STACK_SIZE`]).
pub fn compile_project_with(options: &CompileOptions, plugin_options: &PluginOptions<'_>) -> CompileReport {
    let mut session = crate::build::BuildSession::new(options.clone(), plugin_options, false);
    session.build(&[]).0
}
