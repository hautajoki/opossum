//! The DataModel and everything that reads or writes it: the `place/` folder reader, the `[place]` property
//! table, the mounting API the compiler uses for scripts and packages, the `.rbxl`/`.rbxm` writers with texture
//! packs, and the luau-lsp sourcemap.

mod config;
mod migrate;
mod read;
mod sourcemap;
mod tree;
mod value;
mod write;

use std::fmt;
use std::path::PathBuf;

pub use config::{PlaceConfig, PlaceEntry, config_from_toml, config_to_toml};
pub use migrate::{config_from_meta, convert_rojo_refs, model_json_text};
pub use rbx_dom_weak::{self, Instance, WeakDom, types::Ref, types::Variant};
pub use read::{read_place, resolve_ref, service_for_dir};
pub use sourcemap::sourcemap_json;
pub use tree::{PACKAGES, PackageFile, Place, ScriptKind, package_tree_root};
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
