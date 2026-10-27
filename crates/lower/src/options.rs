//! What the caller turns on for a lowering run, and the outside values the compile-time macros read.

use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Debug, Clone)]
pub struct Options {
    /// Emit native Luau types and `--!strict`.
    pub native_types: bool,
    /// Files compiled as if every function carried `@native` (absolute, forward-slash paths; `[native] files`).
    pub native_files: FxHashSet<String>,
    /// `$env`: the environment plus the project's `.env` files, resolved by the caller.
    pub env: FxHashMap<String, String>,
    /// `$git`: `None` outside a repository.
    pub git: Option<Git>,
    /// `$package`: the project's `package.json` text.
    pub package_json: Option<String>,
    /// `$buildTime`: an ISO-8601 UTC timestamp, fixed for the whole build.
    pub build_time: String,
}

#[derive(Debug, Clone, Default)]
pub struct Git {
    pub sha: String,
    pub branch: String,
    pub tag: String,
    pub dirty: bool,
    /// Unix seconds of the HEAD commit.
    pub timestamp: i64,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            native_types: true,
            native_files: FxHashSet::default(),
            env: FxHashMap::default(),
            git: None,
            package_json: None,
            build_time: String::new(),
        }
    }
}
