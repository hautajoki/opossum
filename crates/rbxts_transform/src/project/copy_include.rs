//! `Project/functions/copyInclude.ts` and `getIncludeFiles.ts`: the runtime library files (`RuntimeLib`, `Promise`
//! and anything else in roblox-ts's `include` folder) copied into the project's include folder (`includePath`,
//! `<project>/include` by default, `-i` / `--includePath` or the tsconfig's `rbxts.includePath`).
//!
//! **When** (`copyInclude`): unless `noInclude` is set, the project type is `package`, or no type is set and the
//! project is a package (a scoped package.json name). A model or a game without a Rojo file gets the copy too, as in
//! roblox-ts; the build then fails on its own Rojo checks (rbxts-rs: before anything is written, see below).
//!
//! **What** (`getIncludeFiles`): every file under the include source, at the same relative path under the include
//! folder; with `luau: false` a `.luau` file is written as `.lua`. Before a `.luau` source's file is written, the
//! output with the other extension is deleted, so Rojo never sees two competing instances after an option change.
//!
//! **Where the files come from.** roblox-ts takes them from its own package (`INCLUDE_PATH`,
//! `<PACKAGE_ROOT>/include`): for `npx rbxtsc` that is the roblox-ts package the project installs. rbxts-rs has no
//! package of its own, so it takes the include folder of the roblox-ts package the project resolves: the first
//! `node_modules/roblox-ts` (with a package.json) from the project folder upwards, as Node resolves a package (a
//! junction or link to a roblox-ts checkout is followed). The include folder then equals stock's byte for byte,
//! whichever roblox-ts version the project pins. The npm roblox-ts 3.0.0 package ships `include/*.lua` and copies
//! the folder whole (no renaming, no deletion of the other extension); master's rules leave `.lua` sources alone, so
//! a 3.0.0 project gets 3.0.0's files, byte-identical. Only when no roblox-ts package is found does rbxts-rs use the
//! files it embeds (`crates/rbxts_transform/include`): those of roblox-ts master at 68648b0, the version this port
//! follows, under roblox-ts's MIT licence (`include/LICENSE`).
//!
//! rbxts-rs: differences from roblox-ts, all writing less:
//! - the copy is part of the commit (DESIGN §14 step 1, after the Luau, before plugin artifacts), so it happens only
//!   when the build succeeds; roblox-ts copies before compiling, also when the build then fails. The files are read
//!   before anything is written, so a missing include source fails the build with nothing written;
//! - a file is written only when its bytes differ (roblox-ts copies every file on every build), through a temporary
//!   file renamed into place;
//! - with an outDir override (parity runs, the harness) the include folder is moved under the override when it lies
//!   in the configured outDir, and not written at all otherwise: an override must never write into the project.

use std::borrow::Cow;
use std::path::Path;

use rbxts_project::setup::is_path_descendant_of;
use rbxts_project::{ProjectOptions, ProjectServices, ProjectType, nodepath};

use crate::build::outputs::{Stamp, content_hash};
use crate::plugins::lifecycle::{remove_file, write_if_changed_timed};

const LUA_EXT: &str = ".lua";
const LUAU_EXT: &str = ".luau";

/// The include files rbxts-rs embeds: (path relative to the include folder, bytes).
pub const EMBEDDED_INCLUDE: &[(&str, &[u8])] = &[
    ("Promise.luau", include_bytes!("../../include/Promise.luau")),
    ("RuntimeLib.luau", include_bytes!("../../include/RuntimeLib.luau")),
];

/// Where the runtime files come from (roblox-ts's `INCLUDE_PATH`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncludeSource {
    /// The `include` folder of the roblox-ts package the project resolves.
    Package(String),
    /// The files embedded in rbxts-rs ([`EMBEDDED_INCLUDE`]).
    Embedded,
}

/// One runtime file to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncludeFile {
    /// The source path (`<embedded>/…` for an embedded file).
    pub input: String,
    /// Where it is copied to.
    pub output: String,
    /// Its bytes.
    pub bytes: Cow<'static, [u8]>,
}

