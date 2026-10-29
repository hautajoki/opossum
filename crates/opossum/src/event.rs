//! What `opossum dev` tells its terminal: one [`Event`] per line, as text, or with `--json` as one JSON object (the
//! schema is DESIGN.md's "Dev events"). Both renderings read the same value, so they cannot drift apart.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sync::{Failure, LogKind, Realm};

use crate::diag::Diag;

pub enum Event<'a> {
    /// The sync server listens.
    Serving {
        project: &'a str,
        port: u16,
    },
    /// A build published generation `generation`: the session's first, or a rebuild after a burst of edits.
    Build {
        rebuilt: bool,
        generation: u32,
        ms: f64,
        phases: &'a [(&'static str, f64)],
        modules: usize,
        changed: usize,
        lowered: usize,
        errors: usize,
        warnings: usize,
    },
    /// One of the build's errors and warnings; the set is shown again only when it changed.
    Diag(&'a Diag),
    Connected {
        realm: Realm,
        place: u64,
        port: u16,
    },
    Disconnected {
        realm: Realm,
        place: u64,
        port: u16,
    },
    /// A DataModel holds a build's generation now, `ms` from the frame's first part arriving to its ack.
    Applied {
        realm: Realm,
        generation: u32,
        ms: f64,
        failed: usize,
    },
    /// A DataModel answered a generation with a verify instead of an ack (`quiet`: an undo asked).
    Verify {
        realm: Realm,
        generation: u32,
        quiet: bool,
    },
    Failed(&'a Failure),
    /// A DataModel's output (or the sync's own notice), with `Instance.Path:line` mapped to `file:line`; `at` is the
    /// first place it mapped, its line unknown when the output line maps to no statement.
    Log {
        realm: Realm,
        kind: LogKind,
        text: &'a str,
        at: Option<(&'a str, Option<u32>)>,
    },
    /// `opossum exec` or `test` ran `name` in a DataModel holding `generation` (`ms` in Studio).
    Exec {
        realm: Realm,
        generation: u32,
        name: &'a str,
        ok: bool,
        ms: f64,
        error: Option<&'a str>,
    },
    /// Asset files whose content changed: their ids still hold what was uploaded, until `opossum upload`.
    Assets {
        changed: &'a [String],
    },
    /// opossum.lock gave these place files' `$asset`s new uploaded content.
    Drawn {
        files: &'a [PathBuf],
    },
    /// The pack capture: packs Studio baked that it saved to opossum.lock, keys whose maps it set again so Studio bakes
    /// them, and keys still unbaked after that (each named once), `after` seconds since they were first missing, with
    /// their class and the files using them.
    Packs {
        baked: usize,
        nudged: usize,
        unbaked: &'a [(&'static str, Vec<PathBuf>)],
        after: u64,
    },
    /// A config file changed: the session starts over.
    Restart {
        file: &'a str,
    },
    /// Something went wrong; `fatal` ends the session.
    Error {
        message: &'a str,
        fatal: bool,
    },
}

/// Where events go: text on stdout (errors on stderr), or JSON lines on stdout and nothing else.
#[derive(Clone)]
pub struct Out {
    pub json: bool,
    /// Files are shown relative to it.
    pub root: PathBuf,
}

impl Out {
    pub fn emit(&self, event: &Event) {
        if self.json {
            println!("{}", event.json(&self.root));
        } else if let Event::Error { message, .. } = event {
            eprintln!("opossum: {message}");
        } else {
            println!("{}", event.human(&self.root));
        }
    }
}

fn shown(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).display().to_string()
}

