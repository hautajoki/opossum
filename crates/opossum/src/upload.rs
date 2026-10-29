//! `opossum upload`: new and changed asset files go up to Roblox and their ids into `opossum.lock`; `opossum adopt
//! asset` settles an upload nothing can tell happened or not. The main thread owns the lock and is its only writer; up
//! to four workers each take one group (every file with one input hash) end to end. Each group's intent is on disk
//! before its request leaves, so a crash or a lost answer never uploads a file twice without saying so.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use cloud::{Client, ErrorKind, OPERATION_DEADLINE, Operation, Upload};
use serde_json::json;

use crate::assets::{self, Asset as File, Compiled};
use crate::config::Config;
use crate::diag::Diag;
use crate::guard::RemoteGuard;
use crate::lock::{self, Asset, Lock};
use crate::plan::{self, State, count};
use crate::{Outcome, canonical_prefix};

/// Groups in flight at once.
const WORKERS: usize = 4;

/// Every file needing Roblox with one input hash: they share one upload and one id.
struct Group {
    hash: String,
    /// Indices into the discovered files.
    members: Vec<usize>,
    job: Job,
}

enum Job {
    /// Upload the compiled file, or put it into the FBX asset `update` in place.
    Send { compiled: Compiled, name: String, update: Option<u64> },
    /// Wait on the operation of an upload an earlier run started.
    Resume(String),
}