/// The include copy of a build: the files, read before anything is written.
#[derive(Debug)]
pub struct IncludeCopy {
    /// Where the files come from.
    pub source: IncludeSource,
    /// The files.
    pub files: Vec<IncludeFile>,
    luau: bool,
}

/// The include source for a project folder: the first `node_modules/roblox-ts` with a package.json from `project_path`
/// upwards (Node's lookup, which skips a `node_modules/node_modules`), else the embedded files.
pub fn find_include_source(project_path: &str) -> IncludeSource {
    include_source_lookup(project_path).1
}

/// [`find_include_source`], with the package.json paths it looked at (a watcher watches them: one appearing or going
/// changes the source).
pub fn include_source_lookup(project_path: &str) -> (Vec<String>, IncludeSource) {
    let mut probed = Vec::new();
    let mut dir = nodepath::resolve(&[project_path]);
    loop {
        if nodepath::basename(&dir) != "node_modules" {
            let package = nodepath::join(&[&dir, "node_modules", "roblox-ts"]);
            let package_json = nodepath::join(&[&package, "package.json"]);
            let found = Path::new(&package_json).is_file();
            probed.push(package_json);
            if found {
                return (probed, IncludeSource::Package(nodepath::join(&[&package, "include"])));
            }
        }
        let parent = nodepath::dirname(&dir).to_owned();
        if parent == dir {
            return (probed, IncludeSource::Embedded);
        }
        dir = parent;
    }
}

/// The output name of an include file: with `luau: false`, `.luau` becomes `.lua`.
fn output_name(relative: &str, luau: bool) -> String {
    match relative.strip_suffix(LUAU_EXT) {
        Some(stem) if !luau => format!("{stem}{LUA_EXT}"),
        _ => relative.to_owned(),
    }
}

/// `getIncludeFiles`: every file of the source with its output under `include_path`, read. Directories are walked
/// recursively, links followed (`statSync`); entries are taken in name order.
pub fn get_include_files(source: &IncludeSource, include_path: &str, luau: bool) -> Result<Vec<IncludeFile>, String> {
    let mut files = Vec::new();
    match source {
        IncludeSource::Embedded => {
            for &(relative, bytes) in EMBEDDED_INCLUDE {
                files.push(IncludeFile {
                    input: format!("<embedded>/{relative}"),
                    output: nodepath::join(&[include_path, &output_name(relative, luau)]),
                    bytes: Cow::Borrowed(bytes),
                });
            }
        }
        IncludeSource::Package(root) => {
            let read_error =
                |path: &str, e: std::io::Error| format!("could not read the roblox-ts include file {path}: {e}");
            let mut stack = vec![root.clone()];
            while let Some(directory) = stack.pop() {
                let entries = std::fs::read_dir(&directory).map_err(|e| read_error(&directory, e))?;
                let mut names: Vec<String> =
                    entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                let mut subdirectories = Vec::new();
                for name in names {
                    let input = nodepath::join(&[&directory, &name]);
                    let metadata = std::fs::metadata(&input).map_err(|e| read_error(&input, e))?;
                    if metadata.is_dir() {
                        subdirectories.push(input);
                        continue;
                    }
                    let relative = nodepath::relative(root, &input);
                    let bytes = std::fs::read(&input).map_err(|e| read_error(&input, e))?;
                    files.push(IncludeFile {
                        output: nodepath::join(&[include_path, &output_name(&relative, luau)]),
                        input,
                        bytes: Cow::Owned(bytes),
                    });
                }
                // popped in name order
                stack.extend(subdirectories.into_iter().rev());
            }
        }
    }
    Ok(files)
}

/// `copyInclude`'s condition: the project gets the runtime files.
pub fn wants_include(options: &ProjectOptions, is_package: bool) -> bool {
    !options.no_include
        && options.project_type != Some(ProjectType::Package)
        && !(options.project_type.is_none() && is_package)
}

