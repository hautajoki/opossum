//! A project as a tsrs program: compiler options built in code (no tsconfig), the overlay file system, loading,
//! reuse after edits, type checking and freeing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;
use tsrs_ast::{Diagnostic, SourceFile, is_json_source_file};
use tsrs_compiler::{Context, Program, ProgramOptions, new_cached_fs_compiler_host, new_program, sort_and_deduplicate_diagnostics};
use tsrs_core::collections::ordered_map::OrderedMap;
use tsrs_core::tspath::{self, ComparePathsOptions};
use tsrs_core::{
    CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, P, ScriptTarget, Tristate,
};
use tsrs_tsoptions::new_parsed_command_line;
use tsrs_vfs::{FS, bundled, osvfs};

use crate::overlay::{MACROS_D_TS, Overlay};
use crate::source_cache::{CachingHost, SourceFileCache};

/// Stack size for any thread that loads, checks or lowers: checking recurses deeply (tsrs's CLI uses the same).
pub const STACK_SIZE: usize = 512 << 20;

/// What [`load`] loads.
pub struct LoadOptions {
    /// The project directory (holds `src/` and `node_modules/`), absolute.
    pub root: String,
    /// The root files, absolute: the project's `.ts`, `.tsx` and `.d.ts` sources.
    pub files: Vec<String>,
    /// Type checkers, each checking and lowering its share of the files on a thread of its own.
    pub checkers: usize,
    /// Strictness flags on top of opossum's options, by their tsconfig names (`noUncheckedIndexedAccess`).
    pub flags: Vec<(String, bool)>,
}

/// A loaded project.
pub struct Project {
    /// The program (leaked: tsrs programs live until [`Project::free`]).
    pub program: &'static Program,
    /// The project directory.
    pub root: String,
    /// Reading, parsing, binding and module resolution of every file.
    pub load_time: Duration,
    fs: Arc<dyn FS>,
    source_cache: Option<Arc<SourceFileCache>>,
    lineage: Arc<Lineage>,
}

/// What the programs of one load and those reused from it share (`reuse_program` hands the loader's data on).
struct Lineage(tsrs_compiler::SharedProgramData);

/// The in-memory file holding the macro declarations.
pub fn macros_file(root: &str) -> String {
    tspath::combine_paths(root, &[".opossum/opossum.d.ts"])
}

/// opossum's compiler options for a project at `root`, with `flags` applied over them. Fails on a flag name opossum
/// does not take (module, lib, paths and jsx are opossum's).
pub fn compiler_options(root: &str, flags: &[(String, bool)]) -> Result<CompilerOptions, String> {
    let mut paths = OrderedMap::default();
    paths.insert("*".to_owned(), Some(vec!["src/*".to_owned()]));
    let mut options = CompilerOptions {
        module: ModuleKind::PRESERVE,
        module_resolution: ModuleResolutionKind::BUNDLER,
        module_detection: ModuleDetectionKind::Force,
        target: ScriptTarget::ES_NEXT,
        no_lib: Tristate::True,
        strict: Tristate::True,
        jsx: JsxEmit::REACT,
        jsx_factory: "React.createElement".to_owned(),
        jsx_fragment_factory: "React.Fragment".to_owned(),
        experimental_decorators: Tristate::True,
        resolve_json_module: Tristate::True,
        allow_synthetic_default_imports: Tristate::True,
        force_consistent_casing_in_file_names: Tristate::True,
        skip_lib_check: Tristate::True,
        no_emit: Tristate::True,
        paths: Some(Arc::new(paths)),
        paths_base_path: root.to_owned(),
        type_roots: Some(vec![tspath::combine_paths(root, &["node_modules/@rbxts"])]),
        // TypeScript 5's automatic type directives: every package of the type root (`@rbxts/types` and
        // `@rbxts/compiler-types` declare the globals)
        types: Some(vec!["*".to_owned()]),
        ..CompilerOptions::default()
    };
    for (name, value) in flags {
        let value = if *value { Tristate::True } else { Tristate::False };
        let slot = match name.as_str() {
            "noUncheckedIndexedAccess" => &mut options.no_unchecked_indexed_access,
            "noImplicitReturns" => &mut options.no_implicit_returns,
            "noImplicitOverride" => &mut options.no_implicit_override,
            "noFallthroughCasesInSwitch" => &mut options.no_fallthrough_cases_in_switch,
            "noPropertyAccessFromIndexSignature" => &mut options.no_property_access_from_index_signature,
            "exactOptionalPropertyTypes" => &mut options.exact_optional_property_types,
            "useUnknownInCatchVariables" => &mut options.use_unknown_in_catch_variables,
            "allowUnreachableCode" => &mut options.allow_unreachable_code,
            "allowUnusedLabels" => &mut options.allow_unused_labels,
            "noUnusedLocals" => &mut options.no_unused_locals,
            "noUnusedParameters" => &mut options.no_unused_parameters,
            "noImplicitAny" => &mut options.no_implicit_any,
            "noImplicitThis" => &mut options.no_implicit_this,
            "strictNullChecks" => &mut options.strict_null_checks,
            "strictFunctionTypes" => &mut options.strict_function_types,
            "strictPropertyInitialization" => &mut options.strict_property_initialization,
            "strictBindCallApply" => &mut options.strict_bind_call_apply,
            "noUncheckedSideEffectImports" => &mut options.no_unchecked_side_effect_imports,
            _ => return Err(format!("unknown or unsupported TypeScript flag `{name}`")),
        };
        *slot = value;
    }
    Ok(options)
}

