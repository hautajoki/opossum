//! Lowering over the program's checkers. A file is lowered by its owner, the checker that checked it: lowering asks
//! that checker about types, and checkers are not shareable. Each checker that owns files runs one group on its own
//! thread with its own macro manager and caches; results come back in the order of the files given, so the output does
//! not depend on the checker count.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, PoisonError};

use luau::Stmt;
use rustc_hash::FxHashSet;
use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_checker::Checker;
use tsrs_core::P;

use crate::classes::diagnostic_service::ReportOnce;
use crate::classes::macro_manager::MacroManager;
use crate::classes::multi_transform_state::MultiTransformState;
use crate::classes::prereqs::reset_temp_ids;
use crate::classes::transform_state::TransformState;
use crate::locate::Locator;
use crate::options::Options;
use crate::nodes::transform_source_file::transform_source_file;
use crate::program::Project;
use crate::shared::diagnostics::internal_error;

/// One file's lowering.
pub struct Lowered {
    pub file: P<SourceFile>,
    /// The Luau tree; `None` when lowering panicked (an internal error, in `diagnostics`).
    pub ast: Option<Vec<Stmt>>,
    pub diagnostics: Vec<P<Diagnostic>>,
    /// Per top-level TypeScript statement that produced Luau: the index in `ast` of its first Luau statement, and
    /// its start and end offsets in the file, in `ast` order. With `luau::render_with_lines` this maps a line of the
    /// output back to the TypeScript that made it.
    pub spans: Vec<(usize, u32, u32)>,
}

impl Lowered {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.category() == tsrs_diagnostics::Category::Error)
    }
}

struct FileOutput {
    lowered: Lowered,
    once: Vec<(ReportOnce, P<Diagnostic>)>,
}

/// Lowers `files`, each on its owner checker; see the module docs. Call from a thread with [`crate::STACK_SIZE`].
pub fn lower_files(project: &Project, locator: &Locator, options: &Options, files: &[P<SourceFile>]) -> Vec<Lowered> {
    let program = project.program;
    let state = program.pool().state();
    let mut groups: Vec<Vec<(usize, P<SourceFile>)>> = Vec::new();
    for (i, &file) in files.iter().enumerate() {
        let owner = state
            .file_indices
            .get(&file)
            .map_or(0, |&f| state.owners[f].load(std::sync::atomic::Ordering::Relaxed) as usize);
        if groups.len() <= owner {
            groups.resize_with(owner + 1, Vec::new);
        }
        groups[owner].push((i, file));
    }
    let slots: Vec<Mutex<Vec<(usize, FileOutput)>>> = groups.iter().map(|_| Mutex::new(Vec::new())).collect();
    program.for_each_checker_parallel(|index, checker| {
        let Some(group) = groups.get(index).filter(|g| !g.is_empty()) else { return };
        let output = run_group(project, locator, options, checker, group);
        *slots[index].lock().unwrap_or_else(PoisonError::into_inner) = output;
    });

    let mut outputs: Vec<Option<FileOutput>> = files.iter().map(|_| None).collect();
    for slot in slots {
        for (i, output) in slot.into_inner().unwrap_or_else(PoisonError::into_inner) {
            outputs[i] = Some(output);
        }
    }
    // a once-per-compilation diagnostic stays with the first file (in the order given) that reported it
    let mut reported: FxHashSet<ReportOnce> = FxHashSet::default();
    outputs
        .into_iter()
        .zip(files)
        .map(|(output, &file)| match output {
            Some(mut output) => {
                let dropped: FxHashSet<P<Diagnostic>> =
                    output.once.iter().filter(|(key, _)| !reported.insert(*key)).map(|&(_, d)| d).collect();
                output.lowered.diagnostics.retain(|d| !dropped.contains(d));
                output.lowered
            }
            None => Lowered {
                file,
                ast: None,
                diagnostics: vec![internal_error(file.as_node(), "not lowered: an earlier file of its checker panicked")],
                spans: Vec::new(),
            },
        })
        .collect()
}

/// One checker's files, in order, on the calling thread. Stops at a panic: the checker may be inconsistent after it.
fn run_group(
    project: &Project,
    locator: &Locator,
    options: &Options,
    checker: &mut Checker,
    files: &[(usize, P<SourceFile>)],
) -> Vec<(usize, FileOutput)> {
    let macro_manager = MacroManager::new(checker);
    let mut multi = MultiTransformState::new();
    let mut outputs = Vec::with_capacity(files.len());
    for &(i, file) in files {
        reset_temp_ids();
        // per file, so a file's output depends only on the program, not on what its checker lowered before
        multi.function_effects.clear();
        multi.binding_keys.clear();
        let mut state = TransformState::new(project, locator, options, checker, &macro_manager, &mut multi, file);
        let result = catch_unwind(AssertUnwindSafe(|| transform_source_file(&mut state, file)));
        let panicked = result.is_err();
        let ast = match result {
            Ok(ast) => Some(ast),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "panic".to_owned());
                state.add_diagnostic(internal_error(file.as_node(), &message));
                None
            }
        };
        let diagnostics = std::mem::take(&mut state.diagnostics.diagnostics);
        let once = std::mem::take(&mut state.diagnostics.once);
        let spans = std::mem::take(&mut state.spans);
        outputs.push((i, FileOutput { lowered: Lowered { file, ast, diagnostics, spans }, once }));
        if panicked {
            break;
        }
    }
    outputs
}
