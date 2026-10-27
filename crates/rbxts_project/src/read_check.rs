//! Whether what a load read ([`ReadLog`]) still reads the same: the check a watch rebuild makes before it reuses the
//! last load (its tsconfig parse, module resolution and project setup), instead of loading again.
//!
//! The check asks every question of the log again and compares the answers: each path's existence and kind, the text
//! of each file read, each directory listing as its reader saw it ([`ListingView`]), each `realpath`. The program's
//! files are compared by the modification time and size their parsed versions have in the source cache, as a load
//! compares them: the ones that differ are the files to parse again, and the rest of the load stands.
//!
//! **One listing per directory.** A file system query costs about as much as a directory listing on Windows (a
//! handle opened per path, through the virus scanner), and the log holds thousands of paths (module resolution's
//! probes), so the check lists the directory of each path once, in parallel, and answers every question about the
//! directory's entries from that listing: about ten times fewer system calls than the load made, which asked them one
//! at a time. A link's target is looked up on its own (its kind and stamp are the target's, as the load saw them).
//!
//! **Texts.** A file read as text is compared by its text the first time (read again and hashed), then by the
//! modification time and size it had when it was found holding that text ([`Settled`]), unless that time is too
//! recent to tell a later edit in the same clock tick apart ([`settled_time`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::read_log::{EntryKind, ReadLog, listing_hash, text_hash};

/// How close to now a modification time must not be for a stamp to rely on it: an edit within the same tick of the
/// file system's clock would get the same time (git's "racily clean" entries).
const RACY: Duration = Duration::from_secs(2);

/// `modified` (nanoseconds since the Unix epoch) when it is far enough in the past to tell a later edit apart, else 0
/// (no time to rely on).
pub fn settled_time(modified: u128) -> u128 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    if modified != 0 && now >= modified + RACY.as_nanos() { modified } else { 0 }
}

/// A time as nanoseconds since the Unix epoch, 0 when unknown.
fn nanos(time: Option<SystemTime>) -> u128 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// The files a check found holding the text the load read, with the modification time (nanoseconds) and size they
/// had then: found the same again, a file is not read again. Kept with the load the log belongs to.
#[derive(Debug, Default)]
pub struct Settled(HashMap<String, (u128, u64)>);

/// A check's answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Recheck {
    /// Everything the load read reads the same, but for these program files (by file name), whose modification time
    /// or size changed since they were parsed.
    Same {
        /// The program files to parse again.
        changed: Vec<String>,
    },
    /// Something reads differently: what, for debugging.
    Different(String),
}

/// One directory entry, as the check sees it.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    /// Links followed; `Other` for a broken link (with `exists` false).
    kind: EntryKind,
    exists: bool,
    link: bool,
    /// The target's modification time and size (a link's are its target's).
    modified: Option<SystemTime>,
    size: u64,
}

/// One directory's listing: `None` when it could not be listed (missing, or not a directory).
type View = Option<HashMap<String, Entry>>;

/// A program file's stamp in the source cache: its file name, and its modification time and size when parsed.
pub type ProgramStamp = (String, Option<(Option<SystemTime>, i64)>);

/// The program file stamps the source cache has.
pub type ProgramStamps<'a> = &'a [ProgramStamp];

/// A path's directory and name, separators either way (a root has no name: `None`).
fn split(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let at = trimmed.rfind(['/', '\\'])?;
    let (dir, name) = (&trimmed[..at], &trimmed[at + 1..]);
    if name.is_empty() {
        return None;
    }
    // `E:/a` is in `E:/`, not `E:`
    let dir = if dir.ends_with(':') || dir.is_empty() { &trimmed[..=at] } else { dir };
    Some((dir, name))
}

/// The key a directory or entry name is looked up by.
fn key(text: &str, case_sensitive: bool) -> String {
    let text = text.replace('\\', "/");
    let text = text.trim_end_matches('/');
    let text = if text.ends_with(':') { format!("{text}/") } else { text.to_owned() };
    if case_sensitive { text } else { text.to_lowercase() }
}

/// Lists a directory as the check sees it.
fn list(dir: &str, case_sensitive: bool) -> View {
    let read = std::fs::read_dir(dir).ok()?;
    let mut entries = HashMap::new();
    for entry in read {
        let Ok(entry) = entry else { return None };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(own) = entry.file_type() else { continue };
        let link = own.is_symlink();
        let metadata = if link { std::fs::metadata(entry.path()) } else { entry.metadata() };
        let (kind, exists, modified, size) = match metadata {
            Ok(m) if m.is_dir() => (EntryKind::Dir, true, m.modified().ok(), m.len()),
            Ok(m) if m.is_file() => (EntryKind::File, true, m.modified().ok(), m.len()),
            Ok(m) => (EntryKind::Other, true, m.modified().ok(), m.len()),
            Err(_) => (EntryKind::Other, !link, None, 0),
        };
        entries.insert(key(&name, case_sensitive), Entry { name, kind, exists, link, modified, size });
    }
    Some(entries)
}

