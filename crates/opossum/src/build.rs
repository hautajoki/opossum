//! The build: routes, the program, lowering, the O2 gate and the place, held in a [`Session`] so that `dev` can
//! redo only what an edit touches. One-shot commands make a session, run it once and read its outputs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use globset::{Glob, GlobSet, GlobSetBuilder};
use lower::graph::{FileFacts, Version};
use lower::{LoadOptions, Location, Locator, Project, SourceFileCache};
use luau::{Expr, ModulePath, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};
use tsrs_ast::SourceFile;
use tsrs_core::P;

use crate::config::Config;
use crate::diag::{self, Diag, Severity};
use crate::inputs;
use crate::packages::{self, Package};

/// Where the runtime library and its Promise mount.
const RUNTIME: &str = "ReplicatedStorage/rbxts";
const RUNTIME_LIB: &str = include_str!("../../../runtime/RuntimeLib.luau");
const PROMISE: &str = include_str!("../../../runtime/Promise.luau");

/// One compiled TypeScript module.
pub struct Compiled {
    pub source: Arc<str>,
    pub lines: Arc<LineMap>,
    /// It requires the runtime library.
    pub runtime: bool,
}

/// How a compiled module's lines map back to its TypeScript.
pub struct LineMap {
    /// The output line each top-level Luau statement starts on.
    pub starts: Vec<u32>,
    /// Per top-level TypeScript statement: the index of its first Luau statement, its first and last line.
    pub spans: Vec<(usize, u32, u32)>,
}

impl LineMap {
    /// The TypeScript line behind a line of the output: the statement holding it, offset by the line's place in
    /// the statement and kept inside it.
    pub fn source_line(&self, luau_line: u32) -> Option<u32> {
        let stmt = self.starts.partition_point(|&l| l <= luau_line).checked_sub(1)?;
        let span = self.spans.partition_point(|&(i, _, _)| i <= stmt).checked_sub(1)?;
        let (first, start, end) = self.spans[span];
        let offset = luau_line.saturating_sub(self.starts[first]);
        Some((start + offset).min(end))
    }
}

