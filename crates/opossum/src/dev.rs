//! `opossum dev`: a full build, then the Studio sync server and a file watcher. Each settled burst of edits (~30 ms
//! quiet) rebuilds what it touched: TypeScript edits reuse the program and re-lower only the files they can affect, a
//! file a `$file` macro reads re-lowers its readers, added or removed files route `src/` again, a `place/` edit
//! re-reads that entry (an edit to a properties file, the whole place), and an edit to `opossum.toml`, `package.json`,
//! bun's lockfile or a `.env` file starts the session over.
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
//! DataModels to come or go. The session keeps a tally of what the playtest logged for stop to print.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use place::Tree;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use sync::{LogKind, Realm, Report};

use crate::build::{LineMap, RUNTIME, Session, script_class, script_kind, slashed};
use crate::config::{self, Config};
use crate::diag::Severity;
use crate::event::{Event, Out};
use crate::{editor, project_path};

const QUIET: Duration = Duration::from_millis(30);
/// How long a request waits for its fence: the watcher's latency plus the build of that burst.
const FENCE_WAIT: Duration = Duration::from_secs(60);
/// The longest time limit an exec may ask for, in seconds.
pub const MAX_TIMEOUT: f64 = 86400.0;
/// The error lines of a playtest `opossum stop` shows.
const FIRST_ERRORS: usize = 5;
/// The changes to the stale files [`Stale`] keeps.
const STALE_KEPT: usize = 64;

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

