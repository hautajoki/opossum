//! A small scoped thread pool for independent per-file work outside the checkers (rendering, the O2 gate, writes).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// At most this many threads.
const MAX_THREADS: usize = 8;

/// Fewer items than this run on the calling thread: a thread costs more than a few items.
const MIN_PARALLEL: usize = 4;

/// `f` over every item, on up to [`MAX_THREADS`] scoped threads taking items in order from a shared index; the
/// results in the items' order. Runs on the calling thread for a handful of items, or when no thread can be made.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
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
            let _ = std::thread::Builder::new().name(format!("opossum-work-{n}")).spawn_scoped(scope, work);
        }
        work();
    });
    results
        .into_iter()
        .map(|r| r.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner).expect("every item was taken"))
        .collect()
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
