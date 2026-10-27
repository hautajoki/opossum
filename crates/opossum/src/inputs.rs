//! The outside values the compile-time macros read: the environment with the project's `.env` files, git's view of
//! the working tree, and the build time.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use lower::options::Git;
use rustc_hash::FxHashMap;

/// `.env`, then `.env.local` over it, then the process environment over both.
pub fn env(root: &Path) -> FxHashMap<String, String> {
    let mut env = FxHashMap::default();
    for name in [".env", ".env.local"] {
        if let Ok(text) = std::fs::read_to_string(root.join(name)) {
            parse_env(&text, &mut env);
        }
    }
    env.extend(std::env::vars());
    env
}

/// `KEY=value` lines; `#` comments, blank lines and an `export ` prefix are skipped, and a value in matching quotes
/// loses them. No interpolation.
fn parse_env(text: &str, env: &mut FxHashMap<String, String>) {
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let quoted = value.len() >= 2 && matches!(value.as_bytes()[0], b'"' | b'\'') && value.ends_with(&value[..1]);
        let unquoted = if quoted { &value[1..value.len() - 1] } else { value };
        env.insert(key.trim().to_owned(), unquoted.to_owned());
    }
}

/// A started read of git's fields: `git log` (sha, author time, decorations for the branch and tag) and
/// `git status` (dirty) run while the build loads.
pub struct GitRead {
    log: Option<std::process::Child>,
    status: Option<std::process::Child>,
}

pub fn start_git(root: &Path) -> GitRead {
    let spawn = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    };
    GitRead { log: spawn(&["log", "-1", "--format=%H%n%at%n%D"]), status: spawn(&["status", "--porcelain"]) }
}

impl GitRead {
    /// The fields, `None` outside a repository (or without git).
    pub fn finish(self) -> Option<Git> {
        let output = |child: Option<std::process::Child>| {
            let out = child?.wait_with_output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let log = output(self.log)?;
        let dirty = output(self.status).is_some_and(|s| !s.trim().is_empty());
        let mut lines = log.lines();
        let sha = lines.next()?.trim();
        let timestamp = lines.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
        let (mut branch, mut tag) = (String::new(), String::new());
        // `HEAD -> main, tag: v1.2, origin/main`
        for decoration in lines.next().unwrap_or("").split(", ") {
            if let Some(name) = decoration.strip_prefix("HEAD -> ") {
                branch = name.to_owned();
            } else if let Some(name) = decoration.strip_prefix("tag: ")
                && tag.is_empty()
            {
                tag = name.to_owned();
            }
        }
        Some(Git { sha: sha.chars().take(7).collect(), branch, tag, dirty, timestamp })
    }
}

/// Now as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn build_time() -> String {
    iso8601(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64)
}

fn iso8601(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates_cross_leap_days() {
        assert_eq!(super::iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(super::iso8601(1_798_761_599), "2026-12-31T23:59:59Z");
    }
}
