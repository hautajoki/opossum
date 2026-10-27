//! Port of `@roblox-ts/rojo-resolver` 1.3.0: maps file system paths to Roblox instance paths through a Rojo project.
//!
//! The behaviour, quirks included, is the JavaScript package's; differences are listed on the items they affect.
//! Paths are compared as Node compares them (`nodepath`): `Map` lookups are exact, partition containment goes through
//! `path.relative` (case-insensitive on Windows).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::nodepath;
use crate::read_log::recorded;

const LUA_EXT: &str = ".lua";
const LUAU_EXT: &str = ".luau";
const INIT_NAME: &str = "init";
const SERVER_SUBEXT: &str = ".server";
const CLIENT_SUBEXT: &str = ".client";
const PLUGIN_SUBEXT: &str = ".plugin";

/// `default.project.json`: a directory holding one is that project rather than a folder.
pub const ROJO_DEFAULT_NAME: &str = "default.project.json";
const ROJO_OLD_NAME: &str = "roblox-project.json";

fn is_module_ext(ext: &str) -> bool {
    matches!(ext, ".luau" | ".json" | ".toml")
}

fn is_script_ext(ext: &str) -> bool {
    ext == LUAU_EXT
}

/// `/^.+\.project\.json$/`: at least one character (other than a line terminator) before the suffix.
pub fn is_rojo_project_file_name(name: &str) -> bool {
    name.strip_suffix(".project.json")
        .is_some_and(|stem| !stem.is_empty() && !stem.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
}

/// A Roblox instance path from the DataModel (or from a package root), one instance name per element.
pub type RbxPath = Vec<String>;

/// One step of a relative Roblox path: up to the parent, or down to a child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelativeRbxPathPart {
    /// Up to the parent instance (`script.Parent`).
    Parent,
    /// Down to the child of that name.
    Child(String),
}

/// The script class a file syncs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RbxType {
    /// A `ModuleScript` (a module, `init.luau`, a `.luau` file).
    ModuleScript,
    /// A server `Script` (`.server.luau`).
    Script,
    /// A `LocalScript` (`.client.luau`).
    LocalScript,
}

/// Where code in a container runs (game projects only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkType {
    /// Runs on both sides (or the side is not known).
    Unknown,
    /// Client only (`StarterPack`, `StarterGui`, `StarterPlayer`).
    Client,
    /// Server only (`ServerScriptService`, `ServerStorage`).
    Server,
}

/// How an importing file and an imported module sit relative to the isolated containers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRelation {
    /// Absolute import.
    OutToOut,
    /// Error.
    OutToIn,
    /// Absolute import.
    InToOut,
    /// Relative import (same container).
    InToIn,
}

/// A directory or file synced as a subtree at `rbx_path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    /// Where the subtree sits in the Roblox tree.
    pub rbx_path: RbxPath,
    /// The synced directory or file on disk (resolved, Node spelling).
    pub fs_path: String,
}

/// Containers whose contents are copied per player or character, so code inside them cannot be imported from outside.
const ISOLATED_CONTAINERS: &[&[&str]] = &[
    &["StarterPack"],
    &["StarterGui"],
    &["StarterPlayer", "StarterPlayerScripts"],
    &["StarterPlayer", "StarterCharacterScripts"],
    &["StarterPlayer", "StarterCharacter"],
    &["PluginDebugService"],
];
const CLIENT_CONTAINERS: &[&[&str]] = &[&["StarterPack"], &["StarterGui"], &["StarterPlayer"]];
const SERVER_CONTAINERS: &[&[&str]] = &[&["ServerStorage"], &["ServerScriptService"]];

/// Compares only up to the shorter length, as the package does: `[]` starts with every container, and
/// `["StarterPlayer"]` starts with `["StarterPlayer", "StarterPlayerScripts"]`.
fn array_starts_with(a: &[String], b: &[&str]) -> bool {
    a.iter().zip(b).all(|(x, y)| x == y)
}

