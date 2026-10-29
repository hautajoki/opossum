use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{Class, Code, Diag, FileType, Kind, Module, Routes, file_type, route};

/// Every routed module under a source root, the loose `.d.ts` files (ambient declarations, which never route) and
/// the route and tree diagnostics. Modules keep the file order (name order from `scan`), and every path is `root`
/// joined with the relative path.
#[derive(Debug, Default)]
pub struct Scan {
    pub root: PathBuf,
    pub modules: Vec<Module>,
    pub decls: Vec<PathBuf>,
    pub diags: Vec<Diag>,
    by_file: HashMap<PathBuf, usize>,
}

/// Walks `root`, routes every source file and checks the routed tree. Dotfiles and dot-directories are skipped.
pub fn scan(root: &Path, routes: &Routes) -> Scan {
    let mut files = Vec::new();
    let mut walked = vec![fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())];
    walk(root, &mut PathBuf::new(), &mut walked, &mut files);
    scan_files(root, files, routes)
}

/// `scan` over a known file list (paths relative to `root`), for a watcher that already holds it.
pub fn scan_files(root: &Path, files: Vec<PathBuf>, routes: &Routes) -> Scan {
    let mut s = Scan { root: root.to_path_buf(), ..Scan::default() };
    let module_dirs: HashSet<PathBuf> = files
        .iter()
        .filter(|rel| {
            let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
            matches!(file_type(name), Some(FileType::Source { init: true, .. }))
        })
        .filter_map(|rel| rel.parent().map(Path::to_path_buf))
        .collect();
    let mut dts = Vec::new();
    for rel in files {
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        match file_type(name) {
            Some(FileType::Dts) => dts.push(rel),
            Some(FileType::Source { kind: Kind::Json, .. }) | None => {}
            Some(FileType::Source { .. }) => match route(routes, &rel, &module_dirs) {
                Ok(Some(mut m)) => {
                    m.file = root.join(&rel);
                    s.by_file.insert(m.file.clone(), s.modules.len());
                    s.modules.push(m);
                }
                Ok(None) => {}
                Err(mut d) => {
                    d.path = root.join(&d.path);
                    s.diags.push(d);
                }
            },
        }
    }

    // `x.d.ts` types `x.luau` beside it, `index.d.ts` types `init.luau`; any other `.d.ts` is a declaration file.
    for rel in dts {
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let stem = &name[..name.len() - 5];
        let luau = rel.with_file_name(if stem == "index" { "init.luau".to_owned() } else { format!("{stem}.luau") });
        let file = root.join(&rel);
        match s.by_file.get(&root.join(luau)).copied() {
            Some(i) => {
                s.modules[i].types = Some(file.clone());
                s.by_file.insert(file, i);
            }
            None => s.decls.push(file),
        }
    }

    // A script under another module is a template that module clones.
    let homes: HashSet<Vec<String>> = s.modules.iter().map(|m| m.instance.clone()).collect();
    for m in &mut s.modules {
        m.template = m.class != Class::ModuleScript && (1..m.instance.len()).any(|k| homes.contains(&m.instance[..k]));
    }
    check(&s.modules, root, &mut s.diags);
    s
}

/// Appends every file under `dir` to `out`, relative to the root, in name order. A linked directory is followed
/// only when its target neither holds nor lies in the root or a directory followed before (`walked`), so
/// link cycles end and no file is walked twice through a link.
fn walk(dir: &Path, rel: &mut PathBuf, walked: &mut Vec<PathBuf>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if name.as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        let Ok(mut ft) = e.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            let Ok(m) = fs::metadata(e.path()) else {
                continue;
            };
            if m.is_dir() {
                let Ok(target) = fs::canonicalize(e.path()) else {
                    continue;
                };
                if walked.iter().any(|w| target.starts_with(w) || w.starts_with(&target)) {
                    continue;
                }
                walked.push(target);
            }
            ft = m.file_type();
        }
        rel.push(&name);
        if ft.is_dir() {
            walk(&e.path(), rel, walked, out);
        } else if ft.is_file() {
            out.push(rel.clone());
        }
        rel.pop();
    }
}

