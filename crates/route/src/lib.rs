//! Pure routing: a source path under `src/` to its instance path, realm and script class, by rogen v2's rules
//! trimmed as DESIGN.md says. `route` decides one file from its path and the folders that are modules; `scan`
//! walks a source root, routes every file and runs the tree checks that need all of them at once.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

mod scan;
mod spell;

pub use scan::{Scan, scan, scan_files};
pub use spell::{boundary, require};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Realm {
    Server,
    Client,
    Shared,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ts,
    Tsx,
    Luau,
    Json,
}

/// `Server` and `Client` are Scripts with that RunContext: they run wherever they land, so a script inside a module
/// is a template (`Module::template`), mounted disabled for its module to clone and enable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    ModuleScript,
    Server,
    Client,
}

/// What routed a module: a routing folder, an `@key` name suffix, or the `*` fallback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Via {
    Folder,
    Suffix,
    Fallback,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Code {
    Unrouted,
    Nested,
    Boundary,
    InstanceClash,
    DeadScript,
    StarterScript,
    PortableName,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Unrouted => "route.unrouted",
            Code::Nested => "route.nested",
            Code::Boundary => "route.boundary",
            Code::InstanceClash => "tree.instanceClash",
            Code::DeadScript => "tree.deadScript",
            Code::StarterScript => "tree.starterScript",
            Code::PortableName => "scan.portableName",
        }
    }

    pub fn is_error(self) -> bool {
        self != Code::DeadScript
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diag {
    pub code: Code,
    pub path: PathBuf,
    /// The other file of a clash or split folder.
    pub other: Option<PathBuf>,
    pub message: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Route {
    /// The route key (`server`), or `*` for the fallback.
    pub key: String,
    /// Service first: `["ServerScriptService", "server"]`.
    pub target: Vec<String>,
    pub realm: Realm,
}

#[derive(Clone, Debug, Default)]
pub struct Routes {
    list: Vec<Route>,
    fallback: Option<usize>,
}

/// One routed source file. `file` is relative to the source root for `route` and full for `scan`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Module {
    pub file: PathBuf,
    pub kind: Kind,
    /// Service first. An init module's path is its folder's.
    pub instance: Vec<String>,
    pub realm: Realm,
    pub class: Class,
    pub init: bool,
    /// Index into `Routes::list`.
    pub route: usize,
    pub via: Via,
    /// The `.d.ts` that gives this Luau module its types.
    pub types: Option<PathBuf>,
    /// A script inside a module: mounted with `Enabled = false`.
    pub template: bool,
}

impl Module {
    /// The instance path as Studio prints it: `ServerScriptService.server.app`.
    pub fn full_name(&self) -> String {
        self.instance.join(".")
    }
}

/// A route's realm: a key named after a realm (`client = "ReplicatedStorage/client"`) is that realm, any other
/// key takes its target service's.
fn route_realm(key: &str, service: &str) -> Realm {
    match key {
        k if key_matches("server", k) => Realm::Server,
        k if key_matches("client", k) => Realm::Client,
        k if key_matches("shared", k) => Realm::Shared,
        _ => realm_of(service),
    }
}

/// The realm code in a service runs in. Services not listed are visible to both sides.
pub fn realm_of(service: &str) -> Realm {
    match service {
        "ServerScriptService" | "ServerStorage" => Realm::Server,
        "StarterPlayer" | "StarterGui" | "StarterPack" | "ReplicatedFirst" => Realm::Client,
        _ => Realm::Shared,
    }
}

/// A route key matches a name exactly except for the first letter's case: `server` matches `Server`.
fn key_matches(key: &str, name: &str) -> bool {
    let (k, n) = (key.as_bytes(), name.as_bytes());
    k.len() == n.len() && !k.is_empty() && k[0].eq_ignore_ascii_case(&n[0]) && k[1..] == n[1..]
}

impl Routes {
    /// Builds the table from config pairs: key → `Service/Folder/...`. A key `*` declares the fallback.
    pub fn new<K: AsRef<str>, T: AsRef<str>>(pairs: impl IntoIterator<Item = (K, T)>) -> Result<Routes, String> {
        let mut routes = Routes::default();
        for (key, target) in pairs {
            let (key, target) = (key.as_ref(), target.as_ref());
            if key != "*" && (key.is_empty() || !key.is_ascii() || key.contains(['/', '@', '.', '*'])) {
                return Err(format!("route key `{key}`: a key is a plain ASCII folder name"));
            }
            let target: Vec<String> = target.split('/').map(str::to_owned).collect();
            if target.iter().any(|s| s.is_empty()) {
                return Err(format!(
                    "route `{key}`: target `{}` has an empty segment",
                    target.join("/")
                ));
            }
            if routes.list.iter().any(|r| key_matches(&r.key, key)) {
                return Err(format!("route key `{key}` is declared twice"));
            }
            if key == "*" {
                routes.fallback = Some(routes.list.len());
            }
            let realm = route_realm(key, &target[0]);
            routes.list.push(Route {
                key: key.to_owned(),
                target,
                realm,
            });
        }
        Ok(routes)
    }

    pub fn list(&self) -> &[Route] {
        &self.list
    }

    pub fn get(&self, index: usize) -> &Route {
        &self.list[index]
    }

    /// The route a folder named exactly after a key selects.
    fn folder(&self, name: &str) -> Option<usize> {
        self.list.iter().position(|r| r.key != "*" && key_matches(&r.key, name))
    }

    /// `name@key` → (route, `name`). An `@` suffix that names no declared key is part of the name.
    fn suffix<'a>(&self, name: &'a str) -> Option<(usize, &'a str)> {
        let at = name.rfind('@')?;
        let route = self.folder(&name[at + 1..])?;
        Some((route, &name[..at]))
    }
}

