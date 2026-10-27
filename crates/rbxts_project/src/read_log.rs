//! What a program load read from the file system (watch mode, DESIGN §14 "Watcher"): every path the tsconfig parse
//! and the program asked about, with what they learned, as tsrs's own watch mode tracks it (`trackingvfs`, Go's
//! `SeenFiles`). Paths that did not exist count as much as files read: they are the places module resolution
//! probed (`node_modules/<name>` in every ancestor, `@types`, `typeRoots`, `paths`), where a file appearing changes
//! the program.
//!
//! JSON files read (package.json, configs) also keep a hash of their text: a rewrite with the same bytes changes
//! nothing the load saw.
//!
//! The log is also what a watch rebuild checks before reusing the load ([`crate::read_check`]), so every answer the
//! load got is kept in enough detail to ask again: the text of every file read, each directory listing as its reader
//! saw it, each `realpath`. The project setup's own reads (the `extends` chain, package.json lookups, the Rojo
//! resolvers' walks) are recorded too ([`recording`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tsrs_vfs::{Entries, FS, FileInfo};

/// What reads of one path learned (several reads of a path add up).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen(u8);

impl Seen {
    /// A file was there (a read or a positive `file_exists` / `stat`).
    pub const FILE: Seen = Seen(1);
    /// A directory was there.
    pub const DIR: Seen = Seen(2);
    /// `file_exists` said no: no file there (maybe a directory).
    pub const NO_FILE: Seen = Seen(4);
    /// `directory_exists` said no: no directory there (maybe a file).
    pub const NO_DIR: Seen = Seen(8);
    /// The directory's entries were listed.
    pub const LISTED: Seen = Seen(16);

    /// Whether every flag of `other` is set.
    pub fn has(self, other: Seen) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the path was there when read.
    pub fn existed(self) -> bool {
        self.0 & (Seen::FILE.0 | Seen::DIR.0) != 0
    }

    /// Whether what is on disk now (`is_file`, `is_dir`) contradicts what was read.
    pub fn contradicted(self, is_file: bool, is_dir: bool) -> bool {
        (self.has(Seen::FILE) && !is_file)
            || (self.has(Seen::DIR) && !is_dir)
            || (self.has(Seen::NO_FILE) && is_file)
            || (self.has(Seen::NO_DIR) && is_dir)
    }
}

impl std::ops::BitOr for Seen {
    type Output = Seen;
    fn bitor(self, other: Seen) -> Seen {
        Seen(self.0 | other.0)
    }
}

/// How a reader saw a directory's entries (what a recorded listing is compared by).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListingView {
    /// TypeScript's `getAccessibleEntries`: the files and the directories (links followed; anything else, or a broken
    /// link, left out).
    Ts,
    /// Every entry with its kind (links followed) and whether it is a link: the Rojo resolvers' walk and setup's
    /// package listing. Kept for [`ReadLog::recheck`] only: the directory reads as there ([`Seen::DIR`]), not as
    /// [`Seen::LISTED`], which tells a watcher to count its entries' events as module resolution's (it has rules of
    /// its own for the folders the Rojo resolvers search and for the package scopes).
    All,
}

/// An entry's kind, links followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A file (or a link to one).
    File,
    /// A directory (or a link to one).
    Dir,
    /// Anything else, or a broken link.
    Other,
}

/// The paths one load read, by their spelling in the reads.
#[derive(Default)]
pub struct ReadLog {
    paths: Mutex<HashMap<String, Seen>>,
    /// [`text_hash`] of the JSON files read, by their spelling in the reads.
    texts: Mutex<HashMap<String, u64>>,
    /// [`text_hash`] of every file read as text (JSON included).
    contents: Mutex<HashMap<String, u64>>,
    /// Each directory listing, by path and view: [`listing_hash`] of what the reader saw.
    listings: Mutex<HashMap<(String, ListingView), u64>>,
    /// Each `realpath` asked, with its answer.
    realpaths: Mutex<HashMap<String, String>>,
}

