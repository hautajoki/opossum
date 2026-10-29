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

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;

pub use migrate::{Write, plan_migration};
pub use rbx_dom_weak::{self, Instance, Ustr, WeakDom, types::Ref, types::Variant, ustr};
pub use read::{
    Place, frame_dir, instance_name, model_classes, model_parts, rbxm_class, read_place, resolve_ref, service_for_dir,
};
pub use tree::{
    Id, NESTED, NONE, Node, Origin, PACKAGES, PackageFile, ROOT, ScriptKind, Source, Tree, package_tree_root,
};
pub use value::{db, descriptor};
pub use write::{PackStats, Surface, TexturePacks, texture_packs, write_rbxl, write_rbxm};

/// One part of an uploaded model as Roblox's import made it, read back once and kept in `opossum.lock`: the MeshPart's
/// mesh, its `InitialSize`, and its SurfaceAppearance's maps (a channel the import gave no id is `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub mesh: u64,
    pub size: [f32; 3],
    pub color: Option<u64>,
    pub normal: Option<u64>,
    pub roughness: Option<u64>,
    pub metalness: Option<u64>,
}

impl Part {
    /// The id this part holds for `channel`.
    pub fn get(&self, channel: Channel) -> Option<u64> {
        match channel {
            Channel::Mesh => Some(self.mesh),
            Channel::Color => self.color,
            Channel::Normal => self.normal,
            Channel::Roughness => self.roughness,
            Channel::Metalness => self.metalness,
        }
    }
}

/// What a part's id is for: its MeshPart's mesh or one of its SurfaceAppearance's maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Mesh,
    Color,
    Normal,
    Roughness,
    Metalness,
}

impl Channel {
    pub const ALL: [Channel; 5] =
        [Channel::Mesh, Channel::Color, Channel::Normal, Channel::Roughness, Channel::Metalness];

    /// The channel a property takes from a glb part (`MeshId`/`MeshContent`, `ColorMap`/`ColorMapContent`, ...); None
    /// for any other property, which takes no `part`.
    pub fn of(property: &str) -> Option<Channel> {
        match property.strip_suffix("Content").unwrap_or(property) {
            "MeshId" | "Mesh" => Some(Channel::Mesh),
            "ColorMap" => Some(Channel::Color),
            "NormalMap" => Some(Channel::Normal),
            "RoughnessMap" => Some(Channel::Roughness),
            "MetalnessMap" => Some(Channel::Metalness),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Mesh => "mesh",
            Channel::Color => "color",
            Channel::Normal => "normal",
            Channel::Roughness => "roughness",
            Channel::Metalness => "metalness",
        }
    }
}

/// What a `$asset` file resolves to: a glb's parts once read back, anything else's id, or nothing while it has no
/// upload (or its glb upload is unread).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Id(u64),
    Parts(BTreeMap<String, Part>),
    Unset,
}

/// The lock as `$asset` references see it, filled by the build from the lock and the asset roots' files:
/// [`Place::resolve_assets`] sets every reference from `by_file`, and `by_id` names the file (and part and channel)
/// behind every id the lock holds, so a literal `rbxassetid://` repeating one is `place.pastedId`.
#[derive(Debug, Default, PartialEq)]
pub struct Resolver {
    /// Every file under an asset root, by project path.
    pub by_file: BTreeMap<String, Value>,
    pub by_id: HashMap<u64, (String, Option<(String, Channel)>)>,
}

/// A model file opossum reads parts back from.
pub fn is_glb(file: &str) -> bool {
    file.len() > 4 && file[file.len() - 4..].eq_ignore_ascii_case(".glb")
}

/// Edits (insert, delete, replace) from `a` to `b`.
pub fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = std::mem::replace(&mut row[0], i + 1);
        for j in 0..b.len() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != b[j])).min(row[j] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// A problem found while reading or mounting, tied to the file (or instance path) it came from.
#[derive(Debug, Clone, PartialEq)]
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

    pub fn warning(code: &'static str, path: impl Into<PathBuf>, message: impl Into<String>) -> Diag {
        Diag { code, severity: Severity::Warning, path: path.into(), message: message.into() }
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
