//! The values the compile-time macros read, from the real environment: the process environment, the git repository
//! holding the sources, `package.json` and the clock.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// `options` with `$env`, `$git`, `$package` and `$buildTime` filled for the project at `project`.
pub fn fill(mut options: lower::Options, project: &Path) -> lower::Options {
    options.env = std::env::vars().collect();
    // the repository holding the sources (`src` may be a link into it)
    let src = project.join("src").canonicalize().unwrap_or_else(|_| project.to_path_buf());
    options.git = git(&src);
    options.package_json = std::fs::read_to_string(project.join("package.json")).ok();
    options.build_time = iso8601(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64));
    options
}

fn run_git(dir: &Path, args: &[&str]) -> Option<String> {
    // no optional locks: `status` would otherwise refresh the index, writing into the repository
    let output = Command::new("git").arg("--no-optional-locks").arg("-C").arg(dir).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// HEAD's short sha, branch (empty when detached), a tag pointing at it, the working tree's dirtiness and HEAD's
/// commit time; `None` outside a repository.
fn git(dir: &Path) -> Option<lower::options::Git> {
    let sha = run_git(dir, &["rev-parse", "--short", "HEAD"])?;
    let branch = run_git(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    let tag = run_git(dir, &["tag", "--points-at", "HEAD"]).and_then(|t| t.lines().next().map(str::to_owned));
    let dirty = run_git(dir, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let timestamp = run_git(dir, &["log", "-1", "--format=%ct"]).and_then(|t| t.parse().ok()).unwrap_or(0);
    Some(lower::options::Git { sha, branch, tag: tag.unwrap_or_default(), dirty, timestamp })
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (civil-from-days, proleptic Gregorian).
fn iso8601(seconds: i64) -> String {
    let (days, rest) = (seconds.div_euclid(86400), seconds.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}
