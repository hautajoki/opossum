//! `.rbxl`/`.rbxm` output through rbx_binary, with texture packs written into the place.
//!
//! When Studio publishes a place it bakes each SurfaceAppearance's, MaterialVariant's and PBR Decal's maps into
//! one texture pack asset (TexturePackContent), and a live server draws PBR only from that pack. A place
//! published through Open Cloud has none, so the packs Studio baked are kept in `place/texture-packs.json`,
//! keyed `Class|map uri|map uri|...` (the class's maps in order, colour first), and every place write sets them.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use rbx_dom_weak::types::{Content, Ref, Variant};
use rbx_dom_weak::{Instance, ustr};

use crate::read::Place;

/// Pack key -> pack uri, as in `place/texture-packs.json`.
pub type TexturePacks = HashMap<String, String>;

/// Each packed class's maps, its colour first. A Decal's colour is its Texture, and a Decal is PBR (so packed)
/// only once it has more than a colour.
const PACKED: &[(&str, [&str; 4])] = &[
    ("SurfaceAppearance", ["ColorMap", "NormalMap", "RoughnessMap", "MetalnessMap"]),
    ("MaterialVariant", ["ColorMap", "NormalMap", "RoughnessMap", "MetalnessMap"]),
    ("Decal", ["Texture", "NormalMap", "RoughnessMap", "MetalnessMap"]),
];

const PACK: &str = "TexturePackContent";

#[derive(Debug, Default)]
pub struct PackStats {
    /// Instances that have a pack, their own or one from the file.
    pub packed: usize,
    /// PBR instances with no pack yet, by class: new or re-uploaded maps that Studio has not baked.
    pub missing: Vec<(String, usize)>,
}

/// A missing file is no packs.
pub fn read_texture_packs(path: &Path) -> Result<TexturePacks, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TexturePacks::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes the whole DataModel as a place, with texture packs set on everything that has none of its own.
pub fn write_rbxl(place: &mut Place, packs: &TexturePacks, out: impl Write) -> Result<PackStats, String> {
    let (stats, set) = apply_packs(place, packs);
    let roots = place.dom.root().children().to_vec();
    let written = rbx_binary::to_writer(out, &place.dom, &roots).map_err(|e| e.to_string());
    for (r, before) in set {
        if let Some(inst) = place.dom.get_by_ref_mut(r) {
            match before {
                Some(v) => inst.properties.insert(ustr(PACK), v),
                None => inst.properties.remove(&ustr(PACK)),
            };
        }
    }
    written.map(|()| stats)
}

/// Writes `roots` and their subtrees as a model.
pub fn write_rbxm(place: &Place, roots: &[Ref], out: impl Write) -> Result<(), String> {
    rbx_binary::to_writer(out, &place.dom, roots).map_err(|e| e.to_string())
}

/// Sets packs from `packs` and returns what each touched instance held before, so the write can put it back.
fn apply_packs(place: &mut Place, packs: &TexturePacks) -> (PackStats, Vec<(Ref, Option<Variant>)>) {
    let mut stats = PackStats::default();
    let mut set = Vec::new();
    let mut missing: HashMap<&str, usize> = HashMap::new();
    for inst in place.dom.descendants() {
        let Some((class, maps)) = PACKED.iter().find(|(class, _)| inst.class == *class) else {
            continue;
        };
        let own = match inst.properties.get(&ustr(PACK)) {
            Some(Variant::Content(c)) => c.as_uri().is_some_and(|uri| !uri.is_empty()),
            _ => false,
        };
        if own {
            stats.packed += 1;
            continue;
        }
        let Some(key) = pack_key(inst, class, maps) else {
            continue;
        };
        match packs.get(&key) {
            Some(uri) => {
                stats.packed += 1;
                set.push((inst.referent(), uri.clone()));
            }
            None => *missing.entry(class).or_default() += 1,
        }
    }
    stats.missing = missing.into_iter().map(|(class, n)| (class.to_string(), n)).collect();
    stats.missing.sort();
    let mut before = Vec::with_capacity(set.len());
    for (r, uri) in set {
        if let Some(inst) = place.dom.get_by_ref_mut(r) {
            before.push((r, inst.properties.insert(ustr(PACK), Variant::Content(Content::from_uri(uri)))));
        }
    }
    (stats, before)
}

/// The pack key of a PBR instance, or None for one that would not be packed.
fn pack_key(inst: &Instance, class: &str, maps: &[&str; 4]) -> Option<String> {
    let mut key = class.to_string();
    let mut pbr = false;
    for (index, map) in maps.iter().enumerate() {
        let uri = map_uri(inst, map);
        pbr |= !uri.is_empty() && (index > 0 || class != "Decal");
        key.push('|');
        key.push_str(uri);
    }
    pbr.then_some(key)
}

/// A map is `<map>Content` (a Content) or, written the older way, `<map>` (a ContentId).
fn map_uri<'a>(inst: &'a Instance, map: &str) -> &'a str {
    if let Some(Variant::Content(c)) = inst.properties.get(&ustr(&format!("{map}Content"))) {
        return c.as_uri().unwrap_or_default();
    }
    match inst.properties.get(&ustr(map)) {
        Some(Variant::ContentId(c)) => c.as_str(),
        Some(Variant::String(s)) => s,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbx_dom_weak::InstanceBuilder;

    // The keys must match place/texture-packs.json byte for byte, or packs silently stop applying.
    #[test]
    fn pack_keys() {
        let decal = |maps: &[(&str, &str)]| {
            let mut b = InstanceBuilder::new("Decal");
            for &(map, uri) in maps {
                b = b.with_property(format!("{map}Content"), Content::from_uri(uri));
            }
            let dom = rbx_dom_weak::WeakDom::new(b);
            pack_key(dom.root(), "Decal", &PACKED[2].1)
        };
        // A colour alone is a plain decal, never packed.
        assert_eq!(decal(&[("Texture", "rbxassetid://1")]), None);
        assert_eq!(
            decal(&[("Texture", "rbxassetid://1"), ("NormalMap", "rbxassetid://2")]).as_deref(),
            Some("Decal|rbxassetid://1|rbxassetid://2||")
        );
    }
}