/// What a worker says about its group: `Created` once Roblox took the request, then exactly one of the others.
enum Event {
    Created(String),
    /// The request failed. A `definite` error was certainly not applied; any other leaves the upload ambiguous.
    Failed(cloud::Error),
    /// The asset id, and whether moderation is still reviewing it.
    Done(u64, bool),
    /// The operation ended without an id the lock may hold: its code and why.
    Terminal(&'static str, String),
    /// The operation is gone, and with it the only way to tell whether the asset exists.
    Expired,
    /// Still running at the deadline, or asking failed: the next upload asks again.
    Waiting(Option<cloud::Error>),
    /// A stop came before the request left.
    NotSent,
    /// The worker panicked (a bug): the lock says how far it got.
    Panicked,
}

#[derive(Default)]
struct Report {
    uploaded: usize,
    reused: usize,
    unchanged: usize,
    dropped: usize,
    pending: Vec<String>,
    /// File and how to settle it.
    ambiguous: Vec<(String, String)>,
    /// File, error and code.
    failed: Vec<(String, String, String)>,
}

/// An upload worked out and checked with nothing sent: the lock as read, with files bound to the ids it already holds
/// for their content, the files, what classifying them found, and the groups to send, each compiled.
pub struct Prepared {
    lock: Lock,
    found: Vec<File>,
    creator: cloud::Creator,
    report: Report,
    groups: Vec<Group>,
}

/// `opossum upload`: hash every asset file, bind files whose content the lock already has an id for, upload the rest
/// (an FBX whose id no other file holds is updated in place), resume uploads earlier runs left in flight, and drop
/// the records of files that are gone. A file that fails its checks or its upload fails alone. Exit 1 when anything
/// failed, stayed pending or is ambiguous; 2 when nothing was sent (errors in the config, the lock or the roots).
pub fn upload(cfg: &Config, client: &Client, guard: &RemoteGuard, stop: &AtomicBool) -> Outcome {
    match prepare(cfg) {
        Ok(Some(prepared)) => send(cfg, client, guard, prepared, stop),
        Ok(None) => idle(),
        Err(refusal) => refusal,
    }
}

/// An upload with no `[assets]`: nothing to do.
pub fn idle() -> Outcome {
    let text = "nothing to upload: opossum.toml declares no [assets]\n".into();
    Outcome { text, ..outcome(Report::default(), None, false, false) }
}

/// Everything an upload does before its first request: hash and classify every file against the lock, bind content
/// it already has an id for, and group and compile what needs sending. None without `[assets]`; refused when the
/// lock or a root has errors or the lock's assets belong to another creator.
pub fn prepare(cfg: &Config) -> Result<Option<Prepared>, Outcome> {
    let root = &cfg.root;
    let Some(creator) = cfg.creator.filter(|_| !cfg.assets.is_empty()) else { return Ok(None) };
    let (mut lock, found, hashes) = plan::survey(cfg, "uploaded")?;
    if let Some(d) = lock.check_creator(root, creator) {
        return Err(Outcome::refused(&d.code, d.message));
    }
    let mut report = Report::default();

    // classify against the lock as it was, binding content it already has an id for (renames, moves, copies), a
    // merge's every candidate included
    let held: HashMap<String, u64> = lock
        .assets
        .values()
        .chain(lock.merged.values().flatten())
        .filter_map(|a| match a {
            Asset { id: Some(id), hash: Some(hash), next: None, .. } => Some((hash.clone(), *id)),
            _ => None,
        })
        .collect();
    let unsettled = plan::unsettled(&lock);
    let mut groups: Vec<(String, Vec<usize>, Option<String>)> = Vec::new();
    let mut by_hash: HashMap<String, usize> = HashMap::new();
    for (i, (a, hash)) in found.iter().zip(&hashes).enumerate() {
        if let Some(warning) = lock.settle(root, &a.file, hash) {
            eprintln!("{}", warning.human(root));
        }
        let record = lock.assets.get(&a.file).cloned().unwrap_or_default();
        let shares = plan::shares(a);
        let (hash, op) = match plan::state(Some(&record), hash) {
            State::Current => {
                report.unchanged += 1;
                continue;
            }
            State::Ambiguous => {
                report.ambiguous.push((a.file.clone(), plan::settle(&a.file)));
                continue;
            }
            State::Rejected => {
                report.failed.push((a.file.clone(), plan::refused(&a.file), "asset.rejected".into()));
                continue;
            }
            // a later change waits for the next run: this one finishes what was started
            State::Pending => (record.next.clone().unwrap_or_default(), record.op.clone()),
            State::New | State::Changed if shares && unsettled.contains_key(hash) => {
                report.ambiguous.push((a.file.clone(), plan::copy_of(&unsettled[hash])));
                continue;
            }
            State::New | State::Changed => match held.get(hash).filter(|_| shares) {
                Some(&id) => {
                    let bound = Asset { id: Some(id), hash: Some(hash.clone()), rejected: None, ..record };
                    lock.assets.insert(a.file.clone(), bound);
                    report.reused += 1;
                    continue;
                }
                None => (hash.clone(), None),
            },
        };
        // files waiting on an upload of this content join it instead of sending it again
        let joined = by_hash.get(&hash).copied().filter(|_| shares);
        let g = joined.unwrap_or_else(|| {
            groups.push((hash.clone(), Vec::new(), None));
            if shares {
                by_hash.insert(hash, groups.len() - 1);
            }
            groups.len() - 1
        });
        groups[g].1.push(i);
        if groups[g].2.is_none() {
            groups[g].2 = op;
        }
    }

    // compile one file per group, before anything is sent
    let mut ready = Vec::new();
    for (hash, members, op) in groups {
        let job = match op {
            Some(op) => Job::Resume(op),
            None => match compile(cfg, &found[members[0]], &hash) {
                Ok(compiled) => {
                    let a = &found[members[0]];
                    let name = a.key.rsplit('/').next().unwrap_or(&a.key).to_owned();
                    let update = in_place(&lock, a, &compiled, members.len());
                    Job::Send { compiled, name, update }
                }
                Err(d) => {
                    for &m in &members {
                        report.failed.push((found[m].file.clone(), d.message.clone(), d.code.clone()));
                    }
                    continue;
                }
            },
        };
        ready.push(Group { hash, members, job });
    }
    if lock.creator.is_none() && (report.reused > 0 || !ready.is_empty()) {
        lock.creator = Some(creator);
    }
    Ok(Some(Prepared { lock, found, creator, report, groups: ready }))
}

impl Prepared {
    /// What would stop a deploy after it sent something: a file that failed its checks, or an upload that may or may
    /// not have happened. Deploy refuses on it first, each file printed on stderr.
    pub fn refusal(&self) -> Option<Outcome> {
        let r = &self.report;
        for (file, error, _) in &r.failed {
            eprintln!("{file}: {error}");
        }
        for (file, how) in &r.ambiguous {
            eprintln!("{file}: {how}");
        }
        if let Some((_, _, code)) = r.failed.first() {
            let error = format!("{} asset files fail their checks, so nothing was sent", count(r.failed.len()));
            return Some(Outcome::refused(code, error));
        }
        let n = r.ambiguous.len();
        (n > 0).then(|| Outcome::refused("asset.ambiguous", plan::ambiguous(n)))
    }
}

/// Sends a prepared upload, resuming what earlier runs left in flight, and drops the records of files that are gone.
pub fn send(cfg: &Config, client: &Client, guard: &RemoteGuard, prepared: Prepared, stop: &AtomicBool) -> Outcome {
    let root = &cfg.root;
    let Prepared { mut lock, found, creator, mut report, groups: ready } = prepared;
    let mut broken = None;
    let mut auth = false;
    let (tx, rx) = mpsc::channel();
    let mut sent = 0;
    let ready = &ready;
    std::thread::scope(|s| {
        let mut flight = 0;
        loop {
            while flight < WORKERS && sent < ready.len() && !(auth || broken.is_some() || stop.load(Ordering::SeqCst)) {
                let g = &ready[sent];
                intent(&mut lock, g, &found);
                if !save(&mut lock, root, guard, &mut broken) {
                    // the intent is not durable, so the request never leaves
                    if let Job::Send { .. } = g.job {
                        for &m in &g.members {
                            forget(&mut lock, &found[m].file, true);
                        }
                    }
                    break;
                }
                progress(g, &found);
                let (tx, i) = (tx.clone(), sent);
                let audio = matches!(found[g.members[0]].ext.as_str(), ".ogg" | ".mp3" | ".wav" | ".flac");
                s.spawn(move || {
                    let say = |e| drop(tx.send((i, e)));
                    // a worker that panics still answers, or the run would wait on it forever holding the guard
                    let worked = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        work(client, creator, &ready[i], audio, stop, &say)
                    }));
                    if worked.is_err() {
                        say(Event::Panicked);
                    }
                });
                sent += 1;
                flight += 1;
            }
            if flight == 0 {
                break;
            }
            let (i, event) = rx.recv().expect("a worker hung up");
            flight -= usize::from(!matches!(event, Event::Created(_)));
            auth |= apply(&mut lock, &ready[i], &found, event, &mut report);
            save(&mut lock, root, guard, &mut broken);
        }
    });
    let stopped = stop.load(Ordering::SeqCst);
    let (code, why) = if broken.is_some() {
        ("lock.changedOnDisk", "opossum.lock could not be saved")
    } else if auth {
        ("cloud.auth", "Roblox refused the API key")
    } else {
        ("upload.stopped", "opossum was stopped")
    };
    for g in &ready[sent..] {
        for &m in &g.members {
            let file = found[m].file.clone();
            match g.job {
                Job::Send { .. } => report.failed.push((file, format!("not sent: {why}"), code.into())),
                Job::Resume(_) => report.pending.push(file),
            }
        }
    }

    // discovery was clean (survey refuses otherwise), so a record with no file is a file that is gone
    let files: std::collections::HashSet<&str> = found.iter().map(|a| a.file.as_str()).collect();
    let before = lock.assets.len();
    lock.assets.retain(|file, _| files.contains(file.as_str()));
    lock.merged.retain(|file, _| files.contains(file.as_str()));
    report.dropped = before - lock.assets.len();
    save(&mut lock, root, guard, &mut broken);
    outcome(report, broken, auth, stopped)
}