impl Event<'_> {
    pub fn human(&self, root: &Path) -> String {
        match *self {
            Event::Serving { project, port } => format!("syncing {project} on 127.0.0.1:{port}"),
            Event::Build { rebuilt: false, generation, ms, modules, lowered, .. } => {
                format!("built {modules} modules in {ms:.0} ms ({lowered} lowered), generation {generation}")
            }
            Event::Build { generation, ms, phases, changed, lowered, errors, .. } => {
                // sub-millisecond phases (publish) keep two decimals
                let phases: Vec<String> = phases
                    .iter()
                    .map(|&(p, t)| if t < 1.0 { format!("{p} {t:.2}") } else { format!("{p} {t:.0}") })
                    .collect();
                format!(
                    "rebuilt in {ms:.0} ms ({} ms): {changed} changed, {lowered} lowered, {errors} errors, generation \
                     {generation}",
                    phases.join(", ")
                )
            }
            Event::Diag(d) => d.human(root),
            Event::Connected { realm, place, port } => {
                format!("[{}] connected: place {place}, port {port}", realm.name())
            }
            Event::Disconnected { realm, .. } => format!("[{}] disconnected", realm.name()),
            Event::Applied { realm, generation, ms, failed } => {
                let failed = if failed > 0 { format!(", {failed} failed") } else { String::new() };
                format!("applied generation {generation} in {} ({ms:.1} ms{failed})", realm.name())
            }
            Event::Verify { realm, generation, quiet } => {
                let why = if quiet { " (an undo's)" } else { "" };
                format!("generation {generation} in {} answered by a verify{why}", realm.name())
            }
            Event::Failed(f) => {
                let at = match (&f.file, &f.path) {
                    (Some(file), Some(path)) => format!("{} ({path})", shown(root, file)),
                    (None, Some(path)) => path.clone(),
                    _ => format!("instance {}", f.id),
                };
                let parked = if f.parked { " (failed again unchanged; parked until it changes or Resync)" } else { "" };
                format!("[sync] error: {at}: {}{parked}", f.message)
            }
            Event::Log { realm, kind, text, .. } => match kind {
                LogKind::Output => format!("[{}] {text}", realm.name()),
                _ => format!("[{}] {}: {text}", realm.name(), kind.name()),
            },
            Event::Exec { realm, generation, name, ok: true, ms, .. } => {
                format!("[{}] ran {name} in {ms:.1} ms, generation {generation}", realm.name())
            }
            Event::Exec { realm, generation, name, error, .. } => {
                let error = error.unwrap_or("failed").lines().next().unwrap_or("");
                format!("[{}] {name} failed, generation {generation}: {error}", realm.name())
            }
            Event::Assets { changed } => {
                let (them, files) = if changed.len() == 1 { ("it", "") } else { ("them", "s") };
                format!(
                    "asset{files} changed since upload: {}; `opossum upload` to see {them} in game",
                    changed.join(", ")
                )
            }
            Event::Drawn { files } => {
                let names: Vec<String> = files.iter().map(|f| shown(root, f)).collect();
                let them = if files.len() == 1 { "file now draws" } else { "files now draw" };
                format!("place: {} {them} uploaded content ({})", files.len(), names.join(", "))
            }
            Event::Packs { baked, nudged, unbaked, after } => {
                let mut said = Vec::new();
                if baked > 0 {
                    said.push(format!("{baked} baked by Studio, saved to opossum.lock"));
                }
                if nudged > 0 {
                    said.push(format!("re-asked Studio for {nudged}"));
                }
                if !unbaked.is_empty() {
                    let mut classes: Vec<(&str, usize)> = Vec::new();
                    for (class, _) in unbaked {
                        match classes.iter_mut().find(|(c, _)| c == class) {
                            Some((_, n)) => *n += 1,
                            None => classes.push((class, 1)),
                        }
                    }
                    let classes: Vec<String> = classes.iter().map(|(c, n)| format!("{c} {n}")).collect();
                    said.push(format!("{} still unbaked after {after} s ({})", unbaked.len(), classes.join(", ")));
                }
                format!("packs: {}", said.join("; "))
            }
            Event::Restart { file } => format!("{file} changed, starting over"),
            Event::Error { message, .. } => format!("opossum: {message}"),
        }
    }

    pub fn json(&self, root: &Path) -> Value {
        match *self {
            Event::Serving { project, port } => json!({"t": "serving", "project": project, "port": port}),
            Event::Build { rebuilt, generation, ms, phases, modules, changed, lowered, errors, warnings } => {
                let phases: Map<String, Value> = phases.iter().map(|&(p, t)| (p.to_owned(), json!(t))).collect();
                json!({
                    "t": if rebuilt { "rebuilt" } else { "built" },
                    "gen": generation,
                    "ms": ms,
                    "phases": phases,
                    "modules": modules,
                    "changed": changed,
                    "lowered": lowered,
                    "errors": errors,
                    "warnings": warnings,
                })
            }
            Event::Diag(d) => {
                let mut out = Map::from_iter([("t".to_owned(), json!("diag"))]);
                if let Value::Object(fields) = d.json(root) {
                    out.extend(fields);
                }
                Value::Object(out)
            }
            Event::Connected { realm, place, port } => {
                json!({"t": "connected", "realm": realm.name(), "place": place, "port": port})
            }
            Event::Disconnected { realm, place, port } => {
                json!({"t": "disconnected", "realm": realm.name(), "place": place, "port": port})
            }
            Event::Applied { realm, generation, ms, failed } => {
                json!({"t": "applied", "realm": realm.name(), "gen": generation, "ms": ms, "failed": failed})
            }
            Event::Verify { realm, generation, quiet } => {
                json!({"t": "verify", "realm": realm.name(), "gen": generation, "quiet": quiet})
            }
            Event::Failed(f) => json!({
                "t": "failed",
                "realm": f.realm.name(),
                "gen": f.generation,
                "id": f.id,
                "op": f.op,
                "message": f.message,
                "file": f.file.as_deref().map(|file| shown(root, file)),
                "path": f.path,
                "parked": f.parked,
            }),
            Event::Log { realm, kind, text, at } => json!({
                "t": "log",
                "realm": realm.name(),
                "kind": kind.name(),
                "text": text,
                "file": at.map(|(file, _)| file),
                "line": at.and_then(|(_, line)| line),
            }),
            Event::Exec { realm, generation, name, ok, ms, error } => json!({
                "t": "exec",
                "realm": realm.name(),
                "gen": generation,
                "name": name,
                "ok": ok,
                "ms": ms,
                "error": error,
            }),
            Event::Assets { changed } => json!({"t": "assets", "changed": changed, "run": "opossum upload"}),
            Event::Drawn { files } => {
                let files: Vec<String> = files.iter().map(|f| shown(root, f)).collect();
                json!({"t": "drawn", "files": files})
            }
            Event::Packs { baked, nudged, unbaked, after } => {
                let unbaked: Vec<Value> = unbaked
                    .iter()
                    .map(|(class, files)| {
                        let files: Vec<String> = files.iter().map(|f| shown(root, f)).collect();
                        json!({"class": class, "usedBy": files})
                    })
                    .collect();
                json!({"t": "packs", "baked": baked, "nudged": nudged, "unbaked": unbaked, "after": after})
            }
            Event::Restart { file } => json!({"t": "restart", "file": file}),
            Event::Error { message, fatal } => json!({"t": "error", "message": message, "fatal": fatal}),
        }
    }
}
