//! rbxts-rs only: TypeScript 5's `program.getSymlinkCache().getSymlinkedDirectoriesByRealpath()`, the map
//! `TransformState::guess_virtual_path` reads.
//!
//! tsrs has the TypeScript 7 cache (`Program::get_symlink_cache`), which differs in what `guessVirtualPath` sees:
//! - it keeps a directory's symlinks in a set, where TypeScript 5 keeps them in the order it met them and
//!   `guessVirtualPath` takes the first;
//! - it also learns links from the `package.json` dependencies of emitted files up front. TypeScript 5.9's
//!   `getSymlinkCache` starts from module and type reference resolutions; its module-specifier generation
//!   (`getAllModulePathsWorker`, run by declaration emit for one) adds package.json runtime-dependency links later,
//!   after the resolution-derived ones. That later addition is not reproduced: it can only matter when a real
//!   directory has no link from a resolution, a narrow corner for roblox-ts output.
//!
//! So the map is rebuilt here the TypeScript 5 way (`setSymlinksFromResolutions`, `processResolution`,
//! `guessDirectorySymlink`, `setSymlinkedDirectory`): over the resolved modules of each file, in the order TypeScript
//! 5's program processes files, then each file's type reference directives in the same order, then the automatic type
//! directives. TypeScript 5 processes each root file depth first (`/// <reference path>`, then `/// <reference types>`,
//! then imports), then the automatic type directives; a file's whole resolution cache enters the map before the files
//! it names are processed, so a file's resolutions all come before those of the files it reaches.

use rustc_hash::{FxHashMap, FxHashSet};
use tsrs_ast::{Kind, SourceFile};
use tsrs_compiler::Program;
use tsrs_core::P;
use tsrs_core::tspath::{self, Path};

/// `ts.inferredTypesContainingFile`: the containing file of the automatic type directive resolutions.
const INFERRED_TYPES_CONTAINING_FILE: &str = "__inferred type names__.ts";

/// One resolution's `originalPath` (the path through the link) and `resolvedFileName` (the real path).
struct Resolution {
    original_path: &'static str,
    resolved_file_name: &'static str,
}

/// TypeScript 5's symlinked directories by real path: real directory (a canonical path with a trailing separator)
/// → the symlinks to it, in the order TypeScript 5 meets them.
#[derive(Default)]
pub struct SymlinkedDirectories {
    by_realpath: FxHashMap<Path, Vec<String>>,
}

impl SymlinkedDirectories {
    /// The map for `program`.
    pub fn new(program: &Program) -> SymlinkedDirectories {
        let walk = Walk::new(program);
        let cwd = program.get_current_directory();
        let case_sensitive = program.use_case_sensitive_file_names();
        let mut cache = SymlinkedDirectories::default();
        let mut seen_symlinks = FxHashSet::default();
        for resolution in walk.modules.iter().chain(&walk.type_references).chain(&walk.automatic_type_references) {
            cache.process_resolution(resolution, cwd, case_sensitive, &mut seen_symlinks);
        }
        cache
    }

    /// The first symlink TypeScript 5 met to the real directory `real_path` (a canonical path with a trailing
    /// separator).
    pub fn first_symlink(&self, real_path: &Path) -> Option<&str> {
        self.by_realpath.get(real_path).and_then(|symlinks| symlinks.first()).map(String::as_str)
    }

    /// `processResolution` and `setSymlinkedDirectory`, for the directory part (files are not needed).
    fn process_resolution(
        &mut self,
        resolution: &Resolution,
        cwd: &str,
        case_sensitive: bool,
        seen_symlinks: &mut FxHashSet<Path>,
    ) {
        if resolution.original_path.is_empty() || resolution.resolved_file_name.is_empty() {
            return;
        }
        let Some((common_resolved, common_original)) =
            guess_directory_symlink(resolution.resolved_file_name, resolution.original_path, cwd, case_sensitive)
        else {
            return;
        };
        let symlink_path = tspath::to_path(&common_original, cwd, case_sensitive);
        if tspath::contains_ignored_path(symlink_path.as_str()) {
            return;
        }
        let symlink_path = symlink_path.ensure_trailing_directory_separator();
        if seen_symlinks.insert(symlink_path) {
            let real_path = tspath::to_path(&common_resolved, cwd, case_sensitive).ensure_trailing_directory_separator();
            self.by_realpath.entry(real_path).or_default().push(common_original);
        }
    }
}

/// `guessDirectorySymlink`: the longest common suffix of the two paths, above `node_modules` and scope directories,
/// as (real directory, symlink directory).
fn guess_directory_symlink(a: &str, b: &str, cwd: &str, case_sensitive: bool) -> Option<(String, String)> {
    let mut a_parts = tspath::get_path_components(&tspath::get_normalized_absolute_path(a, cwd), "");
    let mut b_parts = tspath::get_path_components(&tspath::get_normalized_absolute_path(b, cwd), "");
    let is_scope_or_node_modules = |s: &str| {
        !s.is_empty() && (tspath::get_canonical_file_name(s, case_sensitive) == "node_modules" || s.starts_with('@'))
    };
    let mut is_directory = false;
    while a_parts.len() >= 2
        && b_parts.len() >= 2
        && !is_scope_or_node_modules(&a_parts[a_parts.len() - 2])
        && !is_scope_or_node_modules(&b_parts[b_parts.len() - 2])
        && tspath::get_canonical_file_name(&a_parts[a_parts.len() - 1], case_sensitive)
            == tspath::get_canonical_file_name(&b_parts[b_parts.len() - 1], case_sensitive)
    {
        a_parts.pop();
        b_parts.pop();
        is_directory = true;
    }
    is_directory
        .then(|| (tspath::get_path_from_path_components(&a_parts), tspath::get_path_from_path_components(&b_parts)))
}

