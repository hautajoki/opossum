//! The project's outputs beyond the Luau of its source files (roblox-ts's `Project/functions/getProjectOutputs.ts`):
//! the assets copied from the root directories, the set of files the clean keeps, and the clean that deletes every
//! other file under the output roots after a successful build ("stale outputs": the outputs of deleted or renamed
//! sources, and anything else roblox-ts would remove).
//!
//! The keep set follows `getProjectOutputs` rule by rule, because the clean deletes files a user may care about: it
//! must never delete a file roblox-ts keeps. Per emitted program file (not JSON): its Luau output, its
//! `.transformed.ts` with `writeTransformedFiles` and tsconfig plugins, and the `.d.ts` / `.d.ts.map` TypeScript
//! would emit for it (`ts.getOutputFileNames`, in the `declarationDir` or `outDir`). Per asset in the root
//! directories (outside the output roots and the include path, `node_modules` and `.git`; not a config, a build info
//! or a deployment `*.project.json` beside the tsconfig; `.ts` / `.tsx` only when they are declaration files of a
//! declaration build): its output. Then the build infos (roblox-ts's and tsc's own) and every Rojo project file.
//! In a build with project references ([`GraphOutputs`]), no project takes another's output roots, include folder,
//! build infos or configs for assets, and the keep set holds every Rojo project file of the graph and every runtime
//! project's include folder.
//!
//! The clean, as roblox-ts's: every root is walked without following links; a directory (not a link) is cleaned
//! and removed once empty; anything else that is not kept (files, and links to files or directories, which are
//! removed themselves, never their targets) is deleted; `.git` is skipped.
//!
//! rbxts-rs: differences from roblox-ts, all keeping more or writing less:
//! - assets are written only when their bytes changed (DESIGN §14 commit step 1), not copied on every build;
//! - the include folder is kept whole when it lies under an output root (DESIGN §14: "The include folder is in the
//!   clean step's keep set"), not only the runtime files it copies ([`crate::project::copy_include`]);
//! - plugin artifacts and state files of this build are kept;
//! - declaration assets (project `.d.ts` files of a declaration build) are written only when their printed text
//!   ([`crate::project::transform_declaration_file`]) changed, like the other assets;
//! - an output whose spelling differs from the file on disk only in case is renamed to the build's spelling (a
//!   case-only rename of a source), so the Rojo instance name follows, as in a build into an empty directory.

use std::io;
use std::path::{Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_project::{ProjectServices, nodepath};

use crate::plugins::lifecycle::{commit_staged, remove_file, stage_if_changed_timed};
use crate::project::copy_include::effective_include_path;

/// The outputs of one build.
#[derive(Debug, Default)]
pub struct ProjectOutputs {
    /// The roots the clean walks: the outDir (or its override) and the `declarationDir`.
    pub roots: Vec<String>,
    /// The path keys of every file the clean keeps.
    pub keep: FxHashSet<String>,
    /// Directories the clean never enters (the include path).
    pub keep_dirs: Vec<String>,
    /// Assets: (input, output), copied when their bytes differ.
    pub assets: Vec<(String, String)>,
    /// Declaration assets (`.d.ts` files under a root directory of a declaration build): (input, output), whose
    /// output is the input printed again ([`crate::project::transform_declaration_file`]).
    pub declaration_assets: Vec<(String, String)>,
    /// Every output path of the build (Luau, declarations, transformed files, assets, declaration assets), which
    /// plugin artifacts may not overwrite.
    pub files: Vec<String>,
}

/// A path as a comparison key: normalised, forward slashes, lowercased unless the file system is case-sensitive.
pub fn path_key(services: &ProjectServices, path: &str) -> String {
    let normalized = nodepath::normalize(path).replace('\\', "/");
    if services.use_case_sensitive_file_names { normalized } else { normalized.to_lowercase() }
}

/// `isPathDescendantOf` on keys: `path` is `root` or inside it.
pub(super) fn is_descendant_key(path: &str, root: &str) -> bool {
    path == root || (path.len() > root.len() && path.starts_with(root) && path.as_bytes()[root.len()] == b'/')
}

/// What [`get_project_outputs`] needs to know of the build.
pub struct OutputInputs<'a> {
    /// The project's setup (paths, the path translator).
    pub services: &'a ProjectServices,
    /// The program's compiler options (root directories, declarations).
    pub options: &'a tsrs_core::CompilerOptions,
    /// Per emitted program file: its input, its Luau output (after the override) and the declaration outputs
    /// TypeScript would emit for it (`.d.ts`, `.d.ts.map`).
    pub emitted: &'a [(String, String, Vec<String>)],
    /// `writeTransformedFiles` with tsconfig plugins: the `.transformed.ts` outputs are kept.
    pub transformed_outputs: bool,
    /// The outDir override of the build, if any.
    pub out_dir_override: Option<&'a str>,
    /// tsconfig and `extends` paths, never assets.
    pub config_paths: &'a [String],
    /// tsc's own build info (`getTsBuildInfoEmitOutputFilePath`), besides roblox-ts's `.rbxtsc.tsbuildinfo`.
    pub ts_build_info: Option<&'a str>,
    /// The other projects of a build with project references.
    pub graph: Option<&'a GraphOutputs>,
}