/// Runs `job` on every index below `count` on up to 16 threads, in no particular order.
fn parallel<T: Send>(count: usize, job: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get).clamp(1, 16).min(count.max(1));
    if threads <= 1 {
        return (0..count).map(&job).collect();
    }
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= count {
                            return done;
                        }
                        done.push((i, job(i)));
                    }
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("a check thread")).collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

impl ReadLog {
    /// Asks every question of this log again ([`crate::read_check`]). `program` are the files of the program the load
    /// made, with the stamps the source cache has for them; `settled` is kept from one check of this log to the next.
    pub fn recheck(&self, program: ProgramStamps<'_>, case_sensitive: bool, settled: &mut Settled) -> Recheck {
        let paths = self.entries();
        let listings = self.listings();
        let contents = self.contents();
        let realpaths = self.realpaths();

        // the directories to list: each path's, each listed directory itself, each program file's
        let mut dirs: Vec<String> = Vec::new();
        let mut dir_index: HashMap<String, usize> = HashMap::new();
        let mut want = |dir: &str| {
            let k = key(dir, case_sensitive);
            if let std::collections::hash_map::Entry::Vacant(entry) = dir_index.entry(k) {
                entry.insert(dirs.len());
                dirs.push(dir.to_owned());
            }
        };
        for (path, _) in &paths {
            if let Some((dir, _)) = split(path) {
                want(dir);
            }
        }
        for (path, _, _) in &listings {
            want(path);
        }
        for (path, _) in &contents {
            if let Some((dir, _)) = split(path) {
                want(dir);
            }
        }
        for (file, _) in program {
            if let Some((dir, _)) = split(file) {
                want(dir);
            }
        }
        let views: Vec<View> = parallel(dirs.len(), |i| list(&dirs[i], case_sensitive));
        let view_of = |dir: &str| -> &View { &views[dir_index[&key(dir, case_sensitive)]] };
        let entry_of = |path: &str| -> Option<&Entry> {
            let (dir, name) = split(path)?;
            view_of(dir).as_ref()?.get(&key(name, case_sensitive)).filter(|e| e.exists)
        };

        // existence and kinds
        for (path, seen) in &paths {
            if split(path).is_none() {
                // a root: there, as a directory
                let is_dir = std::fs::metadata(path).is_ok_and(|m| m.is_dir());
                if seen.contradicted(false, is_dir) {
                    return Recheck::Different(format!("{path}: no longer as read"));
                }
                continue;
            }
            let entry = entry_of(path);
            let is_dir = entry.is_some_and(|e| e.kind == EntryKind::Dir);
            let is_file = entry.is_some_and(|e| e.kind != EntryKind::Dir);
            if seen.contradicted(is_file, is_dir) {
                return Recheck::Different(format!("{path}: no longer as read"));
            }
        }
        // listings, as their readers saw them
        for (dir, view, hash) in &listings {
            let entries = view_of(dir).iter().flat_map(|v| v.values()).map(|e| (e.name.clone(), e.kind, e.link));
            if listing_hash(*view, entries) != *hash {
                return Recheck::Different(format!("{dir}: its entries changed"));
            }
        }
        // the program's files: the ones whose stamp changed are parsed again
        let mut program_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut changed = Vec::new();
        for (file, stamp) in program {
            program_keys.insert(key(file, case_sensitive));
            let Some(entry) = entry_of(file).filter(|e| e.kind != EntryKind::Dir) else {
                return Recheck::Different(format!("{file}: gone"));
            };
            let Some((modified, size)) = stamp else {
                return Recheck::Different(format!("{file}: not in the source cache"));
            };
            if entry.modified != *modified || entry.size as i64 != *size {
                changed.push(file.clone());
            }
        }
        // texts read (but the program's files): the same text
        let mut to_read: Vec<(&str, u64, u128, u64)> = Vec::new();
        for (path, hash) in &contents {
            if program_keys.contains(&key(path, case_sensitive)) {
                continue;
            }
            let Some(entry) = entry_of(path).filter(|e| e.kind != EntryKind::Dir) else {
                return Recheck::Different(format!("{path}: gone"));
            };
            let stamp = (nanos(entry.modified), entry.size);
            if stamp.0 != 0 && settled.0.get(path.as_str()) == Some(&stamp) {
                continue;
            }
            to_read.push((path, *hash, stamp.0, stamp.1));
        }
        let read: Vec<bool> = parallel(to_read.len(), |i| {
            let (path, hash, _, _) = to_read[i];
            tsrs_vfs::osvfs::fs().read_file(path).is_some_and(|text| text_hash(&text) == hash)
        });
        for (&(path, _, modified, size), same) in to_read.iter().zip(read) {
            if !same {
                return Recheck::Different(format!("{path}: its text changed"));
            }
            match settled_time(modified) {
                0 => settled.0.remove(path),
                modified => settled.0.insert(path.to_owned(), (modified, size)),
            };
        }
        // links followed
        let resolved: Vec<bool> =
            parallel(realpaths.len(), |i| tsrs_vfs::osvfs::fs().realpath(&realpaths[i].0) == realpaths[i].1);
        if let Some(i) = resolved.iter().position(|same| !same) {
            return Recheck::Different(format!("{}: resolves elsewhere", realpaths[i].0));
        }
        changed.sort();
        Recheck::Same { changed }
    }
}