/// `.lua` paths are looked up as `.luau` (Rojo treats both alike).
fn convert_to_luau(file_path: &str) -> String {
    if nodepath::extname(file_path) == LUA_EXT {
        format!("{}{LUAU_EXT}", &file_path[..file_path.len() - LUA_EXT.len()])
    } else {
        file_path.to_string()
    }
}

fn strip_rojo_exts(file_path: &str) -> &str {
    let ext = nodepath::extname(file_path);
    if !is_module_ext(ext) {
        return file_path;
    }
    let stripped = &file_path[..file_path.len() - ext.len()];
    if is_script_ext(ext) {
        let subext = nodepath::extname(stripped);
        if matches!(subext, SERVER_SUBEXT | CLIENT_SUBEXT | PLUGIN_SUBEXT) {
            return &stripped[..stripped.len() - subext.len()];
        }
    }
    stripped
}

/// The package's own `isPathDescendantOf`, which differs from roblox-ts's: any relative path that does not start with
/// `..` counts. So a path on another drive (where `relative` returns it whole) is a descendant, and a child named
/// `..x` is not.
fn is_path_descendant_of(file_path: &str, dir_path: &str) -> bool {
    use nodepath::Relation;
    dir_path == file_path
        || match nodepath::relation(dir_path, file_path) {
            Relation::Same | Relation::OtherRoot => true,
            Relation::Inside { dotdot_name } => !dotdot_name,
            Relation::Outside => false,
        }
}

/// Object keys in JavaScript's `Object.keys` order: array-index keys ascending, then the rest in insertion order.
pub(crate) fn js_object_keys(map: &Map<String, Value>) -> Vec<&String> {
    fn array_index(key: &str) -> Option<u32> {
        if key == "0" {
            return Some(0);
        }
        if key.is_empty() || key.starts_with('0') || !key.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        key.parse::<u32>().ok().filter(|&n| n != u32::MAX)
    }
    let mut indexed: Vec<(u32, &String)> = Vec::new();
    let mut named: Vec<&String> = Vec::new();
    for key in map.keys() {
        match array_index(key) {
            Some(n) => indexed.push((n, key)),
            None => named.push(key),
        }
    }
    indexed.sort_by_key(|&(n, _)| n);
    indexed.into_iter().map(|(_, k)| k).chain(named).collect()
}

/// The result of looking for a project's Rojo config.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoundRojoConfig {
    /// The project file found; None without one.
    pub path: Option<String>,
    /// What the search warned about (several project files in the directory: the first one is used).
    pub warnings: Vec<String>,
}

/// An error the JavaScript resolver throws (rather than warns about): unreadable files or directories, invalid JSON,
/// broken symbolic links in a synced tree. roblox-ts reports them as `Unable to read Rojo project`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RojoError(pub String);

impl std::fmt::Display for RojoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RojoError {}

fn io_error(what: &str, path: &str, e: &std::io::Error) -> RojoError {
    RojoError(format!("{what} \"{path}\": {e}"))
}

/// A Rojo project read into partitions (synced directories) and single-file mounts.
#[derive(Clone, Debug, Default)]
pub struct RojoResolver {
    partitions: Vec<Partition>,
    file_path_to_rbx_path: HashMap<String, RbxPath>,
    warnings: Vec<String>,
    /// Every project file the resolver read or looked for, in reading order (watched and hashed by incremental and
    /// watch builds: a nested project file's edit changes the resolver).
    config_files: Vec<String>,
    /// The tree's root `$className` is `DataModel` (any nested project's counts too, as in the package).
    pub is_game: bool,
}

struct Builder {
    rbx_path: Vec<String>,
    /// In discovery order; the package `unshift`s, so `finish` reverses.
    partitions: Vec<Partition>,
    file_path_to_rbx_path: HashMap<String, RbxPath>,
    warnings: Vec<String>,
    config_files: Vec<String>,
    is_game: bool,
    /// Directories being searched, to stop at a symbolic-link cycle (where the package recurses until the stack
    /// overflows). Canonical paths, computed only once a link is met.
    search_stack: Vec<(PathBuf, Option<PathBuf>)>,
    /// Project files being read (lowercased), to stop at a `$path` cycle (the same overflow in the package).
    config_stack: Vec<String>,
    /// The same files as written, for error messages.
    config_paths: Vec<String>,
}

