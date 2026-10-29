//! `opossum upload`: new and changed asset files go up to Roblox and their ids into `opossum.lock`, and each glb's
//! upload is read back for the parts its import made; `opossum adopt asset` settles an upload nothing can tell happened
//! or not. The main thread owns the lock and is its only writer; up to four workers each take one group (every file
//! with one input hash, or every file a read serves) end to end. Each group's intent is on disk before its request
//! leaves, so a crash or a lost answer never uploads a file twice without saying so.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use cloud::{Client, ErrorKind, OPERATION_DEADLINE, Operation, Upload};
use serde_json::json;

use crate::assets::{Asset as File, Compiled};
use crate::config::Config;
use crate::diag::Diag;
use crate::guard::RemoteGuard;
use crate::lock::{self, Asset, Lock};
use crate::plan::{self, Class, count};
use crate::{Outcome, canonical_prefix};

/// Groups in flight at once.
const WORKERS: usize = 4;

/// Why a run sends nothing new after a worker's answer: Roblox refused the key, or an answer was lost (a dropped
/// network fails every later create the same way, and each would be one more upload to settle by hand).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Halt {
    Auth,
    Lost,
}

/// Every file needing Roblox with one input hash: they share one upload and one id.
struct Group {
    hash: String,
    /// Indices into the discovered files.
    members: Vec<usize>,
    job: Job,
}

enum Job {
    /// Upload the compiled file, named after its set key and described by its file.
    Send { compiled: Compiled, key: String, file: String },
    /// Wait on the operation of an upload an earlier run started.
    Resume(String),
    /// Read back the parts of a glb's upload no run has read yet. It sends nothing, so it has no intent.
    Read(u64),
}

/// What a worker says about its group: `Created` once Roblox took the request, then exactly one of the others. A glb's
/// `Done` is followed by `Read` or `ReadFailed`, which a `Job::Read` says alone.
enum Event {
    Created(String),
    /// The request failed. A `definite` error was certainly not applied; any other leaves the upload ambiguous.
    Failed(cloud::Error),
    /// The asset id, and whether moderation is still reviewing it.
    Done(u64, bool),
    /// What the import of upload `id` made, by part name.
    Read(u64, BTreeMap<String, lock::Part>),
    /// The upload stays unread: the code, why, and whether Roblox refused the key's reads, so no later one is tried.
    ReadFailed(&'static str, String, bool),
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
    /// Files whose upload's parts were read back.
    read: Vec<String>,
    /// Each uploaded file's new id.
    ids: Vec<(String, u64)>,
    reused: usize,
    unchanged: usize,
    dropped: usize,
    /// File, why and code: an operation still running, or an upload not read back.
    pending: Vec<(String, String, String)>,
    /// Roblox refused the key's reads of uploads; creates went on.
    refused_reads: bool,
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

/// `opossum upload`: hash every asset file, bind files whose content the lock already has an id for, upload the rest,
/// resume uploads earlier runs left in flight, and drop the records of files that are gone. A file that fails its
/// checks or its upload fails alone. Exit 1 when anything failed, stayed pending or is ambiguous; 2 when nothing was
/// sent (errors in the config, the lock or the roots).
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
    Outcome { text, ..outcome(Report::default(), None, None, false) }
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

    let mut classes = plan::classify(root, &mut lock, &found, &hashes);
    let mut compiled = plan::check(cfg, &found, &hashes, &mut classes);
    // a group's job, None for a send (compiled below)
    let mut groups: Vec<(String, Vec<usize>, Option<Job>)> = Vec::new();
    let mut group_of = HashMap::new();
    // the group reading each id, which serves every file bound to it
    let mut reads: HashMap<u64, usize> = HashMap::new();
    for (i, (a, class)) in found.iter().zip(&classes).enumerate() {
        let mut unread = None;
        match class {
            Class::Current => report.unchanged += 1,
            Class::Ambiguous(how) => report.ambiguous.push((a.file.clone(), how.clone())),
            Class::Rejected => {
                // the refusal follows the content to this file, so it outlives the record of the file that sent it
                lock.assets.entry(a.file.clone()).or_default().rejected = Some(hashes[i].clone());
                report.failed.push((a.file.clone(), plan::refused(&a.file), "asset.rejected".into()));
            }
            Class::Invalid(code, message) => report.failed.push((a.file.clone(), message.clone(), code.clone())),
            Class::Reuses(id) => {
                // what the id's import made is a fact of the id, so it comes along with it
                let all = lock.assets.values().chain(lock.merged.values().flatten());
                let parts = all.filter(|r| r.id == Some(*id)).find_map(|r| r.parts.clone());
                if parts.is_none() && place::is_glb(&a.file) {
                    unread = Some(*id);
                }
                let record = lock.assets.entry(a.file.clone()).or_default();
                (record.id, record.hash, record.rejected, record.parts) =
                    (Some(*id), Some(hashes[i].clone()), None, parts);
                report.reused += 1;
            }
            // a later change waits for the next run: this one finishes what was started
            Class::Pending => {
                let record = &lock.assets[&a.file];
                match (&record.op, &record.next) {
                    (Some(op), Some(next)) => {
                        group_of.insert(i, groups.len());
                        groups.push((next.clone(), vec![i], Some(Job::Resume(op.clone()))));
                    }
                    // a glb uploaded and never read back
                    _ => unread = record.id,
                }
            }
            Class::Send { .. } => {
                group_of.insert(i, groups.len());
                groups.push((hashes[i].clone(), vec![i], None));
            }
            Class::Joins(_) => {}
        }
        if let Some(id) = unread {
            match reads.get(&id) {
                Some(&g) => groups[g].1.push(i),
                None => {
                    reads.insert(id, groups.len());
                    groups.push((hashes[i].clone(), vec![i], Some(Job::Read(id))));
                }
            }
        }
    }
    for (i, class) in classes.iter().enumerate() {
        if let Class::Joins(leader) = class {
            groups[group_of[leader]].1.push(i);
        }
    }

    let ready: Vec<Group> = groups
        .into_iter()
        .map(|(hash, members, job)| {
            let job = job.unwrap_or_else(|| {
                let (a, compiled) = (&found[members[0]], compiled.remove(&members[0]).expect("checked"));
                Job::Send { compiled, key: a.key.clone(), file: a.file.clone() }
            });
            Group { hash, members, job }
        })
        .collect();
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
            let error =
                format!("{} failed the checks, so nothing was sent", plan::counted(r.failed.len(), "asset file"));
            return Some(Outcome::refused(code, error));
        }
        let n = r.ambiguous.len();
        (n > 0).then(|| Outcome::refused("asset.ambiguous", plan::ambiguous(n)))
    }
}