/// What one run of [`Session::compile`] did, for the rebuild line.
#[derive(Default)]
pub struct Stats {
    pub checked: usize,
    pub lowered: usize,
    pub times: Vec<(&'static str, f64)>,
}

impl Stats {
    fn time(&mut self, phase: &'static str, since: Instant) {
        self.times.push((phase, since.elapsed().as_secs_f64() * 1000.0));
    }
}

pub struct Session {
    pub cfg: Config,
    exclude: GlobSet,
    native: GlobSet,
    pub routes: route::Routes,
    pub scan: route::Scan,
    locator: Locator,
    options: lower::Options,
    /// git's fields, read while the first program loads.
    git: Option<inputs::GitRead>,
    cache: Arc<SourceFileCache>,
    project: Option<Project>,
    facts: FxHashMap<String, FileFacts>,
    versions: FxHashMap<P<SourceFile>, Version>,
    /// Compiled modules by TypeScript file name (absolute, forward slashes, as the program spells it). A module
    /// that fails keeps its last good output.
    pub compiled: FxHashMap<String, Compiled>,
    /// TypeScript, lowering and gate diagnostics by file name.
    file_diags: FxHashMap<String, Vec<Diag>>,
    global_diags: Vec<Diag>,
    route_diags: Vec<Diag>,
    pub place_diags: Vec<Diag>,
    pub packages: Vec<(Package, Vec<place::PackageFile>)>,
    /// Luau sources by file.
    luau_sources: FxHashMap<PathBuf, String>,
}

/// `path` with forward slashes, as tsrs spells file names.
pub fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn globs(patterns: &[String], what: &str) -> Result<GlobSet, String> {
    let mut set = GlobSetBuilder::new();
    for p in patterns {
        set.add(Glob::new(p).map_err(|e| format!("{what} `{p}`: {e}"))?);
    }
    set.build().map_err(|e| format!("{what}: {e}"))
}

fn realm(r: route::Realm) -> lower::Realm {
    match r {
        route::Realm::Server => lower::Realm::Server,
        route::Realm::Client => lower::Realm::Client,
        route::Realm::Shared => lower::Realm::Shared,
    }
}

fn script_kind(class: route::Class) -> place::ScriptKind {
    match class {
        route::Class::ModuleScript => place::ScriptKind::Module,
        route::Class::Server => place::ScriptKind::Server,
        route::Class::Client => place::ScriptKind::Client,
    }
}

/// `local TS = require(<runtime>)` among the top-level statements.
fn requires_runtime(ast: &[Stmt], runtime: &ModulePath) -> bool {
    ast.iter().any(|s| match s {
        Stmt::VariableDeclaration(d) => d.right.iter().any(|e| matches!(e, Expr::Require(p) if **p == *runtime)),
        _ => false,
    })
}

impl Session {
    /// A session for `cfg`, with the `[profile.<name>]` excludes when a profile is given.
    pub fn new(cfg: Config, profile: Option<&str>) -> Result<Session, String> {
        let git = inputs::start_git(&cfg.root);
        let exclude = match profile {
            Some(name) => globs(&cfg.profile(name)?.exclude, &format!("[profile.{name}] exclude"))?,
            None => GlobSet::empty(),
        };
        let native = globs(&cfg.native, "[native] files")?;
        let routes = route::Routes::new(cfg.routes.iter().map(|(k, v)| (k.as_str(), v.as_str())))?;
        let options = lower::Options {
            env: inputs::env(&cfg.root),
            package_json: std::fs::read_to_string(cfg.root.join("package.json")).ok(),
            build_time: inputs::build_time(),
            ..lower::Options::default()
        };
        Ok(Session {
            cfg,
            exclude,
            native,
            routes,
            scan: route::Scan::default(),
            locator: Locator::default(),
            options,
            git: Some(git),
            cache: Arc::new(SourceFileCache::new()),
            project: None,
            facts: FxHashMap::default(),
            versions: FxHashMap::default(),
            compiled: FxHashMap::default(),
            file_diags: FxHashMap::default(),
            global_diags: Vec::new(),
            route_diags: Vec::new(),
            place_diags: Vec::new(),
            packages: Vec::new(),
            luau_sources: FxHashMap::default(),
        })
    }

    /// Routes `src/` again: the module list, route diagnostics, the source part of the locator and the `@native`
    /// files.
    pub fn rescan(&mut self) {
        let src = self.cfg.src();
        let mut scan = route::scan(&src, &self.routes);
        if !self.exclude.is_empty() {
            let root = &self.cfg.root;
            let mut files: Vec<PathBuf> = Vec::new();
            for m in &scan.modules {
                files.push(m.file.clone());
                files.extend(m.types.clone());
            }
            files.extend(scan.decls.iter().cloned());
            files.extend(scan.diags.iter().map(|d| d.path.clone()));
            files.retain(|f| !self.exclude.is_match(f.strip_prefix(root).unwrap_or(f)));
            files.sort();
            files.dedup();
            let rel = files.iter().filter_map(|f| f.strip_prefix(&src).ok().map(Path::to_path_buf)).collect();
            scan = route::scan_files(&src, rel, &self.routes);
        }
        self.route_diags = scan.diags.iter().map(diag::from_route).collect();
        let mut files = FxHashMap::default();
        let mut native = FxHashSet::default();
        for m in &scan.modules {
            let key = match (m.kind, &m.types) {
                (route::Kind::Luau, Some(types)) => types,
                (route::Kind::Luau, None) => continue,
                _ => &m.file,
            };
            let module =
                ModulePath { names: m.instance.clone(), root: self.routes.get(m.route).target.len(), init: m.init };
            files.insert(slashed(key), Location { module, realm: realm(m.realm) });
            if self.native.is_match(m.file.strip_prefix(&self.cfg.root).unwrap_or(&m.file)) {
                native.insert(slashed(&m.file));
            }
        }
        // the runtime library is a module of its own route root
        let runtime = ModulePath {
            names: vec!["ReplicatedStorage".into(), "rbxts".into(), "RuntimeLib".into()],
            root: 2,
            init: false,
        };
        self.locator = Locator { files, runtime_lib: Some(Location { module: runtime, realm: lower::Realm::Shared }) };
        self.options.native_files = native;
        self.scan = scan;
    }