/// The include folder a build writes: the project's, or under the outDir override when the include folder lies in
/// the configured outDir. None when an override leaves it outside (the project is never written).
pub fn include_target(include_path: &str, out_dir: &str, out_dir_override: Option<&str>) -> Option<String> {
    match out_dir_override {
        None => Some(include_path.to_owned()),
        Some(dir) if is_path_descendant_of(include_path, out_dir) => {
            Some(nodepath::join(&[dir, &nodepath::relative(out_dir, include_path)]))
        }
        Some(_) => None,
    }
}

/// The include folder of a build with this outDir override: [`include_target`], or the project's own when the
/// override leaves it out (nothing is written there then). The clean keeps it whole.
pub fn effective_include_path(services: &ProjectServices, out_dir_override: Option<&str>) -> String {
    let include_path = nodepath::resolve(&[&services.data.options.include_path]);
    include_target(&include_path, &services.path_translator.out_dir, out_dir_override).unwrap_or(include_path)
}

/// The include copy of this build, with every file read, or None when the project gets none. Fails when the include
/// source cannot be read, before anything is written.
pub fn prepare_copy_include(
    services: &ProjectServices,
    out_dir_override: Option<&str>,
) -> Result<Option<IncludeCopy>, String> {
    let data = &services.data;
    if !wants_include(&data.options, data.is_package) {
        return Ok(None);
    }
    let Some(include_path) =
        include_target(&data.options.include_path, &services.path_translator.out_dir, out_dir_override)
    else {
        return Ok(None);
    };
    let source = find_include_source(&data.project_path);
    let files = get_include_files(&source, &include_path, data.options.luau)?;
    Ok(Some(IncludeCopy { source, files, luau: data.options.luau }))
}

/// The include copy of a project that is built by none of its own (a solution config of one deployment, whose
/// referenced projects use its include folder), or None when it gets none.
pub fn prepare_copy_include_of(data: &rbxts_project::ProjectData) -> Result<Option<IncludeCopy>, String> {
    if !wants_include(&data.options, data.is_package) {
        return Ok(None);
    }
    let source = find_include_source(&data.project_path);
    let files = get_include_files(&source, &data.options.include_path, data.options.luau)?;
    Ok(Some(IncludeCopy { source, files, luau: data.options.luau }))
}

impl IncludeCopy {
    /// Writes the files whose bytes differ, each after deleting its output's alternate extension (`.lua` beside a
    /// `.luau` output, or the reverse). Returns the files written.
    pub fn commit(&self) -> Result<Vec<String>, String> {
        Ok(self.commit_stamped()?.into_iter().filter(|(_, written, _)| *written).map(|(path, _, _)| path).collect())
    }