/// What a project of a build with project references knows of the graph's outputs (`getProjectOutputs` with the
/// whole `ProjectGraph`).
#[derive(Clone, Debug, Default)]
pub struct GraphOutputs {
    /// Every project's output roots and the runtime projects' include folders: never assets.
    pub excluded_roots: Vec<String>,
    /// Every project's build infos: never assets.
    pub build_infos: Vec<String>,
    /// Every tsconfig and extended config, and the Rojo project files: never assets.
    pub config_paths: Vec<String>,
    /// Every project's Rojo project files: kept.
    pub rojo_files: Vec<String>,
    /// The runtime projects' include folders: kept whole.
    pub include_paths: Vec<String>,
}

/// Moves an output path under the override, as the Luau outputs are.
pub fn overridden(services: &ProjectServices, path: &str, out_dir_override: Option<&str>) -> String {
    match out_dir_override {
        Some(dir)
            if is_descendant_key(&path_key(services, path), &path_key(services, &services.path_translator.out_dir)) =>
        {
            let relative = nodepath::relative(&services.path_translator.out_dir, path);
            nodepath::join(&[dir, &relative])
        }
        _ => path.to_owned(),
    }
}

/// `getProjectOutputs`: the roots, the keep set and the assets. Fails like roblox-ts when an output root contains a
/// source root ("Output directory … contains source directory …"), which also keeps the clean away from sources, and
/// when two inputs emit to one output.
pub fn get_project_outputs(inputs: &OutputInputs<'_>) -> Result<ProjectOutputs, String> {
    let services = inputs.services;
    let translator = &services.path_translator;
    let mut outputs = ProjectOutputs {
        roots: output_roots(services, inputs.options, inputs.out_dir_override),
        ..ProjectOutputs::default()
    };

    let root_dirs: Vec<String> = {
        let mut dirs = rbxts_project::setup::get_root_dirs(inputs.options);
        if dirs.is_empty() && inputs.options.composite.is_true() {
            dirs.push(services.data.project_path.clone());
        }
        dirs.iter().map(|d| nodepath::resolve(&[d])).collect()
    };
    for root in &outputs.roots {
        let root_key = path_key(services, root);
        for source in &root_dirs {
            if is_descendant_key(&path_key(services, source), &root_key) {
                return Err(format!("Output directory \"{root}\" contains source directory \"{source}\"."));
            }
        }
    }

    // addOutput: an output with two inputs is an error
    let mut owners: FxHashMap<String, String> = FxHashMap::default();
    let mut add_output = |outputs: &mut ProjectOutputs, output: &str, input: &str| -> Result<(), String> {
        let key = path_key(services, output);
        if let Some(previous) = owners.get(&key)
            && previous != input
        {
            return Err(format!("Files \"{previous}\" and \"{input}\" both emit to \"{output}\"."));
        }
        owners.insert(key.clone(), input.to_owned());
        outputs.keep.insert(key);
        outputs.files.push(output.to_owned());
        Ok(())
    };
    for (input, luau, declarations) in inputs.emitted {
        add_output(&mut outputs, luau, input)?;
        if inputs.transformed_outputs {
            let transformed =
                overridden(services, &translator.get_output_transformed_path(input), inputs.out_dir_override);
            add_output(&mut outputs, &transformed, input)?;
        }
        for declaration in declarations {
            add_output(&mut outputs, declaration, input)?;
        }
    }

    let include = nodepath::resolve(&[&services.data.options.include_path]);
    // every output root, with or without the override (roblox-ts: `getOutputRoots` of every project)
    let mut excluded: Vec<String> = outputs.roots.iter().map(|r| path_key(services, r)).collect();
    excluded.extend(output_roots(services, inputs.options, None).iter().map(|r| path_key(services, r)));
    excluded.push(path_key(services, &include));
    let graph = inputs.graph.cloned().unwrap_or_default();
    excluded.extend(graph.excluded_roots.iter().map(|r| path_key(services, r)));
    let config_keys: FxHashSet<String> =
        inputs.config_paths.iter().chain(&graph.config_paths).map(|p| path_key(services, p)).collect();
    let build_infos: FxHashSet<String> = translator
        .build_info_output_path
        .iter()
        .map(String::as_str)
        .chain(inputs.ts_build_info)
        .chain(graph.build_infos.iter().map(String::as_str))
        .map(|p| path_key(services, p))
        .collect();
    let project_dir_key = path_key(services, &services.data.project_path);

    // (path, is a directory); entries' types come from the directory listing (links are followed, as `statSync`)
    let mut stack: Vec<(String, bool)> =
        root_dirs.iter().map(|d| (d.clone(), std::fs::metadata(d).is_ok_and(|m| m.is_dir()))).collect();
    let mut assets = Vec::new();
    let mut declaration_assets = Vec::new();
    // the directories the walk enters, listed ahead in parallel (the walk below visits them in its own order)
    let skipped = |path: &str| {
        let name = nodepath::basename(path);
        let key = path_key(services, path);
        name == "node_modules" || name == ".git" || excluded.iter().any(|root| is_descendant_key(&key, root))
    };
    let roots: Vec<String> = stack.iter().filter(|(d, is_dir)| *is_dir && !skipped(d)).map(|(d, _)| d.clone()).collect();
    let listings = super::parallel::prefetch(&roots, |dir, entry| {
        let file_type = entry.file_type().ok()?;
        let is_dir = if file_type.is_symlink() {
            std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
        } else {
            file_type.is_dir()
        };
        let path = nodepath::join(&[dir, &entry.file_name().to_string_lossy()]);
        (is_dir && !skipped(&path)).then_some(path)
    });
    while let Some((input, is_dir)) = stack.pop() {
        let key = path_key(services, &input);
        if excluded.iter().any(|root| is_descendant_key(&key, root)) {
            continue;
        }
        if is_dir {
            let name = nodepath::basename(&input);
            if name == "node_modules" || name == ".git" {
                continue;
            }
            let Some(entries) = listings.entries(&input) else { continue };
            let mut children = Vec::new();
            for entry in entries {
                let Ok(file_type) = entry.file_type() else { continue };
                let is_dir = if file_type.is_symlink() {
                    std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
                } else {
                    file_type.is_dir()
                };
                children.push((nodepath::join(&[&input, &entry.file_name().to_string_lossy()]), is_dir));
            }
            // pushed in reverse, so the walk visits them in directory order
            children.reverse();
            stack.extend(children);
            continue;
        }
        let base = nodepath::basename(&input);
        let is_deployment_config = path_key(services, nodepath::dirname(&input)) == project_dir_key
            && base.len() > ".project.json".len()
            && base.ends_with(".project.json");
        if config_keys.contains(&key) || build_infos.contains(&key) || is_deployment_config {
            continue;
        }
        let output = overridden(services, &translator.get_output_path(&input), inputs.out_dir_override);
        if input.ends_with(".ts") || input.ends_with(".tsx") {
            if !tsrs_core::tspath::is_declaration_file_name(&input) || !translator.declaration {
                continue;
            }
            add_output(&mut outputs, &output, &input)?;
            declaration_assets.push((input, output));
            continue;
        }
        add_output(&mut outputs, &output, &input)?;
        assets.push((input, output));
    }
    assets.sort();
    outputs.assets = assets;
    declaration_assets.sort();
    outputs.declaration_assets = declaration_assets;

    for build_info in translator.build_info_output_path.iter().map(String::as_str).chain(inputs.ts_build_info) {
        outputs.keep.insert(path_key(services, build_info));
    }
    for config in services.rojo_resolver.config_files().iter().chain(&graph.rojo_files) {
        outputs.keep.insert(path_key(services, config));
    }
    // the include folder this build writes: under the override when it lies in the outDir
    outputs.keep_dirs.push(effective_include_path(services, inputs.out_dir_override));
    // the runtime projects' include folders (roblox-ts keeps their runtime files)
    outputs.keep_dirs.extend(graph.include_paths.iter().cloned());
    Ok(outputs)
}