    /// The program's root files: the TypeScript modules, the `.d.ts` typing Luau modules and the loose ones.
    fn root_files(&self) -> Vec<String> {
        let mut files: Vec<String> = Vec::new();
        for m in &self.scan.modules {
            match m.kind {
                route::Kind::Ts | route::Kind::Tsx => files.push(slashed(&m.file)),
                _ => files.extend(m.types.as_deref().map(slashed)),
            }
        }
        files.extend(self.scan.decls.iter().map(|d| slashed(d)));
        files.sort();
        files
    }

    /// Loads the program afresh (parsed files come from the session's cache), routes the package files it reached
    /// and gathers the packages to mount.
    fn load(&self) -> Result<Project, String> {
        let checkers = std::thread::available_parallelism().map_or(4, usize::from);
        let options = LoadOptions {
            root: slashed(&self.cfg.root),
            files: self.root_files(),
            checkers,
            flags: self.cfg.ts.clone(),
        };
        lower::load(&options, Some(&self.cache))
    }

    /// Package locations and the packages to mount, from the program's files under `node_modules/`.
    fn route_packages(&mut self, project: &Project) {
        let mut reached = Vec::new();
        let mut seen = FxHashSet::default();
        for &file in project.program.source_files() {
            let name = file.file_name();
            let Some((dir, scope)) = packages::package_dir(name) else { continue };
            if let Some(module) = packages::location(name) {
                self.locator.files.insert(name.to_owned(), Location { module, realm: lower::Realm::Shared });
            }
            if seen.insert(dir.clone()) {
                reached.push((dir, scope));
            }
        }
        // a package mounted before keeps the files read then
        let mut old: FxHashMap<PathBuf, Vec<place::PackageFile>> =
            self.packages.drain(..).map(|(p, f)| (p.dir, f)).collect();
        let mut list = Vec::new();
        for p in packages::closure(reached, &self.cfg.root.join("node_modules")) {
            let files = old.remove(&p.dir).unwrap_or_else(|| packages::files(&p));
            if !files.is_empty() {
                list.push((p, files));
            }
        }
        self.packages = list;
    }

    /// A full build of every module.
    pub fn build(&mut self) -> Result<Stats, String> {
        let mut stats = Stats::default();
        let t = Instant::now();
        self.rescan();
        stats.time("route", t);
        let t = Instant::now();
        let project = self.load()?;
        self.route_packages(&project);
        stats.time("load", t);
        let facts = lower::graph::program_facts(project.program, &mut self.versions);
        let files = project.project_files();
        self.compile(&project, &files, &mut stats);
        self.commit(project, facts);
        Ok(stats)
    }

    /// Routes and loads without checking or lowering: enough for the tree's shape (`sourcemap`).
    pub fn shapes(&mut self) -> Result<(), String> {
        self.rescan();
        let project = self.load()?;
        self.route_packages(&project);
        self.project = Some(project);
        Ok(())
    }

    /// Compiles the one TypeScript file `name` (program spelling) of a rescanned session.
    pub fn compile_one(&mut self, name: &str) -> Result<Option<Compiled>, String> {
        let project = self.load()?;
        self.route_packages(&project);
        let Some(file) = project.program.get_source_file(name) else { return Ok(None) };
        self.compile(&project, &[file], &mut Stats::default());
        Ok(self.compiled.remove(name))
    }

