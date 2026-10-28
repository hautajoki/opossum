//! The DataModel and everything that reads or writes it: the `place/` folder reader and its properties files, the
//! [`Tree`] opossum owns (mounting, stable ids, hashes, the sourcemap), how each property reaches Studio, the
//! `.rbxl`/`.rbxm` writers with texture packs, and `opossum migrate`'s conversion of a Rojo-era `place/`.

mod migrate;
mod properties;
pub mod props;
mod read;
mod tree;
mod value;
mod write;

use std::fmt;
use std::path::PathBuf;

pub use migrate::{Write, plan_migration};
pub use properties::{PlaceEntry, entries_from_toml};
pub use rbx_dom_weak::{self, Instance, Ustr, WeakDom, types::Ref, types::Variant, ustr};
pub use read::{Place, read_place, resolve_ref, service_for_dir};
pub use tree::{
    Id, NESTED, NONE, Node, Origin, PACKAGES, PackageFile, ROOT, ScriptKind, Source, Tree, package_tree_root,
};
pub use value::{db, descriptor};
pub use write::{PackStats, TexturePacks, read_texture_packs, write_rbxl, write_rbxm};

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
        Diag { code, severity: Severity::Error, path: path.into(), message: message.into() }
    }

    pub fn note(code: &'static str, path: impl Into<PathBuf>, message: impl Into<String>) -> Diag {
        Diag { code, severity: Severity::Note, path: path.into(), message: message.into() }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        write!(f, "{level}[{}] {}: {}", self.code, self.path.display(), self.message)
    }
}
