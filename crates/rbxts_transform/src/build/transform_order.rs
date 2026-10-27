//! rbxts-rs only: the order roblox-ts transforms a clean build's files in, `getChangedFilePaths` on a fresh builder
//! (`Project/functions/getChangedFilePaths.ts`, TypeScript 5's `BuilderState`).
//!
//! roblox-ts transforms its files, and runs the TS transformer plugins over them, in the order of the changed-file set:
//! each file of the builder's `changedFilesSet` (a fresh builder's holds every program file, in program order) is
//! searched depth first through the reversed `referencedMap` (the files that import it, in program order), adding
//! every file met. So a file is followed at once by its importers, transitively, and that order is visible: TS5
//! Flamework allocates its declaration ids, and fills `flamework.build`'s identifier table, in it. rbxts-rs makes it
//! the order of a build's files (transform, reports, [`rbxts_plugin::FileIndex`]); every build of a program uses the
//! clean build's order, so a watch rebuild or an incremental build orders its files as a clean build does. It is a
//! function of the program, the same for any checker count.
//!
//! `referencedMap` is TypeScript 5's `getReferencedFiles`, rebuilt from the program's resolutions instead of the
//! checker (`getSymbolAtLocation` of each import's module name gives the declaring files of the module symbol):
//! - each import and module augmentation (type-only imports, `export … from`, `import()` and `import("…")` types
//!   included) references what TypeScript 5's `resolveExternalModule` gives for its name: an ambient module of that
//!   name (`declare module "name"` in a script file; a non-relative name only), even when the name also resolves to
//!   a file; else the file it resolves to; else the pattern ambient module (`declare module "*.png"`) whose prefix
//!   is the longest among those matching it (the first in program order on a tie);
//! - an ambient or pattern ambient module references the files declaring it and the module files augmenting it by
//!   that name (`declare module "name"` in a module file); a module file references the module's file and every
//!   file that augments that module;
//! - triple-slash path references (the path as written, resolved against the file's directory) and the file's
//!   resolved type reference directives;
//! - every file references the files declaring an ambient module with more than one declaration (other than itself).

use rustc_hash::{FxHashMap, FxHashSet};
use tsrs_ast::SourceFile;
use tsrs_compiler::Program;
use tsrs_core::P;
use tsrs_core::tspath;

