//! Packages under `node_modules/`: where a package's declaration files point (the Luau module they describe) and
//! which Luau files mount, at `ReplicatedStorage/rbxts/node_modules/<scope>/<name>`.
//!
//! A package mounts as Rojo mounts it: from its `default.project.json` tree `$path` under the project's `name` when
//! it has one, else the package directory under its own name, nested `node_modules/` included. Only packages the
//! program reaches mount, with the packages they depend on (`dependencies` and `peerDependencies`, which the Luau
//! requires at run time without the TypeScript seeing it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use luau::ModulePath;

const MARK: &str = "/node_modules/";

/// How a package compiled by roblox-ts gets its runtime: `TS.import` hands it over through `_G[script]`, but opossum
/// requires packages plainly, so the mount requires the runtime library instead.
const TS_GLOBAL: &str = "local TS = _G[script]";
pub const TS_REQUIRE: &str = "local TS = require(\"@game/ReplicatedStorage/rbxts/RuntimeLib\")";

/// A package to mount.
pub struct Package {
    pub scope: String,
    /// The instance name: the project name or the directory name.
    pub name: String,
    pub dir: PathBuf,
}

/// A package directory's `package.json`, or null.
fn manifest(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// `name` of `default.project.json`, else the directory's name.
fn instance_name(dir: &Path) -> String {
    let project = std::fs::read_to_string(dir.join("default.project.json")).ok();
    let json: Option<serde_json::Value> = project.and_then(|t| serde_json::from_str(&t).ok());
    match json.as_ref().and_then(|j| j["name"].as_str()) {
        Some(name) => name.to_owned(),
        None => dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// The package directory a file belongs to (its length in `file`), its scope and the file's path inside it: the
/// first `node_modules/<@scope>/<name>` of the path. Unscoped packages are not Roblox packages.
fn split(file: &str) -> Option<(usize, &str, &str)> {
    let at = file.find(MARK)?;
    let rest = &file[at + MARK.len()..];
    let mut parts = rest.splitn(3, '/');
    let (scope, name) = (parts.next()?, parts.next()?);
    let inner = parts.next().unwrap_or("");
    if !scope.starts_with('@') {
        return None;
    }
    Some((at + MARK.len() + scope.len() + 1 + name.len(), scope, inner))
}

/// The package directory of a program file under `node_modules/`, with its scope.
pub fn package_dir(file: &str) -> Option<(PathBuf, String)> {
    split(file).map(|(len, scope, _)| (PathBuf::from(&file[..len]), scope.to_owned()))
}

/// The module a package file describes: a package's types entry stands for its `main`, any other declaration
/// file for the Luau file beside it. Packages nested in a package's own `node_modules/` sit inside its mount.
pub fn location(file: &str) -> Option<ModulePath> {
    let mut names: Vec<String> = ["ReplicatedStorage", "rbxts", "node_modules"].map(String::from).to_vec();
    let mut root = 0;
    let mut from = 0;
    loop {
        let (len, scope, inner) = split(&file[from..])?;
        let dir_path = Path::new(&file[..from + len]);
        names.push(scope.to_owned());
        names.push(instance_name(dir_path));
        if root == 0 {
            root = names.len();
        }
        if inner.starts_with("node_modules/") {
            // a nested package: `<dir>/node_modules/@x/y/...`, mounted inside its parent as `files` walks it
            names.push("node_modules".to_owned());
            from += len;
            continue;
        }
        let json = manifest(dir_path);
        let types = json["types"].as_str().or(json["typings"].as_str()).map(|t| t.trim_start_matches("./"));
        let target = match json["main"].as_str() {
            Some(main) if types == Some(inner) => main.trim_start_matches("./"),
            _ => inner,
        };
        // the mount starts at the tree root: `src/init.lua` of a package mounted from `src` is the package itself
        let tree = place::package_tree_root(dir_path);
        let sub = tree.strip_prefix(dir_path).map(|s| s.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let target = match sub.as_str() {
            "" => target,
            sub => target.strip_prefix(sub).map_or(target, |t| t.trim_start_matches('/')),
        };
        let stem =
            [".d.ts", ".luau", ".lua", ".tsx", ".ts"].iter().find_map(|e| target.strip_suffix(e)).unwrap_or(target);
        names.extend(stem.split('/').filter(|s| !s.is_empty()).map(str::to_owned));
        let init = matches!(names.last().map(String::as_str), Some("init" | "index"));
        if init {
            names.pop();
        }
        let init = init || names.len() == root;
        return Some(ModulePath { names, root, init });
    }
}

/// The packages to mount: those holding a file in `reached` (package directories by scope), then what they depend
/// on, resolved from their own `node_modules/` (inside their mount already) or the project's. Keyed by directory, so
/// a package reached under two spellings mounts once.
pub fn closure(reached: Vec<(PathBuf, String)>, node_modules: &Path) -> Vec<Package> {
    let mut seen: BTreeMap<PathBuf, Package> = BTreeMap::new();
    let mut queue = reached;
    while let Some((dir, scope)) = queue.pop() {
        let key = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if seen.contains_key(&key) {
            continue;
        }
        let json = manifest(&dir);
        for field in ["dependencies", "peerDependencies"] {
            let Some(deps) = json[field].as_object() else { continue };
            for dep in deps.keys() {
                let Some((dep_scope, _)) = dep.split_once('/') else { continue };
                if dir.join("node_modules").join(dep).is_dir() {
                    continue;
                }
                let dep_dir = node_modules.join(dep);
                if dep_dir.is_dir() {
                    queue.push((dep_dir, dep_scope.to_owned()));
                }
            }
        }
        let name = instance_name(&dir);
        seen.insert(key, Package { scope, name, dir });
    }
    seen.into_values().collect()
}

/// A package's Luau files, relative to its tree root; a nested package's under
/// `node_modules/<scope>/<instance name>/`.
pub fn files(package: &Package) -> Vec<place::PackageFile> {
    let mut out = Vec::new();
    walk(&place::package_tree_root(&package.dir), "", &mut out);
    out
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<place::PackageFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            if name == "node_modules" {
                nested(&path, &format!("{rel}node_modules/"), out);
            } else {
                walk(&path, &format!("{rel}{name}/"), out);
            }
        } else if (name.ends_with(".luau") || name.ends_with(".lua"))
            && let Ok(source) = std::fs::read_to_string(&path)
        {
            let source = if source.contains(TS_GLOBAL) { source.replacen(TS_GLOBAL, TS_REQUIRE, 1) } else { source };
            out.push(place::PackageFile { rel: format!("{rel}{name}"), source, file: path });
        }
    }
}

fn nested(node_modules: &Path, rel: &str, out: &mut Vec<place::PackageFile>) {
    let Ok(scopes) = std::fs::read_dir(node_modules) else { return };
    for scope in scopes.flatten() {
        let scope_name = scope.file_name().to_string_lossy().into_owned();
        if !scope_name.starts_with('@') {
            continue;
        }
        let Ok(packages) = std::fs::read_dir(scope.path()) else { continue };
        for package in packages.flatten() {
            let dir = package.path();
            let rel = format!("{rel}{scope_name}/{}/", instance_name(&dir));
            walk(&place::package_tree_root(&dir), &rel, out);
        }
    }
}
