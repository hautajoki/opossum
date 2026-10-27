//! Shared helpers for rbxts-rs's tests (`rbxts_transform`'s and `rbxts_cli`'s integration tests).
//!
//! - [`node_modules`] finds the `node_modules` the tests link into their temporary projects: the one
//!   `RBXTS_RS_FIXTURE_NODE_MODULES` names (a path without the packages is an error: a set variable means the tests
//!   should run), else the roblox-ts tests project's, found as `roblox-ts/tests/node_modules` or
//!   `roblox-ts-test/roblox-ts/tests/node_modules` beside the checkout or in a directory above it.
//! - [`node_modules_or_skip`] skips a test without one, saying so on the terminal (outside the captured output), unless
//!   `RBXTS_RS_FIXTURE_REQUIRE=1` or `CI` is set: then the test fails.
//! - [`TempDir`] is a temporary directory removed on drop, panics included, with its links removed as links first
//!   ([`TempDir::link`]), never followed into the shared `node_modules`.
#![warn(missing_docs)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The variable naming the `node_modules` to link.
pub const NODE_MODULES_VAR: &str = "RBXTS_RS_FIXTURE_NODE_MODULES";
/// The packages that `node_modules` must have.
pub const PACKAGES: [&str; 4] = ["compiler-types", "types", "testez", "services"];

/// Whether `node_modules` has every package of [`PACKAGES`].
pub fn has_packages(node_modules: &Path) -> bool {
    PACKAGES.iter().all(|name| node_modules.join("@rbxts").join(name).is_dir())
}

/// The `node_modules` to link into test projects, or why there is none. Panics when `RBXTS_RS_FIXTURE_NODE_MODULES`
/// names a directory without the packages.
pub fn node_modules() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(NODE_MODULES_VAR) {
        let path = PathBuf::from(path);
        assert!(has_packages(&path), "{NODE_MODULES_VAR}={} has no @rbxts/{{{}}}", path.display(), PACKAGES.join(", "));
        return Ok(path);
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .flat_map(|dir| {
            [dir.join("roblox-ts/tests/node_modules"), dir.join("roblox-ts-test/roblox-ts/tests/node_modules")]
        })
        .find(|candidate| has_packages(candidate))
        .ok_or_else(|| {
            format!(
                "no node_modules with @rbxts/{{{}}}: set {NODE_MODULES_VAR}, or have the roblox-ts tests project \
                 (roblox-ts/tests or roblox-ts-test/roblox-ts/tests) with its packages installed beside {} or a \
                 directory above it",
                PACKAGES.join(", "),
                manifest.parent().and_then(Path::parent).unwrap_or(manifest).display()
            )
        })
}

/// Tests must run (a missing `node_modules` fails them): `RBXTS_RS_FIXTURE_REQUIRE=1`, or `CI` set.
pub fn required() -> bool {
    std::env::var_os("RBXTS_RS_FIXTURE_REQUIRE").is_some_and(|v| v == "1") || std::env::var_os("CI").is_some()
}

/// Skips the test `test`, saying why on the terminal: written to stderr directly, since the test harness captures
/// (and, for a passing test, drops) `eprintln!`. A skipped test passes, unless [`required`].
pub fn skip(test: &str, why: &str) {
    assert!(!required(), "{test}: {why} (tests are required: CI or RBXTS_RS_FIXTURE_REQUIRE=1)");
    let _ = writeln!(std::io::stderr(), "{test} skipped: {why}");
}

/// [`node_modules`], or `None` after [`skip`]ping `test`.
pub fn node_modules_or_skip(test: &str) -> Option<PathBuf> {
    match node_modules() {
        Ok(path) => Some(path),
        Err(why) => {
            skip(test, &why);
            None
        }
    }
}

/// Links `link` to the directory `target`: a junction on Windows (it needs no privileges, unlike a directory
/// symlink), a symlink elsewhere.
pub fn link_dir(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        // mklink wants backslashes
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(target.to_string_lossy().replace('/', "\\"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "could not link {} to {}: {}",
            link.display(),
            target.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, link)
        .unwrap_or_else(|e| panic!("could not link {} to {}: {e}", link.display(), target.display()));
}

/// Removes a link made by [`link_dir`] (not its target).
pub fn unlink_dir(link: &Path) {
    if std::fs::remove_dir(link).is_err() {
        let _ = std::fs::remove_file(link);
    }
}

/// Copies the directory `from` to `to`, recursively.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temporary directory, `<temp>/rbxts-rs-<label>-<process id>`, removed on drop, panics included. Links made
/// through [`TempDir::link`] are removed as links first, innermost first.
pub struct TempDir {
    root: PathBuf,
    links: Vec<PathBuf>,
}

impl TempDir {
    /// A new, empty directory (one a killed run with the same process id left is removed first).
    pub fn new(label: &str) -> TempDir {
        let root = std::env::temp_dir().join(format!("rbxts-rs-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        TempDir { root, links: Vec::new() }
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Links `relative` (inside the directory) to `target`, making its parent directories.
    pub fn link(&mut self, relative: &str, target: &Path) -> PathBuf {
        let link = self.root.join(relative);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        link_dir(&link, target);
        self.links.push(link.clone());
        link
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // removing a junction or a directory symlink removes the link, not its target
        for link in self.links.iter().rev() {
            unlink_dir(link);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_removes_links_not_targets() {
        let target = TempDir::new("support-target");
        std::fs::write(target.path().join("kept.txt"), "x").unwrap();
        let removed = {
            let mut dir = TempDir::new("support-dir");
            let link = dir.link("a/b/linked", target.path());
            assert!(link.join("kept.txt").is_file());
            dir.path().to_path_buf()
        };
        assert!(!removed.exists());
        assert!(target.path().join("kept.txt").is_file());
    }
}