/// The hash a read log keeps of a directory listing in a view: the entries sorted by name, each with what the view
/// sees of it.
pub fn listing_hash(view: ListingView, entries: impl IntoIterator<Item = (String, EntryKind, bool)>) -> u64 {
    let mut seen: Vec<(String, u8)> = entries
        .into_iter()
        .filter_map(|(name, kind, link)| match view {
            ListingView::Ts => match kind {
                EntryKind::File => Some((name, b'f')),
                EntryKind::Dir => Some((name, b'd')),
                EntryKind::Other => None,
            },
            ListingView::All => {
                let kind = match kind {
                    EntryKind::File => b'f',
                    EntryKind::Dir => b'd',
                    EntryKind::Other => b'o',
                };
                Some((name, if link { kind.to_ascii_uppercase() } else { kind }))
            }
        })
        .collect();
    seen.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    for (name, kind) in &seen {
        name.bytes().for_each(&mut eat);
        eat(0);
        eat(*kind);
    }
    hash ^ (seen.len() as u64).rotate_left(32)
}

thread_local! {
    /// The log of the recording in progress on this thread ([`recording`]).
    static CURRENT: RefCell<Option<Arc<ReadLog>>> = const { RefCell::new(None) };
}

/// Runs `f` with `log` as this thread's recording: the reads made through [`recorded`] go to it. A thread spawned
/// inside takes it along explicitly (`recording(current().as_ref(), …)`).
pub fn recording<T>(log: Option<&Arc<ReadLog>>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<ReadLog>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            CURRENT.with(|current| *current.borrow_mut() = previous);
        }
    }
    let _restore = Restore(CURRENT.with(|current| current.replace(log.cloned())));
    f()
}

/// This thread's recording, if one is in progress.
pub fn current() -> Option<Arc<ReadLog>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// File system reads that go to this thread's recording ([`recording`]), for the project setup.
pub mod recorded {
    use std::fs;
    use std::io;
    use std::path::Path;

    use super::{EntryKind, ListingView, Seen, current};

    /// `fs::metadata` (links followed), recorded as what it found.
    pub fn metadata(path: impl AsRef<Path>) -> io::Result<fs::Metadata> {
        let path = path.as_ref();
        let result = fs::metadata(path);
        if let Some(log) = current() {
            let seen = match &result {
                Ok(m) if m.is_dir() => Seen::DIR,
                Ok(_) => Seen::FILE,
                Err(_) => Seen::NO_FILE | Seen::NO_DIR,
            };
            log.see(&path.to_string_lossy(), seen);
        }
        result
    }

    /// `Path::exists`, recorded.
    pub fn exists(path: impl AsRef<Path>) -> bool {
        metadata(path).is_ok()
    }

    /// `fs::read_to_string`, recorded with the text.
    pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
        let path = path.as_ref();
        let result = fs::read_to_string(path);
        if let Some(log) = current() {
            log.see_text(&path.to_string_lossy(), result.as_ref().ok().map(String::as_str));
        }
        result
    }

    /// `fs::read`, recorded with the text (bytes that are not text record as missing: a check never reuses them).
    pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        let path = path.as_ref();
        let result = fs::read(path);
        if let Some(log) = current() {
            let text = result.as_ref().ok().and_then(|bytes| std::str::from_utf8(bytes).ok());
            log.see_text(&path.to_string_lossy(), text);
        }
        result
    }

    /// Records the listing of `dir` as its reader saw it: every entry with its kind (links followed) and whether it
    /// is a link ([`ListingView::All`]).
    pub fn listed(dir: &str, entries: impl IntoIterator<Item = (String, EntryKind, bool)>) {
        if let Some(log) = current() {
            log.see_listing(dir, ListingView::All, entries);
        }
    }

    /// A listing that failed: the directory read as missing.
    pub fn not_listed(dir: &str) {
        if let Some(log) = current() {
            log.see(dir, Seen::NO_DIR);
        }
    }

    /// The kind of a directory entry, links followed, and whether it is a link (`Other` for a broken link).
    pub fn entry_kind(entry: &fs::DirEntry) -> (EntryKind, bool) {
        let Ok(own) = entry.file_type() else { return (EntryKind::Other, false) };
        let link = own.is_symlink();
        let kind = if link { fs::metadata(entry.path()).map(|m| m.file_type()) } else { Ok(own) };
        let kind = match kind {
            Ok(t) if t.is_dir() => EntryKind::Dir,
            Ok(t) if t.is_file() => EntryKind::File,
            _ => EntryKind::Other,
        };
        (kind, link)
    }

    /// The entry names of a directory in the system's order, the listing recorded ([`ListingView::All`]).
    pub fn read_dir_names(dir: &str) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                not_listed(dir);
                return Err(e);
            }
        };
        let entries: Vec<fs::DirEntry> = entries.filter_map(Result::ok).collect();
        let names: Vec<String> = entries.iter().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        if current().is_some() {
            listed(
                dir,
                entries.iter().zip(&names).map(|(entry, name)| {
                    let (kind, link) = entry_kind(entry);
                    (name.clone(), kind, link)
                }),
            );
        }
        Ok(names)
    }
}