/// Loads the program. `cache` keeps parsed files across loads (watch mode).
pub fn load(opts: &LoadOptions, cache: Option<&Arc<SourceFileCache>>) -> Result<Project, String> {
    let root = tspath::normalize_path(&opts.root);
    let options = compiler_options(&root, &opts.flags)?;
    let macros = macros_file(&root);
    let mut files = FxHashMap::default();
    files.insert(macros.clone(), MACROS_D_TS.to_owned());
    let fs: Arc<dyn FS> = Arc::new(Overlay { inner: Box::new(bundled::wrap_fs(osvfs::fs())), files });

    let mut root_files: Vec<String> = opts.files.iter().map(|f| tspath::normalize_path(f)).collect();
    root_files.push(macros);
    let options = CompilerOptions { checkers: Some(opts.checkers.max(1) as i64), ..options };
    let compare = ComparePathsOptions {
        use_case_sensitive_file_names: fs.use_case_sensitive_file_names(),
        current_directory: root.clone(),
    };
    let config = P::new(new_parsed_command_line(P::new(options), root_files, Vec::new(), compare));

    let start = Instant::now();
    let mut program_options = ProgramOptions::new(config, new_host(&root, &fs, cache));
    program_options.checker_regions = true;
    program_options.checker_diagnostic_regions = true;
    let program = new_program(program_options);
    // bound before any checker exists, so the files outlive the checkers' regions (`Project::free`)
    program.bind_source_files();
    Ok(Project {
        program,
        root,
        load_time: start.elapsed(),
        lineage: Arc::new(Lineage(tsrs_compiler::shared_program_data(program))),
        fs,
        source_cache: cache.cloned(),
    })
}

fn new_host(root: &str, fs: &Arc<dyn FS>, cache: Option<&Arc<SourceFileCache>>) -> Arc<dyn tsrs_compiler::CompilerHost> {
    let host = new_cached_fs_compiler_host(root, Arc::clone(fs), &bundled::default_lib_path(), None, None, None);
    match cache {
        Some(cache) => Arc::new(CachingHost { inner: host, cache: Arc::clone(cache) }),
        None => host,
    }
}

impl Project {
    /// Frees the program with its checkers and everything allocated while one was held (types, symbols,
    /// diagnostics, lowered nodes) and, with the last program of its lineage, the loader's data. Parsed files stay.
    ///
    /// # Safety
    ///
    /// Nothing made by the program's checkers or under them is read again: the caller copied the diagnostics it
    /// keeps, and no lowered tree refers to the program (lowered Luau is plain owned data).
    pub unsafe fn free(self) {
        // SAFETY: the caller's contract, which includes `free_program`'s.
        unsafe { tsrs_compiler::free_program(self.program) };
        if let Ok(lineage) = Arc::try_unwrap(self.lineage) {
            // SAFETY: every other program of the lineage was freed (each held a reference to `lineage` until then).
            unsafe { lineage.0.free() };
        }
    }

    /// A program for edited files (`changed`, file names), made from this one with tsrs's `reuse_program`: each
    /// edited file is parsed again; every other file, the module resolution and the options are this program's.
    /// `None` when tsrs cannot reuse it (imports changed, or a file is not in the program): load again.
    pub fn reuse(&self, changed: &[String]) -> Option<Project> {
        let host = new_host(&self.root, &self.fs, self.source_cache.as_ref());
        let case_sensitive = self.fs.use_case_sensitive_file_names();
        let start = Instant::now();
        let mut program = self.program;
        for file_name in changed {
            let path = tspath::to_path(file_name, &self.root, case_sensitive);
            program.get_source_file_by_path(&path)?;
            let (reused, _, ok) = program.reuse_program(&path, Arc::clone(&host), None, None);
            if !std::ptr::eq(program, self.program) {
                // SAFETY: a program made by an earlier turn of this loop: no checker of it was made, and the one
                // reused from it shares only the lineage's data, which stays.
                unsafe { tsrs_compiler::free_program(program) };
            }
            program = reused.filter(|_| ok)?;
        }
        program.bind_source_files();
        Some(Project {
            program,
            root: self.root.clone(),
            load_time: start.elapsed(),
            fs: Arc::clone(&self.fs),
            source_cache: self.source_cache.clone(),
            lineage: Arc::clone(&self.lineage),
        })
    }

    /// The files opossum lowers: not declaration files, not JSON modules, not from packages.
    pub fn project_files(&self) -> Vec<P<SourceFile>> {
        self.program
            .source_files()
            .iter()
            .copied()
            .filter(|&f| {
                !f.is_declaration_file()
                    && !is_json_source_file(f)
                    && !self.program.is_source_file_from_external_library(f)
            })
            .collect()
    }

    /// Syntactic, bind and semantic diagnostics of `files` (each on the checker that owns it, one thread per
    /// checker), then the program's and the global diagnostics; sorted and deduplicated.
    pub fn check(&self, ctx: &Context, files: &[P<SourceFile>]) -> Vec<P<Diagnostic>> {
        let program = self.program;
        let mut per_file: Vec<Vec<P<Diagnostic>>> =
            files.iter().map(|&f| program.get_syntactic_diagnostics(ctx, Some(f))).collect();
        let index: HashMap<P<SourceFile>, usize> = files.iter().enumerate().map(|(i, &f)| (f, i)).collect();
        let results = Mutex::new(Vec::with_capacity(files.len()));
        program.for_each_checker_group(files, |checker, _, f| {
            let diags = program.get_semantic_diagnostics_with_checker(ctx, checker, f);
            results.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((index[&f], diags));
        });
        for (i, diags) in results.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
            per_file[i].extend(diags);
        }
        let mut diags: Vec<P<Diagnostic>> = program.get_config_file_parsing_diagnostics();
        diags.extend(program.get_program_diagnostics());
        diags.extend(per_file.into_iter().flatten());
        diags.extend(program.get_global_diagnostics(ctx));
        sort_and_deduplicate_diagnostics(&diags)
    }
}