/// The playtest's DataModels connected now, and what they logged since the first of them connected while none was:
/// counts of errors and warnings and the first error lines, mapped. `play` and `stop` wait on its condvar.
#[derive(Default)]
struct Playtest {
    server: bool,
    client: bool,
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
/// startup included.
pub fn run(start: &Path, json: bool) -> Result<bool, String> {
    let mut out = Out { json, root: start.to_path_buf() };
    let result = config::find(start).and_then(|file| config::load(&file)).and_then(|cfg| {
        out.root = cfg.root.clone();
        editor::write(&cfg.root, &cfg.ts)?;
        session(cfg, &out)
    });
    if let Err(message) = &result
        && json
    {
        out.emit(&Event::Error { message, fatal: true });
    }
    result
}

fn session(cfg: Config, out: &Out) -> Result<bool, String> {
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
        let reload = serve(cfg, &mut dev, &events)?;
        cfg = reload;
        server.configure(cfg.name.clone(), cfg.place.unwrap_or(0), cfg.place.is_none(), cfg.hot_reload);
        let _ = editor::write(&cfg.root, &cfg.ts);
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
    add(&cfg.root.join("place"), RecursiveMode::Recursive)
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
fn serve(cfg: Config, dev: &mut Dev, events: &Receiver<notify::Result<notify::Event>>) -> Result<Config, String> {
    let start = Instant::now();
    let out = dev.out;
    // a fence written before this full build read the files is covered by it
    let covered = dev.fences.lock().unwrap_or_else(|e| e.into_inner()).next;
    let mut session = Session::new(cfg, None)?;
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
        let (mut changed, mut rescan, mut shape, mut luau, mut stale) =
            (Vec::new(), false, false, Vec::new(), Vec::new());
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
        if changed.is_empty() && !rescan && !shape && luau.is_empty() && stale.is_empty() {
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
            dev.edit(&mut session, &luau);
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

    /// Only sources changed: sets them on the tree, unless the shape changed after all: a module that had no output
    /// gained one (its node is a folder, or nothing), or the runtime library came or went.
    fn edit(&mut self, session: &mut Session, luau: &[PathBuf]) {
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

/// What answers `POST /exec`, `/play` and `/stop`: the newest DataModel of the asked realm runs the code once it holds
/// every save made before the request (the module doc), with Studio's output and errors mapped like log lines.
#[derive(Clone)]
struct Execs {
    server: sync::Server,
    fences: Arc<Mutex<Fences>>,
    stale: Arc<Mutex<Stale>>,
    index: Arc<Mutex<LogIndex>>,
    playtest: Playtests,
    /// Held through a play or stop, so two never race each other's checks.
    lifecycle: Arc<Mutex<()>>,
    out: Out,
    root: PathBuf,
}

impl Execs {
    fn handle(&self, request: sync::Request) {
        let answer = self.parse(&request.body).and_then(|body| match request.path.as_str() {
            "/play" => self.play(&body),
            "/stop" => self.stop(&body),
            _ => self.exec(&body),
        });
        let (status, body) = match answer {
            Ok(body) => (200, body),
            Err((status, message)) => (status, json!({ "error": message })),
        };
        let _ = request.reply.send((status, body.to_string()));
    }

    /// The request's JSON body, for this project.
    fn parse(&self, body: &[u8]) -> Result<Value, (u16, String)> {
        let request: Value = serde_json::from_slice(body).map_err(|e| (400, format!("bad request: {e}")))?;
        let root = request["root"].as_str().unwrap_or("");
        let same = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if same(Path::new(root)) != same(&self.root) {
            return Err((400, format!("this opossum dev serves {}, not {root}", self.root.display())));
        }
        Ok(request)
    }

    /// `{root, realm?, name, source, modules?, timeout?, map?, fresh?}` (DESIGN.md, Exec).
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
        let message = json!({
            "t": "exec",
            "name": name,
            "source": request["source"],
            "modules": request["modules"],
        });
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
        if request["map"].as_bool() == Some(true) {
            map_strings(&mut values, &index);
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
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
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
        let started = self.studio(Realm::Edit, "play", &source)?;
        if started["ok"] != true {
            let error = started["error"].as_str().unwrap_or("it did not start");
            let status = if error.contains("already running") { 409 } else { 503 };
            return Err((status, format!("the playtest did not start: {error}")));
        }
        let (lock, changed) = &*self.playtest;
        let left = limit.saturating_sub(start.elapsed());
        let up = |p: &mut Playtest| !(p.server && (run || p.client));
        let (playtest, waited) = changed
            .wait_timeout_while(lock.lock().unwrap_or_else(|e| e.into_inner()), left, up)
            .unwrap_or_else(|e| e.into_inner());
        let mut realms = Vec::new();
        realms.extend(playtest.server.then_some("server"));
        realms.extend(playtest.client.then_some("client"));
        drop(playtest);
        if waited.timed_out() {
            let error = format!("the playtest's DataModels did not all connect within {} s", limit.as_secs_f64());
            return Ok(json!({ "ok": false, "realms": realms, "error": error }));
        }
        // an empty exec there answers once it holds the newest build: at once without hot reload (the build Play
        // copied, maybe older), after it applied the newest with
        let message = json!({ "t": "exec", "name": "play", "source": "return" });
        let ran = self.server.exec(Realm::Server, message, limit, false).map_err(|e| (503, e))?;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let (generation, fresh, stale) = (ran.generation, ran.fresh, self.stale(ran.generation));
        Ok(json!({ "ok": true, "realms": realms, "gen": generation, "fresh": fresh, "stale": stale, "ms": ms }))
    }

    /// `{root, timeout?}` (DESIGN.md, Play): ends the running playtest from its server and answers once its DataModels
    /// disconnected, with the tally of what they logged: `{ok, errors, warnings, first, ms}`, `first` the first error
    /// lines `{realm, text, file, line}`. An error status when nothing runs or it could not be asked to end.
    fn stop(&self, request: &Value) -> Result<Value, (u16, String)> {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
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
        // the play server may close its socket before it answers: that ended it too
        let source = "game:GetService(\"StudioTestService\"):EndTest(\"opossum\")";
        let ended = self.studio(Realm::Server, "stop", source)?;
        if ended["ok"] != true && ended["lost"] != true {
            let error = ended["error"].as_str().unwrap_or("it did not end");
            return Err((503, format!("the playtest did not end: {error}")));
        }
        let (lock, changed) = &*self.playtest;
        let left = limit.saturating_sub(start.elapsed());
        let running = |p: &mut Playtest| p.server || p.client;
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

    /// The files build `generation` holds older code of than their source.
    fn stale(&self, generation: u32) -> Value {
        self.stale.lock().unwrap_or_else(|e| e.into_inner()).of(generation)
    }

    /// Runs one of play's and stop's own snippets in the newest DataModel of `realm` and returns the plugin's result.
    fn studio(&self, realm: Realm, name: &str, source: &str) -> Result<Value, (u16, String)> {
        let message = json!({ "t": "exec", "name": name, "source": source });
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
/// playtest's tally. A socket reports its log lines before it disconnects, so a tally is whole once both are gone.
fn show_reports(reports: Receiver<Report>, index: &Mutex<LogIndex>, playtest: &Playtests, out: &Out, port: u16) {
    let (lock, changed) = &**playtest;
    let connect = |realm: Realm, connected: bool| {
        if !matches!(realm, Realm::Server | Realm::Client) {
            return;
        }
        let mut playtest = lock.lock().unwrap_or_else(|e| e.into_inner());
        if connected && !playtest.server && !playtest.client {
            *playtest = Playtest::default();
        }
        if realm == Realm::Server {
            playtest.server = connected;
        } else {
            playtest.client = connected;
        }
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

/// Maps every string inside `value` like a log line (`opossum test`'s report). In a hexagon case the suite (its first
/// label, the spec's instance name) becomes its file, and a failure's `source` and `line` the project file and line it
/// failed at: its own when that is one, else the first project frame of its trace (hexagon's `expect` names itself), else
/// the place its message names (a suite that failed to load).
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
            if let Some(Value::String(suite)) = fields.get_mut("labels").and_then(|l| l.get_mut(0))
                && let Some((file, _)) = index.get(suite.as_str())
            {
                *suite = file.clone();
            }
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

        // a hexagon failure names its suite by instance, and its place as a bare source and a Luau line: expect's own
        // when it failed an assertion, so the trace's first project frame says where
        let trace = "hexagon.expect:69\nReplicatedStorage.shared.m:7";
        let mut failure = json!({"labels": ["ReplicatedStorage.shared.m", "adds"], "source": "hexagon.expect", "line": 69, "trace": trace});
        map_strings(&mut failure, &index);
        let mapped = json!({"labels": ["src/m.luau", "adds"], "source": "src/m.luau", "line": 7, "trace": "hexagon.expect:69\nsrc/m.luau:7"});
        assert_eq!(failure, mapped);
    }
}
