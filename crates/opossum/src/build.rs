//! The build: routes, the program, lowering, the O2 gate and the place, held in a [`Session`] so that `dev` can
//! redo only what an edit touches. One-shot commands make a session, run it once and read its outputs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use globset::{Glob, GlobSet, GlobSetBuilder};
use lower::graph::{FileFacts, Version};
use lower::{LoadOptions, Location, Locator, Project, SourceFileCache};
use luau::{Expr, ModulePath, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};
use tsrs_ast::SourceFile;
use tsrs_core::P;

use crate::config::Config;
use crate::diag::{self, Diag, Severity};
use crate::editor::{self, Module};
use crate::inputs;
use crate::packages::{self, Package};
use crate::{assets, ids, lock};

/// Where the runtime library and its Promise mount.
pub const RUNTIME: &str = "ReplicatedStorage/rbxts";
/// Where the virtual module `opossum/<name>` mounts, under the runtime's folder.
pub const MODULES: &str = "ReplicatedStorage/rbxts/opossum";
const RUNTIME_LIB: &str = include_str!("../../../runtime/RuntimeLib.luau");
const PROMISE: &str = include_str!("../../../runtime/Promise.luau");

/// One compiled TypeScript module.
pub struct Compiled {
    pub source: place::Source,
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

/// What rendering the id modules again changed, the strongest of: their types (a file came or went: the program
/// loads again, and the tree changes shape), what `$asset` references resolve to (a new id or read: the tree is made
/// again, the modules' sources with it), their sources or diagnostics (the tree takes the sources).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Changed {
    Nothing,
    Sources,
    Place,
    Types,
}