/// The bytes a file's upload sends, read again and checked against the hash its group was formed by.
fn compile(cfg: &Config, a: &File, hash: &str) -> Result<Compiled, Diag> {
    let path = cfg.root.join(&a.file);
    let image = cfg.assets[a.set].image;
    let fail = |message: String| Diag::new("asset.root", crate::diag::Severity::Error, Some(&path), message);
    let bytes = std::fs::read(&path).map_err(|e| fail(e.to_string()))?;
    if assets::hash(&a.ext, image, &bytes) != hash {
        return Err(fail("the file changed while opossum read it; upload again".into()));
    }
    assets::compile(&path, &a.ext, image, bytes)
}

/// The id a changed FBX model replaces in place: Roblox lets only FBX content change. Only when no other file in this
/// lock holds its id (one migrated from tessera may), so one file's change never silently changes another's
/// (copy-on-write: a shared id splits off a new asset instead); an FBX never comes to share one ([`plan::shares`]).
fn in_place(lock: &Lock, a: &File, compiled: &Compiled, members: usize) -> Option<u64> {
    if a.ext != ".fbx" || compiled.kind != cloud::AssetKind::Model || members != 1 {
        return None;
    }
    let id = lock.assets.get(&a.file)?.id?;
    let shared = lock.assets.iter().any(|(file, r)| *file != a.file && r.id == Some(id));
    (!shared).then_some(id)
}

/// Records a group's intent on every member before its request: `next` for a send, `next` and the operation for
/// files joining a resumed one.
fn intent(lock: &mut Lock, g: &Group, found: &[File]) {
    for &m in &g.members {
        let r = lock.assets.entry(found[m].file.clone()).or_default();
        r.next = Some(g.hash.clone());
        r.op = match &g.job {
            Job::Send { .. } => None,
            Job::Resume(op) => Some(op.clone()),
        };
    }
}

/// Forgets a file's upload in flight: its operation, and with `next` the intent too. A record left holding nothing
/// goes.
fn forget(lock: &mut Lock, file: &str, next: bool) {
    let Some(r) = lock.assets.get_mut(file) else { return };
    r.op = None;
    if next {
        r.next = None;
    }
    if r.id.is_none() && r.next.is_none() && r.rejected.is_none() {
        lock.assets.remove(file);
    }
}

/// Saves the lock; after the first failure (git changed it under us) nothing new is sent, and every later save only
/// rewrites the recovered copy, so it ends holding the final state.
fn save(lock: &mut Lock, root: &Path, guard: &RemoteGuard, broken: &mut Option<Diag>) -> bool {
    match lock::save(lock, root, guard) {
        Ok(note) => {
            if let Some(note) = note {
                eprintln!("{note}");
            }
            true
        }
        Err(d) => {
            if broken.is_none() {
                eprintln!("{}", d.human(root));
            }
            *broken = Some(d);
            false
        }
    }
}

fn progress(g: &Group, found: &[File]) {
    let file = &found[g.members[0]].file;
    let verb = match g.job {
        Job::Send { update: Some(id), .. } => format!("updating {id} with"),
        Job::Send { .. } => "uploading".into(),
        Job::Resume(_) => "waiting on the upload of".into(),
    };
    let more = match g.members.len() {
        1 => String::new(),
        n => format!(" (and {} identical)", n - 1),
    };
    eprintln!("{verb} {file}{more}…");
}