/// Sends a prepared upload, resuming what earlier runs left in flight and reading back the glbs no run has read, and
/// drops the records of files that are gone.
pub fn send(cfg: &Config, client: &Client, guard: &RemoteGuard, prepared: Prepared, stop: &AtomicBool) -> Outcome {
    let root = &cfg.root;
    let Prepared { mut lock, found, creator, mut report, groups: ready } = prepared;
    let mut broken = None;
    let mut halt = None;
    // set once Roblox refuses the key's reads: workers skip every read after it
    let refused_reads = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let mut sent = 0;
    let (ready, found, refused_reads) = (&ready, &found, &refused_reads);
    std::thread::scope(|s| {
        let mut flight = 0;
        loop {
            while flight < WORKERS
                && sent < ready.len()
                && !(halt.is_some() || broken.is_some() || stop.load(Ordering::SeqCst))
            {
                let g = &ready[sent];
                let reads_only = matches!(g.job, Job::Read(_));
                intent(&mut lock, g, found);
                if !reads_only && !save(&mut lock, root, guard, &mut broken) {
                    // the intent is not durable, so the request never leaves
                    if let Job::Send { .. } = g.job {
                        for &m in &g.members {
                            forget(&mut lock, &found[m].file, true);
                        }
                    }
                    break;
                }
                progress(g, found, sent, ready.len());
                let (tx, i) = (tx.clone(), sent);
                let first = &found[g.members[0]];
                let audio = matches!(first.ext.as_str(), ".ogg" | ".mp3" | ".wav" | ".flac");
                let glb = place::is_glb(&first.file).then(|| (root.join(&first.file), first.file.as_str()));
                s.spawn(move || {
                    let say = |e| drop(tx.send((i, e)));
                    // a worker that panics still answers, or the run would wait on it forever holding the guard
                    let worked = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let g = &ready[i];
                        let (id, reviewing) = match (&g.job, &glb) {
                            (Job::Read(id), _) => (*id, None),
                            (_, Some(_)) => match work(client, creator, g, audio, stop, &say) {
                                Event::Done(id, reviewing) => {
                                    say(Event::Done(id, reviewing));
                                    (id, Some(reviewing))
                                }
                                event => return say(event),
                            },
                            (_, None) => return say(work(client, creator, g, audio, stop, &say)),
                        };
                        let (path, file) = glb.as_ref().expect("only a glb is read");
                        say(read(client, id, reviewing, g, path, file, refused_reads));
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
            // a glb's upload is read before its group ends
            let glb = place::is_glb(&found[ready[i].members[0]].file);
            flight -= usize::from(!(matches!(event, Event::Created(_)) || glb && matches!(event, Event::Done(..))));
            halt = halt.or(apply(&mut lock, &ready[i], found, event, &mut report));
            refused_reads.store(report.refused_reads, Ordering::SeqCst);
            save(&mut lock, root, guard, &mut broken);
        }
    });
    let stopped = stop.load(Ordering::SeqCst);
    let (code, why) = if broken.is_some() {
        ("lock.changedOnDisk", "opossum.lock could not be saved")
    } else if halt == Some(Halt::Auth) {
        ("cloud.auth", "Roblox refused the API key")
    } else if halt == Some(Halt::Lost) {
        ("upload.halted", "an upload's answer was lost, so nothing more was sent")
    } else {
        ("upload.stopped", "opossum was stopped")
    };
    for g in &ready[sent..] {
        for &m in &g.members {
            let file = found[m].file.clone();
            match g.job {
                Job::Send { .. } => report.failed.push((file, format!("not sent: {why}"), code.into())),
                Job::Resume(_) => report.pending.push((file, format!("not asked: {why}"), code.into())),
                Job::Read(_) => report.pending.push((file, format!("not read: {why}"), code.into())),
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
    outcome(report, broken, halt, stopped)
}

/// Records a group's intent on every member before its request: `next` for a send, `next` and the operation for
/// files joining a resumed one. A read sends nothing, so it has none.
fn intent(lock: &mut Lock, g: &Group, found: &[File]) {
    let op = match &g.job {
        Job::Send { .. } => None,
        Job::Resume(op) => Some(op.clone()),
        Job::Read(_) => return,
    };
    for &m in &g.members {
        let r = lock.assets.entry(found[m].file.clone()).or_default();
        (r.next, r.op) = (Some(g.hash.clone()), op.clone());
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

/// `[37/1,324] uploading <file>…` as group `n` of `total` goes out.
fn progress(g: &Group, found: &[File], n: usize, total: usize) {
    let file = &found[g.members[0]].file;
    let verb = match g.job {
        Job::Send { .. } => "uploading",
        Job::Resume(_) => "waiting on the upload of",
        Job::Read(_) => "reading",
    };
    let more = match g.members.len() {
        1 => String::new(),
        n => format!(" (and {} identical)", n - 1),
    };
    eprintln!("[{}/{}] {verb} {file}{more}…", count(n + 1), count(total));
}

/// A group's upload on a worker thread: send it (unless a stop came first), then wait on its operation. `Created` goes
/// out through `send` as soon as Roblox takes the request; the event that ends the upload is returned.
fn work(
    client: &Client,
    creator: cloud::Creator,
    g: &Group,
    audio: bool,
    stop: &AtomicBool,
    send: &dyn Fn(Event),
) -> Event {
    let op = match &g.job {
        Job::Resume(op) => op.clone(),
        Job::Read(_) => unreachable!("a read uploads nothing"),
        Job::Send { compiled, key, file } => {
            if stop.load(Ordering::SeqCst) {
                return Event::NotSent;
            }
            let upload = Upload {
                kind: compiled.kind,
                name: key,
                description: file,
                creator,
                ext: compiled.ext,
                mime: compiled.mime,
                bytes: &compiled.bytes,
            };
            match client.create_asset(&upload) {
                Err(e) => return Event::Failed(e),
                Ok(op) => {
                    send(Event::Created(op.path.clone()));
                    if op.done {
                        return finished(op, audio);
                    }
                    op.path
                }
            }
        }
    };
    match client.wait_operation(&op, client.now() + OPERATION_DEADLINE, stop) {
        Ok(op) if op.done => finished(op, audio),
        Ok(_) => Event::Waiting(None),
        Err(e) if e.kind == ErrorKind::Status(404) => Event::Expired,
        Err(e) => Event::Waiting(Some(e)),
    }
}

/// Reads back what Roblox's import made of upload `id` of the glb at `path` (`file` in messages) and matches its parts
/// to the glb's by name: the same names, each once. `reviewing` is whether moderation was still reviewing the upload
/// when it finished, None when an earlier run uploaded it. Asset delivery refusing a key it doesn't refuse for review
/// stops every later read; any other failure, the keyless fetch of the location it hands out included, leaves only
/// this upload unread, for the next run.
fn read(
    client: &Client,
    id: u64,
    reviewing: Option<bool>,
    g: &Group,
    path: &Path,
    file: &str,
    refused: &AtomicBool,
) -> Event {
    let unread = |code, error| Event::ReadFailed(code, error, false);
    if refused.load(Ordering::SeqCst) {
        return unread("cloud.auth", "not read: Roblox refused the key's reads".into());
    }
    let names = match &g.job {
        Job::Send { compiled, .. } => media::glb(&compiled.bytes),
        _ => std::fs::read(path).map_err(|e| e.to_string()).and_then(|bytes| media::glb(&bytes)),
    };
    let names = match names {
        Ok(names) => names,
        Err(e) => return unread("asset.read", e),
    };
    let content = match client.asset_content(id) {
        Ok(content) => content,
        Err(e) => {
            return match (e.kind, reviewing) {
                _ if e.op == cloud::ASSET_CONTENT => {
                    unread("asset.read", format!("{e}; `opossum upload` reads it again"))
                }
                (ErrorKind::Status(401), _) | (ErrorKind::Status(403), Some(false)) => {
                    Event::ReadFailed("cloud.auth", e.to_string(), true)
                }
                (ErrorKind::Status(403), Some(true)) => {
                    let why = if e.message.is_empty() { "403" } else { e.message.as_str() };
                    unread("asset.read", format!("Roblox does not deliver it while moderation reviews it ({why})"))
                }
                (ErrorKind::Status(403), None) => {
                    unread("cloud.auth", format!("{e}, or it is still in Roblox's moderation review"))
                }
                _ => unread("asset.read", format!("{e}; `opossum upload` reads it again")),
            };
        }
    };
    let parts = match place::model_parts(&content) {
        Ok(parts) => parts,
        Err(e) => return unread("asset.read", format!("its content is no model Roblox's import made: {e}")),
    };
    let mut made: Vec<&str> = parts.iter().map(|(name, _)| name.as_str()).collect();
    let mut named: Vec<&str> = names.iter().map(String::as_str).collect();
    made.sort_unstable();
    named.sort_unstable();
    if made != named {
        let error = format!("the import has {}; {file} has {}", made.join(", "), named.join(", "));
        return unread("asset.read", error);
    }
    Event::Read(id, parts.into_iter().collect())
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

/// Records what a worker said on every member of its group, and whether the run sends nothing new after it.
fn apply(lock: &mut Lock, g: &Group, found: &[File], event: Event, report: &mut Report) -> Option<Halt> {
    let files = || g.members.iter().map(|&m| found[m].file.as_str());
    // the key its upload was named after
    let sent_as = &found[g.members[0]].key;
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
            return (e.diag() == "cloud.auth").then_some(Halt::Auth);
        }
        Event::Failed(e) => {
            for file in files() {
                lock.assets.entry(file.to_owned()).or_default().op = None;
                report.ambiguous.push((file.into(), format!("{e}; {}", plan::settle(file, sent_as))));
            }
            return Some(Halt::Lost);
        }
        Event::Done(id, reviewing) => {
            for file in files() {
                let bound = Asset { id: Some(id), hash: Some(g.hash.clone()), ..Asset::default() };
                lock.assets.insert(file.into(), bound);
                report.ids.push((file.into(), id));
            }
            report.uploaded += g.members.len();
            let review = if reviewing { ", still in Roblox's moderation review" } else { "" };
            eprintln!("{} is rbxassetid://{id}{review}", found[g.members[0]].file);
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
                report
                    .ambiguous
                    .push((file.into(), format!("operation {op} expired; {}", plan::settle(file, sent_as))));
            }
        }
        Event::Read(id, parts) => {
            // the parts are the id's, so every file bound to it (a rename reused it) gets them
            for r in lock.assets.values_mut().filter(|r| r.id == Some(id)) {
                r.parts = Some(parts.clone());
            }
            report.read.extend(files().map(String::from));
            eprintln!("{} read back: {}", found[g.members[0]].file, plan::counted(parts.len(), "part"));
        }
        Event::ReadFailed(code, error, refused) => {
            for file in files() {
                report.pending.push((file.into(), error.clone(), code.into()));
            }
            report.refused_reads |= refused;
        }
        Event::Waiting(e) => {
            let why =
                e.as_ref().map_or("still running".into(), cloud::Error::to_string) + "; `opossum upload` asks again";
            report.pending.extend(files().map(|f| (f.into(), why.clone(), "asset.pending".into())));
            if let Some(e) = e {
                return (e.diag() == "cloud.auth").then_some(Halt::Auth);
            }
        }
        Event::Panicked => {
            for file in files() {
                // an operation to ask, or an id the read never finished: the next run picks either up
                match lock.assets.get(file).is_some_and(|r| r.op.is_some() || r.id.is_some() && r.next.is_none()) {
                    true => {
                        let why = "opossum failed mid-upload; `opossum upload` picks it up".into();
                        report.pending.push((file.into(), why, "asset.pending".into()))
                    }
                    false => report
                        .ambiguous
                        .push((file.into(), format!("opossum failed mid-upload; {}", plan::settle(file, sent_as)))),
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
    None
}

fn outcome(r: Report, broken: Option<Diag>, halt: Option<Halt>, stopped: bool) -> Outcome {
    let mut text = format!(
        "uploaded {}, read {}, reused {}, unchanged {}, dropped {}\n",
        count(r.uploaded),
        count(r.read.len()),
        count(r.reused),
        count(r.unchanged),
        count(r.dropped)
    );
    for (file, error, _) in &r.failed {
        let _ = writeln!(text, "failed {file}: {error}");
    }
    for (file, why, _) in &r.pending {
        let _ = writeln!(text, "pending {file}: {why}");
    }
    for (file, how) in &r.ambiguous {
        let _ = writeln!(text, "ambiguous {file}: {how}");
    }
    let listed = |all: &[(String, String, String)]| -> Vec<serde_json::Value> {
        all.iter().map(|(file, error, code)| json!({ "file": file, "error": error, "code": code })).collect()
    };
    let json = json!({
        "ok": true, "uploaded": r.uploaded, "read": r.read, "reused": r.reused, "unchanged": r.unchanged,
        "dropped": r.dropped,
        "ids": r.ids.iter().map(|(file, id)| (file.clone(), json!(id))).collect::<serde_json::Map<_, _>>(),
        "pending": listed(&r.pending), "ambiguous": r.ambiguous.iter().map(|(file, _)| file).collect::<Vec<_>>(),
        "failed": listed(&r.failed),
    });
    // the one reason an agent branches on: what stopped the run, else the first kind of trouble a file is in
    let (code, error) = if let Some(d) = broken {
        (d.code, d.message)
    } else if stopped {
        ("upload.stopped".into(), "opossum was stopped; what it sent is recorded".into())
    } else if halt == Some(Halt::Auth) {
        ("cloud.auth".into(), "Roblox refused the API key, so nothing more was sent".into())
    } else if halt == Some(Halt::Lost) {
        let error = format!(
            "{}; nothing more was sent, and `opossum upload` sends the rest",
            plan::ambiguous(r.ambiguous.len())
        );
        ("asset.ambiguous".into(), error)
    } else if r.refused_reads {
        let error = "Roblox refused the key's reads of uploaded models: it needs the legacy-asset:manage scope. Every \
                     upload went on, and `opossum upload` reads them back once the key has it";
        ("cloud.auth".into(), error.into())
    } else if let Some((_, _, code)) = r.failed.first() {
        (code.clone(), format!("{} failed", plan::counted(r.failed.len(), "file")))
    } else if !r.ambiguous.is_empty() {
        ("asset.ambiguous".into(), plan::ambiguous(r.ambiguous.len()))
    } else if !r.pending.is_empty() {
        // why a read failed says more than that an operation is still running
        let code = r.pending.iter().map(|(_, _, code)| code).find(|c| *c != "asset.pending");
        let error = format!("{} pending", plan::counted(r.pending.len(), "file"));
        (code.map_or("asset.pending", String::as_str).into(), error)
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
            // a refusal is of content, so it is forgotten for every file holding it
            for r in lock.assets.values_mut().filter(|r| r.rejected.is_some() && r.rejected == record.rejected) {
                r.rejected = None;
            }
            lock.assets.retain(|file, r| *file == key || r.id.is_some() || r.next.is_some() || r.rejected.is_some());
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
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Barrier};

    use cloud::mock::{Mock, Reply, auto};
    use cloud::{Clock, Fake};
    use media::ImageOptions;
    use serde_json::json;

    use super::*;
    use crate::assets;
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
        assets::hash(".mp4", ImageOptions::DEFAULT, text.as_bytes())
    }

    /// A done operation `operations/<op>` holding asset `id`.
    fn done(op: u64, id: u64) -> Reply {
        Reply::json(200, &json!({ "path": format!("operations/{op}"), "done": true, "response": { "assetId": id } }))
    }

    /// A glb whose `parts` share one textured material, as Blender exports one but with no binary chunk: the mock's
    /// import reads only the JSON.
    pub(crate) fn glb(parts: &[&str]) -> Vec<u8> {
        let gltf = json!({
            "asset": { "version": "2.0" },
            "nodes": parts.iter().enumerate().map(|(i, p)| json!({ "name": p, "mesh": i })).collect::<Vec<_>>(),
            "meshes": parts.iter().enumerate()
                .map(|(i, p)| json!({ "name": p, "primitives": [{ "attributes": { "POSITION": i }, "material": 0 }] }))
                .collect::<Vec<_>>(),
            "accessors": parts.iter().map(|_| json!({ "min": [0, 0, 0], "max": [1, 2, 3] })).collect::<Vec<_>>(),
            "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }],
        });
        let mut chunk = gltf.to_string().into_bytes();
        chunk.resize(chunk.len().next_multiple_of(4), b' ');
        let mut out = b"glTF".to_vec();
        for n in [2, 20 + chunk.len() as u32, chunk.len() as u32] {
            out.extend(n.to_le_bytes());
        }
        out.extend(b"JSON");
        out.extend(chunk);
        out
    }

    /// The import's content holding one MeshPart per name.
    fn import(names: &[&str]) -> Reply {
        let parts: String = names
            .iter()
            .map(|n| {
                format!(
                    "<Item class=\"MeshPart\"><Properties><string name=\"Name\">{n}</string><Content \
                     name=\"MeshContent\"><uri>rbxassetid://5</uri></Content><Vector3 name=\"InitialSize\">\
                     <X>1</X><Y>1</Y><Z>1</Z></Vector3></Properties></Item>"
                )
            })
            .collect();
        Reply::bytes(200, format!("<roblox version=\"4\">{parts}</roblox>").into_bytes())
    }

    #[test]
    fn identical_files_share_one_upload_whose_intent_is_on_disk_first_and_a_rename_sends_nothing() {
        let root = project("share", &[("a.mp4", "x"), ("b.mp4", "x")]);
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
        assert_eq!((record(&root, "art/a.mp4"), record(&root, "art/b.mp4")), (Some(bound.clone()), Some(bound)));

        // the import's parts are the id's, so a rename binding it takes them along
        let part =
            lock::Part { mesh: 9, size: [1.0, 2.0, 3.0], color: None, normal: None, roughness: None, metalness: None };
        let parts = std::collections::BTreeMap::from([("hull".to_owned(), part)]);
        {
            let guard = RemoteGuard::acquire(&Cache { dir: root.join(".cache") }).unwrap();
            let (mut lock, _) = lock::read(&root).unwrap();
            lock.assets.get_mut("art/a.mp4").unwrap().parts = Some(parts.clone());
            lock::save(&mut lock, &root, &guard).unwrap();
        }
        std::fs::rename(root.join("art/a.mp4"), root.join("art/c.mp4")).unwrap();
        let out = run(&root, &mock, &stop);
        let counts = ["uploaded", "reused", "unchanged", "dropped"].map(|k| out.json[k].as_u64().unwrap());
        assert_eq!((out.exit, counts, mock.requests().len()), (0, [0, 1, 1, 1], 2));
        assert_eq!(record(&root, "art/c.mp4").map(|r| (r.id, r.parts)), Some((Some(77), Some(parts))));
        assert_eq!(record(&root, "art/a.mp4"), None);

        // a discovery error sends nothing and keeps every record, even of files that look gone
        std::fs::rename(root.join("art/c.mp4"), root.join("art/c.txt")).unwrap();
        let before = std::fs::read_to_string(root.join(lock::FILE)).unwrap();
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["code"], mock.requests().len()), (2, &json!("asset.unsupported"), 2));
        assert_eq!(std::fs::read_to_string(root.join(lock::FILE)).unwrap(), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_create_clears_its_intent_and_a_lost_answer_keeps_it_until_adopted() {
        let root = project("lost", &[("a.mp4", "x")]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let stop = AtomicBool::new(false);
        mock.on("POST", "assets/v1/assets", Reply::json(400, &json!({ "code": "INVALID_ARGUMENT" })));
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["failed"][0]["code"]), (1, &json!("cloud.http")));
        assert_eq!(record(&root, "art/a.mp4"), None);

        mock.on("POST", "assets/v1/assets", Reply::Reset);
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["ambiguous"]), (1, &json!(["art/a.mp4"])));
        let lost = Asset { next: Some(hash("x")), ..Asset::default() };
        assert_eq!(record(&root, "art/a.mp4"), Some(lost));
        // nothing can say whether it happened, so nothing is sent until the user does, a copy of it included
        std::fs::write(root.join("art/b.mp4"), "x").unwrap();
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["code"], sent(&mock, "POST", "")), (1, &json!("asset.ambiguous"), 2));
        assert_eq!(out.json["ambiguous"], json!(["art/a.mp4", "art/b.mp4"]));

        assert_eq!(adopt(&root, "art/a.mp4", None).exit, 0);
        let out = run(&root, &mock, &stop);
        assert_eq!((out.exit, &out.json["uploaded"], sent(&mock, "POST", "")), (0, &json!(2), 3));
        assert_eq!(adopt(&root, "art/a.mp4", Some(5)).json["code"], "asset.adopt");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A merge's ambiguous create hides behind the other branch's id until something picks it; adopt finds it anyway.
    #[test]
    fn adopt_settles_an_ambiguous_create_a_merge_left_beside_an_older_id() {
        let root = project("adopt-merged", &[("a.mp4", "y")]);
        let (x, y) = (hash("x"), hash("y"));
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/a.mp4\" = {{ id = 5, hash = \"{x}\" }}\n\
             assets.\"art/a.mp4\" = {{ next = \"{y}\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        assert_eq!(adopt(&root, "art/a.mp4", Some(77)).exit, 0);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["unchanged"], mock.requests().len()), (0, &json!(1), 0), "{}", out.json);
        assert_eq!(record(&root, "art/a.mp4").and_then(|r| r.id), Some(77));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Renaming a file whose upload may or may not have happened keeps that upload unsettled under the new name,
    /// rather than dropping it with the old name and sending the content again.
    #[test]
    fn an_ambiguous_upload_follows_its_file_through_a_rename() {
        let root = project("renamed", &[("a.mp4", "x")]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("POST", "assets/v1/assets", Reply::Reset);
        assert_eq!(run(&root, &mock, &stop_flag()).json["ambiguous"], json!(["art/a.mp4"]));
        std::fs::rename(root.join("art/a.mp4"), root.join("art/b.mp4")).unwrap();
        for _ in 0..2 {
            let out = run(&root, &mock, &stop_flag());
            assert_eq!(
                (&out.json["ambiguous"], &out.json["dropped"]),
                (&json!(["art/b.mp4"]), &json!(0)),
                "{}",
                out.json
            );
        }
        assert_eq!(adopt(&root, "art/b.mp4", Some(9)).exit, 0);
        assert_eq!((record(&root, "art/b.mp4").and_then(|r| r.id), sent(&mock, "POST", "")), (Some(9), 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_crash_after_the_create_resumes_its_operation_and_identical_files_join_it() {
        let root = project("resume", &[("a.mp4", "x"), ("b.mp4", "x"), ("c.mp4", "x")]);
        let h = hash("x");
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/a.mp4\" = {{ next = \"{h}\", op = \"operations/7\" }}\n\
             assets.\"art/b.mp4\" = {{ next = \"{h}\", op = \"operations/7\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        let out = adopt(&root, "art/a.mp4", Some(5));
        assert!(out.json["error"].as_str().unwrap().contains("`opossum upload` resumes it"), "{}", out.json);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("GET", "assets/v1/operations/7", done(7, 55));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["uploaded"]), (0, &json!(3)), "{}", out.json);
        assert_eq!((sent(&mock, "POST", ""), sent(&mock, "GET", "")), (0, 1));
        for file in ["art/a.mp4", "art/b.mp4", "art/c.mp4"] {
            assert_eq!(record(&root, file).and_then(|r| r.id), Some(55));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Folding `.opossum/opossum.lock.recovered` back (or a union merge) leaves the old record beside the one whose
    /// create of the file's current content is in flight: that create is resumed, never sent again.
    #[test]
    fn a_folded_back_upload_in_flight_wins_over_the_older_id_and_is_resumed() {
        let root = project("fold", &[("a.mp4", "y")]);
        let (x, y) = (hash("x"), hash("y"));
        let lock = format!(
            "version = 1\ncreator = {{ user = 1 }}\nassets.\"art/a.mp4\" = {{ id = 5, hash = \"{x}\" }}\n\
             assets.\"art/a.mp4\" = {{ id = 5, hash = \"{x}\", next = \"{y}\", op = \"operations/7\" }}\n"
        );
        std::fs::write(root.join(lock::FILE), lock).unwrap();
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("GET", "assets/v1/operations/7", done(7, 55));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", ""), sent(&mock, "GET", "")), (0, 0, 1), "{}", out.json);
        assert_eq!(record(&root, "art/a.mp4"), Some(Asset { id: Some(55), hash: Some(y), ..Asset::default() }));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A glb's read happens inside its group, so it holds one of the four slots: with four reads held open, no fifth
    /// upload starts. Each read lands as the record's parts.
    #[test]
    fn six_glbs_go_up_four_at_a_time_and_each_is_read_back() {
        let root = project("read", &[]);
        for i in 0..6 {
            std::fs::write(root.join(format!("art/{i}.glb")), glb(&["hull", &format!("Part{i}")])).unwrap();
        }
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let four = Arc::new(Barrier::new(4));
        for _ in 0..4 {
            let (four, base) = (four.clone(), mock.base.clone());
            let location = move |r: &cloud::mock::Request| {
                four.wait();
                let id = r.path.rsplit('/').next().unwrap();
                Reply::json(200, &json!({ "location": format!("{base}/_mock/content/{id}") }))
            };
            mock.on("GET", "asset-delivery-api/", Reply::Run(Box::new(location)));
        }
        let out = run(&root, &mock, &stop_flag());
        assert_eq!(
            (out.exit, &out.json["uploaded"], out.json["read"].as_array().map(Vec::len)),
            (0, &json!(6), Some(6))
        );
        assert_eq!((sent(&mock, "POST", "assets/"), sent(&mock, "GET", "asset-delivery-api/"), mock.peak()), (6, 6, 4));
        let text = std::fs::read_to_string(root.join(lock::FILE)).unwrap();
        assert_eq!(text.matches("parts = { Part").count(), 6, "{text}");
        let part = record(&root, "art/0.glb").unwrap().parts.unwrap().remove("hull").unwrap();
        // the material has a colour map and nothing else
        let maps = (part.color.is_some(), part.normal, part.roughness, part.metalness);
        assert_eq!((part.size, maps), ([1.0, 2.0, 3.0], (true, None, None, None)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A read that fails leaves the upload bound and unread, with no intent to settle: the plan calls it pending, and
    /// the next run only reads it.
    #[test]
    fn an_unread_upload_is_read_by_the_next_run_and_never_sent_again() {
        let root = project("unread", &[]);
        std::fs::write(root.join("art/a.glb"), glb(&["Lens", "hull"])).unwrap();
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let state = || plan::plan(&cfg, None).json["assets"][0]["files"][0]["state"].clone();
        let mock = Mock::bind(0, Some(auto())).unwrap();
        for _ in 0..4 {
            mock.on("GET", "_mock/content/", Reply::json(500, &json!({})));
        }
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["code"], &out.json["uploaded"]), (1, &json!("asset.read"), &json!(1)));
        let r = record(&root, "art/a.glb").unwrap();
        assert_eq!((r.id, r.next, r.parts), (Some(1_000_001), None, None));
        assert_eq!(state(), "pending");
        assert!(plan::plan(&cfg, None).text.contains(crate::ids::UNREAD));

        mock.on("GET", "_mock/content/", import(&["hull"]));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!(out.json["pending"][0]["error"], "the import has hull; art/a.glb has Lens, hull");

        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["read"], &out.json["uploaded"]), (0, &json!(["art/a.glb"]), &json!(0)));
        assert_eq!(sent(&mock, "POST", ""), 1);
        let parts = record(&root, "art/a.glb").unwrap().parts.unwrap();
        assert_eq!(parts.keys().collect::<Vec<_>>(), ["Lens", "hull"]);
        assert_eq!(state(), "current");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A key that can create but not read still uploads everything: a refused read stops the reads after it, and the
    /// scope is named once. A refusal while moderation reviews an upload leaves only that one unread.
    #[test]
    fn refused_reads_skip_the_rest_and_an_upload_in_review_stays_unread_alone() {
        let root = project("scope", &[]);
        for i in 0..6 {
            std::fs::write(root.join(format!("art/{i}.glb")), glb(&[&format!("Part{i}")])).unwrap();
        }
        let otherwise = auto();
        let refuse: cloud::mock::Fallback = Box::new(move |r| match r.path.starts_with("asset-delivery-api/") {
            true => Reply::json(401, &json!({ "message": "no" })),
            false => otherwise(r),
        });
        let mock = Mock::bind(0, Some(refuse)).unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["code"], &out.json["uploaded"]), (1, &json!("cloud.auth"), &json!(6)));
        let pending = out.json["pending"].as_array().unwrap();
        let skipped = pending.iter().filter(|p| p["error"].as_str().unwrap().starts_with("not read")).count();
        let asked = sent(&mock, "GET", "asset-delivery-api/");
        assert!(pending.len() == 6 && (1..=4).contains(&asked) && asked + skipped == 6, "{}", out.json);
        let said = format!("{}{}", out.text, out.json["error"]);
        assert_eq!(said.matches("legacy-asset:manage").count(), 1, "{said}");
        let _ = std::fs::remove_dir_all(&root);

        let root = project("review", &[]);
        std::fs::write(root.join("art/a.glb"), glb(&["A"])).unwrap();
        std::fs::write(root.join("art/b.glb"), glb(&["B"])).unwrap();
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let review = json!({ "path": "operations/1", "done": true,
                             "response": { "assetId": 7, "moderationResult": { "moderationState": "Reviewing" } } });
        mock.on("POST", "assets/v1/assets", Reply::json(200, &review));
        mock.on("GET", "asset-delivery-api/v1/assetId/7", Reply::json(403, &json!({ "message": "in review" })));
        let out = run(&root, &mock, &stop_flag());
        let in_review = out.json["ids"].as_object().unwrap().iter().find(|(_, id)| **id == 7).unwrap().0.clone();
        assert_eq!(
            (out.exit, &out.json["code"], out.json["read"].as_array().map(Vec::len)),
            (1, &json!("asset.read"), Some(1))
        );
        assert_eq!(
            out.json["pending"],
            json!([{ "file": in_review, "code": "asset.read",
                     "error": "Roblox does not deliver it while moderation reviews it (in review)" }])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The fetch of the location asset delivery hands out carries no key, so its refusal says nothing about the key's
    /// scopes: only that upload stays unread, and the reads go on.
    #[test]
    fn a_refused_content_fetch_leaves_only_its_upload_unread() {
        let root = project("cdn", &[]);
        std::fs::write(root.join("art/a.glb"), glb(&["A"])).unwrap();
        std::fs::write(root.join("art/b.glb"), glb(&["B"])).unwrap();
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("GET", "_mock/content/", Reply::json(403, &json!({ "message": "AccessDenied" })));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!(
            (out.exit, &out.json["code"], out.json["read"].as_array().map(Vec::len)),
            (1, &json!("asset.read"), Some(1))
        );
        let pending = out.json["pending"].as_array().unwrap();
        assert_eq!((pending.len(), &pending[0]["code"]), (1, &json!("asset.read")));
        assert_eq!(sent(&mock, "GET", "asset-delivery-api/"), 2);
        let said = format!("{}{}", out.text, out.json);
        assert!(!said.contains("legacy-asset:manage") && !said.contains("lacks a scope"), "{said}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn four_uploads_run_at_once_and_a_file_that_fails_its_checks_fails_alone() {
        let files: Vec<(String, String)> = (0..6).map(|i| (format!("{i}.mp4"), i.to_string())).collect();
        let mut files: Vec<(&str, &str)> = files.iter().map(|(f, t)| (f.as_str(), t.as_str())).collect();
        files.push(("empty.mp4", ""));
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
            json!([{ "file": "art/empty.mp4", "code": "asset.tooLarge",
            "error": "the file is empty; Roblox takes no empty asset" }])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_key_or_a_stop_sends_nothing_new_and_finishes_what_is_in_flight() {
        let files: Vec<(String, String)> = (0..6).map(|i| (format!("{i}.mp4"), i.to_string())).collect();
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

    /// A dropped network loses every later create's answer too: the first lost one stops the run, so at most the
    /// uploads already in flight need settling by hand.
    #[test]
    fn a_lost_answer_sends_nothing_new() {
        let files: Vec<(String, String)> = (0..12).map(|i| (format!("{i}.mp4"), i.to_string())).collect();
        let files: Vec<(&str, &str)> = files.iter().map(|(f, t)| (f.as_str(), t.as_str())).collect();
        let root = project("lost-net", &files);
        let mock = Mock::bind(0, Some(Box::new(|_| Reply::Reset))).unwrap();
        let out = run(&root, &mock, &stop_flag());
        let ambiguous = out.json["ambiguous"].as_array().unwrap().len();
        assert_eq!(
            (out.exit, &out.json["code"], ambiguous + out.json["failed"].as_array().unwrap().len()),
            (1, &json!("asset.ambiguous"), 12)
        );
        assert!((1..=WORKERS).contains(&ambiguous) && sent(&mock, "POST", "") == ambiguous, "{}", out.json);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lock_git_rewrote_mid_upload_is_recovered_instead_of_overwritten() {
        let root = project("git", &[("a.mp4", "x")]);
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
        assert!(recovered.contains(&format!("\"art/a.mp4\" = {{ id = 9, hash = \"{}\" }}", hash("x"))), "{recovered}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every moderation hit counts against the account, so refused bytes stay unsent until the user says otherwise.
    #[test]
    fn content_roblox_refused_is_not_sent_again_until_it_changes_or_the_refusal_is_forgotten() {
        let root = project("rejected", &[("a.mp4", "x")]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let rejected = json!({ "path": "operations/1", "done": true,
                               "response": { "assetId": 5, "moderationResult": { "moderationState": "Rejected" } } });
        mock.on("POST", "assets/v1/assets", Reply::json(200, &rejected));
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["failed"][0]["code"]), (1, &json!("asset.moderated")));
        assert_eq!(record(&root, "art/a.mp4"), Some(Asset { rejected: Some(hash("x")), ..Asset::default() }));

        let out = run(&root, &mock, &stop_flag());
        assert_eq!(
            (out.exit, &out.json["failed"][0]["code"], sent(&mock, "POST", "")),
            (1, &json!("asset.rejected"), 1)
        );
        // the refusal is the content's: a rename or a copy carries it, and forgetting it on one forgets it for all
        std::fs::rename(root.join("art/a.mp4"), root.join("art/b.mp4")).unwrap();
        std::fs::write(root.join("art/c.mp4"), "x").unwrap();
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, &out.json["dropped"], sent(&mock, "POST", "")), (1, &json!(1), 1), "{}", out.json);
        assert_eq!(out.json["failed"].as_array().unwrap().len(), 2);
        std::fs::remove_file(root.join("art/c.mp4")).unwrap();
        assert_eq!(adopt(&root, "art/b.mp4", None).exit, 0);
        assert_eq!(record(&root, "art/b.mp4"), None);
        let out = run(&root, &mock, &stop_flag());
        assert_eq!((out.exit, sent(&mock, "POST", "")), (0, 2));
        let _ = std::fs::remove_dir_all(&root);
    }
}