/// The checks that need the whole routed tree: instance clashes, dead scripts and case-only sibling names.
fn check(modules: &[Module], root: &Path, diags: &mut Vec<Diag>) {
    let mut at: HashMap<&[String], usize> = HashMap::new();
    let mut folders: HashMap<&[String], usize> = HashMap::new();
    for (i, m) in modules.iter().enumerate() {
        for k in 1..m.instance.len() {
            folders.entry(&m.instance[..k]).or_insert(i);
        }
    }
    for (i, m) in modules.iter().enumerate() {
        if let Some(&j) = at.get(m.instance.as_slice()) {
            diags.push(Diag {
                code: Code::InstanceClash,
                path: m.file.clone(),
                other: Some(modules[j].file.clone()),
                message: format!("lands on {}, as {} does", m.full_name(), shown(root, &modules[j].file)),
            });
            continue;
        }
        at.insert(&m.instance, i);
        if !m.init
            && let Some(&j) = folders.get(m.instance.as_slice())
        {
            diags.push(Diag {
                code: Code::InstanceClash,
                path: m.file.clone(),
                other: Some(modules[j].file.clone()),
                message: format!(
                    "lands on {}, which is also the folder holding {}; make it the folder's init module or rename it",
                    m.full_name(),
                    shown(root, &modules[j].file)
                ),
            });
        }
    }

    // Templates are cloned into place by their module, so only loose scripts can be dead or doubled.
    for m in modules.iter().filter(|m| m.class != Class::ModuleScript && !m.template) {
        if m.instance[0].starts_with("Starter") {
            diags.push(Diag {
                code: Code::StarterScript,
                path: m.file.clone(),
                other: None,
                message: format!(
                    "a {:?} at {} runs there and again from every player's copy; route it to ReplicatedStorage",
                    m.class,
                    m.full_name()
                ),
            });
        } else if !runs_at(m.class, &m.instance) {
            diags.push(Diag {
                code: Code::DeadScript,
                path: m.file.clone(),
                other: None,
                message: format!("a {:?} at {} never runs there", m.class, m.full_name()),
            });
        }
    }

    let mut seen: HashMap<(&[String], String), (&str, usize)> = HashMap::new();
    let mut reported: HashSet<(&[String], String)> = HashSet::new();
    for (i, m) in modules.iter().enumerate() {
        for k in 1..=m.instance.len() {
            let name = m.instance[k - 1].as_str();
            let key = (&m.instance[..k - 1], name.to_lowercase());
            match seen.get(&key) {
                None => {
                    seen.insert(key, (name, i));
                }
                Some(&(other, j)) if other != name && reported.insert(key) => diags.push(Diag {
                    code: Code::PortableName,
                    path: m.file.clone(),
                    other: Some(modules[j].file.clone()),
                    message: format!(
                        "`{name}` and `{other}` differ only in case under {}",
                        m.instance[..k - 1].join(".")
                    ),
                }),
                Some(_) => {}
            }
        }
    }
}

/// Whether a RunContext script runs where it lands: ServerStorage never runs scripts, and the client never sees
/// the server-only services.
fn runs_at(class: Class, instance: &[String]) -> bool {
    match class {
        Class::Server => instance[0] != "ServerStorage",
        Class::Client => !matches!(instance[0].as_str(), "ServerStorage" | "ServerScriptService"),
        Class::ModuleScript => true,
    }
}

fn shown(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).display().to_string()
}

impl Scan {
    /// The module a source file (or the `.d.ts` typing a Luau module) routes to. A relative path is taken
    /// relative to the root.
    pub fn find(&self, file: &Path) -> Option<&Module> {
        let i = if file.is_absolute() { self.by_file.get(file) } else { self.by_file.get(&self.root.join(file)) };
        i.map(|&i| &self.modules[i])
    }

    /// Whether `path` is a scanned file (a module, its typing, a declaration file) or a directory holding one.
    pub fn holds(&self, path: &Path) -> bool {
        self.by_file.keys().chain(&self.decls).any(|f| f.starts_with(path))
    }

    /// `where <Instance.Path[:line]>`: the module behind an instance path as Studio prints it, and the line.
    /// A client script's runtime copy (`Players.<name>.PlayerScripts...`) maps back to StarterPlayerScripts.
    pub fn where_instance(&self, path: &str) -> Option<(&Module, Option<u32>)> {
        let (path, line) = match path.rsplit_once(':') {
            Some((p, l)) if !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()) => (p, l.parse().ok()),
            _ => (path, None),
        };
        let copied = path
            .strip_prefix("Players.")
            .and_then(|p| p.split_once('.'))
            .and_then(|(_, p)| p.strip_prefix("PlayerScripts."));
        let path = match copied {
            Some(rest) => format!("StarterPlayer.StarterPlayerScripts.{rest}"),
            None => path.to_owned(),
        };
        let m = self.modules.iter().find(|m| m.full_name() == path)?;
        Some((m, line))
    }
}