/// Whether a directory entry is a file or a directory, following links as `fs.statSync(fs.realpathSync(p))` does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Other,
}

impl RojoResolver {
    /// `RojoResolver.findRojoConfigFilePath`: `default.project.json` in `project_path`, else the first
    /// `*.project.json` (or the old `roblox-project.json`) in directory order, warning when there are several.
    pub fn find_rojo_config_file_path(project_path: &str) -> Result<FoundRojoConfig, RojoError> {
        let mut warnings = Vec::new();
        let default_path = nodepath::join(&[project_path, ROJO_DEFAULT_NAME]);
        if recorded::exists(&default_path) {
            return Ok(FoundRojoConfig { path: Some(default_path), warnings });
        }
        let mut candidates = Vec::new();
        for name in read_dir_names(project_path)? {
            if name != ROJO_DEFAULT_NAME && (name == ROJO_OLD_NAME || is_rojo_project_file_name(&name)) {
                candidates.push(nodepath::join(&[project_path, &name]));
            }
        }
        if candidates.len() > 1 {
            warnings.push(format!("Multiple *.project.json files found, using {}", candidates[0]));
        }
        Ok(FoundRojoConfig { path: candidates.into_iter().next(), warnings })
    }

    /// `RojoResolver.fromPath`: reads the project file and everything it mounts.
    pub fn from_path(rojo_config_file_path: &str) -> Result<RojoResolver, RojoError> {
        let mut b = Builder::new();
        b.parse_config(&nodepath::resolve(&[rojo_config_file_path]), true)?;
        Ok(b.finish())
    }

    /// `RojoResolver.synthetic`: a package's resolver, the whole of `base_path` mounted at the root, so every import
    /// is relative.
    pub fn synthetic(base_path: &str) -> Result<RojoResolver, RojoError> {
        let mut tree = Map::new();
        tree.insert("$path".into(), Value::String(base_path.into()));
        Self::from_tree(base_path, &Value::Object(tree))
    }

    /// `RojoResolver.fromTree`: a resolver for an in-memory tree, with `$path`s relative to `base_path`.
    pub fn from_tree(base_path: &str, tree: &Value) -> Result<RojoResolver, RojoError> {
        let mut b = Builder::new();
        b.parse_tree(base_path, "", tree, true)?;
        Ok(b.finish())
    }

    /// Warnings collected while reading (invalid or missing project files).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Every project file read or looked for (missing ones included), in reading order.
    pub fn config_files(&self) -> &[String] {
        &self.config_files
    }

    /// Synced directories, most recently discovered first (the order lookups try them).
    pub fn partitions(&self) -> &[Partition] {
        &self.partitions
    }

    /// Files mounted directly by `$path` (module files: `.lua(u)`, `.json`, `.toml`), by path as resolved.
    pub fn file_mounts(&self) -> &HashMap<String, RbxPath> {
        &self.file_path_to_rbx_path
    }

    /// `getRbxPathFromFilePath`: the instance path of a file, or None when the project does not sync it.
    /// `init.lua(u)` names its folder; `.server` / `.client` / `.plugin` and the extension are dropped.
    pub fn get_rbx_path_from_file_path(&self, file_path: &str) -> Option<RbxPath> {
        let file_path = convert_to_luau(&nodepath::resolve(&[file_path]));
        if let Some(rbx_path) = self.file_path_to_rbx_path.get(&file_path) {
            return Some(rbx_path.clone());
        }
        let ext = nodepath::extname(&file_path);
        for partition in &self.partitions {
            if is_path_descendant_of(&file_path, &partition.fs_path) {
                let stripped = strip_rojo_exts(&file_path);
                let relative = nodepath::relative(&partition.fs_path, stripped);
                let mut parts: Vec<String> = if relative.is_empty() {
                    Vec::new()
                } else {
                    relative.split(nodepath::SEP).map(str::to_owned).collect()
                };
                if is_script_ext(ext) && parts.last().is_some_and(|p| p == INIT_NAME) {
                    parts.pop();
                }
                let mut rbx_path = partition.rbx_path.clone();
                rbx_path.extend(parts);
                return Some(rbx_path);
            }
        }
        None
    }