    /// [`IncludeCopy::commit`], returning every output with whether it was written and the stamp of the bytes it
    /// holds now, made from the bytes copied (a watcher restores an include file edited by hand as any other output).
    pub fn commit_stamped(&self) -> Result<Vec<(String, bool, Stamp)>, String> {
        let mut done = Vec::with_capacity(self.files.len());
        for file in &self.files {
            if file.input.ends_with(LUAU_EXT) {
                // Rojo must not see both extensions as competing instances after an option change
                let (extension, alternate) = if self.luau { (LUAU_EXT, LUA_EXT) } else { (LUA_EXT, LUAU_EXT) };
                if let Some(stem) = file.output.strip_suffix(extension) {
                    let alternate = format!("{stem}{alternate}");
                    remove_file(Path::new(&alternate)).map_err(|e| format!("could not delete {alternate}: {e}"))?;
                }
            }
            let (written, modified) = write_if_changed_timed(Path::new(&file.output), &file.bytes)
                .map_err(|e| format!("could not write {}: {e}", file.output))?;
            let stamp = Stamp { size: file.bytes.len() as u64, hash: content_hash(&file.bytes), modified };
            done.push((file.output.clone(), written, stamp));
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rbxts-rs-include-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn s(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn conditions() {
        let mut options = ProjectOptions::default();
        assert!(wants_include(&options, false));
        assert!(!wants_include(&options, true), "an inferred package");
        options.project_type = Some(ProjectType::Game);
        assert!(wants_include(&options, true), "an explicit type wins over package.json");
        options.project_type = Some(ProjectType::Model);
        assert!(wants_include(&options, false));
        options.project_type = Some(ProjectType::Package);
        assert!(!wants_include(&options, false));
        options.project_type = None;
        options.no_include = true;
        assert!(!wants_include(&options, false));
    }

    #[test]
    fn targets_under_an_override() {
        let out = nodepath::resolve(&["E:/p/out"]);
        let include = nodepath::resolve(&["E:/p/include"]);
        assert_eq!(include_target(&include, &out, None), Some(include.clone()));
        assert_eq!(include_target(&include, &out, Some("E:/x")), None, "never into the project");
        let inside = nodepath::resolve(&["E:/p/out/include"]);
        assert_eq!(include_target(&inside, &out, Some("E:/x")), Some(nodepath::join(&["E:/x", "include"])));
        assert_eq!(include_target(&out, &out, Some("E:/x")), Some(nodepath::join(&["E:/x", ""])));
    }

    #[test]
    fn sources_and_names() {
        let root = temp("source");
        let project = root.join("a").join("game");
        std::fs::create_dir_all(project.join("node_modules")).unwrap();
        assert_eq!(find_include_source(&s(&project)), IncludeSource::Embedded);

        // an ancestor's package, as Node finds it
        let package = root.join("node_modules").join("roblox-ts");
        std::fs::create_dir_all(package.join("include").join("nested")).unwrap();
        std::fs::write(package.join("package.json"), "{}").unwrap();
        std::fs::write(package.join("include").join("RuntimeLib.luau"), "rt").unwrap();
        std::fs::write(package.join("include").join("Promise.lua"), "p").unwrap();
        std::fs::write(package.join("include").join("nested").join("x.luau"), "x").unwrap();
        let source = find_include_source(&s(&project));
        assert_eq!(source, IncludeSource::Package(nodepath::join(&[&s(&package), "include"])));

        let target = s(&project.join("include"));
        let files = get_include_files(&source, &target, false).unwrap();
        let outputs: Vec<String> =
            files.iter().map(|f| nodepath::relative(&target, &f.output).replace('\\', "/")).collect();
        assert_eq!(outputs, ["Promise.lua", "RuntimeLib.lua", "nested/x.lua"]);
        let files = get_include_files(&source, &target, true).unwrap();
        let outputs: Vec<String> =
            files.iter().map(|f| nodepath::relative(&target, &f.output).replace('\\', "/")).collect();
        assert_eq!(outputs, ["Promise.lua", "RuntimeLib.luau", "nested/x.luau"]);
        assert_eq!(files[1].bytes.as_ref(), b"rt");

        // a package without an include folder fails before anything is written
        std::fs::remove_dir_all(package.join("include")).unwrap();
        assert!(get_include_files(&source, &target, true).unwrap_err().contains("could not read"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn commit_removes_the_alternate_and_writes_only_changes() {
        let root = temp("commit");
        let include = root.join("include");
        std::fs::create_dir_all(&include).unwrap();
        std::fs::write(include.join("RuntimeLib.lua"), "old").unwrap();
        std::fs::write(include.join("Other.lua"), "kept").unwrap();
        let files = get_include_files(&IncludeSource::Embedded, &s(&include), true).unwrap();
        let copy = IncludeCopy { source: IncludeSource::Embedded, files, luau: true };
        assert_eq!(copy.commit().unwrap().len(), 2);
        assert!(!include.join("RuntimeLib.lua").exists());
        assert!(include.join("Other.lua").exists(), "files of others are kept");
        assert_eq!(std::fs::read(include.join("RuntimeLib.luau")).unwrap(), EMBEDDED_INCLUDE[1].1);
        assert!(copy.commit().unwrap().is_empty(), "unchanged files are not written");

        // back to `.lua`: the `.luau` files go
        let files = get_include_files(&IncludeSource::Embedded, &s(&include), false).unwrap();
        let copy = IncludeCopy { source: IncludeSource::Embedded, files, luau: false };
        assert_eq!(copy.commit().unwrap().len(), 2);
        assert!(include.join("RuntimeLib.lua").exists() && !include.join("RuntimeLib.luau").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