pub struct Session {
    pub cfg: Config,
    /// The environment whose ids `opossum/resources` holds; None without `[environments]`.
    environment: Option<String>,
    /// The virtual modules `opossum/<name>`, rendered from the lock and the asset roots' files.
    pub modules: Vec<Module>,
    /// What rendering them reported: asset discovery, the lock, resources not provisioned, the O2 gate.
    module_diags: Vec<Diag>,
    /// The lock's texture packs, read with the modules: every place write sets them.
    pub packs: place::TexturePacks,
    /// What `$asset` references resolve to, read with the modules: every tree resolves the place with it.
    resolver: place::Resolver,
    /// The files whose `$asset`s the last [`Session::render_modules`] gave new uploaded content.
    pub drawn: Vec<PathBuf>,
    /// The asset files discovery found, relative to the root: an event on one that is still there changed its
    /// content, never the modules.
    pub asset_files: FxHashSet<String>,
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
    /// The files each module's `$file` macros read, by module file name: the module lowers again when one changes.
    reads: FxHashMap<String, Vec<String>>,
    global_diags: Vec<Diag>,
    route_diags: Vec<Diag>,
    pub place_diags: Vec<Diag>,
    /// Problems resolving `$asset`s and mounting scripts and packages into the tree, made again with each tree.
    mount_diags: Vec<Diag>,
    pub packages: Vec<(Package, Vec<place::PackageFile>)>,
    /// Luau sources by file, with the modification time and size they were read at: a change the watcher reports
    /// only on a directory (a move, a coalesced checkout) is still read again.
    luau_sources: FxHashMap<PathBuf, (Option<SystemTime>, u64, place::Source)>,
    /// What `place/` holds, read once and then entry by entry as files change.
    place: Option<place::Place>,
    /// RuntimeLib and Promise.
    runtime: [place::Source; 2],
    /// Modules rendered since `dev` last took them (program spelling).
    pub rendered: Vec<String>,
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

pub fn script_kind(class: route::Class) -> place::ScriptKind {
    match class {
        route::Class::ModuleScript => place::ScriptKind::Module,
        route::Class::Server => place::ScriptKind::Server,
        route::Class::Client => place::ScriptKind::Client,
    }
}

/// The class a script of `kind` mounts as.
pub fn script_class(kind: place::ScriptKind) -> &'static str {
    match kind {
        place::ScriptKind::Module => "ModuleScript",
        place::ScriptKind::Server | place::ScriptKind::Client => "Script",
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
    /// A session for `cfg`, with the `[profile.<name>]` excludes when a profile is given, its id modules holding
    /// `environment`'s ids (`--environment`: the default one when None).
    pub fn new(cfg: Config, profile: Option<&str>, environment: Option<&str>) -> Result<Session, String> {
        let environment = cfg.build_environment(environment)?.map(|e| e.name.clone());
        // from the real source directory: the repository is the one that holds the code, even through a link
        let git = inputs::start_git(&cfg.src().canonicalize().unwrap_or_else(|_| cfg.root.clone()));
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
        let mut session = Session {
            cfg,
            environment,
            modules: Vec::new(),
            module_diags: Vec::new(),
            packs: place::TexturePacks::new(),
            resolver: place::Resolver::default(),
            drawn: Vec::new(),
            asset_files: FxHashSet::default(),
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
            reads: FxHashMap::default(),
            global_diags: Vec::new(),
            route_diags: Vec::new(),
            place_diags: Vec::new(),
            mount_diags: Vec::new(),
            packages: Vec::new(),
            luau_sources: FxHashMap::default(),
            place: None,
            runtime: [RUNTIME_LIB, PROMISE].map(|text| place::Source::new(text.into())),
            rendered: Vec::new(),
        };
        session.render_modules()?;
        Ok(session)
    }

    /// Renders the virtual modules again from the lock and the asset roots as they are now, and takes the lock's packs
    /// and `$asset` resolver, saying what changed (packs never change the modules or the tree). Fails only when the
    /// lock cannot be read.
    pub fn render_modules(&mut self) -> Result<Changed, String> {
        let root = &self.cfg.root;
        let environment = self.cfg.build_environment(self.environment.as_deref())?;
        let (lock, mut diags) = lock::read(root)?;
        let (found, discovered) = assets::discover(root, &self.cfg.assets);
        diags.extend(discovered);
        let (modules, warnings) = ids::modules(&self.cfg, environment, &lock, &found);
        diags.extend(warnings);
        for m in &modules {
            if let Some(Err(e)) = m.luau.as_deref().map(|luau| luau::gate::compile(luau, 2, 1)) {
                let file = editor::module_luau(&self.cfg.root, &m.name);
                let mut d = Diag::new("luau.gate", Severity::Error, Some(&file), e.message);
                (d.line, d.col) = (e.line, 1);
                diags.push(d);
            }
        }
        self.asset_files = found.iter().map(|a| a.file.clone()).collect();
        let resolver = ids::resolver(&lock, &found);
        let types = |m: &[Module]| m.iter().map(|m| (m.name.clone(), m.types.clone())).collect::<Vec<_>>();
        let sources = |m: &[Module]| m.iter().map(|m| m.luau.clone()).collect::<Vec<_>>();
        self.drawn = self.place.as_ref().map_or_else(Vec::new, |place| {
            let mut drawn: Vec<PathBuf> = place
                .asset_refs()
                .filter(|(file, _)| {
                    let now = resolver.by_file.get(*file);
                    now != self.resolver.by_file.get(*file) && !matches!(now, None | Some(place::Value::Unset))
                })
                .map(|(_, spec)| spec.to_path_buf())
                .collect();
            drawn.sort();
            drawn.dedup();
            drawn
        });
        let changed = if types(&modules) != types(&self.modules) {
            Changed::Types
        } else if resolver != self.resolver {
            Changed::Place
        } else if sources(&modules) != sources(&self.modules) || diags != self.module_diags {
            Changed::Sources
        } else {
            Changed::Nothing
        };
        self.modules = modules;
        self.module_diags = diags;
        self.packs = lock.packs;
        self.resolver = resolver;
        Ok(changed)
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
        // the runtime library and the virtual modules are modules of their own route root
        let root = slashed(&self.cfg.root);
        let shared = |names: Vec<String>| Location {
            module: ModulePath { names, root: 2, init: false },
            realm: lower::Realm::Shared,
        };
        for m in self.modules.iter().filter(|m| m.luau.is_some()) {
            let names = MODULES.split('/').map(str::to_owned).chain([m.name.clone()]).collect();
            files.insert(lower::program::module_file(&root, &m.name), shared(names));
        }
        let runtime = shared(vec!["ReplicatedStorage".into(), "rbxts".into(), "RuntimeLib".into()]);
        self.locator = Locator { files, runtime_lib: Some(runtime) };
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

    /// Whether `src/` holds TypeScript. A project without (Luau, or assets and a place alone) has no program to
    /// check: its globals need the Roblox types, which only TypeScript needs installed.
    fn ts(&self) -> bool {
        self.scan.modules.iter().any(|m| matches!(m.kind, route::Kind::Ts | route::Kind::Tsx))
    }

    /// Loads the program afresh (parsed files come from the session's cache), routes the package files it reached
    /// and gathers the packages to mount.
    fn load(&self) -> Result<Project, String> {
        // without them no global exists (Array, print, Instance), and every file fails a dozen ways that hide why
        let types = ["@rbxts/compiler-types", "@rbxts/types"];
        let missing: Vec<&str> =
            types.into_iter().filter(|p| !self.cfg.root.join("node_modules").join(p).is_dir()).collect();
        if self.ts() && !missing.is_empty() {
            return Err(format!(
                "node_modules has no {}: install the Roblox types as dev dependencies (`bun add -d {}`, or your \
                 package manager's add)",
                missing.join(" or "),
                missing.join(" ")
            ));
        }
        let checkers = std::thread::available_parallelism().map_or(4, usize::from);
        let root = slashed(&self.cfg.root);
        let options = LoadOptions {
            overlay: editor::overlay(&root, &self.modules),
            root,
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
    /// The modules whose `$file` macros read `path` (absolute, forward slashes).
    pub fn readers(&self, path: &str) -> Vec<String> {
        self.reads.iter().filter(|(_, files)| files.iter().any(|f| f == path)).map(|(m, _)| m.clone()).collect()
    }

    /// Rebuilds after an edit: `changed` TypeScript files, `rescan` when the file set changed, and `stale` modules
    /// that must lower again although their source did not change (a file their `$file` macros read did).
    pub fn update(&mut self, changed: &[String], rescan: bool, stale: &[String]) -> Result<Stats, String> {
        let mut stats = Stats::default();
        let t = Instant::now();
        let before = rescan.then(|| std::mem::take(&mut self.locator.files));
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
            self.reads.remove(gone);
        }
        // a module that routes elsewhere now changes how its importers spell it, which no fact of the program shows
        let moved = before
            .is_some_and(|old| old.iter().any(|(name, l)| self.locator.files.get(name).is_some_and(|now| now != l)));
        let all = project.project_files();
        let files: Vec<P<SourceFile>> = if invalidation.all || moved {
            all
        } else {
            all.into_iter()
                .filter(|f| {
                    invalidation.affected.contains(&f.path().to_string()) || stale.iter().any(|s| s == f.file_name())
                })
                .collect()
        };
        self.compile(&project, &files, &mut stats);
        self.commit(project, facts);
        Ok(stats)
    }

    /// Keeps `project` and its facts, freeing the program it replaces. A reuse with no edited file (only `$file`
    /// inputs changed) hands back the same program: the old `Project` is then only dropped, which releases its
    /// lineage reference.
    fn commit(&mut self, project: Project, facts: Vec<(String, FileFacts)>) {
        self.facts = facts.into_iter().collect();
        let program = project.program;
        if let Some(old) = self.project.replace(project)
            && !std::ptr::eq(old.program, program)
        {
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
        let checked = if self.ts() { project.check(&ctx, files) } else { Vec::new() };
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
                source: place::Source::new(Arc::from(text)),
                lines: Arc::new(LineMap { starts: lines, spans }),
            };
            let gate = luau::gate::compile(&compiled.source.text, 2, 1).err();
            Some((compiled, gate))
        });
        for (l, rendered) in lowered.iter().zip(rendered) {
            let name = l.file.file_name();
            let reads: Vec<String> = l
                .inputs
                .iter()
                .filter_map(|i| match i {
                    lower::Input::File(path) => Some(path.clone()),
                    _ => None,
                })
                .collect();
            if reads.is_empty() {
                self.reads.remove(name);
            } else {
                self.reads.insert(name.to_owned(), reads);
            }
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
                self.rendered.push(name.to_owned());
            }
        }
        stats.time("render", t);
    }