enum FileType<'a> {
    Source {
        kind: Kind,
        stem: &'a str,
        class: Class,
        init: bool,
    },
    Dts,
}

/// Splits a file name into what it is. `None` for files that never take part in routing: dotfiles and
/// anything that is not TS, Luau or JSON (`.md`, `.txt`).
fn file_type(name: &str) -> Option<FileType<'_>> {
    if name.starts_with('.') {
        return None;
    }
    if name.ends_with(".d.ts") {
        return Some(FileType::Dts);
    }
    let (kind, stem) = if let Some(s) = name.strip_suffix(".tsx") {
        (Kind::Tsx, s)
    } else if let Some(s) = name.strip_suffix(".ts") {
        (Kind::Ts, s)
    } else if let Some(s) = name.strip_suffix(".luau") {
        (Kind::Luau, s)
    } else {
        (Kind::Json, name.strip_suffix(".json")?)
    };
    let (stem, class) = match (stem.strip_suffix(".server"), stem.strip_suffix(".client")) {
        _ if kind == Kind::Json => (stem, Class::ModuleScript),
        (Some(s), _) => (s, Class::Server),
        (_, Some(s)) => (s, Class::Client),
        _ => (stem, Class::ModuleScript),
    };
    let init = match kind {
        Kind::Luau => stem == "init",
        Kind::Ts | Kind::Tsx => stem == "init" || stem == "index",
        Kind::Json => false,
    };
    Some(FileType::Source {
        kind,
        stem,
        class,
        init,
    })
}

fn diag(code: Code, path: &Path, message: String) -> Diag {
    Diag {
        code,
        path: path.to_path_buf(),
        other: None,
        message,
    }
}