/// Every program file (declaration files included), in roblox-ts's clean-build transform order.
pub fn transform_order(program: &Program) -> Vec<P<SourceFile>> {
    let files = program.source_files();
    let cwd = program.get_current_directory();
    let case_sensitive = program.use_case_sensitive_file_names();
    // a referenced project's source resolves to the declaration the program holds for it
    let redirects = !program.get_resolved_project_references().is_empty();
    let key = |name: &str| {
        tspath::to_path(&super::graph::program_file_of(program, redirects, name), cwd, case_sensitive).to_string()
    };
    let index: FxHashMap<String, usize> = files.iter().enumerate().map(|(i, f)| (f.path().to_string(), i)).collect();
    let resolved_modules = program.get_resolved_modules();
    let type_references = program.get_resolved_type_reference_directives();

    // the files declaring each ambient module (one entry per declaration), and the pattern ambient modules in program
    // order (TypeScript 5's `patternAmbientModules`: names with exactly one `*`)
    let mut ambient: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
    let mut patterns: Vec<(&str, &str, &str)> = Vec::new();
    for (i, &file) in files.iter().enumerate() {
        for &name in file.ambient_module_names() {
            ambient.entry(name).or_default().push(i);
            if let Some((prefix, suffix)) = name.split_once('*')
                && !suffix.contains('*')
            {
                patterns.push((name, prefix, suffix));
            }
        }
    }
    // `tryFindAmbientModule`: an ambient module by that name, which a non-relative name gives before its resolution
    let ambient_module = |name: &str| (!tspath::is_external_module_name_relative(name)).then(|| ambient.get(name)).flatten();
    // `findBestPatternMatch`: the longest prefix wins, the first one on a tie
    let pattern_module = |name: &str| {
        let mut best: Option<(&str, usize)> = None;
        for &(pattern, prefix, suffix) in &patterns {
            let matches = name.len() >= prefix.len() + suffix.len() && name.starts_with(prefix) && name.ends_with(suffix);
            if matches && best.is_none_or(|(_, longest)| prefix.len() > longest) {
                best = Some((pattern, prefix.len()));
            }
        }
        best.map(|(pattern, _)| pattern)
    };
    // the modules each file augments: by the module's file, or by name for an ambient module (or a name that resolves
    // to no file)
    let mut augmenters: FxHashMap<String, Vec<usize>> = FxHashMap::default();
    let mut ambient_augmenters: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
    for (i, &file) in files.iter().enumerate() {
        let augmentations: FxHashSet<&str> = file
            .module_augmentations()
            .iter()
            .filter(|&&name| tsrs_ast::is_string_literal(name))
            .map(|name| name.text())
            .collect();
        if augmentations.is_empty() {
            continue;
        }
        let mut to_files: FxHashSet<&str> = FxHashSet::default();
        if let Some(resolutions) = resolved_modules.get(file.path()) {
            for (cache_key, resolved) in resolutions {
                if augmentations.contains(cache_key.name) && resolved.is_resolved() && ambient_module(cache_key.name).is_none()
                {
                    augmenters.entry(key(resolved.resolved_file_name)).or_default().push(i);
                    to_files.insert(cache_key.name);
                }
            }
        }
        for &name in augmentations.iter().filter(|name| !to_files.contains(*name)) {
            ambient_augmenters.entry(name).or_default().push(i);
        }
    }
    // `getAmbientModules()` with more than one declaration: every file references their declaring files
    let mut everyone: Vec<usize> =
        ambient.values().filter(|declaring| declaring.len() > 1).flatten().copied().collect();
    everyone.sort_unstable();
    everyone.dedup();

    // the reversed referencedMap: per referenced file, the files referencing it, in program order
    let mut importers: Vec<Vec<usize>> = vec![Vec::new(); files.len()];
    let mut referenced: FxHashSet<usize> = FxHashSet::default();
    for (i, &file) in files.iter().enumerate() {
        referenced.clear();
        let add_key = |referenced: &mut FxHashSet<usize>, path: &str| {
            if let Some(&target) = index.get(path) {
                referenced.insert(target);
            }
        };
        if let Some(resolutions) = resolved_modules.get(file.path()) {
            for (cache_key, resolved) in resolutions {
                // TS5's `getSymbolAtLocation` gives the merged ambient module: its declarations and the module files
                // augmenting it (they don't count in `getAmbientModules`, whose symbol is the unmerged one)
                let name = cache_key.name;
                let declaring = match ambient_module(name) {
                    Some(declaring) => Some(declaring),
                    None if resolved.is_resolved() => {
                        let module = key(resolved.resolved_file_name);
                        add_key(&mut referenced, &module);
                        if let Some(augmenting) = augmenters.get(&module) {
                            referenced.extend(augmenting.iter().copied());
                        }
                        continue;
                    }
                    None => pattern_module(name).and_then(|p| ambient.get(p)),
                };
                if let Some(declaring) = declaring {
                    referenced.extend(declaring.iter().copied());
                    if let Some(augmenting) = ambient_augmenters.get(name) {
                        referenced.extend(augmenting.iter().copied());
                    }
                }
            }
        }
        let directory = tspath::get_directory_path(file.file_name());
        for reference in file.referenced_files() {
            add_key(&mut referenced, tspath::to_path(&reference.file_name, &directory, case_sensitive).as_str());
        }
        if let Some(directives) = type_references.get(file.path()) {
            for resolved in directives.values() {
                if !resolved.resolved_file_name.is_empty() {
                    add_key(&mut referenced, &key(resolved.resolved_file_name));
                }
            }
        }
        referenced.extend(everyone.iter().copied().filter(|&d| d != i));
        for &target in &referenced {
            importers[target].push(i);
        }
    }

    // `search`, depth first from each file in program order; a file joins the order when first met
    let mut seen = vec![false; files.len()];
    let mut order = Vec::with_capacity(files.len());
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..files.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        order.push(files[start]);
        stack.push((start, 0));
        while let Some((file, next)) = stack.last_mut() {
            match importers[*file].get(*next) {
                Some(&importer) => {
                    *next += 1;
                    if !seen[importer] {
                        seen[importer] = true;
                        order.push(files[importer]);
                        stack.push((importer, 0));
                    }
                }
                None => {
                    stack.pop();
                }
            }
        }
    }
    order
}