/// One group end to end on a worker thread: send it (unless a stop came first), then wait on its operation.
fn work(client: &Client, creator: cloud::Creator, g: &Group, audio: bool, stop: &AtomicBool, send: &dyn Fn(Event)) {
    let op = match &g.job {
        Job::Resume(op) => op.clone(),
        Job::Send { compiled, name, update } => {
            if stop.load(Ordering::SeqCst) {
                return send(Event::NotSent);
            }
            let upload = Upload {
                kind: compiled.kind,
                name,
                creator,
                ext: compiled.ext,
                mime: compiled.mime,
                bytes: &compiled.bytes,
            };
            let sent = match update {
                Some(id) => client.update_asset(*id, &upload),
                None => client.create_asset(&upload),
            };
            match sent {
                Err(e) => return send(Event::Failed(e)),
                Ok(op) => {
                    send(Event::Created(op.path.clone()));
                    if op.done {
                        return send(finished(op, audio));
                    }
                    op.path
                }
            }
        }
    };
    send(match client.wait_operation(&op, client.now() + OPERATION_DEADLINE, stop) {
        Ok(op) if op.done => finished(op, audio),
        Ok(_) => Event::Waiting(None),
        Err(e) if e.kind == ErrorKind::Status(404) => Event::Expired,
        Err(e) => Event::Waiting(Some(e)),
    })
}

/// A done operation: an id to bind, unless Roblox failed it or its moderation rejected it (a rejected id must never
/// spread through the lock).
fn finished(op: Operation, audio: bool) -> Event {
    let (rejected, reviewing) = (op.rejected(), op.reviewing());
    match (op.error, op.asset_id) {
        (Some((code, message)), _) => {
            let why =
                if audio { "Roblox rejected the audio during transcoding or moderation" } else { "Roblox failed it" };
            Event::Terminal("cloud.operation", format!("{why} ({code}: {message})"))
        }
        _ if rejected => {
            Event::Terminal("asset.moderated", "Roblox's moderation rejected it, so its id is not used".into())
        }
        (None, Some(id)) if id > 0 => Event::Done(id, reviewing),
        _ => Event::Terminal("cloud.operation", "the upload completed without an asset id".into()),
    }
}

/// Records what a worker said on every member of its group. True when Roblox refused the key: nothing new is sent.
fn apply(lock: &mut Lock, g: &Group, found: &[File], event: Event, report: &mut Report) -> bool {
    let files = || g.members.iter().map(|&m| found[m].file.as_str());
    match event {
        Event::Created(op) => {
            for file in files() {
                lock.assets.entry(file.to_owned()).or_default().op = Some(op.clone());
            }
        }
        Event::Failed(e) if e.definite() => {
            for file in files() {
                forget(lock, file, true);
                report.failed.push((file.into(), e.to_string(), e.diag().into()));
            }
            return e.diag() == "cloud.auth";
        }
        Event::Failed(e) => {
            for file in files() {
                lock.assets.entry(file.to_owned()).or_default().op = None;
                report.ambiguous.push((file.into(), format!("{e}; {}", plan::settle(file))));
            }
        }
        Event::Done(id, reviewing) => {
            for file in files() {
                let bound = Asset { id: Some(id), hash: Some(g.hash.clone()), ..Asset::default() };
                lock.assets.insert(file.into(), bound);
            }
            report.uploaded += g.members.len();
            if reviewing {
                eprintln!("{} is {id}, still in Roblox's moderation review", found[g.members[0]].file);
            }
        }
        Event::Terminal(code, message) => {
            // the same bytes would be refused again (and each moderation hit counts against the account)
            for file in files() {
                forget(lock, file, true);
                lock.assets.entry(file.to_owned()).or_default().rejected = Some(g.hash.clone());
                let message = format!("{message}; not sent again until the file changes");
                report.failed.push((file.into(), message, code.into()));
            }
        }
        Event::Expired => {
            let op = lock.assets.get(&found[g.members[0]].file).and_then(|r| r.op.clone()).unwrap_or_default();
            for file in files() {
                forget(lock, file, false);
                report.ambiguous.push((file.into(), format!("operation {op} expired; {}", plan::settle(file))));
            }
        }
        Event::Waiting(e) => {
            report.pending.extend(files().map(String::from));
            if let Some(e) = e {
                eprintln!("{e}");
                return e.diag() == "cloud.auth";
            }
        }
        Event::Panicked => {
            for file in files() {
                match lock.assets.get(file).is_some_and(|r| r.op.is_some()) {
                    true => report.pending.push(file.into()),
                    false => report
                        .ambiguous
                        .push((file.into(), format!("opossum failed mid-upload; {}", plan::settle(file)))),
                }
            }
        }
        Event::NotSent => {
            for file in files() {
                forget(lock, file, true);
                report.failed.push((file.into(), "not sent: opossum was stopped".into(), "upload.stopped".into()));
            }
        }
    }
    false
}

