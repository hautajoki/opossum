//! The plugins' recorded inputs in watch mode ([`BuildSession::recorded_input_watches`]): globs recorded by files,
//! `setup` and `finish`, and the reads `finish` made (`read_dir`, `exists`, `read_file`).
//!
//! - **Watched:** a glob's root recursively, a listed directory and the directory of each read path flat, and while
//!   one is missing its nearest existing ancestor (the missing directories on the way count).
//! - **Events:** one the rest of the watcher drops but a recorded input may care about is a candidate; the session
//!   asks the inputs it concerns again ([`BuildSession::recorded_inputs_changed`], a glob only for paths its
//!   matcher's `affected_by` accepts). A different answer starts a rebuild with those paths (the session matches a
//!   file's glob again, or runs setup or `finish` again); an equal one starts nothing, and the watches are brought
//!   up to date (a root that appeared is now watched recursively). Candidates beside a rebuild that starts anyway
//!   go with it.
//! - **New watches:** a read made before its directory was watched raised no event, so when an update makes input
//!   watches, every recorded input is asked once more ([`BuildSession::recorded_inputs_changed_since_build`]). Only
//!   a watch made counts: one that fails is tried again at every update, and asking each time would start a rebuild
//!   after every rebuild while an answer differs (a build stopping before `finish` keeps the last finish's answers).

use std::time::SystemTime;

use rustc_hash::FxHashSet;

use rbxts_transform::BuildSession;
use rbxts_transform::build::plugin_inputs::InputWatch;

use super::{Own, Subscriptions, Watches, is_dir, is_own, is_under, key, parent_of, watched_set};

/// What the watcher watches for the recorded inputs.
#[derive(Clone, Debug, Default)]
pub(super) struct InputWatches {
    /// Glob roots that exist, watched recursively.
    pub recursive: Vec<String>,
    /// Listed directories, the nearest existing directory at or above each read path or missing input.
    pub flat: Vec<String>,
    /// Path keys of glob roots: any event under one is a candidate.
    trees: Vec<String>,
    /// Path keys of listed directories: an event of the directory or of an entry is a candidate.
    entries: FxHashSet<String>,
    /// Path keys of read paths and of the missing directories on the way to an input.
    paths: FxHashSet<String>,
    /// Every recorded input's path (glob roots, listed directories, read paths).
    inputs: Vec<String>,
}

impl InputWatches {
    /// The watches of the last build's recorded inputs.
    pub fn of(session: &BuildSession<'_>) -> InputWatches {
        let mut watches = InputWatches::default();
        for watch in session.recorded_input_watches() {
            let path = watch.path().to_owned();
            watches.inputs.push(path.clone());
            match &watch {
                InputWatch::Tree(_) => {
                    watches.trees.push(key(&path));
                    if is_dir(&path) {
                        watches.recursive.push(path.clone());
                    }
                }
                InputWatch::Entries(_) => {
                    watches.entries.insert(key(&path));
                    if is_dir(&path) {
                        watches.flat.push(path.clone());
                    }
                }
                InputWatch::Path(_) => {
                    watches.paths.insert(key(&path));
                }
            }
            watches.cover(&path);
        }
        watches
    }

    /// Watches the nearest existing directory above `path` (its own events: created, removed, renamed, written),
    /// the missing directories on the way counting.
    fn cover(&mut self, path: &str) {
        let mut dir = match parent_of(path) {
            Some(parent) => parent.to_owned(),
            None => return,
        };
        while !is_dir(&dir) {
            self.paths.insert(key(&dir));
            match parent_of(&dir) {
                Some(parent) => dir = parent.to_owned(),
                None => return,
            }
        }
        self.flat.push(dir);
    }

    /// An event at `path_key` a recorded input may care about.
    pub fn is_candidate(&self, path_key: &str) -> bool {
        self.paths.contains(path_key)
            || self.entries.contains(path_key)
            || parent_of(path_key).is_some_and(|parent| self.entries.contains(parent))
            || self.trees.iter().any(|root| is_under(path_key, root))
    }

    /// Some directory these inputs watch is watched in `after` (the watches after an update) and was not in
    /// `before`: a watch the update made. A watch that failed is in neither.
    pub fn has_new_watches(&self, before: &FxHashSet<(String, bool)>, after: &FxHashSet<(String, bool)>) -> bool {
        let tree = |active: &FxHashSet<(String, bool)>, dir: &String| active.contains(&(dir.clone(), true));
        let flat = |active: &FxHashSet<(String, bool)>, dir: &String| {
            active.contains(&(dir.clone(), false)) || covered(dir, active)
        };
        self.recursive.iter().any(|dir| tree(after, dir) && !tree(before, dir))
            || self.flat.iter().any(|dir| flat(after, dir) && !flat(before, dir))
    }
}

/// A directory inside an active recursive watch.
fn covered(dir: &str, active: &FxHashSet<(String, bool)>) -> bool {
    let k = key(dir);
    active.iter().any(|(watched, recursive)| *recursive && is_under(&k, &key(watched)))
}

/// One of rbxts-rs's temporary files, or this watcher's own write or deletion as it left it.
fn own_write(path: &str, own: &Own) -> bool {
    let k = key(path);
    let name = rbxts_project::nodepath::basename(&k);
    name.contains(".rbxts-tmp-") || name.contains(".rbxts-old-") || is_own(path, &k, own)
}

/// The events of a burst the rest of the watcher dropped that a recorded input may care about.
pub(super) fn candidates(
    dropped: Vec<(String, bool)>,
    watches: &InputWatches,
    own: &Own,
) -> Vec<String> {
    dropped
        .into_iter()
        // a directory's own modification: its entries' events say more
        .filter(|(path, folder_modify)| !folder_modify && watches.is_candidate(&key(path)) && !own_write(path, own))
        .map(|(path, _)| path)
        .collect()
}

/// After candidates that changed no answer: brings the watches up to date (a glob root or listed directory that
/// appeared is watched now) and returns the paths that start a rebuild after all (what changed in a new watch before
/// it existed, and a recorded input answering differently once it is watched).
pub(super) fn refresh(
    session: &BuildSession<'_>,
    subscriptions: &mut Subscriptions,
    watched: &mut Watches,
    own: &Own,
    since: SystemTime,
) -> Vec<String> {
    if let Some(set) = watched_set(session) {
        watched.good = set;
        watched.failed = None;
    }
    let inputs = watched.current().inputs.clone();
    let before = subscriptions.active.clone();
    let mut stale = subscriptions.update(watched.current(), since, own);
    let new_watches = inputs.has_new_watches(&before, &subscriptions.active);
    stale.extend(recheck(session, &inputs, new_watches));
    stale
}

/// When `new_watches` (the update made input watches): every recorded input asked again (a read made before its
/// directory was watched raised no event). When one answers differently, the inputs' paths, as the paths of a
/// rebuild (each concerns its input, so the session matches the globs of files again too).
pub(super) fn recheck(session: &BuildSession<'_>, inputs: &InputWatches, new_watches: bool) -> Vec<String> {
    if !new_watches || !session.recorded_inputs_changed_since_build() {
        return Vec::new();
    }
    inputs.inputs.clone()
}