/// The assets' bytes, in `outputs.assets` order, then the declaration assets' printed texts, in
/// `outputs.declaration_assets` order: read before the commit writes anything.
pub fn read_assets(
    outputs: &ProjectOutputs,
    paths: &crate::project::transformers::transform_paths::TransformPaths,
) -> Result<Vec<Vec<u8>>, String> {
    let read = |input: &String| std::fs::read(input).map_err(|e| format!("could not read {input}: {e}"));
    let mut bytes: Vec<Vec<u8>> = outputs.assets.iter().map(|(input, _)| read(input)).collect::<Result<_, _>>()?;
    for (input, _) in &outputs.declaration_assets {
        let text = String::from_utf8_lossy(&read(input)?).into_owned();
        bytes.push(
            crate::project::transform_declaration_file::transform_declaration_file(paths, input, &text).into_bytes(),
        );
    }
    Ok(bytes)
}

/// Commit step 1 for the Luau outputs and the assets: `(path, bytes, match case)` per file, all or nothing (DESIGN
/// §14: a failing build writes nothing).
/// 1. Every file whose bytes differ (or that is missing) is staged in a temporary file beside it, in parallel
///    ([`super::parallel::map`]); a directory in the way is an error here. When any staging fails, the temporary
///    files and the directories staging created are removed and nothing else happens.
/// 2. With `listing`, the case of each output flagged so is matched ([`match_case`]), one file at a time, since a
///    case-only rename can rename a directory other outputs share (a temporary file inside moves with it). Every
///    file is tried; each failure is reported.
/// 3. The old bytes of each output about to be replaced are kept: a hard link beside it (a copy where the file
///    system has none), so nothing is renamed away and the replacement stays one atomic rename per file.
/// 4. The temporary files are renamed into place, in parallel.
///
/// A failure in step 2, 3 or 4 (a rename the file system refuses, a target another process keeps holding past the
/// retries) rolls the commit back: the outputs replaced so far get their kept bytes again (an atomic rename back),
/// the new ones are removed, the case renames are undone, the directories staging created are removed. The tree is
/// then the last build's again, one rename at a time as it was written (no file system renames several files at
/// once, and Rojo syncs each rename as it sees it). Only a rollback step the file system refuses too leaves a file of
/// this build in place; the error says which, and the next build repairs it. Per file, in `files` order: what the
/// commit did ([`Written`]), or the error (a file not written because another failed: `Ok` without a stamp).
pub fn write_outputs(
    services: &ProjectServices,
    listing: Option<&Listing>,
    files: &[(&str, &[u8], bool)],
) -> Vec<Result<Written, String>> {
    let error = |path: &str, e: io::Error| format!("could not write {path}: {e}");
    // the directories staging may create, deepest first, to remove again when it fails
    let mut created: Vec<&Path> = Vec::new();
    let mut seen: FxHashSet<&Path> = FxHashSet::default();
    for &(path, _, _) in files {
        let mut dir = Path::new(path).parent();
        while let Some(d) = dir
            && !d.as_os_str().is_empty()
            && seen.insert(d)
            && !d.exists()
        {
            created.push(d);
            dir = d.parent();
        }
    }
    created.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    let staged = super::parallel::map(files, |&(path, bytes, _)| stage_if_changed_timed(Path::new(path), bytes));
    let remove_created = || {
        for dir in &created {
            let _ = std::fs::remove_dir(dir);
        }
    };
    if staged.iter().any(Result::is_err) {
        for temp in staged.iter().filter_map(|s| s.as_ref().ok().and_then(|(temp, _)| temp.as_ref())) {
            let _ = std::fs::remove_file(temp);
        }
        remove_created();
        return files
            .iter()
            .zip(staged)
            .map(|(&(path, _, _), s)| s.map(|_| Written::default()).map_err(|e| error(path, e)))
            .collect();
    }
    let (staged, modified): (Vec<Option<PathBuf>>, Vec<u128>) =
        staged.into_iter().map(Result::unwrap_or_default).unzip();
    let mut rollback = Rollback::default();
    // a failure before the renames into place: the files that failed, with their errors
    let mut failed: Vec<(usize, String)> = Vec::new();
    if let Some(listing) = listing {
        for (i, &(path, _, _)) in files.iter().enumerate().filter(|(_, f)| f.2) {
            if let Err(e) = match_case_logged(services, path, listing, &mut rollback.case_renames) {
                failed.push((i, format!("could not rename to {path}: {e}")));
            }
        }
    }
    let jobs: Vec<(usize, &str, &PathBuf)> = files
        .iter()
        .zip(&staged)
        .enumerate()
        .filter_map(|(i, (&(path, _, _), temp))| temp.as_ref().map(|temp| (i, path, temp)))
        .collect();
    if failed.is_empty() {
        let backups = super::parallel::map(&jobs, |&(_, path, _)| keep_old_bytes(Path::new(path)));
        for (&(i, path, _), backup) in jobs.iter().zip(backups) {
            match backup {
                Ok(backup) => rollback.backups.push((path.to_owned(), backup)),
                Err(e) => failed.push((i, format!("could not keep the old bytes of {path}: {e}"))),
            }
        }
    }
    let results: Vec<Result<(), String>> = if failed.is_empty() {
        super::parallel::map(&jobs, |&(_, path, temp)| commit_staged(temp, Path::new(path)).map_err(|e| error(path, e)))
    } else {
        Vec::new()
    };
    if failed.is_empty() && results.iter().all(Result::is_ok) {
        rollback.discard();
        // the stamp of the bytes each path holds now: this build's, with the time they were staged or found with
        return files
            .iter()
            .zip(staged.iter().zip(modified))
            .map(|(&(_, bytes, _), (temp, modified))| {
                let stamp = Stamp { size: bytes.len() as u64, hash: content_hash(bytes), modified };
                Ok(Written { written: temp.is_some(), stamp: Some(stamp) })
            })
            .collect();
    }
    let committed: Vec<&str> =
        jobs.iter().zip(&results).filter(|(_, result)| result.is_ok()).map(|(&(_, path, _), _)| path).collect();
    for (&(i, _, _), result) in jobs.iter().zip(&results) {
        if let Err(message) = result {
            failed.push((i, message.clone()));
        }
    }
    // the temporary files not renamed into place (`commit_staged` removed those whose rename failed)
    for &(_, _, temp) in &jobs {
        let _ = std::fs::remove_file(temp);
    }
    let left = rollback.undo(&committed);
    remove_created();
    let mut out: Vec<Result<Written, String>> = vec![Ok(Written::default()); files.len()];
    for (i, mut message) in failed {
        if !left.is_empty() {
            message.push_str(&format!("; rolling back left this build's {}", left.join(", ")));
        }
        out[i] = Err(message);
    }
    out
}