    /// `getRbxTypeFromFilePath`: `.server` → Script, `.client` → LocalScript, `.plugin` → Script; only for Luau files.
    pub fn get_rbx_type_from_file_path(&self, file_path: &str) -> RbxType {
        rbx_type_from_file_path(file_path)
    }

    fn get_container(&self, from: &'static [&'static [&'static str]], rbx_path: Option<&[String]>) -> Option<usize> {
        if !self.is_game {
            return None;
        }
        let rbx_path = rbx_path?;
        from.iter().position(|container| array_starts_with(rbx_path, container))
    }

    /// `getFileRelation`: whether an import from `file_rbx_path` of `module_rbx_path` crosses an isolated container.
    pub fn get_file_relation(&self, file_rbx_path: &[String], module_rbx_path: &[String]) -> FileRelation {
        let file_container = self.get_container(ISOLATED_CONTAINERS, Some(file_rbx_path));
        let module_container = self.get_container(ISOLATED_CONTAINERS, Some(module_rbx_path));
        match (file_container, module_container) {
            (Some(f), Some(m)) if f == m => FileRelation::InToIn,
            (Some(_), Some(_)) => FileRelation::OutToIn,
            (Some(_), None) => FileRelation::InToOut,
            (None, Some(_)) => FileRelation::OutToIn,
            (None, None) => FileRelation::OutToOut,
        }
    }

    /// `isIsolated`: inside one of the isolated containers (games only).
    pub fn is_isolated(&self, rbx_path: &[String]) -> bool {
        self.get_container(ISOLATED_CONTAINERS, Some(rbx_path)).is_some()
    }

    /// `getNetworkType`: server or client containers (games only).
    pub fn get_network_type(&self, rbx_path: &[String]) -> NetworkType {
        if self.get_container(SERVER_CONTAINERS, Some(rbx_path)).is_some() {
            NetworkType::Server
        } else if self.get_container(CLIENT_CONTAINERS, Some(rbx_path)).is_some() {
            NetworkType::Client
        } else {
            NetworkType::Unknown
        }
    }

    /// `RojoResolver.relative`: the steps from one instance path to another.
    pub fn relative(rbx_from: &[String], rbx_to: &[String]) -> Vec<RelativeRbxPathPart> {
        let max = rbx_from.len().max(rbx_to.len());
        let diff_index = (0..max).find(|&i| rbx_from.get(i) != rbx_to.get(i)).unwrap_or(max);
        let mut result = Vec::new();
        for _ in diff_index..rbx_from.len().max(diff_index) {
            result.push(RelativeRbxPathPart::Parent);
        }
        for name in rbx_to.iter().skip(diff_index) {
            result.push(RelativeRbxPathPart::Child(name.clone()));
        }
        result
    }
}

/// `getRbxTypeFromFilePath` (needs no resolver state).
pub fn rbx_type_from_file_path(file_path: &str) -> RbxType {
    let file_path = convert_to_luau(file_path);
    let ext = nodepath::extname(&file_path);
    if !is_script_ext(ext) {
        // non-script extensions cannot use .server, .client, etc.
        return RbxType::ModuleScript;
    }
    match nodepath::extname(nodepath::basename_without(&file_path, ext)) {
        SERVER_SUBEXT | PLUGIN_SUBEXT => RbxType::Script,
        CLIENT_SUBEXT => RbxType::LocalScript,
        _ => RbxType::ModuleScript,
    }
}

/// Directory entry names in the order Node's `readdirSync` gives them: the system's order on Windows, sorted by
/// bytes elsewhere (libuv sorts its `scandir` results).
fn read_dir_names(dir: &str) -> Result<Vec<String>, RojoError> {
    Ok(read_dir_entries(dir)?.into_iter().map(|(name, _)| name).collect())
}

fn read_dir_entries(dir: &str) -> Result<Vec<(String, fs::DirEntry)>, RojoError> {
    let read = fs::read_dir(dir).map_err(|e| {
        recorded::not_listed(dir);
        io_error("cannot read directory", dir, &e)
    })?;
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(|e| io_error("cannot read directory", dir, &e))?;
        entries.push((entry.file_name().to_string_lossy().into_owned(), entry));
    }
    // a watch rebuild compares the entries and their kinds again (`read_check`)
    if crate::read_log::current().is_some() {
        recorded::listed(
            dir,
            entries.iter().map(|(name, entry)| {
                let (kind, link) = recorded::entry_kind(entry);
                (name.clone(), kind, link)
            }),
        );
    }
    if cfg!(not(windows)) {
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    }
    Ok(entries)
}