#[cfg(test)]
mod tests {
    use super::{Recheck, Settled, split};
    use crate::read_log::{EntryKind, ListingView, ReadLog, Seen};

    #[test]
    fn splits() {
        assert_eq!(split("E:/a/b.ts"), Some(("E:/a", "b.ts")));
        assert_eq!(split("E:\\a\\b"), Some(("E:\\a", "b")));
        assert_eq!(split("E:/a/"), Some(("E:/", "a")));
        assert_eq!(split("E:/"), None);
        assert_eq!(split("/a"), Some(("/", "a")));
    }

    /// Each kind of read, read the same and then differently.
    #[test]
    fn rechecks_every_kind_of_read() {
        let dir = std::env::temp_dir().join(format!("rbxts-rs-read-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        let root = dir.to_string_lossy().replace('\\', "/");
        let at = |p: &str| format!("{root}/{p}");
        std::fs::write(at("pkg/package.json"), "{}").unwrap();
        std::fs::write(at("a.ts"), "export {};").unwrap();
        let case_sensitive = !cfg!(windows);
        let stamp = |p: &str| {
            let m = std::fs::metadata(p).unwrap();
            Some((m.modified().ok(), m.len() as i64))
        };
        let program = vec![(at("a.ts"), stamp(&at("a.ts")))];
        let log = ReadLog::default();
        log.see_text(&at("pkg/package.json"), Some("{}"));
        log.see(&at("pkg/index.d.ts"), Seen::NO_FILE);
        log.see(&at("missing/deeper/x.ts"), Seen::NO_FILE | Seen::NO_DIR);
        log.see_listing(&at("pkg"), ListingView::Ts, [("package.json".to_owned(), EntryKind::File, false)]);
        let mut settled = Settled::default();
        let check = |settled: &mut Settled| log.recheck(&program, case_sensitive, settled);
        assert_eq!(check(&mut settled), Recheck::Same { changed: Vec::new() });
        assert_eq!(check(&mut settled), Recheck::Same { changed: Vec::new() });

        // a program file edited: to parse again
        std::fs::write(at("a.ts"), "export const a = 1;").unwrap();
        assert_eq!(check(&mut settled), Recheck::Same { changed: vec![at("a.ts")] });
        // a probed path appears (and the listing changes with it)
        std::fs::write(at("pkg/index.d.ts"), "").unwrap();
        assert!(matches!(check(&mut settled), Recheck::Different(_)));
        std::fs::remove_file(at("pkg/index.d.ts")).unwrap();
        assert!(matches!(check(&mut settled), Recheck::Same { .. }));
        // a missing directory on the way to a probe appears: nothing below it yet, the same
        std::fs::create_dir_all(at("missing/deeper")).unwrap();
        assert!(matches!(check(&mut settled), Recheck::Same { .. }));
        std::fs::write(at("missing/deeper/x.ts"), "").unwrap();
        assert!(matches!(check(&mut settled), Recheck::Different(_)));
        std::fs::remove_dir_all(at("missing")).unwrap();
        // a JSON file rewritten with the same text, then with another
        std::fs::write(at("pkg/package.json"), "{}").unwrap();
        assert!(matches!(check(&mut settled), Recheck::Same { .. }));
        std::fs::write(at("pkg/package.json"), "{ \"types\": \"x\" }").unwrap();
        assert!(matches!(check(&mut settled), Recheck::Different(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