/// What [`write_outputs`] did for one file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// The file was written (its bytes were missing or differed).
    pub written: bool,
    /// The stamp of the bytes the commit left at the path, made from the bytes given (never read back after the
    /// commit, where a hand edit landing after the rename would pass for the commit's bytes); `None` when the commit
    /// failed.
    pub stamp: Option<Stamp>,
}

/// What [`write_outputs`] undoes when it fails after it started renaming.
#[derive(Default)]
struct Rollback {
    /// The case renames made, in order: (from, to).
    case_renames: Vec<(String, String)>,
    /// Per output to replace: the kept old bytes (`None`: the output is new).
    backups: Vec<(String, Option<PathBuf>)>,
}

impl Rollback {
    /// Undoes the renames into place of `committed` (the kept bytes back, a new output removed) and the case renames;
    /// removes the kept bytes. Returns the outputs it could not restore.
    fn undo(self, committed: &[&str]) -> Vec<String> {
        let committed: FxHashSet<&str> = committed.iter().copied().collect();
        let mut left = Vec::new();
        for (path, backup) in &self.backups {
            let restored = if !committed.contains(path.as_str()) {
                Ok(())
            } else {
                match backup {
                    Some(backup) => commit_staged(backup, Path::new(path)),
                    None => remove_file(Path::new(path)).map(|_| ()),
                }
            };
            if restored.is_err() {
                left.push(path.clone());
            }
            if let Some(backup) = backup {
                let _ = std::fs::remove_file(backup);
            }
        }
        for (from, to) in self.case_renames.iter().rev() {
            if std::fs::rename(to, from).is_err() {
                left.push(to.clone());
            }
        }
        left
    }