/// The entry's kind, following links. The entry's own type needs no extra system call; only links are resolved.
/// Also whether the entry is a link. Paths are only spelled out for error messages.
fn entry_kind(entry: &fs::DirEntry, directory: &str, name: &str) -> Result<(Kind, bool), RojoError> {
    let spelled = || nodepath::join(&[directory, name]);
    let own_type = entry.file_type().map_err(|e| io_error("cannot stat", &spelled(), &e))?;
    let is_link = own_type.is_symlink();
    let file_type = if is_link {
        // A broken link makes the package's realpathSync throw.
        fs::metadata(entry.path()).map_err(|e| io_error("cannot resolve link", &spelled(), &e))?.file_type()
    } else {
        own_type
    };
    let kind = if file_type.is_dir() {
        Kind::Dir
    } else if file_type.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    Ok((kind, is_link))
}

impl Builder {
    fn new() -> Self {
        Builder {
            rbx_path: Vec::new(),
            partitions: Vec::new(),
            file_path_to_rbx_path: HashMap::new(),
            warnings: Vec::new(),
            config_files: Vec::new(),
            is_game: false,
            search_stack: Vec::new(),
            config_stack: Vec::new(),
            config_paths: Vec::new(),
        }
    }

    fn finish(mut self) -> RojoResolver {
        self.partitions.reverse();
        RojoResolver {
            partitions: self.partitions,
            file_path_to_rbx_path: self.file_path_to_rbx_path,
            warnings: self.warnings,
            config_files: self.config_files,
            is_game: self.is_game,
        }
    }

    fn parse_config(&mut self, rojo_config_file_path: &str, do_not_push: bool) -> Result<(), RojoError> {
        self.config_files.push(rojo_config_file_path.to_owned());
        if !recorded::exists(rojo_config_file_path) {
            self.warnings.push(format!("RojoResolver: Path does not exist \"{rojo_config_file_path}\""));
            return Ok(());
        }
        let text = recorded::read_to_string(rojo_config_file_path)
            .map_err(|e| io_error("cannot read", rojo_config_file_path, &e))?;
        let config: Value = serde_json::from_str(&text)
            .map_err(|e| RojoError(format!("invalid JSON in \"{rojo_config_file_path}\": {e}")))?;
        match validate_rojo_config(&config) {
            Ok(()) => {
                let key = rojo_config_file_path.to_lowercase();
                if self.config_stack.contains(&key) {
                    return Err(RojoError(format!("$path cycle through \"{rojo_config_file_path}\"")));
                }
                self.config_stack.push(key);
                self.config_paths.push(rojo_config_file_path.to_owned());
                let name = config["name"].as_str().unwrap_or_default().to_owned();
                let result =
                    self.parse_tree(nodepath::dirname(rojo_config_file_path), &name, &config["tree"], do_not_push);
                self.config_stack.pop();
                self.config_paths.pop();
                result
            }
            Err(text) => {
                self.warnings.push(format!("RojoResolver: Invalid configuration! {text}"));
                Ok(())
            }
        }
    }

