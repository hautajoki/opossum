//! How a property reaches Studio, from the reflection database: the names nodes carry, the engine metadata that is
//! never compared, the groups the plugin writes through engine methods, and what a plugin cannot write at all.

use rbx_dom_weak::types::Variant;
use rbx_dom_weak::{Ustr, ustr};
use rbx_reflection::{ClassTag, PropertyKind, PropertySerialization, PropertyTag, Scriptability};

use crate::value::{db, descriptor};

/// Engine metadata: carried in blobs as is, left out of hashes and change detection.
pub const META: &[&str] = &[
    "Capabilities",
    "SourceAssetId",
    "ScriptGuid",
    "SerializedOverrides",
    "UniqueId",
    "HistoryId",
    "NeedsPivotMigration",
    "InertiaMigrated",
    "SlimHash",
    "ModelMeshCFrame",
    "ModelMeshData",
    "ModelMeshSize",
    "Sandboxed",
];

/// A MeshPart's mesh: any change is one `ApplyMesh`. `MeshId` is listed for nodes read before migration.
const MESH: &[&str] = &[
    "MeshId",
    "MeshContent",
    "InitialSize",
    "PhysicsData",
    "PhysicalConfigData",
    "AeroMeshData",
    "UnscaledCofm",
    "UnscaledVolume",
    "VertexCount",
    "HasSkinnedMesh",
    "HasJointOffset",
    "JointOffset",
    "CollisionFidelity",
    "RenderFidelity",
    "FluidFidelityInternal",
];

/// A union's geometry: any change is one `SubstituteGeometry`. The TriangleMeshPart data derived from it rides along.
const CSG: &[&str] = &[
    "MeshData",
    "ChildData",
    "AssetId",
    "Content",
    "PhysicalConfigData",
    "InitialSize",
    "AeroMeshData",
    "UnscaledCofm",
    "UnscaledVolume",
    "FluidFidelityInternal",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Write {
    /// Never compared or sent.
    Meta,
    /// Copied by name: `live[name] = carrier[name]`, or the plugin's own handling of Attributes, Tags, Source, Scale.
    Name,
    /// Written through an engine method: `__mesh`, `__csg`, `__pivot`, `__materialColors`.
    Group(&'static str),
    /// Not writable by a plugin: a creatable instance is replaced, a frame of reference keeps it build-only.
    Never,
}

pub fn is_a(class: &str, base: &str) -> bool {
    let db = db();
    match (db.classes.get(class), db.classes.get(base)) {
        (Some(c), Some(b)) => db.has_superclass(c, b),
        _ => false,
    }
}

/// `Instance.new` can make it.
pub fn creatable(class: &str) -> bool {
    db().classes
        .get(class)
        .is_some_and(|c| !c.tags.contains(&ClassTag::NotCreatable) && !c.tags.contains(&ClassTag::Service))
}

pub fn is_service(class: &str) -> bool {
    db().classes.get(class).is_some_and(|c| c.tags.contains(&ClassTag::Service))
}

/// How `class.name` is written.
pub fn write_kind(class: &str, name: &str) -> Write {
    if META.contains(&name) {
        return Write::Meta;
    }
    if matches!(name, "Attributes" | "Tags" | "Source") || (name == "Scale" && is_a(class, "Model")) {
        return Write::Name;
    }
    if MESH.contains(&name) && is_a(class, "MeshPart") {
        return Write::Group("__mesh");
    }
    if CSG.contains(&name) && is_a(class, "PartOperation") {
        return Write::Group("__csg");
    }
    if name == "WorldPivotData" && is_a(class, "Model") {
        return Write::Group("__pivot");
    }
    if name == "MaterialColors" && class == "Terrain" {
        return Write::Group("__materialColors");
    }
    match descriptor(class, name) {
        Some(p) if !matches!(p.scriptability, Scriptability::ReadWrite) => Write::Never,
        // a property the database does not know is copied under pcall
        _ => Write::Name,
    }
}

/// Every property of `class` a verify copies whether or not a node sets it, so the blob instance's defaults reset
/// what someone changed: the ones a plugin can write by name (a Model's `Scale` through `ScaleTo`) and rbx_binary
/// serializes, by canonical name, not deprecated. Metadata, the groups (an unset pivot is not verified), `Name`,
/// `Source`, `Attributes` and `Tags` have their own paths and are left out.
pub fn verified(class: &str) -> Vec<Ustr> {
    let db = db();
    let mut out = Vec::new();
    let mut at = db.classes.get(class);
    while let Some(c) = at {
        for prop in c.properties.values() {
            let PropertyKind::Canonical { serialization } = &prop.kind else { continue };
            if matches!(serialization, PropertySerialization::DoesNotSerialize | PropertySerialization::Migrate(_))
                || prop.tags.contains(&PropertyTag::Deprecated)
                || matches!(prop.name, "Name" | "Source" | "Attributes" | "Tags")
                || write_kind(class, prop.name) != Write::Name
            {
                continue;
            }
            out.push(ustr(prop.name));
        }
        at = c.superclass.and_then(|s| db.classes.get(s));
    }
    out.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
    out
}

/// A property as a node holds it: the name rbx_binary serializes under, ContentId migrated to Content. A script's
/// `Enabled` (which does not serialize) becomes `Disabled`; anything else that does not serialize is dropped.
pub fn node_prop(class: &str, name: Ustr, value: Variant) -> Option<(Ustr, Variant)> {
    let Some(prop) = descriptor(class, &name) else { return Some((name, value)) };
    let PropertyKind::Canonical { serialization } = &prop.kind else { return Some((name, value)) };
    match (serialization, value) {
        (PropertySerialization::DoesNotSerialize, Variant::Bool(on))
            if prop.name == "Enabled" && is_a(class, "BaseScript") =>
        {
            Some((ustr("Disabled"), Variant::Bool(!on)))
        }
        (PropertySerialization::DoesNotSerialize, _) => None,
        (PropertySerialization::Migrate(migration), value) => {
            match (migration.new_property_names(), migration.perform(&value)) {
                ([to], Ok(migrated)) => Some((ustr(to), migrated)),
                _ => Some((ustr(prop.name), value)),
            }
        }
        (_, value) => Some((ustr(prop.name), value)),
    }
}