    /// The commit went through: the kept bytes go.
    fn discard(self) {
        for backup in self.backups.iter().filter_map(|(_, backup)| backup.as_ref()) {
            let _ = remove_file(backup);
        }
    }
}

/// Keeps an output's bytes before a commit replaces it: a hard link to it beside it (`.<name>.rbxts-old-…`), or a
/// copy where links can't be made. `None` when there is no file yet.
fn keep_old_bytes(path: &Path) -> io::Result<Option<PathBuf>> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let backup = dir.join(format!(".{name}.rbxts-old-{}-{n}", std::process::id()));
    if std::fs::hard_link(path, &backup).is_err() {
        std::fs::copy(path, &backup)?;
    }
    Ok(Some(backup))
}

/// The files under the output roots before a build writes anything, taken once per build: for the missing-output
/// check, the case check and the clean. The include folder and `.git` are not entered, nor links.
#[derive(Debug, Default)]
pub struct Listing {
    /// (path, key) of every file, and of every link (to a file or a directory).
    pub files: Vec<(String, String)>,
    /// Path key → the path as spelled on disk.
    pub spelled: FxHashMap<String, String>,
    /// The links among `files` (removed as links, never followed).
    pub links: FxHashSet<String>,
    /// The directories that were empty (roblox-ts's clean removes empty directories too).
    pub empty_dirs: Vec<String>,
    /// The roots listed, as spelled.
    pub roots: Vec<String>,
    /// Path key → the file's size and modification time as the listing found them (a link's own).
    pub found: FxHashMap<String, Found>,
    /// Path key → the committed output's stamp, once [`Listing::intact`] found it as committed (`None`: not).
    checked: std::sync::Mutex<FxHashMap<String, Option<Stamp>>>,
}

