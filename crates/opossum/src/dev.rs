//! `opossum dev`: a full build, then the Studio sync server and a file watcher. Each settled burst of edits (~30 ms
//! quiet) rebuilds what it touched: TypeScript edits reuse the program and re-lower only the files they can affect, a
//! file a `$file` macro reads re-lowers its readers, added or removed files route `src/` again, a `place/` edit
//! re-reads that entry (an edit to a properties file, the whole place), `opossum.lock` or a file coming or going under
//! an asset root renders the id modules again (new ids only set sources, new keys load the program again), a file's
//! new content there is an `assets` event and nothing else, and an edit to `opossum.toml`, `package.json`, bun's
//! lockfile or a `.env` file starts the session over.
//!
//! The session keeps one [`place::Tree`] for its whole life. An edit that only changed sources sets them on it
//! (a few µs); anything that changed the shape builds a fresh tree and pairs it in, then rewrites the sourcemap and
//! the log index. Studio's output comes back over the socket and prints with compiled lines mapped to their
//! TypeScript. Everything the session says is an [`Event`], text or (`--json`) JSON lines.
//!
//! `opossum exec` and `opossum test` reach the session through the sync server's `POST /exec` on the port written to
//! `.opossum/port`. Each request is answered from code at least as new as every file saved before it arrived: it writes
//! a fence file, `.opossum/fence/<n>`, and waits for the watcher to bring it. FSEvents delivers one stream's events in
//! order, so by then every earlier save has been seen too, and the build of the burst holding the fence is published
//! before the fence is answered. The sync server then runs the code once the DataModel holds that build.
//!
//! `opossum play` and `opossum stop` reach it the same way (`POST /play`, `/stop`): play is an exec in edit that starts
//! a playtest through StudioTestService, stop one in the play server that ends it, and each waits for the playtest's
//! DataModels to come or go. The session keeps a tally of what the playtest logged for stop to print. `opossum pull`
//! (`POST /pull`) is an exec in edit that serializes an instance, which the session writes into `place/` for its own
//! watcher to read back, telling the sync server so edit pairs it with the instances it came from.
//!
//! Several devs (one per worktree) share one Studio through their ports: `POST /status`, which any project may ask,
//! says what this one serves and which DataModels it holds, and `POST /release` (`opossum attach` in another project)
//! tells its plugins to connect to another dev's port.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use luau::luau_string;
use notify::{RecursiveMode, Watcher};
use place::Tree;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use sync::{LogKind, Realm, Report};

use crate::build::{Changed, LineMap, MODULES, RUNTIME, Session, script_class, script_kind, slashed};
use crate::config::{self, Config};
use crate::diag::Severity;
use crate::event::{Event, Out};
use crate::{canonical_prefix, editor, lock, project_path};

const QUIET: Duration = Duration::from_millis(30);
/// How long a request waits for its fence: the watcher's latency plus the build of that burst.
const FENCE_WAIT: Duration = Duration::from_secs(60);
/// The longest time limit an exec may ask for, in seconds.
pub const MAX_TIMEOUT: f64 = 86400.0;
/// The error lines of a playtest `opossum stop` shows.
const FIRST_ERRORS: usize = 5;
/// The changes to the stale files [`Stale`] keeps.
const STALE_KEPT: usize = 64;
/// How long a release waits for the plugins it told to leave.
const RELEASE_WAIT: Duration = Duration::from_secs(10);
/// Bytes of base64 a pull's answer may carry: an 18 MB model, within the 32 MB a message up the socket may hold.
const PULL_LIMIT: usize = 24 << 20;
/// Bytes of a spec report's JSON text a test's answer may carry, far past any run's, within what a message up holds.
const REPORT_LIMIT: usize = 8 << 20;

/// Instance full name (`ServerScriptService.server.app`) → its file relative to the root, and for a compiled
/// module the map from output lines to TypeScript lines.
pub type LogIndex = FxHashMap<String, (String, Option<Arc<LineMap>>)>;

/// Requests waiting for their fence file to come back through the watcher, by number.
#[derive(Default)]
struct Fences {
    next: u64,
    waiting: Vec<(u64, Sender<u32>)>,
}

impl Fences {
    /// Answers every fence up to `upto` with `generation`.
    fn answer(&mut self, upto: u64, generation: u32) {
        self.waiting.retain(|(n, tx)| {
            if *n > upto {
                return true;
            }
            let _ = tx.send(generation);
            false
        });
    }
}

/// How many of the playtest's DataModels are connected now (a test started by hand may have several clients), and
/// what they logged since the first of them connected while none was: counts of errors and warnings and the first error
/// lines, mapped. `play` and `stop` wait on its condvar.
#[derive(Default)]
struct Playtest {
    server: usize,
    client: usize,
    errors: usize,
    warnings: usize,
    first: Vec<Value>,
}

type Playtests = Arc<(Mutex<Playtest>, Condvar)>;

/// The files each build left running older code than their source ([`Session::stale`], relative to the root), from
/// the generation they changed in, oldest first: an answer names the ones of the build its DataModel holds.
#[derive(Default)]
struct Stale(Vec<(u32, Vec<String>)>);

impl Stale {
    fn set(&mut self, generation: u32, files: Vec<String>) {
        if self.0.last().is_some_and(|(_, last)| *last == files) {
            return;
        }
        if self.0.len() == STALE_KEPT {
            self.0.remove(0);
        }
        self.0.push((generation, files));
    }

    /// The stale files of build `generation`; null when it is not known (0, or older than the changes kept).
    fn of(&self, generation: u32) -> Value {
        self.0.iter().rev().find(|(g, _)| *g <= generation).map_or(Value::Null, |(_, files)| json!(files))
    }
}

fn fence_dir(root: &Path) -> PathBuf {
    editor::dir(root).join("fence")
}

/// Runs the project found from `start` until a fatal error, which with `--json` is an event too, a broken config at
/// startup included. `environment` (`--environment`, else the default) is the one whose ids `opossum/resources` holds.
pub fn run(start: &Path, json: bool, environment: Option<&str>) -> Result<bool, String> {
    let mut out = Out { json, root: start.to_path_buf() };
    let result = config::find(start).and_then(|file| config::load(&file)).and_then(|cfg| {
        out.root = cfg.root.clone();
        cfg.build_environment(environment)?;
        let dir = editor::dir(&cfg.root);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        session(cfg, &out, environment)
    });
    if let Err(message) = &result
        && json
    {
        out.emit(&Event::Error { message, fatal: true });
    }
    result
}

