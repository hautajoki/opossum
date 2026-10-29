//! The machine-wide remote guard and the cache directory it lives in. One opossum at a time changes Roblox state or
//! `opossum.lock` on this machine, across every checkout and worktree, so two runs never upload the same files.

use std::fs;
use std::io::{Read as _, Seek as _, Write as _};
use std::path::PathBuf;

/// opossum's per-user cache: saved place versions and the guard's lock file. Tests make one in a temporary directory;
/// only `main.rs` asks for the real one.
pub struct Cache {
    pub dir: PathBuf,
}

impl Cache {
    /// `~/Library/Caches/opossum` on macOS, else `$XDG_CACHE_HOME/opossum` or `~/.cache/opossum`.
    pub fn default_dir() -> Result<Cache, String> {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = || var("HOME").ok_or("HOME is not set, so opossum has no cache directory");
        let base = match var("XDG_CACHE_HOME") {
            _ if cfg!(target_os = "macos") => home()?.join("Library/Caches"),
            Some(dir) => dir,
            None => home()?.join(".cache"),
        };
        Ok(Cache { dir: base.join("opossum") })
    }
}

/// An exclusive lock on `<cache>/remote.lock`, held for a whole run by the commands that change Roblox state or the
/// lock, and released when dropped or when the process dies. Acquired once per process (a second acquire would wait
/// on itself); whatever writes the lock takes `&RemoteGuard`, so writing without it does not compile.
pub struct RemoteGuard {
    _file: fs::File,
}

impl RemoteGuard {
    /// Takes the guard, waiting (and saying for whom) while another opossum holds it.
    pub fn acquire(cache: &Cache) -> Result<RemoteGuard, String> {
        let path = cache.dir.join("remote.lock");
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        fs::create_dir_all(&cache.dir).map_err(|e| format!("{}: {e}", cache.dir.display()))?;
        let mut file =
            fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                let mut pid = String::new();
                let _ = file.read_to_string(&mut pid);
                eprintln!("waiting for another opossum (pid {}) to finish with Roblox…", pid.trim());
                file.lock().map_err(io)?;
            }
            Err(fs::TryLockError::Error(e)) => return Err(io(e)),
        }
        file.set_len(0).and_then(|_| file.rewind()).and_then(|_| write!(file, "{}", std::process::id())).map_err(io)?;
        Ok(RemoteGuard { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_excludes_every_other_open_of_its_file_until_dropped() {
        let dir = std::env::temp_dir().join(format!("opossum-guard-{}", std::process::id()));
        let cache = Cache { dir: dir.clone() };
        let guard = RemoteGuard::acquire(&cache).unwrap();
        let other = fs::File::open(dir.join("remote.lock")).unwrap();
        assert!(matches!(other.try_lock(), Err(fs::TryLockError::WouldBlock)));
        assert_eq!(fs::read_to_string(dir.join("remote.lock")).unwrap(), std::process::id().to_string());
        drop(guard);
        // a child another test forks right now holds a copy of the descriptor, and with it the lock, until it execs
        let start = std::time::Instant::now();
        while other.try_lock().is_err() {
            assert!(start.elapsed().as_secs() < 5, "the lock outlived the guard");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
