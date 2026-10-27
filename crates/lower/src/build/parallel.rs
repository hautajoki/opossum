//! A small scoped thread pool for independent per-file work outside the checkers (the commit's file writes).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// At most this many threads: writes are latency-bound (the file system, a virus scanner on close), and past a few
/// threads they queue in the file system instead.
const MAX_THREADS: usize = 8;

/// Fewer items than this run on the calling thread: a thread costs more than a few writes.
const MIN_PARALLEL: usize = 4;

/// `f` over every item, on up to [`MAX_THREADS`] scoped threads taking items in order from a shared index; the
/// results in the items' order. Runs on the calling thread for a handful of items, or when no thread can be made.
pub(crate) fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(MAX_THREADS).min(items.len());
    if items.len() < MIN_PARALLEL || threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    let work = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(item) = items.get(i) else { break };
            let result = f(item);
            *results[i].lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
        }
    };
    std::thread::scope(|scope| {
        // the calling thread works too; a thread that cannot be made leaves its share to the others
        for n in 1..threads {
            let _ = std::thread::Builder::new().name(format!("rbxts-write-{n}")).spawn_scoped(scope, work);
        }
        work();
    });
    results
        .into_iter()
        .map(|r| r.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner).expect("every item was taken"))
        .collect()
}

/// Directory listings made ahead of a walk, in parallel ([`prefetch`]): the walk takes each listing from here
/// ([`Listings::take`]), the same entries in the system's order as its own `read_dir` would give, and lists any
/// directory not here itself.
#[derive(Default)]
pub(crate) struct Listings(Mutex<rustc_hash::FxHashMap<String, Vec<std::fs::DirEntry>>>);

impl Listings {
    /// The entries of `dir` (spelled as the walk spells it), once; `None` when it was not listed ahead (or could not
    /// be: the walk's own `read_dir` then meets the error).
    pub(crate) fn take(&self, dir: &str) -> Option<Vec<std::fs::DirEntry>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(dir)
    }

    /// `dir`'s entries: listed ahead, else listed now. `None` when it cannot be listed.
    pub(crate) fn entries(&self, dir: &str) -> Option<Vec<std::fs::DirEntry>> {
        self.take(dir).or_else(|| std::fs::read_dir(dir).ok().map(|entries| entries.flatten().collect()))
    }
}

/// Lists the directories of a walk ahead, in parallel ([`map`]), level by level from `roots`: `enter` gives, for an
/// entry of a listed directory (spelled as the walk spells it), the path of a subdirectory the walk will enter, if it
/// will. A directory listing costs a file system round trip (and a virus scanner's look on Windows): a walk of a few
/// hundred directories spends most of its time waiting on them one at a time.
pub(crate) fn prefetch(roots: &[String], enter: impl Fn(&str, &std::fs::DirEntry) -> Option<String> + Sync) -> Listings {
    let listings = Listings::default();
    let mut level: Vec<String> = roots.to_vec();
    while !level.is_empty() {
        let listed = map(&level, |dir| {
            let entries: Option<Vec<std::fs::DirEntry>> =
                std::fs::read_dir(dir).ok().map(|entries| entries.flatten().collect());
            let children: Vec<String> =
                entries.iter().flatten().filter_map(|entry| enter(dir, entry)).collect();
            (entries, children)
        });
        let mut next = Vec::new();
        let mut map = listings.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (dir, (entries, children)) in level.into_iter().zip(listed) {
            if let Some(entries) = entries {
                map.insert(dir, entries);
            }
            next.extend(children);
        }
        drop(map);
        level = next;
    }
    listings
}

#[cfg(test)]
mod tests {
    use super::map;

    #[test]
    fn results_keep_the_items_order() {
        let items: Vec<usize> = (0..1000).collect();
        assert_eq!(map(&items, |&i| i * 2), items.iter().map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(map(&items[..2], |&i| i + 1), vec![1, 2]);
        assert!(map(&[] as &[usize], |&i| i).is_empty());
    }
}