    /// Every diagnostic: routing, the place, then each file's (by file, then line), then the program's.
    pub fn diagnostics(&self) -> Vec<Diag> {
        let mut out: Vec<Diag> = self.route_diags.clone();
        out.extend(self.module_diags.iter().cloned());
        out.extend(self.place_diags.iter().cloned());
        out.extend(self.mount_diags.iter().cloned());
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

    /// Every SurfaceAppearance and MaterialVariant with maps in the last tree's place.
    pub fn surfaces(&self) -> Vec<place::Surface> {
        self.place.as_ref().map_or_else(Vec::new, place::Place::surfaces)
    }

    /// The last tree's `place.unknownPart` warnings as errors. A build draws such a part empty and goes on, since the
    /// upload that brings it may come after the build; a deploy, which has uploaded and read everything, refuses them.
    pub fn unknown_parts(&self) -> Vec<Diag> {
        let parts = self.mount_diags.iter().filter(|d| d.code == "place.unknownPart");
        parts.map(|d| Diag { severity: Severity::Error, ..d.clone() }).collect()
    }

    /// Reads a Luau module's source, from the session's cache while the file's modification time and size are
    /// the ones it was read at.
    fn luau(&mut self, file: &Path) -> Result<place::Source, String> {
        let meta = std::fs::metadata(file).map_err(|e| e.to_string())?;
        let (time, size) = (meta.modified().ok(), meta.len());
        if let Some((t, n, source)) = self.luau_sources.get(file)
            && (*t, *n) == (time, size)
        {
            return Ok(source.clone());
        }
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let source = place::Source::new(text.into());
        self.luau_sources.insert(file.to_path_buf(), (time, size, source.clone()));
        Ok(source)
    }

    /// The source a routed module mounts with: its Luau, or its compiled output (None while it has none).
    pub fn module_source(&mut self, m: &route::Module) -> Option<place::Source> {
        match m.kind {
            route::Kind::Luau => self.luau(&m.file).ok(),
            _ => self.compiled.get(&slashed(&m.file)).map(|c| c.source.clone()),
        }
    }

    /// `place/` changed at `path`: re-reads that entry, replacing the diagnostics of what it re-read (the properties
    /// files among it) and every unresolved ref (a re-read reports those whole). None when `path` is no content;
    /// Some(true) when all of `place/` was read again, which covers every other event of the burst.
    pub fn place_changed(&mut self, path: &Path) -> Option<bool> {
        let dir = self.cfg.root.join("place");
        let place = self.place.as_mut()?;
        let Some((scopes, diags)) = place.replace_entry(&dir, &self.cfg.asset_roots(), path) else {
            // a top-level directory that named no service and went away takes its error along
            let top = dir.join(path.strip_prefix(&dir).ok()?.components().next()?);
            let before = self.place_diags.len();
            self.place_diags.retain(|d| d.file.as_deref() != Some(top.as_path()));
            return (self.place_diags.len() != before).then_some(false);
        };
        self.place_diags.retain(|d| {
            let file = d.file.as_deref();
            let scoped = file.is_some_and(|f| scopes.iter().any(|s| f.starts_with(s)));
            !scoped && d.code != "place.badRef"
        });
        self.place_diags.extend(diags.iter().map(diag::from_place));
        Some(scopes == [dir])
    }

    /// A fresh tree of the whole DataModel: `place/` with its properties files and `$asset`s resolved, every routed
    /// module, the packages and the runtime. With `shapes`, a TypeScript module with no output mounts empty (the
    /// sourcemap needs only the tree).
    pub fn tree(&mut self, shapes: bool) -> place::Tree {
        let dir = self.cfg.root.join("place");
        if self.place.is_none() {
            let (place, mut place_diags) = place::read_place(&dir, &self.cfg.asset_roots());
            if !dir.exists() {
                // no place/ is an empty place, not an error
                place_diags.retain(|d| d.code != "place.io" || d.path != dir);
            }
            self.place_diags = place_diags.iter().map(diag::from_place).collect();
            self.place = Some(place);
        }
        let place = self.place.as_mut().expect("read above");
        let mut diags: Vec<Diag> = place.resolve_assets(&self.resolver).iter().map(diag::from_place).collect();
        let mut tree = place::Tree::new();
        tree.add_place(place);
        let runtime = self.needs_runtime();
        let modules = std::mem::take(&mut self.scan.modules);
        for m in &modules {
            let source = match self.module_source(m) {
                Some(source) => source,
                None if m.kind == route::Kind::Luau => {
                    diags.push(Diag::new("place.io", Severity::Error, Some(&m.file), "cannot read the file".into()));
                    continue;
                }
                None if shapes => place::Source::new("".into()),
                None => continue,
            };
            let file: Arc<Path> = Arc::from(m.file.as_path());
            match tree.set_script(&m.instance.join("/"), script_kind(m.class), source, Some(file)) {
                Ok(id) if m.template => tree.disable(id),
                Ok(_) => {}
                Err(d) => diags.push(diag::from_place(&d)),
            }
        }
        self.scan.modules = modules;
        for (p, files) in &self.packages {
            if let Err(d) = tree.mount_package(&p.scope, &p.name, files) {
                diags.push(diag::from_place(&d));
            }
        }
        for m in &self.modules {
            let Some(luau) = &m.luau else { continue };
            let file: Arc<Path> = Arc::from(editor::module_luau(&self.cfg.root, &m.name).as_path());
            let source = place::Source::new(luau.as_str().into());
            if let Err(d) =
                tree.set_script(&format!("{MODULES}/{}", m.name), place::ScriptKind::Module, source, Some(file))
            {
                diags.push(diag::from_place(&d));
            }
        }
        if runtime || shapes {
            for (name, source) in ["RuntimeLib", "Promise"].iter().zip(&self.runtime) {
                let path = format!("{RUNTIME}/{name}");
                if let Err(d) = tree.set_script(&path, place::ScriptKind::Module, source.clone(), None) {
                    diags.push(diag::from_place(&d));
                }
            }
        }
        self.mount_diags = diags;
        tree
    }

    /// The compiled modules whose current source has no output: it failed to compile, so the tree holds their last
    /// good output, or nothing.
    pub fn stale(&self) -> Vec<&Path> {
        let failed = |name: &str| {
            !self.compiled.contains_key(name) || self.file_diags.get(name).is_some_and(|d| d.iter().any(Diag::is_error))
        };
        let modules = self.scan.modules.iter().filter(|m| m.kind != route::Kind::Luau && failed(&slashed(&m.file)));
        modules.map(|m| m.file.as_path()).collect()
    }

    /// Whether a compiled module holds this build's `$buildTime()`, so no two builds have the same bytes.
    pub fn timestamped(&self) -> bool {
        self.compiled.values().any(|c| c.source.text.contains(self.options.build_time.as_str()))
    }

    /// Whether the tree mounts the runtime library: a compiled module or a package requires it.
    pub fn needs_runtime(&self) -> bool {
        self.scan.modules.iter().any(|m| self.compiled.get(&slashed(&m.file)).is_some_and(|c| c.runtime))
            || self.packages.iter().any(|(_, files)| files.iter().any(|f| f.source.text.contains(packages::TS_REQUIRE)))
    }

    /// The Luau of every routed module (compiled or source) and of the id modules, by instance path.
    pub fn outputs(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for m in &self.scan.modules {
            let source = match m.kind {
                route::Kind::Luau => self.luau_sources.get(&m.file).map(|(_, _, s)| s.text.to_string()),
                _ => self.compiled.get(&slashed(&m.file)).map(|c| c.source.text.to_string()),
            };
            if let Some(source) = source {
                out.insert(m.instance.join("/"), source);
            }
        }
        for m in &self.modules {
            if let Some(luau) = &m.luau {
                out.insert(format!("{MODULES}/{}", m.name), luau.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The id modules build like any module, and what an edit under an asset root or to the lock changes decides how
    /// dev takes it: a new id makes the tree again (a `$asset` may name it), a file coming or going loads the program
    /// again, new bytes nothing.
    #[test]
    fn id_modules_build_and_say_what_an_edit_changed() {
        let root = std::env::temp_dir().join(format!("opossum-build-ids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // the type packages empty: a build needs them, and the program's global types missing is only its own errors
        for dir in ["src/shared", "art/sfx", "node_modules/@rbxts/types", "node_modules/@rbxts/compiler-types"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let write = |file: &str, text: &str| std::fs::write(root.join(file), text).unwrap();
        write(
            "opossum.toml",
            "name = \"ids\"\ncreator = { user = 1 }\n[routes]\nshared = \"ReplicatedStorage/shared\"\n\
             [assets]\nsfx = \"art/sfx\"\n",
        );
        write("art/sfx/boom.ogg", "ogg");
        let lock = |id: u64| {
            format!("version = 1\nassets.\"art/sfx/boom.ogg\" = {{ id = {id}, hash = \"{}\" }}\n", "0".repeat(32))
        };
        write("opossum.lock", &lock(5));
        write("src/shared/a.ts", "import sfx from \"opossum/sfx\";\nexport const boom: string = sfx.boom;\n");
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let (mut session, errors, tree) = std::thread::Builder::new()
            .stack_size(lower::STACK_SIZE)
            .spawn(move || {
                let mut session = Session::new(cfg, None, None).unwrap();
                session.build().unwrap();
                let tree = session.tree(false);
                let errors: Vec<Diag> =
                    session.diagnostics().into_iter().filter(|d| d.is_error() && d.file.is_some()).collect();
                (session, errors, tree)
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let module = tree.get(tree.find(&format!("{MODULES}/sfx")).unwrap()).unwrap();
        assert!(module.source.as_ref().unwrap().text.contains("boom = \"rbxassetid://5\""));
        let a = &session.compiled[&slashed(&session.cfg.root.join("src/shared/a.ts"))].source.text;
        assert!(a.contains("require(\"@game/ReplicatedStorage/rbxts/opossum/sfx\")"), "{a}");

        write("art/sfx/boom.ogg", "new bytes");
        assert_eq!(session.render_modules().unwrap(), Changed::Nothing);
        write("opossum.lock", &lock(6));
        assert_eq!(session.render_modules().unwrap(), Changed::Place);
        write("art/sfx/bang.ogg", "ogg");
        assert_eq!(session.render_modules().unwrap(), Changed::Types);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A project of a place alone, with no source, no routes and no node_modules, checks clean: it has no program
    /// whose globals would need the Roblox types.
    #[test]
    fn a_place_alone_checks_clean() {
        let root = std::env::temp_dir().join(format!("opossum-build-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("place/workspace")).unwrap();
        std::fs::write(root.join("opossum.toml"), "name = \"place\"\n").unwrap();
        std::fs::write(root.join("place/workspace/crate.model.json"), r#"{ "className": "Part" }"#).unwrap();
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let (diags, tree) = std::thread::Builder::new()
            .stack_size(lower::STACK_SIZE)
            .spawn(move || {
                let mut session = Session::new(cfg, None, None).unwrap();
                session.build().unwrap();
                let tree = session.tree(false);
                (session.diagnostics(), tree)
            })
            .unwrap()
            .join()
            .unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert!(tree.find("Workspace/crate").is_some());
    }
}
