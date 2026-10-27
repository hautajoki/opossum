//! The transform over the program's checkers (S3-par).
//!
//! A file is transformed by its owner, the checker that checked it: the transform asks that checker about types, and
//! checkers are not shareable. tsrs assigns every file to a checker when the pool is made (a function of the program,
//! `compute_associations`), and none of our passes moves files between checkers (only tsrs's own type-check pass
//! steals work), so the owner is `Program::checker_index_of_file`.
//!
//! Each checker that owns files to transform runs one group on its own thread (`for_each_checker_parallel`, 512 MiB
//! stacks): it makes its own `MacroManager` (merged symbols are per checker), `MultiTransformState` (caches keyed by its
//! types) and `PluginHost` (the plugin instances, made on that thread, `CheckerInfo { index, count }`), then transforms
//! its files in the given order (the transform order). When one checker owns them all (one checker, or a small rebuild), the group runs on the
//! calling thread. The results come back in the files' order, so what the build reports and writes does not depend on
//! the checker count or the assignment: per-file state is per file, cross-file plugin data travels in file parts
//! merged in that order, and the once-per-compilation diagnostics are deduplicated in it afterwards
//! (`drop_reported_earlier`).

use std::sync::{Arc, Mutex, PoisonError};

use rustc_hash::FxHashMap;

use rbxts_plugin::{CheckerInfo, FileIndex};
use rbxts_plugin::erased::ErasedShared;
use rbxts_project::{Project, ProjectServices};
use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_checker::Checker;
use tsrs_compiler::Context;
use tsrs_core::P;

use crate::build::plugin_inputs::EnvSnapshot;
use crate::classes::macro_manager::MacroManager;
use crate::classes::multi_transform_state::MultiTransformState;
use crate::compile::{FileTransform, transform_file};
use crate::plugins::host::HookTime;
use crate::plugins::lifecycle::ActivePlugin;
use crate::plugins::{HookKind, PluginHost};

/// Hook times per plugin and hook kind.
pub(crate) type HookTimes = Vec<[HookTime; HookKind::ALL.len()]>;

/// The plugins of a build's transform.
pub(crate) struct GroupPlugins<'a, 'p> {
    pub active: &'a [ActivePlugin<'p>],
    pub shared: &'a [ErasedShared],
    pub env: &'a Arc<EnvSnapshot>,
    /// The checker's diagnostics of each file (warnings and suggestions only: an error stops the build first).
    pub file_diagnostics: &'a FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>>,
    /// Each program file's [`rbxts_plugin::FileIndex`] (its place in the transform order).
    pub file_indices: &'a FxHashMap<P<SourceFile>, FileIndex>,
    pub claims_all: bool,
}

/// What the transform of a build's files gave.
pub(crate) struct Transformed {
    /// Per file, in the order of the files given: its transform, or `None` after the first file whose transform
    /// panicked (an internal error), whichever checker had it.
    pub files: Vec<Option<FileTransform>>,
    /// The plugins' hook times, summed over the groups (thread time: with several checkers they overlap).
    pub hook_times: Option<HookTimes>,
    /// Errors that stop the build before any file is transformed (the macro manager, unsupported plugin interests):
    /// every group meets the same ones, and the first group's are kept. `files` is then empty.
    pub setup_fatal: Option<Vec<String>>,
    /// The transform stopped by an internal error: its message (at most one).
    pub stopped: Vec<String>,
}

/// One group's results.
struct GroupOutput {
    files: Vec<(usize, FileTransform)>,
    hook_times: Option<HookTimes>,
    setup_fatal: Option<Vec<String>>,
}