/// The OS file system, its reads recorded in this thread's recording ([`recording`]) when there is one: for
/// resolutions setup makes through tsrs (a bare-name `extends`).
pub struct ThreadRecordingFs;

impl FS for ThreadRecordingFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        tsrs_vfs::osvfs::fs().use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        let exists = tsrs_vfs::osvfs::fs().file_exists(path);
        if let Some(log) = current() {
            log.see(path, if exists { Seen::FILE } else { Seen::NO_FILE });
        }
        exists
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let text = tsrs_vfs::osvfs::fs().read_file(path);
        if let Some(log) = current() {
            log.see_text(path, text.as_deref());
        }
        text
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), String> {
        tsrs_vfs::osvfs::fs().write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), String> {
        tsrs_vfs::osvfs::fs().append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        tsrs_vfs::osvfs::fs().remove(path)
    }

    fn chtimes(&self, path: &str, a_time: SystemTime, m_time: SystemTime) -> Result<(), String> {
        tsrs_vfs::osvfs::fs().chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        let exists = tsrs_vfs::osvfs::fs().directory_exists(path);
        if let Some(log) = current() {
            log.see(path, if exists { Seen::DIR } else { Seen::NO_DIR });
        }
        exists
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let entries = tsrs_vfs::osvfs::fs().get_accessible_entries(path);
        if let Some(log) = current() {
            let seen = entries
                .files
                .iter()
                .map(|name| (name.clone(), EntryKind::File, false))
                .chain(entries.directories.iter().map(|name| (name.clone(), EntryKind::Dir, false)));
            log.see_listing(path, ListingView::Ts, seen);
        }
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        let info = tsrs_vfs::osvfs::fs().stat(path);
        if let Some(log) = current() {
            let seen = match &info {
                Some(info) if info.is_dir() => Seen::DIR,
                Some(_) => Seen::FILE,
                None => Seen::NO_FILE | Seen::NO_DIR,
            };
            log.see(path, seen);
        }
        info
    }

    fn realpath(&self, path: &str) -> String {
        let answer = tsrs_vfs::osvfs::fs().realpath(path);
        if let Some(log) = current() {
            log.see_realpath(path, &answer);
        }
        answer
    }
}

/// The hash a read log keeps of a JSON file's text (FNV-1a over the UTF-8 bytes, a leading byte-order mark left
/// out): equal hashes of a file's text now and as read mean the same text.
pub fn text_hash(text: &str) -> u64 {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash ^ (text.len() as u64).rotate_left(32)
}

impl ReadLog {
    /// Records what a read of `path` learned (setup's reads outside the program's file system go here too).
    pub fn see(&self, path: &str, seen: Seen) {
        // the bundled lib files are not on disk
        if path.starts_with("bundled:") {
            return;
        }
        let mut paths = self.paths.lock().expect("read log");
        match paths.get_mut(path) {
            Some(entry) => *entry = *entry | seen,
            None => {
                paths.insert(path.to_owned(), seen);
            }
        }
    }

    /// Records a file read: there with `text`, or not there. The text is hashed ([`text_hash`]).
    pub fn see_text(&self, path: &str, text: Option<&str>) {
        self.see(path, if text.is_some() { Seen::FILE } else { Seen::NO_FILE });
        if let Some(text) = text
            && !path.starts_with("bundled:")
        {
            let hash = text_hash(text);
            if path.ends_with(".json") {
                self.texts.lock().expect("read log").insert(path.to_owned(), hash);
            }
            self.contents.lock().expect("read log").insert(path.to_owned(), hash);
        }
    }