    /// Rebuilds after edits: `changed` TypeScript files (program spelling) by reusing the program, or a fresh load
    /// when the file set changed (`rescan`) or tsrs cannot reuse it; then only the files the edit can affect.
    pub fn update(&mut self, changed: &[String], rescan: bool) -> Result<Stats, String> {
        let mut stats = Stats::default();
        let t = Instant::now();
        if rescan {
            self.rescan();
        }
        let reused = match (&self.project, rescan) {
            (Some(p), false) => p.reuse(changed),
            _ => None,
        };
        let project = match reused {
            Some(p) => p,
            None => {
                let p = self.load()?;
                self.route_packages(&p);
                p
            }
        };
        stats.time("load", t);
        let facts = lower::graph::program_facts(project.program, &mut self.versions);
        let invalidation = lower::graph::invalidate(&self.facts, |f| f, &facts);
        for gone in &invalidation.deleted {
            self.file_diags.remove(gone);
            self.compiled.remove(gone);
        }
        let all = project.project_files();
        let files: Vec<P<SourceFile>> = if invalidation.all {
            all
        } else {
            all.into_iter().filter(|f| invalidation.affected.contains(&f.path().to_string())).collect()
        };
        self.compile(&project, &files, &mut stats);
        self.commit(project, facts);
        Ok(stats)
    }

    /// Keeps `project` and its facts, freeing the program it replaces.
    fn commit(&mut self, project: Project, facts: Vec<(String, FileFacts)>) {
        self.facts = facts.into_iter().collect();
        if let Some(old) = self.project.replace(project) {
            // SAFETY: every diagnostic was copied into an owned `Diag` and every lowered tree rendered into text;
            // nothing reads the old program or its checkers again. Its files stay in the source cache.
            unsafe { old.free() };
        }
    }

    /// Checks, lowers, renders and gates `files`, replacing their diagnostics and outputs.
    fn compile(&mut self, project: &Project, files: &[P<SourceFile>], stats: &mut Stats) {
        if let Some(git) = self.git.take() {
            self.options.git = git.finish();
        }
        let file_of = |name: &str| PathBuf::from(name);
        let t = Instant::now();
        let ctx = tsrs_compiler::Context::background();
        let checked = project.check(&ctx, files);
        stats.time("check", t);
        stats.checked = files.len();
        let names: FxHashSet<&str> = files.iter().map(|f| f.file_name()).collect();
        for &f in files {
            self.file_diags.insert(f.file_name().to_owned(), Vec::new());
        }
        let mut global = Vec::new();
        for d in checked {
            let owned = diag::from_ts(d, &file_of);
            match d.file().filter(|f| names.contains(f.file_name())) {
                Some(f) => self.file_diags.entry(f.file_name().to_owned()).or_default().push(owned),
                None => global.push(owned),
            }
        }
        self.global_diags = global;

        let t = Instant::now();
        let lowered = lower::lower_files(project, &self.locator, &self.options, files);
        stats.time("lower", t);
        stats.lowered = lowered.len();

        let t = Instant::now();
        let locator = &self.locator;
        let runtime = locator.runtime_lib.as_ref().map(|l| &l.module);
        let rendered = lower::parallel::map(&lowered, |l| {
            // a tree with lowering errors holds placeholders that do not render
            let (Some(ast), Some(location)) = (&l.ast, locator.get(l.file.file_name())) else { return None };
            if l.has_errors() {
                return None;
            }
            let (text, lines) = luau::render_with_lines(ast, &location.module);
            let line = |pos: u32| diag::line_col(l.file, pos).0;
            let spans = l.spans.iter().map(|&(i, start, end)| (i, line(start), line(end))).collect();
            let compiled = Compiled {
                runtime: runtime.is_some_and(|r| requires_runtime(ast, r)),
                source: Arc::from(text),
                lines: Arc::new(LineMap { starts: lines, spans }),
            };
            let gate = luau::gate::compile(&compiled.source, 2, 1).err();
            Some((compiled, gate))
        });
        for (l, rendered) in lowered.iter().zip(rendered) {
            let name = l.file.file_name();
            let diags = self.file_diags.entry(name.to_owned()).or_default();
            diags.extend(l.diagnostics.iter().map(|&d| diag::from_ts(d, &file_of)));
            let Some((compiled, gate)) = rendered else { continue };
            if let Some(e) = gate {
                let mut d = Diag::new(
                    "luau.gate",
                    Severity::Error,
                    Some(Path::new(name)),
                    format!("output line {}: {}", e.line, e.message),
                );
                d.line = compiled.lines.source_line(e.line).unwrap_or(0);
                d.col = u32::from(d.line > 0);
                diags.push(d);
            }
            if !diags.iter().any(Diag::is_error) {
                self.compiled.insert(name.to_owned(), compiled);
            }
        }
        stats.time("render", t);
    }