/// A file's size and modification time (nanoseconds since the Unix epoch, 0 when unknown).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The size in bytes.
    pub size: u64,
    /// The modification time, in nanoseconds since the Unix epoch (0 when unknown).
    pub modified: u128,
}

impl Found {
    /// What metadata says.
    pub fn of_metadata(metadata: &std::fs::Metadata) -> Found {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        Found { size: metadata.len(), modified }
    }

    /// What the file (a link itself) at `path` is, `None` when there is none.
    pub fn at(path: &str) -> Option<Found> {
        std::fs::symlink_metadata(path).ok().filter(|m| !m.is_dir()).map(|m| Found::of_metadata(&m))
    }
}

/// What a committed output is compared by, so one changed by hand since (or deleted) is written again: its size and
/// a hash of its bytes (FNV-1a), which the incremental state keeps. `modified` is only a shortcut, not kept: a
/// file with the size and modification time the stamp was made with is not read again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    /// The size in bytes.
    pub size: u64,
    /// [`content_hash`] of the bytes.
    pub hash: u64,
    /// The modification time the stamp was made with (nanoseconds since the Unix epoch), 0 when unknown.
    pub modified: u128,
}

/// FNV-1a, 64-bit: the same in every process.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl Stamp {
    /// The stamp of the file at `path`, read now; `None` when there is none.
    pub fn read(path: &str) -> Option<Stamp> {
        let found = Found::at(path)?;
        let bytes = std::fs::read(path).ok()?;
        let modified = crate::plugins::lifecycle::settled_time(found.modified);
        Some(Stamp { size: bytes.len() as u64, hash: content_hash(&bytes), modified })
    }

    /// The file at `path` (`found` as its metadata) still holds what this stamp describes: the stamp to keep for it
    /// (with its modification time now, unless too recent to tell a later edit apart: `settled_time`), `None` when it
    /// changed or is gone.
    pub fn check(&self, path: &str, found: Option<Found>) -> Option<Stamp> {
        let found = found?;
        if found.size != self.size {
            return None;
        }
        if self.modified != 0 && found.modified == self.modified {
            return Some(*self);
        }
        let bytes = std::fs::read(path).ok()?;
        (bytes.len() as u64 == self.size && content_hash(&bytes) == self.hash)
            .then(|| Stamp { modified: crate::plugins::lifecycle::settled_time(found.modified), ..*self })
    }
}

impl Listing {
    /// A file with this key was there.
    pub fn contains(&self, key: &str) -> bool {
        self.spelled.contains_key(key)
    }

    /// The output with this key is there as the last commit left it: listed, and holding what the stamp
    /// `committed` has for it says (an output no stamp is known of counts when it is there).
    pub fn intact(&self, key: &str, committed: &FxHashMap<String, Stamp>) -> bool {
        match committed.get(key) {
            Some(stamp) => self.checked(key, stamp).is_some(),
            None => self.contains(key),
        }
    }