/// Routes one file, given its path relative to the source root and the folders (relative too) that are modules,
/// holding an init file. `Ok(None)` for files that never route (`.d.ts`, dotfiles, docs); the returned module's
/// `file` is `rel`. The outermost route signal governs: a routing folder (consumed) or an `@key` suffix on a folder
/// or the file (kept, minus the suffix). A later folder for the same key stays an ordinary folder and a later suffix
/// for it comes off; a signal for another key is `route.nested`. Inside a module folder nothing routes: its
/// children are its own parts, reached through `@self`, and keep their names as written.
pub fn route(routes: &Routes, rel: &Path, module_dirs: &HashSet<PathBuf>) -> Result<Option<Module>, Diag> {
    let mut parts: Vec<&str> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => match s.to_str() {
                Some(s) => parts.push(s),
                None => return Err(diag(Code::PortableName, rel, "the path is not valid UTF-8".into())),
            },
            _ => {
                return Err(diag(
                    Code::PortableName,
                    rel,
                    "the path must be relative to the source root".into(),
                ));
            }
        }
    }
    let Some((&file, dirs)) = parts.split_last() else {
        return Ok(None);
    };
    let Some(FileType::Source {
        kind,
        stem,
        class,
        init,
    }) = file_type(file)
    else {
        return Ok(None);
    };

    let mut governing: Option<(usize, Via, usize)> = None; // route, via, index of the part that routed
    let mut names: Vec<String> = Vec::new();
    let last = if init { dirs.len() } else { parts.len() };
    // the parts after the outermost module folder are plain names
    let plain =
        (1..=dirs.len()).find(|&k| module_dirs.contains(&dirs[..k].iter().collect::<PathBuf>())).unwrap_or(last);
    for (i, &part) in parts[..last].iter().enumerate() {
        let is_file = i == dirs.len();
        let name = if is_file { stem } else { part };
        if i >= plain {
            names.push(name.to_owned());
            continue;
        }
        let (signal, kept) = match routes.folder(name) {
            Some(r) if !is_file => (Some((r, Via::Folder)), ""),
            _ => match routes.suffix(name) {
                Some((r, head)) => (Some((r, Via::Suffix)), head),
                None => (None, name),
            },
        };
        if let Some((r, via)) = signal {
            match governing {
                None => governing = Some((r, via, i)),
                Some((g, _, gi)) if g != r => {
                    let outer: PathBuf = parts[..=gi].iter().collect();
                    let (key, outer_key) = (&routes.get(r).key, &routes.get(g).key);
                    return Err(diag(
                        Code::Nested,
                        rel,
                        format!(
                            "`{part}` routes to {key} inside `{}`, which routes to {outer_key}; the outermost route wins, so move the file or rename `{part}`",
                            outer.display()
                        ),
                    ));
                }
                // The governing key restated: a folder stays an ordinary folder, a suffix comes off.
                Some(_) if via == Via::Folder => {
                    names.push(name.to_owned());
                    continue;
                }
                Some(_) => {}
            }
        }
        if !kept.is_empty() {
            names.push(kept.to_owned());
        } else if is_file {
            return Err(diag(
                Code::PortableName,
                rel,
                format!("`{file}` leaves an empty instance name"),
            ));
        }
    }

    let (route, via) = match (governing, routes.fallback) {
        (Some((r, via, _)), _) => (r, via),
        (None, Some(f)) => (f, Via::Fallback),
        (None, None) => {
            let keys: Vec<&str> = routes.list.iter().map(|r| r.key.as_str()).collect();
            return Err(diag(
                Code::Unrouted,
                rel,
                format!(
                    "no route matches; put it under a routing folder ({}) or add an @key suffix",
                    keys.join(", ")
                ),
            ));
        }
    };
    let r = routes.get(route);
    if init && names.is_empty() && r.target.len() == 1 {
        return Err(diag(
            Code::InstanceClash,
            rel,
            format!(
                "an init module at the root of route {} would replace the service {}",
                r.key, r.target[0]
            ),
        ));
    }
    let mut instance = r.target.clone();
    instance.extend(names);
    Ok(Some(Module {
        file: rel.to_path_buf(),
        kind,
        instance,
        realm: r.realm,
        class,
        init,
        route,
        via,
        types: None,
        template: false,
    }))
}

#[cfg(test)]
mod tests;