/// Transforms `files` (transform order, `build::transform_order`), each on its owner checker; see the module docs.
pub(crate) fn transform_files(
    project: &Project,
    services: &ProjectServices,
    ctx: &Context,
    plugins: Option<&GroupPlugins<'_, '_>>,
    files: &[P<SourceFile>],
) -> Transformed {
    let program = project.program;
    let count = program.checker_count().max(1);
    let mut groups: Vec<Vec<(usize, P<SourceFile>)>> = vec![Vec::new(); count];
    for (i, &file) in files.iter().enumerate() {
        let owner = if count == 1 { 0 } else { program.checker_index_of_file(file).unwrap_or(0).min(count - 1) };
        groups[owner].push((i, file));
    }
    let owners: Vec<usize> = (0..count).filter(|&c| !groups[c].is_empty()).collect();
    let outputs: Vec<GroupOutput> = match owners.as_slice() {
        [] => Vec::new(),
        &[only] => {
            // no thread needed: lock the owner's checker here
            let mut checker = program.get_type_checker_for_file_exclusive(ctx, groups[only][0].1);
            vec![run_group(project, services, plugins, &mut checker, CheckerInfo { index: only, count }, &groups[only])]
        }
        _ => {
            let slots: Vec<Mutex<Option<GroupOutput>>> = (0..count).map(|_| Mutex::new(None)).collect();
            program.for_each_checker_parallel(|index, checker| {
                let Some(group) = groups.get(index).filter(|g| !g.is_empty()) else { return };
                let output = run_group(project, services, plugins, checker, CheckerInfo { index, count }, group);
                *slots[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(output);
            });
            slots.into_iter().filter_map(|slot| slot.into_inner().unwrap_or_else(PoisonError::into_inner)).collect()
        }
    };

    let mut transformed = Transformed {
        files: files.iter().map(|_| None).collect(),
        hook_times: None,
        setup_fatal: None,
        stopped: Vec::new(),
    };
    let mut first_panic: Option<usize> = None;
    for output in outputs {
        if let Some(fatal) = output.setup_fatal {
            transformed.setup_fatal.get_or_insert(fatal);
            continue;
        }
        if let Some(times) = output.hook_times {
            match &mut transformed.hook_times {
                None => transformed.hook_times = Some(times),
                Some(sum) => {
                    for (sum, times) in sum.iter_mut().zip(&times) {
                        for (sum, time) in sum.iter_mut().zip(times) {
                            sum.calls += time.calls;
                            sum.time += time.time;
                        }
                    }
                }
            }
        }
        for (i, file) in output.files {
            if file.panicked && first_panic.is_none_or(|first| i < first) {
                first_panic = Some(i);
            }
            transformed.files[i] = Some(file);
        }
    }
    if transformed.setup_fatal.is_some() {
        transformed.files.iter_mut().for_each(|f| *f = None);
    } else if let Some(first) = first_panic {
        // rbxts-rs: a group stops at its first panic (its checker may be inconsistent, FOLLOWUPS T1); the other groups
        // go on. What the report holds is cut at the first panic in transform order, so it is what one checker gives,
        // for any checker count: every file before it, and nothing after it.
        transformed.files.iter_mut().skip(first + 1).for_each(|f| *f = None);
        let name = files[first].get().file_name();
        transformed.stopped =
            vec![format!("the transform stopped after an internal error in {name}; later files were not transformed")];
    }
    transformed
}

/// One checker's files, in transform order, on the calling thread.
fn run_group(
    project: &Project,
    services: &ProjectServices,
    plugins: Option<&GroupPlugins<'_, '_>>,
    checker: &mut Checker,
    info: CheckerInfo,
    files: &[(usize, P<SourceFile>)],
) -> GroupOutput {
    let mut output = GroupOutput { files: Vec::with_capacity(files.len()), hook_times: None, setup_fatal: None };
    let macro_manager = match MacroManager::new(checker) {
        Ok(manager) => manager,
        Err(error) => {
            output.setup_fatal = Some(error.messages);
            return output;
        }
    };
    let host = plugins.map(|plugins| {
        let mut host = PluginHost::new(
            project.program,
            plugins.active,
            plugins.shared,
            info,
            plugins.file_diagnostics.clone(),
            plugins.file_indices.clone(),
            plugins.claims_all,
        );
        host.env = Arc::clone(plugins.env);
        host
    });
    let mut multi = MultiTransformState::new();
    for &(i, file) in files {
        let transformed = transform_file(project, checker, services, &macro_manager, &mut multi, host.as_ref(), file);
        let panicked = transformed.panicked;
        output.files.push((i, transformed));
        if panicked {
            // rbxts-rs: after a panic this checker may be inconsistent (FOLLOWUPS T1): its group stops here
            break;
        }
    }
    output.hook_times = host.map(|host| host.times.borrow().clone());
    output
}