fn session(cfg: Config, out: &Out, environment: Option<&str>) -> Result<bool, String> {
    let (report_tx, reports) = mpsc::channel();
    let (request_tx, requests) = mpsc::channel();
    let server = sync::Server::start(sync::Config {
        project: cfg.name.clone(),
        place: cfg.place.unwrap_or(0),
        unpublished: cfg.place.is_none(),
        hot_reload: cfg.hot_reload,
        port: sync::DEFAULT_PORT,
        reports: report_tx,
        requests: request_tx,
    })
    .map_err(|e| format!("sync server: {e}"))?;
    let port = server.port();
    let file = editor::dir(&cfg.root).join("port");
    std::fs::write(&file, port.to_string()).map_err(|e| format!("{}: {e}", file.display()))?;
    out.emit(&Event::Serving { project: &cfg.name, port });

    let index: Arc<Mutex<LogIndex>> = Arc::default();
    let playtest: Playtests = Arc::default();
    let (printer, studio, tally) = (Arc::clone(&index), out.clone(), Arc::clone(&playtest));
    std::thread::Builder::new()
        .name("studio-reports".into())
        .spawn(move || show_reports(reports, &printer, &tally, &studio, port))
        .map_err(|e| e.to_string())?;
    let fences: Arc<Mutex<Fences>> = Arc::default();
    let stale: Arc<Mutex<Stale>> = Arc::default();
    let execs = Execs {
        server: server.clone(),
        fences: Arc::clone(&fences),
        stale: Arc::clone(&stale),
        index: Arc::clone(&index),
        playtest,
        lifecycle: Arc::default(),
        out: out.clone(),
        root: cfg.root.clone(),
    };
    std::thread::Builder::new()
        .name("exec-requests".into())
        .spawn(move || {
            for request in requests {
                let execs = execs.clone();
                let _ = std::thread::Builder::new().name("exec".into()).spawn(move || execs.handle(request));
            }
        })
        .map_err(|e| e.to_string())?;

    let (tx, events) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).map_err(|e| format!("watcher: {e}"))?;
    let mut cfg = cfg;
    // ids live as long as the server, across config reloads
    let mut dev = Dev {
        server: &server,
        tree: Tree::new(),
        index: &index,
        fences: &fences,
        stale: &stale,
        out,
        shown: Vec::new(),
    };
    loop {
        watch(&mut watcher, &cfg)?;
        let reload = serve(cfg, &mut dev, &events, environment)?;
        cfg = reload;
        server.configure(cfg.name.clone(), cfg.place.unwrap_or(0), cfg.place.is_none(), cfg.hot_reload);
    }
}

fn watch(watcher: &mut notify::RecommendedWatcher, cfg: &Config) -> Result<(), String> {
    let mut add = |path: &Path, mode| {
        if path.exists() {
            watcher.watch(path, mode).map_err(|e| format!("watching {}: {e}", path.display()))?;
        }
        Ok::<(), String>(())
    };
    let fences = fence_dir(&cfg.root);
    std::fs::create_dir_all(&fences).map_err(|e| format!("{}: {e}", fences.display()))?;
    add(&fences, RecursiveMode::NonRecursive)?;
    add(&cfg.root, RecursiveMode::NonRecursive)?;
    add(&cfg.src(), RecursiveMode::Recursive)?;
    let place = cfg.root.join("place");
    add(&place, RecursiveMode::Recursive)?;
    // asset roots elsewhere, the outermost of nested ones
    let roots = cfg.asset_roots();
    for root in &roots {
        if !root.starts_with(&place) && !roots.iter().any(|r| r != root && root.starts_with(r)) {
            add(root, RecursiveMode::Recursive)?;
        }
    }
    Ok(())
}

/// The number of a fence file among the watcher's paths.
fn fence_of(path: &Path) -> Option<u64> {
    let dir = path.parent()?;
    let editor = dir.parent()?.file_name()? == editor::DIR;
    (editor && dir.file_name()? == "fence").then(|| path.file_name()?.to_str()?.parse().ok()).flatten()
}

/// The session's tree and what publishing a build into it reaches, for the whole run.
struct Dev<'a> {
    server: &'a sync::Server,
    tree: Tree,
    index: &'a Mutex<LogIndex>,
    fences: &'a Mutex<Fences>,
    stale: &'a Mutex<Stale>,
    out: &'a Out,
    shown: Vec<String>,
}

/// Builds and serves one configuration until its config files change; returns the new configuration.
fn serve(
    cfg: Config,
    dev: &mut Dev,
    events: &Receiver<notify::Result<notify::Event>>,
    environment: Option<&str>,
) -> Result<Config, String> {
    let start = Instant::now();
    let out = dev.out;
    // a fence written before this full build read the files is covered by it
    let covered = dev.fences.lock().unwrap_or_else(|e| e.into_inner()).next;
    let mut session = Session::new(cfg, None, environment)?;
    editor::write(&session.cfg.root, &session.cfg.ts, &session.modules)?;
    let stats = session.build()?;
    dev.shown.clear();
    dev.reshape(&mut session);
    dev.publish(&session, false, start, &stats.times, 0, stats.lowered);
    dev.answer(covered);

    loop {
        let paths = settle(events, out)?;
        let fenced = paths.iter().filter_map(|p| fence_of(p)).max().unwrap_or(0);
        let start = Instant::now();
        let root = session.cfg.root.clone();
        let src = session.cfg.src();
        let asset_roots = session.cfg.asset_roots();
        let (mut changed, mut rescan, mut shape, mut luau, mut stale) =
            (Vec::new(), false, false, Vec::new(), Vec::new());
        // `ids`: the lock changed, or an asset file came or went; `edited`: asset files whose content changed
        let (mut ids, mut edited) = (false, Vec::new());
        // a full read of place/ already holds every later place/ event of the burst
        let mut place_read = false;
        for path in &paths {
            let path = project_path(&session.cfg, path);
            stale.extend(session.readers(&slashed(&path)));
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            let name = rel.to_string_lossy();
            // a lockfile changes when an install rewrote node_modules, which is not watched
            if matches!(name.as_ref(), config::FILE | "package.json" | "bun.lock" | "bun.lockb" | ".env" | ".env.local")
            {
                match config::load(&root.join(config::FILE)) {
                    Ok(cfg) => {
                        out.emit(&Event::Restart { file: &name });
                        return Ok(cfg);
                    }
                    // keep serving the last good configuration until the file is fixed
                    Err(message) => out.emit(&Event::Error { message: &message, fatal: false }),
                }
            }
            if name == lock::FILE {
                ids = true;
                continue;
            }
            if let Some(asset_root) = asset_roots.iter().find(|r| path.starts_with(r)) {
                let file = name.to_string();
                if path.is_file() && session.asset_files.contains(&file) {
                    edited.push(file);
                } else if !is_hidden(&path, asset_root) {
                    ids = true;
                }
                continue;
            }
            if rel.starts_with("place") {
                if !place_read {
                    let read = session.place_changed(&path);
                    shape |= read.is_some();
                    place_read = read == Some(true);
                }
                continue;
            }
            if !path.starts_with(&src) || is_hidden(&path, &src) {
                continue;
            }
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let source = [".ts", ".tsx", ".luau"].iter().any(|e| file.ends_with(e));
            let known = session.scan.find(&path).is_some() || session.scan.decls.contains(&path);
            match (path.is_file(), known) {
                (true, true) if file.ends_with(".luau") => luau.push(path),
                (true, true) => changed.push(slashed(&path)),
                // a new or deleted source file, or a directory made or removed: the file set changed. A vanished
                // path that held nothing scanned (an editor's backup or probe file) changes nothing.
                (exists, _) if source || path.is_dir() || (!exists && session.scan.holds(&path)) => rescan = true,
                _ => {}
            }
        }
        if !edited.is_empty() {
            out.emit(&Event::Assets { changed: &edited });
        }
        // the id modules: new types load the program again, new sources (or warnings) only publish
        let mut modules = false;
        if ids {
            match session.render_modules() {
                Ok(Changed::Nothing) => {}
                Ok(changed) => {
                    rescan |= changed == Changed::Types;
                    modules = true;
                    let cfg = &session.cfg;
                    if let Err(message) = editor::write(&cfg.root, &cfg.ts, &session.modules) {
                        out.emit(&Event::Error { message: &message, fatal: false });
                    }
                }
                Err(message) => out.emit(&Event::Error { message: &message, fatal: false }),
            }
        }
        if changed.is_empty() && !rescan && !shape && luau.is_empty() && stale.is_empty() && !modules {
            dev.answer(fenced);
            continue;
        }
        let packages: Vec<PathBuf> = session.packages.iter().map(|(p, _)| p.dir.clone()).collect();
        let stats = if !changed.is_empty() || rescan || !stale.is_empty() {
            session.update(&changed, rescan, &stale)?
        } else {
            Default::default()
        };
        let t = Instant::now();
        let shape = shape || rescan || !session.packages.iter().map(|(p, _)| &p.dir).eq(packages.iter());
        if shape {
            dev.reshape(&mut session);
        } else {
            dev.edit(&mut session, &luau, modules);
        }
        let mut phases = stats.times;
        phases.push(("publish", t.elapsed().as_secs_f64() * 1000.0));
        dev.publish(&session, true, start, &phases, changed.len() + luau.len(), stats.lowered);
        dev.answer(fenced);
    }
}