/// The program's resolutions in TypeScript 5's processing order (`createProgram`, `findSourceFileWorker`).
struct Walk<'p> {
    program: &'p Program,
    visited: FxHashSet<P<SourceFile>>,
    /// `resolvedModules`: per file in the order its entry was made, each file's in module name order.
    modules: Vec<Resolution>,
    /// `resolvedTypeReferenceDirectiveNames`, likewise.
    type_references: Vec<Resolution>,
    /// `automaticTypeDirectiveResolutions`.
    automatic_type_references: Vec<Resolution>,
}

impl<'p> Walk<'p> {
    fn new(program: &'p Program) -> Walk<'p> {
        let mut walk = Walk {
            program,
            visited: FxHashSet::default(),
            modules: Vec::new(),
            type_references: Vec::new(),
            automatic_type_references: Vec::new(),
        };
        let command_line = program.command_line();
        for root in command_line.file_names() {
            walk.visit_file_name(root);
        }
        walk.automatic_type_directives();
        walk
    }

    fn visit_file_name(&mut self, file_name: &str) {
        if let Some(file) = self.program.get_source_file(file_name) {
            self.visit(file);
        }
    }

    /// `findSourceFileWorker` for a new file: referenced files, type reference directives, then imports.
    fn visit(&mut self, file: P<SourceFile>) {
        if !self.visited.insert(file) {
            return;
        }
        let program = self.program;
        for reference in file.referenced_files() {
            self.visit_file_name(&tspath::resolve_tripleslash_reference(&reference.file_name, file.file_name()));
        }

        // processTypeReferenceDirectives: the file's whole cache goes into the map before the loop that visits the
        // files it names, so all of a file's resolutions come before its children's
        let mut children = Vec::new();
        for &reference in file.type_reference_directives() {
            let Some(resolved) =
                program.get_resolved_type_reference_directive_from_type_reference_directive(reference, file)
            else {
                continue;
            };
            self.type_references.push(Resolution {
                original_path: resolved.original_path,
                resolved_file_name: resolved.resolved_file_name,
            });
            if resolved.is_resolved() {
                children.push(resolved.resolved_file_name);
            }
        }
        for child in children.drain(..) {
            self.visit_file_name(child);
        }

        // processImportedModules: imports, then string-named module augmentations (not followed); as above, the file's
        // resolutions before its children's
        let imports = file.imports();
        let augmentations = file.module_augmentations.get().iter().filter(|a| a.kind() == Kind::StringLiteral);
        let mut names = FxHashSet::default();
        for (index, &name) in imports.iter().chain(augmentations).enumerate() {
            let mode = program.get_mode_for_usage_location(file, name);
            let Some(resolved) = program.get_resolved_module(file, name.text(), mode) else { continue };
            // a ModeAwareCache keeps the first position of a name it sees again
            if names.insert((name.text(), mode)) {
                self.modules.push(Resolution {
                    original_path: resolved.original_path,
                    resolved_file_name: resolved.resolved_file_name,
                });
            }
            if resolved.is_resolved() && index < imports.len() {
                children.push(resolved.resolved_file_name);
            }
        }
        for child in children {
            self.visit_file_name(child);
        }
    }

    /// The `types` option's resolutions (or every type root package's), after the root files.
    fn automatic_type_directives(&mut self) {
        let program = self.program;
        let Some((_, resolutions)) = program
            .get_resolved_type_reference_directives()
            .iter()
            .find(|(path, _)| path.as_str().ends_with(&format!("/{INFERRED_TYPES_CONTAINING_FILE}")))
        else {
            return;
        };
        // `getAutomaticTypeDirectiveNames`: the `types` option as written, else the type roots' packages
        let mut names: Vec<(&str, P<_>)> = resolutions.iter().map(|(key, resolved)| (key.name, *resolved)).collect();
        match &program.options().types {
            Some(types) => names.sort_by_key(|(name, _)| types.iter().position(|t| t == name)),
            // rbxts-rs: unreachable on tsrs, which (as TypeScript 6+) loads no automatic type directives when `types` is
            // unset; TypeScript 5 would take the type roots in order, each in directory order
            None => names.sort_by_key(|(name, _)| *name),
        }
        for (_, resolved) in names {
            self.automatic_type_references.push(Resolution {
                original_path: resolved.original_path,
                resolved_file_name: resolved.resolved_file_name,
            });
            if resolved.is_resolved() {
                self.visit_file_name(resolved.resolved_file_name);
            }
        }
    }
}