    fn parse_tree(&mut self, base_path: &str, name: &str, tree: &Value, do_not_push: bool) -> Result<(), RojoError> {
        if !do_not_push {
            self.rbx_path.push(name.to_owned());
        }
        // Children that are not objects only get past the schema under keys its pattern skips ("" and keys with
        // line breaks). The package reads them as JavaScript values: an array's items mount under their indexes, and
        // a number, a boolean or "" has no members, so it mounts nothing.
        let tree = match tree {
            Value::Object(tree) => tree,
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.parse_tree(base_path, &index.to_string(), item, false)?;
                }
                if !do_not_push {
                    self.rbx_path.pop();
                }
                return Ok(());
            }
            // rbxts-rs: fixes the package crashing on `null` (a TypeError reading `$path`) and on a non-empty string
            // (endless recursion through its characters, each a string with itself at index 0): an error naming the
            // entry (and the project file, when it is not the one the error is reported for)
            Value::Null | Value::String(_) if !tree.as_str().is_some_and(str::is_empty) => {
                let entry = Value::from(self.rbx_path.clone());
                let file = match self.config_paths.as_slice() {
                    [_, .., nested] => format!(" in \"{nested}\""),
                    _ => String::new(),
                };
                return Err(RojoError(format!("the tree entry {entry}{file} is {tree}, which roblox-ts cannot read")));
            }
            _ => {
                if !do_not_push {
                    self.rbx_path.pop();
                }
                return Ok(());
            }
        };
        if let Some(path) = tree.get("$path") {
            let path = match path {
                Value::String(s) => Some(s.as_str()),
                Value::Object(o) => o.get("optional").and_then(Value::as_str),
                _ => None,
            };
            // `path.resolve(basePath, undefined)` throws in the package.
            let path = path.ok_or_else(|| RojoError("a $path object has no \"optional\" string".into()))?;
            self.parse_path(&nodepath::resolve(&[base_path, path]))?;
        }
        if tree.get("$className").and_then(Value::as_str) == Some("DataModel") {
            self.is_game = true;
        }
        for child in js_object_keys(tree) {
            if !child.starts_with('$') {
                self.parse_tree(base_path, child, &tree[child.as_str()], false)?;
            }
        }
        if !do_not_push {
            self.rbx_path.pop();
        }
        Ok(())
    }

    fn parse_path(&mut self, item_path: &str) -> Result<(), RojoError> {
        let item_path = convert_to_luau(item_path);
        // a project file contributes its tree at the mount point, without adding its project name
        if is_rojo_project_file_name(nodepath::basename(&item_path)) {
            if recorded::exists(&item_path) {
                self.parse_config(&item_path, true)?;
            } else {
                // its creation changes the resolver
                self.config_files.push(item_path);
            }
            return Ok(());
        }
        let ext = nodepath::extname(&item_path);
        if is_module_ext(ext) {
            self.file_path_to_rbx_path.insert(item_path.clone(), self.rbx_path.clone());
            return Ok(());
        }
        let is_directory = recorded::metadata(&item_path).is_ok_and(|m| m.is_dir());
        if is_directory && read_dir_names(&item_path)?.iter().any(|n| n == ROJO_DEFAULT_NAME) {
            return self.parse_config(&nodepath::join(&[&item_path, ROJO_DEFAULT_NAME]), true);
        }
        self.partitions.push(Partition { fs_path: item_path.clone(), rbx_path: self.rbx_path.clone() });
        if is_directory {
            self.search_directory(&item_path, None)?;
        }
        Ok(())
    }

    /// Looks for nested project files below a partition; files themselves need no walk (lookups are by path).
    fn search_directory(&mut self, directory: &str, item: Option<&str>) -> Result<(), RojoError> {
        let entries = read_dir_entries(directory)?;
        if entries.iter().any(|(name, _)| name == ROJO_DEFAULT_NAME) {
            return self.parse_config(&nodepath::join(&[directory, ROJO_DEFAULT_NAME]), false);
        }
        if let Some(item) = item {
            self.rbx_path.push(item.to_owned());
        }
        self.search_stack.push((PathBuf::from(directory), None));
        let result = self.search_entries(directory, &entries);
        self.search_stack.pop();
        result?;
        if item.is_some() {
            self.rbx_path.pop();
        }
        Ok(())
    }

    fn search_entries(&mut self, directory: &str, entries: &[(String, fs::DirEntry)]) -> Result<(), RojoError> {
        let mut kinds = Vec::with_capacity(entries.len());
        for (name, entry) in entries {
            let (kind, is_link) = entry_kind(entry, directory, name)?;
            kinds.push((name, kind, is_link));
        }
        // *.project.json
        for (name, kind, _) in &kinds {
            if *kind == Kind::File && name.as_str() != ROJO_DEFAULT_NAME && is_rojo_project_file_name(name) {
                self.parse_config(&nodepath::join(&[directory, name]), false)?;
            }
        }
        // folders
        for (name, kind, is_link) in &kinds {
            if *kind == Kind::Dir {
                let child_path = nodepath::join(&[directory, name]);
                if *is_link {
                    self.check_link_cycle(&child_path)?;
                }
                self.search_directory(&child_path, Some(name))?;
            }
        }
        Ok(())
    }

    /// A linked directory that leads back to a directory being searched would recurse forever in the package
    /// (until the stack overflows, which roblox-ts reports as an unreadable project). Report it instead.
    fn check_link_cycle(&mut self, child_path: &str) -> Result<(), RojoError> {
        let Ok(target) = fs::canonicalize(child_path) else { return Ok(()) };
        for (path, canonical) in &mut self.search_stack {
            if canonical.is_none() {
                *canonical = fs::canonicalize(&*path).ok();
            }
            if canonical.as_deref() == Some(target.as_path()) {
                return Err(RojoError(format!("symbolic link cycle at \"{child_path}\"")));
            }
        }
        Ok(())
    }
}

