//! `opossum dev`: a full build, then the Studio sync server and a file watcher. Each settled burst of edits
//! (~30 ms quiet) rebuilds what it touched: TypeScript edits reuse the program and re-lower only the files they can
//! affect, a file a `$file` macro reads re-lowers its readers, added or removed files route `src/` again, `place/`
//! edits re-read the place, and an edit to
//! `opossum.toml`, `package.json` or a `.env` file starts the session over. Studio's output comes back through
//! `POST /log` and prints with compiled lines mapped to their TypeScript.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use place::{Place, Variant};
use rustc_hash::FxHashMap;
use sync::{LogKind, LogLine, Node, Prop, Snapshot};

use crate::build::{LineMap, Session, slashed};
use crate::config::{self, Config};
use crate::{editor, project_path};

const QUIET: Duration = Duration::from_millis(30);

/// Instance full name (`ServerScriptService.server.app`) → its file relative to the root, and for a compiled
/// module the map from output lines to TypeScript lines.
type LogIndex = FxHashMap<String, (String, Option<Arc<LineMap>>)>;

pub fn run(cfg: Config) -> Result<bool, String> {
    let (log_tx, log_rx) = mpsc::channel();
    let roots: Vec<String> = cfg
        .routes
        .iter()
        .map(|(_, target)| target.clone())
        .filter(|t| t.contains('/'))
        .chain(["ReplicatedStorage/rbxts".to_owned()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let server = sync::Server::start(sync::Config {
        project: cfg.name.clone(),
        place: cfg.place.unwrap_or(0),
        unpublished: cfg.place.is_none(),
        roots,
        port: sync::DEFAULT_PORT,
        log: Some(log_tx),
    })
    .map_err(|e| format!("sync server: {e}"))?;
    println!("syncing {} on 127.0.0.1:{}", cfg.name, server.port());

    let index: Arc<Mutex<LogIndex>> = Arc::default();
    let printer = Arc::clone(&index);
    std::thread::Builder::new()
        .name("studio-log".into())
        .spawn(move || print_logs(log_rx, &printer))
        .map_err(|e| e.to_string())?;

    let (tx, events) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).map_err(|e| format!("watcher: {e}"))?;
    let mut cfg = cfg;
    loop {
        watch(&mut watcher, &cfg)?;
        let reload = serve(cfg, &server, &index, &events)?;
        cfg = reload;
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

/// Builds and serves one configuration until its config files change; returns the new configuration.
fn serve(
    cfg: Config,
    server: &sync::Server,
    index: &Mutex<LogIndex>,
    events: &Receiver<notify::Result<notify::Event>>,
) -> Result<Config, String> {
    let start = Instant::now();
    let mut session = Session::new(cfg, None)?;
    let stats = session.build()?;
    let mut shown = Vec::new();
    let generation = publish(&mut session, server, index, &mut shown);
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
        let (mut changed, mut rescan, mut touched, mut stale) = (Vec::new(), false, false, Vec::new());
        for path in &paths {
            let path = project_path(&session.cfg, path);
            stale.extend(session.readers(&slashed(&path)));
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            let name = rel.to_string_lossy();
            if matches!(name.as_ref(), config::FILE | "package.json" | ".env" | ".env.local") {
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
                touched = true;
                continue;
            }
            if !path.starts_with(&src) || is_hidden(&path, &src) {
                continue;
            }
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let source = [".ts", ".tsx", ".luau"].iter().any(|e| file.ends_with(e));
            let known = session.scan.find(&path).is_some() || session.scan.decls.contains(&path);
            match (path.is_file(), known) {
                (true, true) if file.ends_with(".luau") => {
                    session.reread_luau(&path);
                    touched = true;
                }
                (true, true) => changed.push(slashed(&path)),
                // a new or deleted source file, or a directory: the file set changed
                (exists, _) if source || !exists || path.is_dir() => rescan = true,
                _ => {}
            }
        }
        if changed.is_empty() && !rescan && !touched && stale.is_empty() {
            continue;
        }
        let stats = if !changed.is_empty() || rescan || !stale.is_empty() {
            session.update(&changed, rescan, &stale)?
        } else {
            Default::default()
        };
        let t = Instant::now();
        let generation = publish(&mut session, server, index, &mut shown);
        let mut phases: Vec<String> = stats.times.iter().map(|(p, ms)| format!("{p} {ms:.0}")).collect();
        phases.push(format!("publish {}", t.elapsed().as_millis()));
        let errors = session.diagnostics().iter().filter(|d| d.is_error()).count();
        println!(
            "rebuilt in {} ms ({} ms): {} changed, {} lowered, {errors} errors, generation {generation}",
            start.elapsed().as_millis(),
            phases.join(", "),
            changed.len(),
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

/// Mounts the place, publishes its snapshot and refreshes the log index and the sourcemap; prints the errors and
/// warnings when they differ from the last ones shown. Returns the generation.
fn publish(session: &mut Session, server: &sync::Server, index: &Mutex<LogIndex>, shown: &mut Vec<String>) -> u64 {
    let place = session.mount(false);
    let root = session.cfg.root.clone();
    let diags: Vec<String> = session
        .diagnostics()
        .iter()
        .filter(|d| d.severity != crate::diag::Severity::Note)
        .map(|d| d.human(&root))
        .collect();
    if diags != *shown {
        for d in &diags {
            println!("{d}");
        }
        *shown = diags;
    }
    let mut map = LogIndex::default();
    for m in &session.scan.modules {
        let rel = m.file.strip_prefix(&root).unwrap_or(&m.file).display().to_string();
        let lines = session.compiled.get(&slashed(&m.file)).map(|c| Arc::clone(&c.lines));
        map.insert(m.full_name(), (rel, lines));
    }
    *index.lock().unwrap_or_else(|e| e.into_inner()) = map;
    let sourcemap = editor::dir(&root).join("sourcemap.json");
    if let Err(e) = editor::write_if_changed(&sourcemap, &place::sourcemap_json(&place, Some(&root))) {
        eprintln!("{e}");
    }
    server.publish(snapshot(&place, &root))
}

/// The instances opossum mounted (scripts and the folders made for them), with the instances above them up to
/// their service.
fn snapshot(place: &Place, root: &Path) -> Snapshot {
    let mut snap = Snapshot::new();
    for inst in place.dom.descendants() {
        let r = inst.referent();
        if !place.is_owned(r) {
            continue;
        }
        let path = place.path_of(r);
        let mut props = Vec::new();
        let mut source = None;
        for (key, value) in &inst.properties {
            match (key.as_str(), value) {
                ("Source", Variant::String(s)) => source = Some(Arc::from(s.as_str())),
                ("RunContext", Variant::Enum(e)) => {
                    let context = match e.to_u32() {
                        1 => "Server",
                        2 => "Client",
                        _ => "Legacy",
                    };
                    props.push(("RunContext".to_owned(), Prop::Enum("RunContext".into(), context.into())));
                }
                ("Disabled", Variant::Bool(b)) => props.push(("Enabled".to_owned(), Prop::Bool(!*b))),
                _ => {}
            }
        }
        let file = place.file_of(r).map(|f| f.strip_prefix(root).unwrap_or(f).display().to_string());
        // the instances above it that opossum did not make, below the service
        let mut parent = inst.parent();
        let mut at = path.as_str();
        while let Some((up, _)) = at.rsplit_once('/') {
            if !up.contains('/') || snap.contains_key(up) {
                break;
            }
            if let Some(p) = place.dom.get_by_ref(parent) {
                snap.insert(
                    up.to_owned(),
                    Node { class: p.class.to_string(), source: None, props: Vec::new(), file: None },
                );
                parent = p.parent();
            }
            at = up;
        }
        snap.insert(path, Node { class: inst.class.to_string(), source, props, file });
    }
    snap
}

fn print_logs(lines: Receiver<LogLine>, index: &Mutex<LogIndex>) {
    for line in lines {
        let text = {
            let index = index.lock().unwrap_or_else(|e| e.into_inner());
            map_log(&line.text, &index)
        };
        let realm = match line.realm {
            sync::Realm::Edit => "edit",
            sync::Realm::Server => "server",
            sync::Realm::Client => "client",
        };
        match line.kind {
            LogKind::Warning => println!("[{realm}] warning: {text}"),
            LogKind::Error => println!("[{realm}] error: {text}"),
            LogKind::Output | LogKind::Info => println!("[{realm}] {text}"),
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