    /// Re-reads a Luau source after an edit.
    pub fn reread_luau(&mut self, file: &Path) {
        self.luau_sources.remove(file);
    }

    /// Every diagnostic: routing, the place, then each file's (by file, then line), then the program's.
    pub fn diagnostics(&self) -> Vec<Diag> {
        let mut out: Vec<Diag> = self.route_diags.clone();
        out.extend(self.place_diags.iter().cloned());
        let mut files: Vec<&String> = self.file_diags.keys().collect();
        files.sort();
        for f in files {
            let mut diags = self.file_diags[f].clone();
            diags.sort_by_key(|d| (d.line, d.col));
            out.extend(diags);
        }
        out.extend(self.global_diags.iter().cloned());
        out
    }

    /// The DataModel: `place/` with the `[place]` properties, every routed module, the packages and the runtime.
    /// With `shapes`, a TypeScript module with no output mounts empty (the sourcemap needs only the tree).
    pub fn mount(&mut self, shapes: bool) -> place::Place {
        let dir = self.cfg.root.join("place");
        let (mut place, mut place_diags) = place::read_place(&dir, &self.cfg.place_props);
        if !dir.exists() {
            // no place/ is an empty place, not an error
            place_diags.retain(|d| d.code != "place.io" || d.path != dir);
        }
        let mut diags: Vec<Diag> = place_diags.iter().map(diag::from_place).collect();
        let mut runtime = false;
        for m in &self.scan.modules {
            let source = match m.kind {
                route::Kind::Luau => match self.luau_sources.get(&m.file) {
                    Some(text) => text.clone(),
                    None => match std::fs::read_to_string(&m.file) {
                        Ok(text) => {
                            self.luau_sources.insert(m.file.clone(), text.clone());
                            text
                        }
                        Err(e) => {
                            diags.push(Diag::new("place.io", Severity::Error, Some(&m.file), e.to_string()));
                            continue;
                        }
                    },
                },
                _ => match self.compiled.get(&slashed(&m.file)) {
                    Some(c) => {
                        runtime |= c.runtime;
                        c.source.to_string()
                    }
                    None if shapes => String::new(),
                    None => continue,
                },
            };
            match place.set_script(&m.instance.join("/"), script_kind(m.class), source, Some(m.file.clone())) {
                Ok(r) if m.template => place.disable(r),
                Ok(_) => {}
                Err(d) => diags.push(diag::from_place(&d)),
            }
        }
        for (p, files) in &self.packages {
            runtime |= files.iter().any(|f| f.source.contains(packages::TS_REQUIRE));
            let files = files.iter().map(|f| place::PackageFile {
                rel: f.rel.clone(),
                source: f.source.clone(),
                file: f.file.clone(),
            });
            if let Err(d) = place.mount_package(&p.scope, &p.name, files.collect()) {
                diags.push(diag::from_place(&d));
            }
        }
        if runtime || shapes {
            for (name, source) in [("RuntimeLib", RUNTIME_LIB), ("Promise", PROMISE)] {
                if let Err(d) =
                    place.set_script(&format!("{RUNTIME}/{name}"), place::ScriptKind::Module, source.to_owned(), None)
                {
                    diags.push(diag::from_place(&d));
                }
            }
        }
        self.place_diags = diags;
        place
    }

    /// The Luau of every routed module (compiled or source), by instance path.
    pub fn outputs(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for m in &self.scan.modules {
            let source = match m.kind {
                route::Kind::Luau => self.luau_sources.get(&m.file).cloned(),
                _ => self.compiled.get(&slashed(&m.file)).map(|c| c.source.to_string()),
            };
            if let Some(source) = source {
                out.insert(m.instance.join("/"), source);
            }
        }
        out
    }
}

/// Writes `outputs` under `dir`, one `<instance path>.luau` each.
pub fn emit(dir: &Path, outputs: &BTreeMap<String, String>) -> Result<(), String> {
    for (path, source) in outputs {
        let file = dir.join(format!("{path}.luau"));
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&file, source).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}
