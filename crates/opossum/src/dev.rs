//! `opossum dev`: a full build, then the Studio sync server and a file watcher. Each settled burst of edits (~30 ms
//! quiet) rebuilds what it touched: TypeScript edits reuse the program and re-lower only the files they can affect, a
//! file a `$file` macro reads re-lowers its readers, added or removed files route `src/` again, a `place/` edit
//! re-reads that entry (an edit to a properties file, the whole place), and an edit to `opossum.toml`, `package.json`,
//! bun's lockfile or a `.env` file starts the session over.
//!
//! The session keeps one [`place::Tree`] for its whole life. An edit that only changed sources sets them on it
//! (a few µs); anything that changed the shape builds a fresh tree and pairs it in, then rewrites the sourcemap and
//! the log index. Studio's output comes back over the socket and prints with compiled lines mapped to their
//! TypeScript.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use place::Tree;
use rustc_hash::FxHashMap;
use sync::{LogKind, Report};

use crate::build::{LineMap, RUNTIME, Session, script_class, script_kind, slashed};
use crate::config::{self, Config};
use crate::{editor, project_path};

const QUIET: Duration = Duration::from_millis(30);

/// Instance full name (`ServerScriptService.server.app`) → its file relative to the root, and for a compiled
/// module the map from output lines to TypeScript lines.
type LogIndex = FxHashMap<String, (String, Option<Arc<LineMap>>)>;

pub fn run(cfg: Config) -> Result<bool, String> {
    let (log_tx, log_rx) = mpsc::channel();
    let server = sync::Server::start(sync::Config {
        project: cfg.name.clone(),
        place: cfg.place.unwrap_or(0),
        unpublished: cfg.place.is_none(),
        hot_reload: cfg.hot_reload,
        port: sync::DEFAULT_PORT,
        reports: log_tx,
    })
    .map_err(|e| format!("sync server: {e}"))?;
    println!("syncing {} on 127.0.0.1:{}", cfg.name, server.port());

    let index: Arc<Mutex<LogIndex>> = Arc::default();
    let printer = Arc::clone(&index);
    std::thread::Builder::new()
        .name("studio-log".into())
        .spawn(move || print_reports(log_rx, &printer))
        .map_err(|e| e.to_string())?;

    let (tx, events) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).map_err(|e| format!("watcher: {e}"))?;
    let mut cfg = cfg;
    // ids live as long as the server, across config reloads
    let mut tree = Tree::new();
    loop {
        watch(&mut watcher, &cfg)?;
        let reload = serve(cfg, &server, &mut tree, &index, &events)?;
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
    add(&cfg.root, RecursiveMode::NonRecursive)?;
    add(&cfg.src(), RecursiveMode::Recursive)?;
    add(&cfg.root.join("place"), RecursiveMode::Recursive)
}

/// What a burst of edits did to the tree.
struct Dev<'a> {
    server: &'a sync::Server,
    tree: &'a mut Tree,
    index: &'a Mutex<LogIndex>,
    shown: Vec<String>,
}

/// Builds and serves one configuration until its config files change; returns the new configuration.
fn serve(
    cfg: Config,
    server: &sync::Server,
    tree: &mut Tree,
    index: &Mutex<LogIndex>,
    events: &Receiver<notify::Result<notify::Event>>,
) -> Result<Config, String> {
    let start = Instant::now();
    let mut session = Session::new(cfg, None)?;
    let stats = session.build()?;
    let mut dev = Dev { server, tree, index, shown: Vec::new() };
    let generation = dev.reshape(&mut session);
    println!(
        "built {} modules in {} ms ({} lowered), generation {generation}",
        session.scan.modules.len(),
        start.elapsed().as_millis(),
        stats.lowered
    );

    loop {
        let paths = settle(events)?;
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
                        println!("{name} changed, starting over");
                        return Ok(cfg);
                    }
                    // keep serving the last good configuration until the file is fixed
                    Err(e) => eprintln!("opossum: {e}"),
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
        let generation = if shape { dev.reshape(&mut session) } else { dev.edit(&mut session, &luau) };
        let mut phases: Vec<String> = stats.times.iter().map(|(p, ms)| format!("{p} {ms:.0}")).collect();
        phases.push(format!("publish {:.2}", t.elapsed().as_secs_f64() * 1000.0));
        let errors = session.diagnostics().iter().filter(|d| d.is_error()).count();
        println!(
            "rebuilt in {} ms ({} ms): {} changed, {} lowered, {errors} errors, generation {generation}",
            start.elapsed().as_millis(),
            phases.join(", "),
            changed.len() + luau.len(),
            stats.lowered
        );
    }
}