    /// [`Stamp::check`] for the listed file with this key, once per listing.
    pub fn checked(&self, key: &str, stamp: &Stamp) -> Option<Stamp> {
        let lock = || self.checked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(known) = lock().get(key) {
            return *known;
        }
        // checked without the lock (a check may read the file), so checks run in parallel ([`Listing::check_all`])
        let now = self.spelled.get(key).and_then(|path| stamp.check(path, self.found.get(key).copied()));
        *lock().entry(key.to_owned()).or_insert(now)
    }

    /// [`Listing::intact`] for many outputs at once, in parallel (a check after a build that wrote the output reads
    /// its bytes), for the answers [`Listing::intact`] then gives.
    pub fn check_all(&self, keys: &[String], committed: &FxHashMap<String, Stamp>) {
        let unknown: Vec<(&str, &Stamp)> = keys
            .iter()
            .filter_map(|key| Some((key.as_str(), committed.get(key)?)))
            .filter(|(key, stamp)| stamp.modified == 0 || self.found.get(*key).is_none_or(|f| f.modified != stamp.modified))
            .collect();
        super::parallel::map(&unknown, |&(key, stamp)| self.checked(key, stamp));
    }
}

/// The output roots of a build: the outDir (or its override), and the `declarationDir` of a declaration build
/// without an override.
pub fn output_roots(
    services: &ProjectServices,
    options: &tsrs_core::CompilerOptions,
    out_dir_override: Option<&str>,
) -> Vec<String> {
    let mut roots = vec![out_dir_override.map_or_else(|| services.path_translator.out_dir.clone(), str::to_owned)];
    if out_dir_override.is_none() && options.get_emit_declarations() && !options.declaration_dir.is_empty() {
        let declaration_dir = nodepath::resolve(&[&services.data.project_path, &options.declaration_dir]);
        if !roots.iter().any(|r| path_key(services, r) == path_key(services, &declaration_dir)) {
            roots.push(declaration_dir);
        }
    }
    roots
}

/// Lists the files under `roots` (the include folder the build with this outDir override writes is not entered).
pub fn list(services: &ProjectServices, roots: &[String], out_dir_override: Option<&str>) -> Listing {
    let include = path_key(services, &effective_include_path(services, out_dir_override));
    let mut listing = Listing { roots: roots.to_vec(), ..Listing::default() };
    // a root that is a link is not entered (roblox-ts's `existsSync` + `readdirSync` would follow it; its clean of
    // the link's contents would delete files outside the project)
    let mut stack: Vec<String> = roots
        .iter()
        .filter(|r| std::fs::symlink_metadata(r.as_str()).is_ok_and(|m| m.file_type().is_dir()))
        .cloned()
        .collect();
    // the directories the walk enters, listed ahead in parallel (the walk below visits them in its own order)
    let listings = super::parallel::prefetch(&stack, |dir, entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type().ok()?;
        if name == ".git" || !file_type.is_dir() || file_type.is_symlink() {
            return None;
        }
        let path = nodepath::join(&[dir, &name]);
        (!is_descendant_key(&path_key(services, &path), &include)).then_some(path)
    });
    while let Some(directory) = stack.pop() {
        let Some(entries) = listings.entries(&directory) else { continue };
        let mut count = 0;
        for entry in entries {
            count += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let path = nodepath::join(&[&directory, &name]);
            let Ok(file_type) = entry.file_type() else { continue };
            let key = path_key(services, &path);
            if file_type.is_dir() && !file_type.is_symlink() {
                if !is_descendant_key(&key, &include) {
                    stack.push(path);
                }
            } else {
                if file_type.is_symlink() {
                    listing.links.insert(key.clone());
                }
                // the directory entry's own metadata (on Windows, from the listing itself: no file is opened)
                if let Ok(metadata) = entry.metadata() {
                    listing.found.insert(key.clone(), Found::of_metadata(&metadata));
                }
                listing.spelled.insert(key.clone(), path.clone());
                listing.files.push((path, key));
            }
        }
        if count == 0 {
            listing.empty_dirs.push(directory);
        }
    }
    listing
}

/// rbxts-rs: a deliberate deviation from roblox-ts, which writes into the existing file and keeps its old name, so
/// after a case-only rename of a source the Rojo instance keeps the old spelling (a roblox-ts bug).
///
/// When the listing has `path` spelled differently (only in case), renames the differing components below the
/// output root on disk to `path`'s spelling, from the root down, so a build into a used directory names its outputs
/// as a build into an empty one does. Nothing above the output root is touched.
pub fn match_case(services: &ProjectServices, path: &str, listing: &Listing) -> io::Result<()> {
    match_case_logged(services, path, listing, &mut Vec::new())
}