fn is_hidden(path: &Path, src: &Path) -> bool {
    path.strip_prefix(src).is_ok_and(|rel| rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')))
}

/// The paths of the next burst of events: blocks for the first, then takes more until none came for [`QUIET`].
fn settle(events: &Receiver<notify::Result<notify::Event>>, out: &Out) -> Result<BTreeSet<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    let first = events.recv().map_err(|_| "the watcher stopped".to_owned())?;
    let mut take = |event: notify::Result<notify::Event>| match event {
        Ok(event) => paths.extend(event.paths),
        Err(e) => out.emit(&Event::Error { message: &format!("watcher: {e}"), fatal: false }),
    };
    take(first);
    loop {
        match events.recv_timeout(QUIET) {
            Ok(event) => take(event),
            Err(RecvTimeoutError::Timeout) => return Ok(paths),
            Err(RecvTimeoutError::Disconnected) => return Err("the watcher stopped".into()),
        }
    }
}

impl Dev<'_> {
    /// Fences up to `upto` came back: everything saved before them is published.
    fn answer(&self, upto: u64) {
        self.fences.lock().unwrap_or_else(|e| e.into_inner()).answer(upto, self.server.generation());
    }

    /// The shape changed (or this is the first build): pairs a fresh tree into the long-lived one, rewrites the
    /// sourcemap and the log index.
    fn reshape(&mut self, session: &mut Session) {
        session.rendered.clear();
        let fresh = session.tree(false);
        self.tree.update(fresh);
        let root = session.cfg.root.clone();
        *self.index.lock().unwrap_or_else(|e| e.into_inner()) = log_index(session);
        let sourcemap = editor::dir(&root).join("sourcemap.json");
        if let Err(message) = editor::write_if_changed(&sourcemap, &self.tree.sourcemap_json(Some(&root))) {
            self.out.emit(&Event::Error { message: &message, fatal: false });
        }
    }

    /// Only sources changed (with `modules`, the id modules' too): sets them on the tree, unless the shape changed
    /// after all: a module that had no output gained one (its node is a folder, or nothing), or the runtime library
    /// came or went.
    fn edit(&mut self, session: &mut Session, luau: &[PathBuf], modules: bool) {
        let mut files: Vec<PathBuf> = session.rendered.drain(..).map(PathBuf::from).collect();
        files.extend(luau.iter().cloned());
        let root = session.cfg.root.clone();
        if self.tree.find(&format!("{RUNTIME}/RuntimeLib")).is_some() != session.needs_runtime() {
            return self.reshape(session);
        }
        let mut edits = Vec::with_capacity(files.len());
        let mut entries = Vec::with_capacity(files.len());
        for file in &files {
            let Some(m) = session.scan.find(file).cloned() else { continue };
            // the node at the module's path must be its own script, not a folder made for its children or a clashing
            // place/ instance
            let id = self.tree.find(&m.instance.join("/")).filter(|&id| {
                self.tree.get(id).is_some_and(|n| {
                    n.origin == place::Origin::Script
                        && n.file.as_deref() == Some(m.file.as_path())
                        && n.class == script_class(script_kind(m.class))
                })
            });
            match (id, session.module_source(&m)) {
                (Some(id), Some(source)) => edits.push((id, source)),
                _ => return self.reshape(session),
            }
            entries.push((m.full_name(), log_entry(session, &root, file)));
        }
        for m in session.modules.iter().filter(|_| modules) {
            let Some(luau) = &m.luau else { continue };
            match self.tree.find(&format!("{MODULES}/{}", m.name)) {
                Some(id) => edits.push((id, place::Source::new(luau.as_str().into()))),
                None => return self.reshape(session),
            }
        }
        self.tree.set_sources(edits);
        self.index.lock().unwrap_or_else(|e| e.into_inner()).extend(entries);
    }

    /// Shows the build's errors and warnings when they differ from the last ones shown, says it built and keeps its
    /// stale files; then publishes the tree, so Studio's acks of the generation always come after. The session is the
    /// only publisher, so the generation is the next one.
    fn publish(
        &mut self,
        session: &Session,
        rebuilt: bool,
        start: Instant,
        phases: &[(&'static str, f64)],
        changed: usize,
        lowered: usize,
    ) {
        let root = &session.cfg.root;
        let diags: Vec<_> = session.diagnostics().into_iter().filter(|d| d.severity != Severity::Note).collect();
        let text: Vec<String> = diags.iter().map(|d| d.human(root)).collect();
        if text != self.shown {
            for d in &diags {
                self.out.emit(&Event::Diag(d));
            }
            self.shown = text;
        }
        let errors = diags.iter().filter(|d| d.is_error()).count();
        let generation = self.server.generation() + 1;
        self.out.emit(&Event::Build {
            rebuilt,
            generation,
            ms: start.elapsed().as_secs_f64() * 1000.0,
            phases,
            modules: session.scan.modules.len(),
            changed,
            lowered,
            errors,
            warnings: diags.len() - errors,
        });
        let stale = session.stale().iter().map(|f| f.strip_prefix(root).unwrap_or(f).display().to_string()).collect();
        self.stale.lock().unwrap_or_else(|e| e.into_inner()).set(generation, stale);
        let published = self.server.publish(self.tree.clone());
        debug_assert_eq!(published, generation);
    }
}

/// What answers every `POST` dev takes. `/exec`, and the execs dev writes itself for `/play`, `/stop` and `/pull`, are
/// this project's only: the newest DataModel of the asked realm runs the code once it holds every save made before the
/// request (the module doc), with Studio's output and errors mapped like log lines. `/status` and `/release` run nothing
/// in Studio and answer any project.
#[derive(Clone)]
struct Execs {
    server: sync::Server,
    fences: Arc<Mutex<Fences>>,
    stale: Arc<Mutex<Stale>>,
    index: Arc<Mutex<LogIndex>>,
    playtest: Playtests,
    /// Held through a play, stop or pull, so two never race each other's checks or files ([`Execs::lifecycle`]).
    lifecycle: Arc<Mutex<()>>,
    out: Out,
    root: PathBuf,
}

impl Execs {
    fn handle(&self, request: sync::Request) {
        // asked by other projects' devs and CLIs
        let answer = match request.path.as_str() {
            "/status" => Ok(self.status()),
            "/release" => self.parse(&request.body, false).and_then(|body| self.release(&body)),
            _ => self.parse(&request.body, true).and_then(|body| match request.path.as_str() {
                "/play" => self.play(&body),
                "/stop" => self.stop(&body),
                "/pull" => self.pull(&body),
                _ => self.exec(&body),
            }),
        };
        let (status, body) = match answer {
            Ok(body) => (200, body),
            Err((status, message)) => (status, json!({ "error": message })),
        };
        let _ = request.reply.send((status, body.to_string()));
    }

    /// The request's JSON body, with `ours` for this project.
    fn parse(&self, body: &[u8], ours: bool) -> Result<Value, (u16, String)> {
        let request: Value = serde_json::from_slice(body).map_err(|e| (400, format!("bad request: {e}")))?;
        if !ours {
            return Ok(request);
        }
        let root = request["root"].as_str().unwrap_or("");
        let same = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if same(Path::new(root)) != same(&self.root) {
            return Err((400, format!("this opossum dev serves {}, not {root}", self.root.display())));
        }
        Ok(request)
    }

    /// `{root, realm?, name, source, timeout?, report?, fresh?}` (DESIGN.md, Exec).
    fn exec(&self, request: &Value) -> Result<Value, (u16, String)> {
        let connected = self.server.realms();
        let realm = match request["realm"].as_str() {
            Some("edit") => Realm::Edit,
            Some("server") => Realm::Server,
            Some("client") => Realm::Client,
            // by default a DataModel that holds every build: the play server only when it takes frames
            None if connected.contains(&Realm::Server) && self.server.hot_reload() => Realm::Server,
            None => Realm::Edit,
            Some(other) => return Err((400, format!("no realm {other}: edit, server or client"))),
        };
        if !connected.contains(&realm) {
            let port = self.server.port();
            return Err((503, format!("no {} DataModel is connected to opossum dev on port {port}", realm.name())));
        }
        self.fence().map_err(|e| (503, e))?;
        let timeout = timeout(request);
        let name = request["name"].as_str().unwrap_or("exec");
        let report = request["report"] == true;
        let mut message = json!({
            "t": "exec",
            "name": name,
            "source": request["source"],
        });
        if report {
            message["limit"] = REPORT_LIMIT.into();
        }
        let fresh = request["fresh"].as_bool() == Some(true);
        let ran = self.server.exec(realm, message, timeout, fresh).map_err(|e| (503, e))?;
        let index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        let map = |v: &Value| v.as_str().map(|text| map_log(&sync::clean(text), &index).0);
        let result = &ran.result;
        let output: Vec<Value> = result["output"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|line| json!({ "kind": line[0], "text": map(&line[1]) }))
            .collect();
        let mut values = result["values"].clone();
        // a spec run returns its report as JSON text: decoded, with its places mapped to the files
        if report
            && let Some(mut decoded) = values[0].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok())
        {
            map_strings(&mut decoded, &index);
            values[0] = decoded;
        }
        let (error, trace) = (map(&result["error"]), map(&result["trace"]));
        drop(index);
        let ok = result["ok"].as_bool().unwrap_or(false);
        let ms = result["ms"].as_f64().unwrap_or(0.0);
        let (generation, fresh) = (ran.generation, ran.fresh);
        self.out.emit(&Event::Exec { realm, generation, name, ok, ms, error: error.as_deref() });
        Ok(json!({
            "ok": ok,
            "realm": realm.name(),
            "gen": generation,
            "fresh": fresh,
            "stale": self.stale(generation),
            "ms": ms,
            "output": output,
            "values": values,
            "error": error,
            "trace": trace,
            "lost": result["lost"] == true,
        }))
    }

    /// `{root, run?, timeout?}` (DESIGN.md, Play): once edit holds every save made before the request, starts a Play
    /// Solo (or with `run`, Run) playtest from the edit plugin, and answers when its DataModels connected and the play
    /// server holds the newest build it will: `{ok, realms, gen, fresh, stale, ms}`, or `ok` false with an `error` when they
    /// did not come within the time limit. An error status when it never started.
    fn play(&self, request: &Value) -> Result<Value, (u16, String)> {
        let _lifecycle = self.lifecycle()?;
        let start = Instant::now();
        let (limit, run) = (timeout(request), request["run"] == true);
        let connected = self.server.realms();
        if connected.contains(&Realm::Server) || connected.contains(&Realm::Client) {
            return Err((409, "a playtest is already running (opossum stop ends it)".into()));
        }
        if !connected.contains(&Realm::Edit) {
            let port = self.server.port();
            return Err((503, format!("no edit DataModel is connected to opossum dev on port {port}")));
        }
        self.fence().map_err(|e| (503, e))?;
        // deferred, so it starts after the exec's recording ends; the method yields until the playtest does
        let method = if run { "ExecuteRunModeAsync" } else { "ExecutePlayModeAsync" };
        let source = format!(
            "local StudioTestService = game:GetService(\"StudioTestService\")
if not StudioTestService.EditModeActive then error(\"a playtest is already running\", 0) end
task.defer(StudioTestService.{method}, StudioTestService, \"opossum\")"
        );
        let started = self.studio(Realm::Edit, "play", &source, false)?;
        if started["ok"] != true {
            let error = started["error"].as_str().unwrap_or("it did not start");
            let status = if error.contains("already running") { 409 } else { 503 };
            return Err((status, format!("the playtest did not start: {error}")));
        }
        let (lock, changed) = &*self.playtest;
        let left = limit.saturating_sub(start.elapsed());
        let up = |p: &mut Playtest| !(p.server > 0 && (run || p.client > 0));
        let (playtest, waited) = changed
            .wait_timeout_while(lock.lock().unwrap_or_else(|e| e.into_inner()), left, up)
            .unwrap_or_else(|e| e.into_inner());
        let mut realms = Vec::new();
        realms.extend((playtest.server > 0).then_some("server"));
        realms.extend((playtest.client > 0).then_some("client"));
        drop(playtest);
        if waited.timed_out() {
            let error = format!("the playtest's DataModels did not all connect within {} s", limit.as_secs_f64());
            return Ok(json!({ "ok": false, "realms": realms, "error": error }));
        }
        // an empty exec there answers once it holds the newest build: at once without hot reload (the build Play
        // copied, maybe older), after it applied the newest with. The playtest runs whatever it answers, so a failure
        // is ok false, never an error status saying it never started.
        let message = json!({ "t": "exec", "name": "play", "source": "return" });
        let ms = || start.elapsed().as_secs_f64() * 1000.0;
        let error = match self.server.exec(Realm::Server, message, limit, false) {
            Ok(ran) if ran.result["ok"] == true => {
                let (generation, fresh, stale) = (ran.generation, ran.fresh, self.stale(ran.generation));
                let ms = ms();
                return Ok(
                    json!({ "ok": true, "realms": realms, "gen": generation, "fresh": fresh, "stale": stale, "ms": ms }),
                );
            }
            Ok(ran) => ran.result["error"].as_str().unwrap_or("it did not answer").to_owned(),
            Err(e) => e,
        };
        let error = format!("the playtest is running, but its server did not say which build it holds: {error}");
        Ok(json!({ "ok": false, "realms": realms, "error": error, "ms": ms() }))
    }

    /// `{root, timeout?}` (DESIGN.md, Play): ends the running playtest from its server and answers once its DataModels
    /// disconnected, with the tally of what they logged: `{ok, errors, warnings, first, ms}`, `first` the first error
    /// lines `{realm, text, file, line}`. An error status when nothing runs or it could not be asked to end.
    fn stop(&self, request: &Value) -> Result<Value, (u16, String)> {
        let _lifecycle = self.lifecycle()?;
        let start = Instant::now();
        let limit = timeout(request);
        let connected = self.server.realms();
        if !connected.contains(&Realm::Server) {
            let error = if connected.contains(&Realm::Client) {
                "the playtest's server DataModel is not connected, so it cannot be ended from here"
            } else {
                "no playtest is running"
            };
            return Err((409, error.into()));
        }
        // the play server may close its socket before it answers: that ended it too. It needs no build, so it goes
        // at once, never behind frames or an exec running there
        let source = "game:GetService(\"StudioTestService\"):EndTest(\"opossum\")";
        let ended = self.studio(Realm::Server, "stop", source, true)?;
        if ended["ok"] != true && ended["lost"] != true {
            let error = ended["error"].as_str().unwrap_or("it did not end");
            return Err((503, format!("the playtest did not end: {error}")));
        }
        let (lock, changed) = &*self.playtest;
        let left = limit.saturating_sub(start.elapsed());
        let running = |p: &mut Playtest| p.server + p.client > 0;
        let (playtest, waited) = changed
            .wait_timeout_while(lock.lock().unwrap_or_else(|e| e.into_inner()), left, running)
            .unwrap_or_else(|e| e.into_inner());
        if waited.timed_out() {
            let error = format!("the playtest's DataModels were still connected after {} s", limit.as_secs_f64());
            return Ok(json!({ "ok": false, "error": error }));
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let (errors, warnings, first) = (playtest.errors, playtest.warnings, playtest.first.clone());
        Ok(json!({ "ok": true, "errors": errors, "warnings": warnings, "first": first, "ms": ms }))
    }

    /// `{root, path, file?, timeout?}` (DESIGN.md, Pull): the instance at `path` (`/`-separated) in edit, serialized by
    /// the plugin and written as an `.rbxm`: to `file`, or where `place/` reads it from ([`destination`]). Answers once
    /// the build reading it is published, `{ok, path, class, file, bytes, gen, ms, warning?}`; an error status when
    /// nothing was written.
    fn pull(&self, request: &Value) -> Result<Value, (u16, String)> {
        let _lifecycle = self.lifecycle()?;
        let start = Instant::now();
        let path = request["path"].as_str().unwrap_or("");
        let names: Vec<&str> = path.split('/').collect();
        if names.iter().any(|n| n.is_empty() || n.starts_with('.')) {
            return Err((400, format!("{path:?} is no instance path pull can write (a name is empty or hidden)")));
        }
        if !self.server.realms().contains(&Realm::Edit) {
            let port = self.server.port();
            return Err((503, format!("no edit DataModel is connected to opossum dev on port {port}")));
        }
        self.fence().map_err(|e| (503, e))?;
        let tree = self.server.tree();
        if place::frame_dir(path).is_some() || tree.find(path).is_some_and(|id| tree.is_frame(id)) {
            let error = format!(
                "{path} is a service (or a container like one), which cannot be serialized; its properties live in \
                 place/*.toml"
            );
            return Err((409, error));
        }
        let place = self.root.join("place");
        let found = destination(&tree, &self.root, path);
        let file = match request["file"].as_str() {
            Some(file) if !file.ends_with(".rbxm") => return Err((400, format!("{file}: pull writes .rbxm files"))),
            Some(file) if Path::new(file).is_dir() => return Err((409, format!("{file} is a directory"))),
            // the place reader's own spelling of it, so a pull into place/ is one however the path was spelled
            Some(file) => match &found {
                Ok(at) if canonical_prefix(at).ok() == canonical_prefix(Path::new(file)).ok() => at.clone(),
                _ => PathBuf::from(file),
            },
            None => found.clone().map_err(|e| (409, e))?,
        };
        let in_place =
            canonical_prefix(&file).ok().zip(canonical_prefix(&place).ok()).is_some_and(|(f, p)| f.starts_with(p));
        if in_place {
            clash(&tree, &self.root, path, &file).map_err(|e| (409, e))?;
        }

        // a copy, so a root's stamp stays out of the file, and so does what routes mount inside it (src/ makes that)
        let lookup: Vec<String> = names.iter().map(|n| luau_string(n)).collect();
        let source = format!(
            "local at: Instance = game
for _, name in {{ {} }} do
	local child = at:FindFirstChild(name)
	if not child then error(`{{at:GetFullName()}} has no child {{name}}`, 0) end
	at = child
end
local copy = at:Clone()
if not copy then error(`{{at:GetFullName()}} is not Archivable, so it cannot be copied`, 0) end
copy:SetAttribute(\"opossum\", nil)
for _, names in {{ {} }} do
	local synced: Instance? = copy
	for _, name in names do
		synced = synced and synced:FindFirstChild(name)
	end
	if synced then synced:Destroy() end
end
local data = game:GetService(\"SerializationService\"):SerializeInstancesAsync({{ copy }})
copy:Destroy()
return at.ClassName, buffer.tostring(game:GetService(\"EncodingService\"):Base64Encode(data))",
            lookup.join(", "),
            mounted(&tree, path).join(", ")
        );
        let message = json!({ "t": "exec", "name": "pull", "source": source, "limit": PULL_LIMIT });
        let ran = self.server.exec(Realm::Edit, message, timeout(request), false).map_err(|e| (503, e))?;
        let result = &ran.result;
        if result["ok"] != true {
            let error = result["error"].as_str().unwrap_or("it did not answer");
            return Err((409, format!("Studio did not serialize {path}: {error}")));
        }
        let (class, text) = (result["values"][0].as_str().unwrap_or(""), result["values"][1].as_str().unwrap_or(""));
        if text.ends_with(" more bytes cut)") {
            return Err((
                413,
                format!("{path} is over {} MB as an .rbxm, more than pull carries", (PULL_LIMIT / 4 * 3) >> 20),
            ));
        }
        let bytes = base64(text).ok_or((502, "the plugin's answer is not base64".to_owned()))?;
        // what place/ could not read back is never written
        match place::rbxm_class(&bytes) {
            Ok(read) if read == class => {}
            Ok(read) => return Err((502, format!("Studio serialized a {read}, not the {class} at {path}"))),
            Err(e) => return Err((502, format!("opossum cannot read what Studio serialized: {e}"))),
        }

        let into_place = found.as_ref().is_ok_and(|f| *f == file);
        if into_place {
            self.server.adopt(path);
        }
        let io = |e: std::io::Error| (500, format!("{}: {e}", file.display()));
        let dir = file.parent().unwrap_or(&place);
        std::fs::create_dir_all(dir).map_err(io)?;
        // whole or not at all: a name the place reader skips, then renamed over
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let part = dir.join(format!(".{name}.pull"));
        std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &file)).map_err(io)?;

        let shown = file.strip_prefix(&self.root).unwrap_or(&file).display().to_string();
        let mut answer = json!({
            "ok": true, "path": path, "class": class, "file": shown, "bytes": bytes.len(), "gen": Value::Null,
        });
        match self.fence() {
            Ok(generation) => answer["gen"] = generation.into(),
            Err(e) => answer["warning"] = format!("written, but {e}").into(),
        }
        // read back as the instance pulled, or for another file of place/ as any instance
        let tree = self.server.tree();
        let from = |id: place::Id| tree.get(id).is_some_and(|n| n.file.as_deref() == Some(&*file));
        let read = if into_place { tree.find(path).is_some_and(from) } else { tree.preorder().into_iter().any(from) };
        if in_place && answer["gen"].is_u64() && !read {
            answer["warning"] = format!("opossum dev did not read {shown} back: see its diagnostics").into();
        }
        answer["ms"] = (start.elapsed().as_secs_f64() * 1000.0).into();
        Ok(answer)
    }

    /// What `opossum status` and `attach` read of this dev: the sync server's status (its project, place, builds and
    /// the DataModels it holds) with the project's root, the port and the process id.
    fn status(&self) -> Value {
        let mut status = self.server.status();
        status["root"] = self.root.to_string_lossy().into();
        status["port"] = self.server.port().into();
        status["pid"] = std::process::id().into();
        status
    }

    /// `{port, force?}` (DESIGN.md, Attach): tells every DataModel connected here to connect to the dev on `port`
    /// instead and answers once their sockets closed, `{ok, realms}`, or `ok` false with an `error` when they did not
    /// within [`RELEASE_WAIT`]. An error status when nothing was told: nothing is connected, or a playtest, an exec, a
    /// play, stop or pull runs and `force` is not set.
    fn release(&self, request: &Value) -> Result<Value, (u16, String)> {
        let port = request["port"].as_u64().and_then(|p| u16::try_from(p).ok());
        let port = port.filter(|&p| p != self.server.port()).ok_or((400, "release takes another dev's port".into()))?;
        let force = request["force"] == true;
        let lifecycle = self.lifecycle.try_lock();
        if lifecycle.is_err() && !force {
            return Err((409, "a play, stop or pull is running (--force moves Studio anyway)".into()));
        }
        let realms = self.server.release(port, force).map_err(|e| (409, e))?;
        let text = format!("Studio is moving to the opossum dev on port {port}");
        self.out.emit(&Event::Log { realm: Realm::Sync, kind: LogKind::Output, text: &text, at: None });
        let names: Vec<&str> = realms.iter().map(|r| r.name()).collect();
        let start = Instant::now();
        while !self.server.realms().is_empty() {
            if start.elapsed() > RELEASE_WAIT {
                let error = format!(
                    "Studio's plugin did not leave within {} s (an older plugin does not know how: opossum plugin \
                     install, then restart Studio)",
                    RELEASE_WAIT.as_secs()
                );
                return Ok(json!({ "ok": false, "realms": names, "error": error }));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(json!({ "ok": true, "realms": names }))
    }

    /// The lifecycle lock for a play, stop or pull. One asked while another holds it is refused, never queued: it could
    /// wait out the other's whole time limit and then act long after its CLI gave up, on a playtest started since.
    fn lifecycle(&self) -> Result<MutexGuard<'_, ()>, (u16, String)> {
        match self.lifecycle.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(e)) => Ok(e.into_inner()),
            Err(TryLockError::WouldBlock) => Err((409, "a play, stop or pull is running".into())),
        }
    }

    /// The files build `generation` holds older code of than their source.
    fn stale(&self, generation: u32) -> Value {
        self.stale.lock().unwrap_or_else(|e| e.into_inner()).of(generation)
    }

    /// Runs one of play's and stop's own snippets in the newest DataModel of `realm` and returns the plugin's result:
    /// once it holds the newest build, or with `now` at once, beside whatever it is doing.
    fn studio(&self, realm: Realm, name: &str, source: &str, now: bool) -> Result<Value, (u16, String)> {
        let message = json!({ "t": "exec", "name": name, "source": source, "now": now });
        let ran = self.server.exec(realm, message, Duration::from_secs(10), false).map_err(|e| (503, e))?;
        Ok(ran.result)
    }

    /// Waits until the watcher brought a fence file written now, and the build of its burst is published.
    fn fence(&self) -> Result<u32, String> {
        let (tx, rx) = mpsc::channel();
        let n = {
            let mut fences = self.fences.lock().unwrap_or_else(|e| e.into_inner());
            fences.next += 1;
            let n = fences.next;
            fences.waiting.push((n, tx));
            n
        };
        let file = fence_dir(&self.root).join(n.to_string());
        let written = std::fs::write(&file, "");
        let answer = written.as_ref().ok().map(|()| rx.recv_timeout(FENCE_WAIT));
        let _ = std::fs::remove_file(&file);
        match (written, answer) {
            (Err(e), _) => Err(format!("{}: {e}", file.display())),
            (_, Some(Ok(generation))) => Ok(generation),
            _ => Err(format!("the file watcher did not bring {} within {} s", file.display(), FENCE_WAIT.as_secs())),
        }
    }
}

/// The `.rbxm` the place reader takes the instance at `path` from, or would: its own file when it comes from an
/// `.rbxm`, else `<name>.rbxm` in its parent's directory, when that parent is a service, a nested container or a
/// directory of `place/`. An error says why no such file can be written: it comes from another kind of file (never
/// overwritten), is inside a model file, is a folder or a synced module, or its parent is not in the files.
fn destination(tree: &Tree, root: &Path, path: &str) -> Result<PathBuf, String> {
    let shown = |file: &Path| file.strip_prefix(root).unwrap_or(file).display().to_string();
    // the node and the nearest of it and its ancestors (below its frame of reference) a file made
    let holder = |id: place::Id| {
        let mut at = id;
        loop {
            let node = tree.get(at)?;
            if let Some(file) = &node.file {
                return Some((at, file.clone()));
            }
            if tree.is_frame(node.parent) {
                return None;
            }
            at = node.parent;
        }
    };
    if let Some(id) = tree.find(path) {
        let node = tree.get(id).expect("found");
        return match (node.origin, holder(id)) {
            (place::Origin::Script, file) => {
                let from = file.map_or("src/".to_owned(), |(_, f)| shown(&f));
                Err(format!("{path} is synced from {from}; name an .rbxm file to write it to instead"))
            }
            (place::Origin::Folder, _) => Err(format!("{path} is a folder of modules synced from src/")),
            (_, Some((at, file))) if at == id && file.extension().is_some_and(|e| e == "rbxm") => {
                Ok(file.to_path_buf())
            }
            (_, Some((at, file))) if at == id => Err(format!(
                "{path} comes from {}, which pull never overwrites or writes beside; name an .rbxm file elsewhere to \
                 export a copy",
                shown(&file)
            )),
            (_, Some((at, file))) => Err(format!(
                "{path} is inside {}, which holds {}: pull that, or name a file",
                shown(&file),
                tree.path_of(at)
            )),
            (_, None) => Err(format!(
                "{path} is a folder of place/ (a directory, or made by a properties file): pull what is in it, or \
                 name a file"
            )),
        };
    }
    // new: written into its parent's directory, mirroring the instance path
    let (parent, name) = path.rsplit_once('/').ok_or_else(|| format!("{path} is not in this place"))?;
    let place = root.join("place");
    // a nested container's directory may also sit in its service's (place/workspace/Terrain)
    let frame_dir = |frame: &str| {
        let own = place.join(place::frame_dir(frame)?);
        let (service, child) = frame.split_once('/').unwrap_or((frame, ""));
        let inside = place.join(place::frame_dir(service)?).join(child);
        Some(if !child.is_empty() && inside.is_dir() { inside } else { own })
    };
    if let Some(dir) = frame_dir(parent) {
        return Ok(dir.join(format!("{name}.rbxm")));
    }
    let not_in_files = || format!("{path}'s parent {parent} is not in the files: pull it instead, or name a file");
    let id = tree.find(parent).ok_or_else(not_in_files)?;
    let node = tree.get(id).expect("found");
    if node.origin != place::Origin::Place || holder(id).is_some() {
        return Err(not_in_files());
    }
    let mut frame = id;
    while !tree.is_frame(frame) {
        frame = tree.get(frame).map_or(place::ROOT, |n| n.parent);
    }
    let frame_path = tree.path_of(frame);
    let below = &parent[frame_path.len() + 1..];
    let dir = frame_dir(&frame_path).ok_or_else(not_in_files)?.join(below);
    if !dir.is_dir() {
        return Err(format!("{parent} is made by a properties file, not a directory: name a file"));
    }
    Ok(dir.join(format!("{name}.rbxm")))
}

/// What routes mount inside the place instance at `path`, each as a Luau list of names from it: the tops of the
/// subtrees `src/` makes there, which a pull leaves out of the file.
fn mounted(tree: &Tree, path: &str) -> Vec<String> {
    let Some(id) = tree.find(path) else { return Vec::new() };
    let synced = |d: &place::Id| {
        tree.get(*d).is_some_and(|n| {
            n.origin != place::Origin::Place && tree.get(n.parent).is_some_and(|p| p.origin == place::Origin::Place)
        })
    };
    let base = tree.path_of(id).len() + 1;
    let tops = tree.preorder_from(id).into_iter().skip(1).filter(synced);
    tops.map(|d| {
        let below = tree.path_of(d)[base..].split('/').map(luau_string).collect::<Vec<_>>();
        format!("{{ {} }}", below.join(", "))
    })
    .collect()
}

/// Refuses a `file` of `place/` that another entry of its directory would make the same instance as: the place reader
/// keeps only one of the two (`tree.instanceClash`), and on a case-insensitive filesystem a name differing only by case
/// is the other entry's file itself. Only the instance's own file may be written over.
fn clash(tree: &Tree, root: &Path, path: &str, file: &Path) -> Result<(), String> {
    let shown = |file: &Path| file.strip_prefix(root).unwrap_or(file).display().to_string();
    let own = tree.find(path).and_then(|id| tree.get(id)).and_then(|n| n.file.clone());
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else { return Ok(()) };
    let made = place::instance_name(name, false);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        let at = entry.path();
        if other.starts_with('.') || !place::instance_name(&other, at.is_dir()).eq_ignore_ascii_case(&made) {
            continue;
        }
        if other == name && own.as_deref() == Some(file) {
            continue;
        }
        return Err(if other == name {
            format!("{} is another entry's file, which pull never overwrites", shown(file))
        } else if other.eq_ignore_ascii_case(name) {
            format!(
                "{} differs from {}, another instance's file, only by case (one file where names ignore it)",
                shown(file),
                shown(&at)
            )
        } else {
            format!("{} already makes {made} there, so place/ would not read {}", shown(&at), shown(file))
        });
    }
    Ok(())
}

/// Standard base64, as EncodingService:Base64Encode writes it; None for anything else.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in text.trim_end_matches('=').as_bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6 | u32::from(v)) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// A request's time limit (default 60 s, at most [`MAX_TIMEOUT`]).
fn timeout(request: &Value) -> Duration {
    let seconds = request["timeout"].as_f64().filter(|t| t.is_finite() && *t > 0.0);
    Duration::from_secs_f64(seconds.map_or(60.0, |t| t.min(MAX_TIMEOUT)))
}

/// The log index of `session`'s build: every routed module by its instance's full name.
pub fn log_index(session: &Session) -> LogIndex {
    let root = &session.cfg.root;
    session.scan.modules.iter().map(|m| (m.full_name(), log_entry(session, root, &m.file))).collect()
}

fn log_entry(session: &Session, root: &Path, file: &Path) -> (String, Option<Arc<LineMap>>) {
    let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();
    (rel, session.compiled.get(&slashed(file)).map(|c| Arc::clone(&c.lines)))
}

/// Shows what the sync server reports until it stops, Studio's output mapped to the files it came from, and keeps the
/// playtest's tally. A socket reports its log lines before it disconnects, so a tally is whole once all are gone.
fn show_reports(reports: Receiver<Report>, index: &Mutex<LogIndex>, playtest: &Playtests, out: &Out, port: u16) {
    let (lock, changed) = &**playtest;
    let connect = |realm: Realm, connected: bool| {
        if !matches!(realm, Realm::Server | Realm::Client) {
            return;
        }
        let mut playtest = lock.lock().unwrap_or_else(|e| e.into_inner());
        if connected && playtest.server + playtest.client == 0 {
            *playtest = Playtest::default();
        }
        let count = if realm == Realm::Server { &mut playtest.server } else { &mut playtest.client };
        *count = if connected { *count + 1 } else { count.saturating_sub(1) };
        changed.notify_all();
    };
    for report in reports {
        match report {
            Report::Log(line) => {
                let (text, at) = map_log(&line.text, &index.lock().unwrap_or_else(|e| e.into_inner()));
                if matches!(line.realm, Realm::Server | Realm::Client) {
                    let mut playtest = lock.lock().unwrap_or_else(|e| e.into_inner());
                    match line.kind {
                        LogKind::Error => {
                            playtest.errors += 1;
                            if playtest.first.len() < FIRST_ERRORS {
                                let (file, number) = at.clone().unzip();
                                let realm = line.realm.name();
                                let entry =
                                    json!({ "realm": realm, "text": text, "file": file, "line": number.flatten() });
                                playtest.first.push(entry);
                            }
                        }
                        LogKind::Warning => playtest.warnings += 1,
                        LogKind::Output => {}
                    }
                }
                let at = at.as_ref().map(|(file, line)| (file.as_str(), *line));
                out.emit(&Event::Log { realm: line.realm, kind: line.kind, text: &text, at });
            }
            Report::Connected { realm, place } => {
                connect(realm, true);
                out.emit(&Event::Connected { realm, place, port });
            }
            Report::Disconnected { realm, place } => {
                connect(realm, false);
                out.emit(&Event::Disconnected { realm, place, port });
            }
            Report::Applied { realm, generation, ms, failed } => {
                out.emit(&Event::Applied { realm, generation, ms, failed });
            }
            Report::Verify { realm, generation, quiet } => out.emit(&Event::Verify { realm, generation, quiet }),
            Report::Failed(failure) => out.emit(&Event::Failed(&failure)),
        }
    }
}

/// Maps every string inside `value` like a log line (a spec report). A failure's `source` and `line` become the project
/// file and line it failed at: its own when that is one, else the first project frame of its trace, else the place its
/// message names (a suite that failed to load).
pub fn map_strings(value: &mut Value, index: &LogIndex) {
    match value {
        Value::String(text) => *text = map_log(text, index).0,
        Value::Array(items) => items.iter_mut().for_each(|v| map_strings(v, index)),
        Value::Object(fields) => {
            let source = fields.get("source").and_then(Value::as_str).zip(fields.get("line").and_then(Value::as_i64));
            let at = source.and_then(|(source, line)| {
                let own = map_log(&format!("{source}:{line}"), index).1;
                own.or_else(|| map_log(fields.get("trace")?.as_str()?, index).1)
                    .or_else(|| map_log(fields.get("message")?.as_str()?, index).1)
            });
            fields.values_mut().for_each(|v| map_strings(v, index));
            if let Some((file, line)) = at {
                fields.insert("source".into(), file.into());
                fields.insert("line".into(), line.into());
            }
        }
        _ => {}
    }
}

/// Rewrites each `Instance.Path:line` (or a stack line's `Script 'Instance.Path', Line line`) of routed modules in
/// `text` to `file:line` (`Script 'file', Line line`), the line mapped to the TypeScript for compiled modules (None
/// when that output line maps to no statement); also returns the first place it mapped. A client script's runtime
/// copy (`Players.<name>.PlayerScripts...`) maps to its StarterPlayerScripts original.
pub fn map_log(text: &str, index: &LogIndex) -> (String, Option<(String, Option<u32>)>) {
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@');
    let mut first = None;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(is_name) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_name(c)).unwrap_or(rest.len());
        let (name, after) = rest.split_at(end);
        // `name:12`, or a stack line's `Script 'name', Line 12`
        let sep = [":", "', Line "].into_iter().find(|s| after.starts_with(s)).unwrap_or("");
        let at = &after[sep.len()..];
        let digits = if sep.is_empty() { 0 } else { at.find(|c: char| !c.is_ascii_digit()).unwrap_or(at.len()) };
        let line: Option<u32> = (digits > 0).then(|| at[..digits].parse().ok()).flatten();
        let copied = name
            .strip_prefix("Players.")
            .and_then(|p| p.split_once('.'))
            .and_then(|(_, p)| p.strip_prefix("PlayerScripts."))
            .map(|p| format!("StarterPlayer.StarterPlayerScripts.{p}"));
        match (index.get(copied.as_deref().unwrap_or(name)), line) {
            (Some((file, lines)), Some(line)) => {
                let mapped = lines.as_ref().map_or(Some(line), |l| l.source_line(line));
                match mapped {
                    Some(ts) => out.push_str(&format!("{file}{sep}{ts}")),
                    None => out.push_str(&format!("{file}{sep}?")),
                }
                first.get_or_insert_with(|| (file.clone(), mapped));
                rest = &at[digits..];
            }
            _ => {
                out.push_str(name);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_name_typescript_lines() {
        // statements start on output lines 1 (header), 3 and 10; TypeScript statements made statements 1 and 2
        let lines = LineMap { starts: vec![1, 3, 10], spans: vec![(1, 5, 9), (2, 20, 30)] };
        let mut index = LogIndex::default();
        index.insert(
            "StarterPlayer.StarterPlayerScripts.client.app".into(),
            ("src/app.ts".into(), Some(Arc::new(lines))),
        );
        index.insert("ReplicatedStorage.shared.m".into(), ("src/m.luau".into(), None));
        let text = "Players.Bob.PlayerScripts.client.app:4: boom\nReplicatedStorage.shared.m:7 function f";
        // line 4 is one line into the statement from line 5; line 12 stays inside the one from 20..30
        let at = Some(("src/app.ts".to_owned(), Some(6)));
        assert_eq!(map_log(text, &index), ("src/app.ts:6: boom\nsrc/m.luau:7 function f".to_owned(), at));
        assert_eq!(map_log("StarterPlayer.StarterPlayerScripts.client.app:99", &index).0, "src/app.ts:30");
        let stack = "Script 'ReplicatedStorage.shared.m', Line 7 - function f";
        assert_eq!(map_log(stack, &index).0, "Script 'src/m.luau', Line 7 - function f");

        // a failure that names no project place of its own (a case of a suite that failed to load, say) takes the
        // trace's first project frame
        let trace = "spec:69\nReplicatedStorage.shared.m:7";
        let mut failure = json!({"labels": ["src/m.luau", "adds"], "source": "unknown", "line": 0, "trace": trace});
        map_strings(&mut failure, &index);
        let mapped = json!({"labels": ["src/m.luau", "adds"], "source": "src/m.luau", "line": 7, "trace": "spec:69\nsrc/m.luau:7"});
        assert_eq!(failure, mapped);
    }

    #[test]
    fn pull_writes_where_place_reads_and_never_over_other_files() {
        let root = std::env::temp_dir().join(format!("opossum-pull-{}", std::process::id()));
        let models = root.join("place/replicated-storage/models");
        std::fs::create_dir_all(&models).unwrap();
        let door = r#"{ "className": "Model", "children": [{ "name": "hinge", "className": "Part" }] }"#;
        std::fs::write(models.join("door.model.json"), door).unwrap();
        std::fs::write(models.join("hull.model.json"), r#"{ "className": "Part" }"#).unwrap();
        let (place, _) = place::read_place(&root.join("place"), &[]);
        let mut bytes = Vec::new();
        place::write_rbxm(&place, &[place.find("ReplicatedStorage/models/hull").unwrap()], &mut bytes).unwrap();
        assert_eq!(place::rbxm_class(&bytes).as_deref(), Ok("Part"));
        std::fs::remove_file(models.join("hull.model.json")).unwrap();
        std::fs::write(models.join("hull.rbxm"), &bytes).unwrap();
        let (place, diags) = place::read_place(&root.join("place"), &[]);
        let mut fresh = Tree::new();
        fresh.add_place(&place);
        let module = place::Source::new(Arc::from("return 1"));
        let file = Some(Arc::from(root.join("src/shared/m.luau")));
        fresh.set_script("ReplicatedStorage/shared/m", place::ScriptKind::Module, module.clone(), file).unwrap();
        // a route mounting inside a place model
        let script = Some(Arc::from(root.join("src/ctl/x.luau")));
        fresh.set_script("ReplicatedStorage/models/hull/ctl/x", place::ScriptKind::Module, module, script).unwrap();
        let mut tree = Tree::new();
        tree.update(fresh);
        let at = |path| destination(&tree, &root, path).map(|f| f.strip_prefix(&root).unwrap().display().to_string());
        let written = at("ReplicatedStorage/models/hull");
        let new =
            [at("ReplicatedStorage/models/boat"), at("Workspace/boat"), at("StarterPlayer/StarterPlayerScripts/x")];
        let refused = [
            "ReplicatedStorage/models/door",
            "ReplicatedStorage/models/door/hinge",
            "ReplicatedStorage/models/door/hinge/x",
            "ReplicatedStorage/models",
            "ReplicatedStorage/shared/m",
            "ReplicatedStorage/shared/x",
            "Workspace/map/boat",
        ]
        .map(|path| at(path).is_err());
        // beside a model.json making the same instance, or a file differing only by case; over its own file, or new
        let clashes = [("door", "door.rbxm"), ("Hull", "Hull.rbxm"), ("hull", "hull.rbxm"), ("boat", "boat.rbxm")]
            .map(|(name, file)| clash(&tree, &root, &format!("ReplicatedStorage/models/{name}"), &models.join(file)));
        std::fs::remove_dir_all(&root).unwrap();

        assert!(diags.is_empty(), "{diags:?}");
        // its own .rbxm, or the directory of a parent place/ reads, as the reader would name it
        assert_eq!(written.as_deref(), Ok("place/replicated-storage/models/hull.rbxm"));
        let new = new.map(Result::unwrap);
        assert_eq!(
            new,
            [
                "place/replicated-storage/models/boat.rbxm",
                "place/workspace/boat.rbxm",
                "place/starter-player-scripts/x.rbxm"
            ]
        );
        // a model.json and what it holds, a folder, a module, and what is under neither files nor a service
        assert_eq!(refused, [true; 7]);
        // what routes mount inside it stays out of its file
        assert_eq!(mounted(&tree, "ReplicatedStorage/models/hull"), ["{ \"ctl\" }"]);
        assert_eq!(clashes.each_ref().map(Result::is_err), [true, true, false, false], "{clashes:?}");
        assert_eq!(base64("TWFueQ=="), Some(b"Many".to_vec()));
        assert_eq!(base64("TW!u"), None);
    }
}