/// Validation against the package's `rojo-schema.json` (draft 7, checked with Ajv's defaults: stop at the first
/// error, except that `anyOf` reports every branch). The error text is Ajv's `errorsText`.
pub fn validate_rojo_config(config: &Value) -> Result<(), String> {
    let mut errors: Vec<(String, String)> = Vec::new();
    validate_root(config, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.iter().map(|(path, message)| format!("data{path} {message}")).collect::<Vec<_>>().join(", "))
    }
}

fn pointer(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn is_integer(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0),
        _ => false,
    }
}

fn validate_root(config: &Value, errors: &mut Vec<(String, String)>) {
    let Value::Object(root) = config else {
        errors.push((String::new(), "must be object".into()));
        return;
    };
    for required in ["name", "tree"] {
        if !root.contains_key(required) {
            errors.push((String::new(), format!("must have required property '{required}'")));
            return;
        }
    }
    if !root["name"].is_string() {
        errors.push(("/name".into(), "must be string".into()));
        return;
    }
    if let Some(port) = root.get("servePort")
        && !is_integer(port)
    {
        errors.push(("/servePort".into(), "must be integer".into()));
        return;
    }
    validate_tree(&root["tree"], "/tree", errors);
}

/// The `tree` schema: typed `$` properties, and every key matching `^[^$].*$` a tree again.
fn validate_tree(tree: &Value, at: &str, errors: &mut Vec<(String, String)>) -> bool {
    let Value::Object(tree) = tree else {
        errors.push((at.into(), "must be object".into()));
        return false;
    };
    if let Some(v) = tree.get("$className")
        && !v.is_string()
    {
        errors.push((pointer(at, "$className"), "must be string".into()));
        return false;
    }
    if let Some(v) = tree.get("$ignoreUnknownInstances")
        && !v.is_boolean()
    {
        errors.push((pointer(at, "$ignoreUnknownInstances"), "must be boolean".into()));
        return false;
    }
    if let Some(v) = tree.get("$path") {
        let path_at = pointer(at, "$path");
        if !v.is_string() {
            let mut branch_errors = vec![(path_at.clone(), "must be string".to_owned())];
            let ok = match v {
                Value::Object(o) => match o.get("optional") {
                    Some(optional) if !optional.is_string() => {
                        branch_errors.push((pointer(&path_at, "optional"), "must be string".into()));
                        false
                    }
                    _ => true,
                },
                _ => {
                    branch_errors.push((path_at.clone(), "must be object".into()));
                    false
                }
            };
            if !ok {
                errors.extend(branch_errors);
                errors.push((path_at, "must match a schema in anyOf".into()));
                return false;
            }
        }
    }
    if let Some(v) = tree.get("$properties")
        && !v.is_object()
    {
        errors.push((pointer(at, "$properties"), "must be object".into()));
        return false;
    }
    for key in js_object_keys(tree) {
        let matches_pattern =
            key.chars().next().is_some_and(|c| c != '$') && !key.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
        if matches_pattern && !validate_tree(&tree[key.as_str()], &pointer(at, key), errors) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_errors_read_like_ajv() {
        assert_eq!(validate_rojo_config(&json!({"tree": {}})), Err("data must have required property 'name'".into()));
        assert_eq!(validate_rojo_config(&json!([])), Err("data must be object".into()));
        assert_eq!(
            validate_rojo_config(&json!({"name": "x", "tree": {"A": {"$path": 5}}})),
            Err("data/tree/A/$path must be string, data/tree/A/$path must be object, data/tree/A/$path must match a schema in anyOf".into())
        );
        assert_eq!(
            validate_rojo_config(&json!({"name": "x", "tree": {"a/b": {"$className": 1}}})),
            Err("data/tree/a~1b/$className must be string".into())
        );
        assert_eq!(
            validate_rojo_config(&json!({"name": "x", "servePort": 1.5, "tree": {}})),
            Err("data/servePort must be integer".into())
        );
        assert!(
            validate_rojo_config(&json!({"name": "x", "servePort": 34872, "tree": {"$path": {"optional": "a"}}}))
                .is_ok()
        );
    }

    #[test]
    fn tree_entries_that_are_not_objects() {
        // the schema skips "" and keys with line breaks, so only those can hold anything else; the package crashes
        // on `null` and on a non-empty string (errors here), and ignores numbers, booleans and "" (here too)
        for (value, text) in [(json!(null), "null"), (json!("abc"), "\"abc\""), (json!([null]), "null")] {
            let config = json!({"name": "x", "tree": {"$className": "DataModel", "A": {"": value.clone()}}});
            assert!(validate_rojo_config(&config).is_ok());
            let Err(error) = RojoResolver::from_tree("/p", &config["tree"]) else { panic!("an error") };
            let entry = if value.is_array() { "[\"A\",\"\",\"0\"]" } else { "[\"A\",\"\"]" };
            assert_eq!(error.0, format!("the tree entry {entry} is {text}, which roblox-ts cannot read"));
        }
        for value in [json!(5), json!(false), json!(""), json!([1, true])] {
            let config = json!({"name": "x", "tree": {"$className": "DataModel", "A": {"": value}, "A\nB": 1}});
            assert!(validate_rojo_config(&config).is_ok());
            assert!(RojoResolver::from_tree("/p", &config["tree"]).is_ok());
        }
    }

    #[test]
    fn object_key_order_matches_javascript() {
        let v = json!({"b": {}, "10": {}, "a": {}, "2": {}, "01": {}});
        let keys: Vec<&str> = js_object_keys(v.as_object().unwrap()).into_iter().map(String::as_str).collect();
        assert_eq!(keys, ["2", "10", "b", "a", "01"]);
    }

    #[test]
    fn rbx_types() {
        assert_eq!(rbx_type_from_file_path("a/b.server.lua"), RbxType::Script);
        assert_eq!(rbx_type_from_file_path("a/b.client.luau"), RbxType::LocalScript);
        assert_eq!(rbx_type_from_file_path("a/b.plugin.luau"), RbxType::Script);
        assert_eq!(rbx_type_from_file_path("a/b.server.json"), RbxType::ModuleScript);
        assert_eq!(rbx_type_from_file_path("a/init.luau"), RbxType::ModuleScript);
    }

    #[test]
    fn relative_rbx_paths() {
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        use RelativeRbxPathPart::*;
        assert_eq!(
            RojoResolver::relative(&p(&["A", "B", "C"]), &p(&["A", "D"])),
            vec![Parent, Parent, Child("D".into())]
        );
        assert_eq!(RojoResolver::relative(&p(&["A"]), &p(&["A"])), vec![]);
        assert_eq!(RojoResolver::relative(&p(&["A"]), &p(&["A", "B"])), vec![Child("B".into())]);
    }
}