    /// Records a directory listing as a reader saw it (`entries`: every entry, with its kind and whether it is a link;
    /// the view keeps what the reader looked at).
    pub fn see_listing(&self, dir: &str, view: ListingView, entries: impl IntoIterator<Item = (String, EntryKind, bool)>) {
        if dir.starts_with("bundled:") {
            return;
        }
        self.see(dir, if view == ListingView::Ts { Seen::LISTED } else { Seen::DIR });
        self.listings.lock().expect("read log").insert((dir.to_owned(), view), listing_hash(view, entries));
    }

    /// Records a `realpath` answer.
    pub fn see_realpath(&self, path: &str, answer: &str) {
        if path.starts_with("bundled:") {
            return;
        }
        self.realpaths.lock().expect("read log").insert(path.to_owned(), answer.to_owned());
    }

    /// Every file read as text, with the hash of its text (unordered).
    pub fn contents(&self) -> Vec<(String, u64)> {
        self.contents.lock().expect("read log").iter().map(|(p, h)| (p.clone(), *h)).collect()
    }

    /// Every directory listing, with its view and hash (unordered).
    pub fn listings(&self) -> Vec<(String, ListingView, u64)> {
        self.listings.lock().expect("read log").iter().map(|((p, v), h)| (p.clone(), *v, *h)).collect()
    }

    /// Every `realpath` asked, with its answer (unordered).
    pub fn realpaths(&self) -> Vec<(String, String)> {
        self.realpaths.lock().expect("read log").iter().map(|(p, a)| (p.clone(), a.clone())).collect()
    }

    /// The JSON files read with the hash of their text (unordered).
    pub fn texts(&self) -> Vec<(String, u64)> {
        self.texts.lock().expect("read log").iter().map(|(p, h)| (p.clone(), *h)).collect()
    }

    /// Every path read, with what was learned (unordered).
    pub fn entries(&self) -> Vec<(String, Seen)> {
        self.paths.lock().expect("read log").iter().map(|(p, s)| (p.clone(), *s)).collect()
    }

    /// How many paths were read.
    pub fn len(&self) -> usize {
        self.paths.lock().expect("read log").len()
    }

    /// Whether nothing was read.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A file system that records reads in a [`ReadLog`]. Writes are not recorded.
pub(crate) struct RecordingFs<T: FS> {
    pub inner: T,
    pub log: std::sync::Arc<ReadLog>,
}

impl<T: FS> FS for RecordingFs<T> {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        let exists = self.inner.file_exists(path);
        self.log.see(path, if exists { Seen::FILE } else { Seen::NO_FILE });
        exists
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let text = self.inner.read_file(path);
        self.log.see_text(path, text.as_deref());
        text
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        self.inner.remove(path)
    }

    fn chtimes(&self, path: &str, a_time: SystemTime, m_time: SystemTime) -> Result<(), String> {
        self.inner.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        let exists = self.inner.directory_exists(path);
        self.log.see(path, if exists { Seen::DIR } else { Seen::NO_DIR });
        exists
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let entries = self.inner.get_accessible_entries(path);
        let seen = entries
            .files
            .iter()
            .map(|name| (name.clone(), EntryKind::File, false))
            .chain(entries.directories.iter().map(|name| (name.clone(), EntryKind::Dir, false)));
        self.log.see_listing(path, ListingView::Ts, seen);
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        let info = self.inner.stat(path);
        let seen = match &info {
            Some(info) if info.is_dir() => Seen::DIR,
            Some(_) => Seen::FILE,
            None => Seen::NO_FILE | Seen::NO_DIR,
        };
        self.log.see(path, seen);
        info
    }

    fn realpath(&self, path: &str) -> String {
        let answer = self.inner.realpath(path);
        self.log.see_realpath(path, &answer);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::{Seen, text_hash};

    #[test]
    fn contradictions() {
        assert!(Seen::NO_FILE.contradicted(true, false));
        assert!(!Seen::NO_FILE.contradicted(false, true));
        assert!((Seen::NO_FILE | Seen::NO_DIR).contradicted(false, true));
        assert!(Seen::FILE.contradicted(false, false));
        assert!(!Seen::DIR.contradicted(false, true));
        assert!(!Seen::LISTED.contradicted(false, false));
    }

    #[test]
    fn text_hashes() {
        assert_eq!(text_hash("{}"), text_hash("\u{feff}{}"));
        assert_ne!(text_hash("{}"), text_hash("{ }"));
        assert_ne!(text_hash(""), text_hash("\0"));
    }
}