fn is_hidden(path: &Path, src: &Path) -> bool {
    path.strip_prefix(src).is_ok_and(|rel| rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')))
}

/// The paths of the next burst of events: blocks for the first, then takes more until none came for [`QUIET`].
fn settle(events: &Receiver<notify::Result<notify::Event>>) -> Result<BTreeSet<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    let first = events.recv().map_err(|_| "the watcher stopped".to_owned())?;
    let mut take = |event: notify::Result<notify::Event>| match event {
        Ok(event) => paths.extend(event.paths),
        Err(e) => eprintln!("watcher: {e}"),
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
    /// The shape changed (or this is the first build): pairs a fresh tree into the long-lived one, rewrites the
    /// sourcemap and the log index, publishes. Returns the generation.
    fn reshape(&mut self, session: &mut Session) -> u32 {
        session.rendered.clear();
        let fresh = session.tree(false);
        self.tree.update(fresh);
        let root = session.cfg.root.clone();
        let mut map = LogIndex::default();
        for m in &session.scan.modules {
            map.insert(m.full_name(), log_entry(session, &root, &m.file));
        }
        *self.index.lock().unwrap_or_else(|e| e.into_inner()) = map;
        let sourcemap = editor::dir(&root).join("sourcemap.json");
        if let Err(e) = editor::write_if_changed(&sourcemap, &self.tree.sourcemap_json(Some(&root))) {
            eprintln!("{e}");
        }
        self.show(session);
        self.server.publish(self.tree.clone())
    }

    /// Only sources changed: sets them on the tree and publishes, unless the shape changed after all: a module that
    /// had no output gained one (its node is a folder, or nothing), or the runtime library came or went.
    fn edit(&mut self, session: &mut Session, luau: &[PathBuf]) -> u32 {
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
        self.show(session);
        self.server.publish(self.tree.clone())
    }

    /// Prints the errors and warnings when they differ from the last ones shown.
    fn show(&mut self, session: &Session) {
        let root = &session.cfg.root;
        let diags: Vec<String> = session
            .diagnostics()
            .iter()
            .filter(|d| d.severity != crate::diag::Severity::Note)
            .map(|d| d.human(root))
            .collect();
        if diags != self.shown {
            for d in &diags {
                println!("{d}");
            }
            self.shown = diags;
        }
    }
}

fn log_entry(session: &Session, root: &Path, file: &Path) -> (String, Option<Arc<LineMap>>) {
    let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();
    (rel, session.compiled.get(&slashed(file)).map(|c| Arc::clone(&c.lines)))
}

/// Prints what the sync server reports, Studio's output mapped to the files it came from.
fn print_reports(reports: Receiver<Report>, index: &Mutex<LogIndex>) {
    for report in reports {
        match report {
            Report::Log(line) => {
                let text = map_log(&line.text, &index.lock().unwrap_or_else(|e| e.into_inner()));
                match line.kind {
                    LogKind::Output => println!("[{}] {text}", line.realm.name()),
                    kind => println!("[{}] {}: {text}", line.realm.name(), kind.name()),
                }
            }
            Report::Connected { realm, place } => println!("[{}] connected: place {place}", realm.name()),
            Report::Disconnected { realm, .. } => println!("[{}] disconnected", realm.name()),
            Report::Applied { realm, generation, ms, failed } => {
                let failed = if failed > 0 { format!(", {failed} failed") } else { String::new() };
                println!("applied generation {generation} in {} ({ms:.1} ms{failed})", realm.name());
            }
            Report::Verify { realm, generation, quiet } => {
                let why = if quiet { " (an undo's)" } else { "" };
                println!("generation {generation} in {} answered by a verify{why}", realm.name());
            }
            Report::Failed(f) => {
                let at = match (&f.file, &f.path) {
                    (Some(file), Some(path)) => format!("{} ({path})", file.display()),
                    (None, Some(path)) => path.clone(),
                    _ => format!("instance {}", f.id),
                };
                let parked = if f.parked { " (failed again unchanged; parked until it changes or Resync)" } else { "" };
                println!("[sync] error: {at}: {}{parked}", f.message);
            }
        }
    }
}

/// Rewrites each `Instance.Path:line` of routed modules in `text` to `file:line`, the line mapped to the
/// TypeScript for compiled modules. A client script's runtime copy (`Players.<name>.PlayerScripts...`) maps to
/// its StarterPlayerScripts original.
fn map_log(text: &str, index: &LogIndex) -> String {
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(is_name) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_name(c)).unwrap_or(rest.len());
        let (name, after) = rest.split_at(end);
        let digits = after.strip_prefix(':').map_or(0, |a| a.find(|c: char| !c.is_ascii_digit()).unwrap_or(a.len()));
        let line: Option<u32> = (digits > 0).then(|| after[1..=digits].parse().ok()).flatten();
        let copied = name
            .strip_prefix("Players.")
            .and_then(|p| p.split_once('.'))
            .and_then(|(_, p)| p.strip_prefix("PlayerScripts."))
            .map(|p| format!("StarterPlayer.StarterPlayerScripts.{p}"));
        match (index.get(copied.as_deref().unwrap_or(name)), line) {
            (Some((file, lines)), Some(line)) => {
                let mapped = lines.as_ref().map_or(Some(line), |l| l.source_line(line));
                match mapped {
                    Some(ts) => out.push_str(&format!("{file}:{ts}")),
                    None => out.push_str(&format!("{file}:?")),
                }
                rest = &after[1 + digits..];
            }
            _ => {
                out.push_str(name);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
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
        assert_eq!(map_log(text, &index), "src/app.ts:6: boom\nsrc/m.luau:7 function f");
        assert_eq!(map_log("StarterPlayer.StarterPlayerScripts.client.app:99", &index), "src/app.ts:30");
    }
}