fn outcome(r: Report, broken: Option<Diag>, auth: bool, stopped: bool) -> Outcome {
    let mut text = format!(
        "uploaded {}, reused {}, unchanged {}, dropped {}\n",
        count(r.uploaded),
        count(r.reused),
        count(r.unchanged),
        count(r.dropped)
    );
    for (file, error, _) in &r.failed {
        let _ = writeln!(text, "failed {file}: {error}");
    }
    for file in &r.pending {
        let _ = writeln!(text, "pending {file} (`opossum upload` asks again)");
    }
    for (file, how) in &r.ambiguous {
        let _ = writeln!(text, "ambiguous {file}: {how}");
    }
    let failed: Vec<_> =
        r.failed.iter().map(|(file, error, code)| json!({ "file": file, "error": error, "code": code })).collect();
    let json = json!({
        "ok": true, "uploaded": r.uploaded, "reused": r.reused, "unchanged": r.unchanged, "dropped": r.dropped,
        "pending": r.pending, "ambiguous": r.ambiguous.iter().map(|(file, _)| file).collect::<Vec<_>>(),
        "failed": failed,
    });
    // the one reason an agent branches on: what stopped the run, else the first kind of trouble a file is in
    let (code, error) = if let Some(d) = broken {
        (d.code, d.message)
    } else if stopped {
        ("upload.stopped".into(), "opossum was stopped; what it sent is recorded".into())
    } else if auth {
        ("cloud.auth".into(), "Roblox refused the API key, so nothing more was sent".into())
    } else if let Some((_, _, code)) = r.failed.first() {
        (code.clone(), format!("{} failed", plan::counted(r.failed.len(), "file")))
    } else if !r.ambiguous.is_empty() {
        ("asset.ambiguous".into(), plan::ambiguous(r.ambiguous.len()))
    } else if !r.pending.is_empty() {
        ("asset.pending".into(), "uploads are still running; `opossum upload` asks again".into())
    } else {
        return Outcome { exit: 0, json, text };
    };
    Outcome::failed(json, text, code, error)
}