/// [`match_case`], adding each rename it makes to `renames` (from, to).
fn match_case_logged(
    services: &ProjectServices,
    path: &str,
    listing: &Listing,
    renames: &mut Vec<(String, String)>,
) -> io::Result<()> {
    let key = path_key(services, path);
    let Some(existing) = listing.spelled.get(&key) else { return Ok(()) };
    let wanted = nodepath::normalize(path).replace('\\', "/");
    if existing.replace('\\', "/") == wanted {
        return Ok(());
    }
    let Some(root) = listing.roots.iter().find(|r| is_descendant_key(&key, &path_key(services, r))) else {
        return Ok(());
    };
    let root = nodepath::normalize(root).replace('\\', "/");
    let root = root.trim_end_matches('/');
    let Some(below) = wanted.get(root.len()..).map(|rest| rest.trim_start_matches('/')) else { return Ok(()) };
    let mut directory = root.to_owned();
    for part in below.split('/') {
        let next = format!("{directory}/{part}");
        let on_disk = std::fs::read_dir(&directory)?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|name| name.to_lowercase() == part.to_lowercase());
        if let Some(on_disk) = on_disk
            && on_disk != part
        {
            let from = format!("{directory}/{on_disk}");
            std::fs::rename(&from, &next)?;
            renames.push((from, next.clone()));
        }
        directory = next;
    }
    Ok(())
}

/// Commit step 3 for stale outputs: deletes every listed file and link that is not kept (a link itself, never what
/// it points to), then the directories that leaves empty and those that were empty (never a root). Returns the
/// files deleted.
pub fn clean(services: &ProjectServices, outputs: &ProjectOutputs, listing: &Listing) -> Result<Vec<String>, String> {
    let roots: Vec<String> = outputs.roots.iter().map(|r| path_key(services, r)).collect();
    let mut deleted = Vec::new();
    // the directories that may be left empty: every ancestor of a deleted file below its root
    let mut candidates: FxHashMap<String, String> = FxHashMap::default();
    for (path, key) in &listing.files {
        let Some(root) = roots.iter().find(|root| is_descendant_key(key, root)) else { continue };
        if outputs.keep.contains(key) {
            continue;
        }
        let removed =
            if listing.links.contains(key) { remove_link(Path::new(path)) } else { remove_file(Path::new(path)) };
        if removed.map_err(|e| format!("could not delete {path}: {e}"))? {
            deleted.push(path.clone());
        }
        let mut directory = nodepath::dirname(path).to_owned();
        loop {
            let directory_key = path_key(services, &directory);
            if directory_key == *root || !is_descendant_key(&directory_key, root) {
                break;
            }
            let parent = nodepath::dirname(&directory).to_owned();
            let shorter = parent.len() < directory.len();
            if candidates.insert(directory_key, directory).is_some() || !shorter {
                break;
            }
            directory = parent;
        }
    }
    for directory in &listing.empty_dirs {
        candidates.insert(path_key(services, directory), directory.clone());
        let mut parent = nodepath::dirname(directory).to_owned();
        while let Some(root) = roots.iter().find(|root| is_descendant_key(&path_key(services, &parent), root)) {
            let key = path_key(services, &parent);
            if key == *root || candidates.insert(key, parent.clone()).is_some() {
                break;
            }
            parent = nodepath::dirname(&parent).to_owned();
        }
    }
    candidates.retain(|key, _| !roots.contains(key));
    // deepest first, so a parent is looked at after its children
    let mut candidates: Vec<String> = candidates.into_values().collect();
    candidates.sort_by_key(|d| std::cmp::Reverse(d.split(['/', '\\']).count()));
    for directory in candidates {
        if std::fs::read_dir(&directory).is_ok_and(|mut d| d.next().is_none()) {
            std::fs::remove_dir(&directory).map_err(|e| format!("could not delete {directory}: {e}"))?;
        }
    }
    deleted.sort();
    Ok(deleted)
}

/// Removes a link itself: a file link with `remove_file`, a directory link (a junction or a directory symlink) with
/// `remove_dir`, which on a link removes the link and leaves its target alone.
fn remove_link(path: &Path) -> io::Result<bool> {
    match remove_file(path) {
        Ok(removed) => Ok(removed),
        Err(_) => std::fs::remove_dir(path).map(|()| true),
    }
}
