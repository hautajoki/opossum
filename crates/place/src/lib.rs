//! The DataModel and everything that reads it: the `place/` folder reader, the `[place]` property table and
//! the mounting API the compiler uses for scripts and packages.

mod config;
mod read;
mod tree;
mod value;

use std::fmt;
use std::path::PathBuf;

pub use config::{PlaceConfig, PlaceEntry, config_from_toml, config_to_toml};
pub use rbx_dom_weak::{self, Instance, WeakDom, types::Ref, types::Variant};
pub use read::{read_place, resolve_ref, service_for_dir};
pub use tree::{PACKAGES, PackageFile, Place, ScriptKind, package_tree_root};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// A problem found while reading or mounting, tied to the file (or instance path) it came from.
#[derive(Debug, Clone)]
pub struct Diag {
    pub code: &'static str,
    pub severity: Severity,
    pub path: PathBuf,
    pub message: String,
}

impl Diag {
    pub fn error(code: &'static str, path: impl Into<PathBuf>, message: impl Into<String>) -> Diag {
        Diag {
            code,
            severity: Severity::Error,
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn note(code: &'static str, path: impl Into<PathBuf>, message: impl Into<String>) -> Diag {
        Diag {
            code,
            severity: Severity::Note,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        write!(
            f,
            "{level}[{}] {}: {}",
            self.code,
            self.path.display(),
            self.message
        )
    }
}