/// `opossum adopt asset <file> (<id> | --none)`: settles an upload whose create may or may not have happened. With an
/// id, the file is bound to it as holding the content the upload sent; with `--none` (`id` None) the upload, or
/// Roblox's refusal of the content, is forgotten, so the next `opossum upload` sends it again. An upload with an
/// operation to ask is `opossum upload`'s to resume, unless it is forgotten.
pub fn adopt_asset(cfg: &Config, guard: &RemoteGuard, cwd: &Path, file: &str, id: Option<u64>) -> Outcome {
    let root = &cfg.root;
    let key = match canonical_prefix(&cwd.join(file)).map(|p| p.strip_prefix(root).map(Path::to_path_buf)) {
        Ok(Ok(rel)) => rel.to_string_lossy().replace('\\', "/"),
        _ => file.to_owned(),
    };
    let (mut lock, diags) = match lock::read(root) {
        Ok(read) => read,
        Err(error) => return Outcome::refused("lock.io", error),
    };
    if let Some(d) = diags.iter().find(|d| d.is_error()) {
        return Outcome::refused(&d.code, d.human(root));
    }
    let refuse = |error: String| Outcome::refused("asset.adopt", error);
    // after a merge, the record with the upload to settle, which a build's pick (the highest id) hides
    let merged = lock.merged.get(&key).into_iter().flatten();
    let in_flight =
        merged.clone().find(|a| a.next.is_some() && a.op.is_none()).or(merged.clone().find(|a| a.next.is_some()));
    let Some(record) = in_flight.or(lock.assets.get(&key)).cloned() else {
        return refuse(format!("{key} has no record in opossum.lock"));
    };
    lock.assets.insert(key.clone(), record.clone());
    let text = match (id, record.next, &record.op) {
        (Some(_), Some(_), Some(op)) => {
            return refuse(format!("{key}'s upload has an operation ({op}) to ask; `opossum upload` resumes it"));
        }
        (Some(id), Some(next), None) => {
            lock.assets.insert(key.clone(), Asset { id: Some(id), hash: Some(next), ..Asset::default() });
            format!("adopted {key} as {id}\n")
        }
        (None, next, _) if next.is_some() || record.rejected.is_some() => {
            if let Some(r) = lock.assets.get_mut(&key) {
                r.rejected = None;
            }
            forget(&mut lock, &key, true);
            let what = if next.is_some() { "the upload" } else { "Roblox's refusal" };
            format!("forgot {what} of {key}; `opossum upload` sends it again\n")
        }
        _ => return refuse(format!("{key} has no upload in flight, so there is nothing to adopt")),
    };
    lock.merged.remove(&key);
    if let Err(d) = lock::save(&mut lock, root, guard) {
        return Outcome::refused(&d.code, d.message);
    }
    let json = match id {
        Some(id) => json!({ "ok": true, "asset": key, "id": id }),
        None => json!({ "ok": true, "asset": key }),
    };
    Outcome { exit: 0, json, text }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Barrier};

    use cloud::mock::{Mock, Reply, auto};
    use cloud::{Clock, Fake};
    use media::ImageOptions;
    use serde_json::json;

    use super::*;
    use crate::guard::Cache;

    /// A project with one asset set `art` and these files under it.
    fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("opossum-upload-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("art")).unwrap();
        let toml = "name = \"u\"\ncreator = { user = 1 }\n[routes]\nshared = \"ReplicatedStorage/shared\"\n[assets]\nart = \"art\"\n";
        std::fs::write(root.join("opossum.toml"), toml).unwrap();
        for (file, text) in files {
            std::fs::write(root.join("art").join(file), text).unwrap();
        }
        std::fs::canonicalize(root).unwrap()
    }

    fn run(root: &Path, mock: &Mock, stop: &AtomicBool) -> Outcome {
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let client = Client::new("test-key", &mock.base, Clock::Fake(Fake::new())).unwrap();
        let guard = RemoteGuard::acquire(&Cache { dir: root.join(".cache") }).unwrap();
        upload(&cfg, &client, &guard, stop)
    }

    fn stop_flag() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn adopt(root: &Path, file: &str, id: Option<u64>) -> Outcome {
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let guard = RemoteGuard::acquire(&Cache { dir: root.join(".cache") }).unwrap();
        adopt_asset(&cfg, &guard, root, file, id)
    }

    fn sent(mock: &Mock, method: &str, prefix: &str) -> usize {
        mock.requests().iter().filter(|r| r.method == method && r.path.starts_with(prefix)).count()
    }

    fn record(root: &Path, file: &str) -> Option<Asset> {
        lock::read(root).unwrap().0.assets.get(file).cloned()
    }

    fn hash(text: &str) -> String {
        assets::hash(".glb", ImageOptions::DEFAULT, text.as_bytes())
    }

    /// A done operation `operations/<op>` holding asset `id`.
    fn done(op: u64, id: u64) -> Reply {
        Reply::json(200, &json!({ "path": format!("operations/{op}"), "done": true, "response": { "assetId": id } }))
    }

    #[test]
    fn identical_files_share_one_upload_whose_intent_is_on_disk_first_and_a_rename_sends_nothing() {
        let root = project("share", &[("a.glb", "x"), ("b.glb", "x")]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let lock = root.join(lock::FILE);
        mock.on(
            "POST",
            "assets/v1/assets",
            Reply::Run(Box::new(move |_| {
                let text = std::fs::read_to_string(&lock).unwrap();
                assert_eq!(text.matches(&format!("next = \"{}\"", hash("x"))).count(), 2, "{text}");
                Reply::json(200, &json!({ "path": "operations/1", "done": false }))
            })),
        );
        mock.on("GET", "assets/v1/operations/1", done(1, 77));
        let stop = AtomicBool::new(false);
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["uploaded"]), (0, &json!(2)), "{}", out.json);
        assert_eq!((sent(&mock, "POST", ""), sent(&mock, "GET", "")), (1, 1));
        let bound = Asset { id: Some(77), hash: Some(hash("x")), ..Asset::default() };
        assert_eq!((record(&root, "art/a.glb"), record(&root, "art/b.glb")), (Some(bound.clone()), Some(bound)));

        std::fs::rename(root.join("art/a.glb"), root.join("art/c.glb")).unwrap();
        let out = run(&root, &mock, &stop);
        let counts = ["uploaded", "reused", "unchanged", "dropped"].map(|k| out.json[k].as_u64().unwrap());
        assert_eq!((out.exit, counts, mock.requests().len()), (0, [0, 1, 1, 1], 2));
        assert_eq!(record(&root, "art/c.glb").and_then(|r| r.id), Some(77));
        assert_eq!(record(&root, "art/a.glb"), None);

        // a discovery error sends nothing and keeps every record, even of files that look gone
        std::fs::rename(root.join("art/c.glb"), root.join("art/c.txt")).unwrap();
        let before = std::fs::read_to_string(root.join(lock::FILE)).unwrap();
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["code"], mock.requests().len()), (2, &json!("asset.unsupported"), 2));
        assert_eq!(std::fs::read_to_string(root.join(lock::FILE)).unwrap(), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_create_clears_its_intent_and_a_lost_answer_keeps_it_until_adopted() {
        let root = project("lost", &[("a.glb", "x")]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let stop = AtomicBool::new(false);
        mock.on("POST", "assets/v1/assets", Reply::json(400, &json!({ "code": "INVALID_ARGUMENT" })));
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["failed"][0]["code"]), (1, &json!("cloud.http")));
        assert_eq!(record(&root, "art/a.glb"), None);

        mock.on("POST", "assets/v1/assets", Reply::Reset);
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["ambiguous"]), (1, &json!(["art/a.glb"])));
        let lost = Asset { next: Some(hash("x")), ..Asset::default() };
        assert_eq!(record(&root, "art/a.glb"), Some(lost));
        // nothing can say whether it happened, so nothing is sent until the user does, a copy of it included
        std::fs::write(root.join("art/b.glb"), "x").unwrap();
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["code"], sent(&mock, "POST", "")), (1, &json!("asset.ambiguous"), 2));
        assert_eq!(out.json["ambiguous"], json!(["art/a.glb", "art/b.glb"]));

        assert_eq!(adopt(&root, "art/a.glb", None).exit, 0);
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["uploaded"], sent(&mock, "POST", "")), (0, &json!(2), 3));
        assert_eq!(adopt(&root, "art/a.glb", Some(5)).json["code"], "asset.adopt");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A merge's ambiguous create hides behind the other branch's id until something picks it; adopt finds it anyway.
    #[test]
    fn adopt_settles_an_ambiguous_create_a_merge_left_beside_an_older_id() {
        let root = project("adopt-merged", &[("a.glb", "y")]);
        let (x, y) = (hash("x"), hash("y"));
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/a.glb\" = {{ id = 5, hash = \"{x}\" }}\n\
             assets.\"art/a.glb\" = {{ next = \"{y}\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        assert_eq!(adopt(&root, "art/a.glb", Some(77)).exit, 0);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["unchanged"], mock.requests().len()), (0, &json!(1), 0), "{}", out.json);
        assert_eq!(record(&root, "art/a.glb").and_then(|r| r.id), Some(77));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_crash_after_the_create_resumes_its_operation_and_identical_files_join_it() {
        let root = project("resume", &[("a.glb", "x"), ("b.glb", "x"), ("c.glb", "x")]);
        let h = hash("x");
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/a.glb\" = {{ next = \"{h}\", op = \"operations/7\" }}\n\
             assets.\"art/b.glb\" = {{ next = \"{h}\", op = \"operations/7\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        let out = adopt(&root, "art/a.glb", Some(5));
        assert!(out.json["error"].as_str().unwrap().contains("`opossum upload` resumes it"), "{}", out.json);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("GET", "assets/v1/operations/7", done(7, 55));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["uploaded"]), (0, &json!(3)), "{}", out.json);
        assert_eq!((sent(&mock, "POST", ""), sent(&mock, "GET", "")), (0, 1));
        for file in ["art/a.glb", "art/b.glb", "art/c.glb"] {
            assert_eq!(record(&root, file).and_then(|r| r.id), Some(55));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Folding `.opossum/opossum.lock.recovered` back (or a union merge) leaves the old record beside the one whose
    /// create of the file's current content is in flight: that create is resumed, never sent again.
    #[test]
    fn a_folded_back_upload_in_flight_wins_over_the_older_id_and_is_resumed() {
        let root = project("fold", &[("a.glb", "y")]);
        let (x, y) = (hash("x"), hash("y"));
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/a.glb\" = {{ id = 5, hash = \"{x}\" }}\n\
             assets.\"art/a.glb\" = {{ id = 5, hash = \"{x}\", next = \"{y}\", op = \"operations/7\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("GET", "assets/v1/operations/7", done(7, 55));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", ""), sent(&mock, "GET", "")), (0, 0, 1), "{}", out.json);
        assert_eq!(record(&root, "art/a.glb"), Some(Asset { id: Some(55), hash: Some(y), ..Asset::default() }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn four_uploads_run_at_once_and_a_file_that_fails_its_checks_fails_alone() {
        let files: Vec<(String, String)> = (0..6).map(|i| (format!("{i}.glb"), i.to_string())).collect();
        let mut files: Vec<(&str, &str)> = files.iter().map(|(f, t)| (f.as_str(), t.as_str())).collect();
        files.push(("empty.glb", ""));
        let root = project("flight", &files);
        let mock = Mock::start();
        // the first four answer only once four are in flight; a fifth at once would show in the peak
        let four = Arc::new(Barrier::new(4));
        for i in 0..6 {
            let four = four.clone();
            let reply = move |_: &cloud::mock::Request| {
                if i < 4 {
                    four.wait();
                }
                done(i, 100 + i)
            };
            mock.on("POST", "assets/v1/assets", Reply::Run(Box::new(reply)));
        }
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["uploaded"], mock.peak()), (1, &json!(6), 4), "{}", out.json);
        assert_eq!(
            out.json["failed"],
            json!([{ "file": "art/empty.glb", "code": "asset.tooLarge",
            "error": "the file is empty; Roblox takes no empty asset" }])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_key_or_a_stop_sends_nothing_new_and_finishes_what_is_in_flight() {
        let files: Vec<(String, String)> = (0..6).map(|i| (format!("{i}.glb"), i.to_string())).collect();
        let files: Vec<(&str, &str)> = files.iter().map(|(f, t)| (f.as_str(), t.as_str())).collect();
        let root = project("halt", &files);
        let refuse: cloud::mock::Fallback = Box::new(|_| Reply::json(401, &json!({ "message": "bad key" })));
        let mock = Mock::bind(0, Some(refuse)).unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["code"], sent(&mock, "POST", "")), (1, &json!("cloud.auth"), 4));
        let failed = out.json["failed"].as_array().unwrap();
        assert_eq!(failed.iter().filter(|f| f["error"].as_str().unwrap().starts_with("not sent")).count(), 2);
        assert!(lock::read(&root).unwrap().0.assets.is_empty());

        // ctrl-c while four uploads are out: they finish and are recorded, the other two never leave
        let stop = Arc::new(AtomicBool::new(false));
        let all = Arc::new(Barrier::new(4));
        let mock = Mock::start();
        for i in 0..4 {
            let (stop, all) = (stop.clone(), all.clone());
            mock.on(
                "POST",
                "assets/v1/assets",
                Reply::Run(Box::new(move |_| {
                    all.wait();
                    stop.store(true, Ordering::SeqCst);
                    done(i, 200 + i)
                })),
            );
        }
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["code"], &out.json["uploaded"]), (1, &json!("upload.stopped"), &json!(4)));
        let (lock, _) = lock::read(&root).unwrap();
        assert_eq!(lock.assets.values().filter(|a| a.id.is_some() && a.next.is_none()).count(), 4);
        assert_eq!(lock.assets.len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lock_git_rewrote_mid_upload_is_recovered_instead_of_overwritten() {
        let root = project("git", &[("a.glb", "x")]);
        let lock = root.join(lock::FILE);
        let mock = Mock::start();
        mock.on(
            "POST",
            "assets/v1/assets",
            Reply::Run(Box::new(move |_| {
                std::fs::write(&lock, "version = 1\n").unwrap();
                done(1, 9)
            })),
        );
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["code"]), (1, &json!("lock.changedOnDisk")));
        assert_eq!(std::fs::read_to_string(root.join(lock::FILE)).unwrap(), "version = 1\n");
        let recovered = std::fs::read_to_string(root.join(".opossum/opossum.lock.recovered")).unwrap();
        assert!(recovered.contains(&format!("\"art/a.glb\" = {{ id = 9, hash = \"{}\" }}", hash("x"))), "{recovered}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_changed_fbx_is_updated_in_place_unless_another_file_holds_its_id() {
        let root = project("fbx", &[("model.fbx", "new"), ("alias.fbx", "old")]);
        let old = assets::hash(".fbx", ImageOptions::DEFAULT, b"old");
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/alias.fbx\" = {{ id = 9001, hash = \"{old}\" }}\n\
             assets.\"art/model.fbx\" = {{ id = 9001, hash = \"{old}\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", "assets/v1/assets"), sent(&mock, "PATCH", "")), (0, 1, 0));
        let split = record(&root, "art/model.fbx").and_then(|r| r.id).unwrap();
        assert_ne!(split, 9001);
        assert_eq!(record(&root, "art/alias.fbx").and_then(|r| r.id), Some(9001));

        std::fs::write(root.join("art/model.fbx"), "newer").unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", "assets/v1/assets"), sent(&mock, "PATCH", "")), (0, 1, 1));
        assert_eq!(
            mock.requests().iter().find(|r| r.method == "PATCH").unwrap().path,
            format!("assets/v1/assets/{split}")
        );
        let newer = assets::hash(".fbx", ImageOptions::DEFAULT, b"newer");
        assert_eq!(
            record(&root, "art/model.fbx"),
            Some(Asset { id: Some(split), hash: Some(newer), ..Asset::default() })
        );

        // a copy gets an asset of its own: on another branch, the next in-place update would change it unseen
        std::fs::write(root.join("art/copy.fbx"), "newer").unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", "assets/v1/assets")), (0, 2));
        assert!(record(&root, "art/copy.fbx").and_then(|r| r.id).is_some_and(|id| id != split));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every moderation hit counts against the account, so refused bytes stay unsent until the user says otherwise.
    #[test]
    fn content_roblox_refused_is_not_sent_again_until_it_changes_or_the_refusal_is_forgotten() {
        let root = project("rejected", &[("a.glb", "x")]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let rejected = json!({ "path": "operations/1", "done": true,
                               "response": { "assetId": 5, "moderationResult": { "moderationState": "Rejected" } } });
        mock.on("POST", "assets/v1/assets", Reply::json(200, &rejected));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["failed"][0]["code"]), (1, &json!("asset.moderated")));
        assert_eq!(record(&root, "art/a.glb"), Some(Asset { rejected: Some(hash("x")), ..Asset::default() }));

        let out = run(&root, &mock, &stop_flag());
        assert_eq!(
            (out.exit, &out.json["failed"][0]["code"], sent(&mock, "POST", "")),
            (1, &json!("asset.rejected"), 1)
        );
        assert_eq!(adopt(&root, "art/a.glb", None).exit, 0);
        assert_eq!(record(&root, "art/a.glb"), None);
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", "")), (0, 2));
        let _ = std::fs::remove_dir_all(&root);
    }
}
