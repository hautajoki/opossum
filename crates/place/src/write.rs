//! `.rbxl`/`.rbxm` output through rbx_binary, with texture packs written into the place.
//!
//! Studio bakes each SurfaceAppearance's and MaterialVariant's maps into one texture pack asset
//! (TexturePackContent), and a live server draws PBR only from that pack. A place published through Open Cloud has
//! none, so the packs Studio baked are kept in opossum.lock, keyed `Class|map uri|map uri|...` (the class's maps in
//! order, colour first), and every place write sets them. `opossum dev` captures them: Studio serializes what it baked,
//! and [`texture_packs`] reads each key's pack back out.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;

use rbx_dom_weak::types::{Content, Ref, Variant};
use rbx_dom_weak::{Instance, ustr};

use crate::read::Place;

/// Pack key -> the pack's asset id, as opossum.lock's `packs.` lines hold them.
pub type TexturePacks = BTreeMap<String, u64>;

/// Each packed class's maps, its colour first. PBR decals are left out: Studio never bakes them in edit mode, so no
/// Decal pack can be kept.
pub(crate) const PACKED: &[(&str, [&str; 4])] = &[
    ("SurfaceAppearance", ["ColorMap", "NormalMap", "RoughnessMap", "MetalnessMap"]),
    ("MaterialVariant", ["ColorMap", "NormalMap", "RoughnessMap", "MetalnessMap"]),
];

const PACK: &str = "TexturePackContent";

/// A SurfaceAppearance or MaterialVariant with maps, as `plan` reports packs and dev captures them.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub class: &'static str,
    /// Its pack key, as opossum.lock's `packs.` lines hold it; None while a map's `$asset` has no upload or read yet,
    /// so the key it will have is not known.
    pub key: Option<String>,
    /// It holds a pack of its own (one Studio baked into a model file), which the lock's never replaces.
    pub packed: bool,
    /// The file it was read from.
    pub file: PathBuf,
}

#[derive(Debug, Default)]
pub struct PackStats {
    /// Instances that have a pack, their own or one from the file.
    pub packed: usize,
    /// PBR instances with no pack yet, by class: new or re-uploaded maps that Studio has not baked.
    pub missing: Vec<(String, usize)>,
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
        if own_pack(inst).is_some() {
            stats.packed += 1;
            continue;
        }
        let Some(key) = pack_key(inst, class, maps) else {
            continue;
        };
        match packs.get(&key) {
            Some(&id) => {
                stats.packed += 1;
                set.push((inst.referent(), id));
            }
            None => *missing.entry(class).or_default() += 1,
        }
    }
    stats.missing = missing.into_iter().map(|(class, n)| (class.to_string(), n)).collect();
    stats.missing.sort();
    let mut before = Vec::with_capacity(set.len());
    for (r, id) in set {
        if let Some(inst) = place.dom.get_by_ref_mut(r) {
            let pack = Variant::Content(Content::from_uri(format!("rbxassetid://{id}")));
            before.push((r, inst.properties.insert(ustr(PACK), pack)));
        }
    }
    (stats, before)
}

/// The packs Studio baked, from what `SerializeInstancesAsync` made of the instances holding them (an `.rbxm`): each
/// SurfaceAppearance's and MaterialVariant's key, computed from its own serialized maps, and the id of its
/// `TexturePackContent`. One Studio has not baked yet is left out.
pub fn texture_packs(bytes: &[u8]) -> Result<Vec<(String, u64)>, String> {
    let model = rbx_binary::from_reader(bytes).map_err(|e| e.to_string())?;
    let mut packs = Vec::new();
    for inst in model.descendants() {
        let Some((class, maps)) = PACKED.iter().find(|(class, _)| inst.class == *class) else {
            continue;
        };
        let pack = own_pack(inst).and_then(|uri| uri.strip_prefix("rbxassetid://")?.parse().ok());
        if let (Some(key), Some(id)) = (pack_key(inst, class, maps), pack) {
            packs.push((key, id));
        }
    }
    Ok(packs)
}

/// The uri of the pack an instance holds itself, None when it holds none.
pub(crate) fn own_pack(inst: &Instance) -> Option<&str> {
    match inst.properties.get(&ustr(PACK)) {
        Some(Variant::Content(c)) => c.as_uri().filter(|uri| !uri.is_empty()),
        _ => None,
    }
}

/// The pack key of an instance with maps, or None for one with none.
pub(crate) fn pack_key(inst: &Instance, class: &str, maps: &[&str; 4]) -> Option<String> {
    let mut key = class.to_string();
    let mut pbr = false;
    for map in maps {
        let uri = map_uri(inst, map);
        pbr |= !uri.is_empty();
        key.push('|');
        key.push_str(uri);
    }
    pbr.then_some(key)
}

/// A map is `<map>Content` (a Content) or, written the older way, `<map>` (a ContentId).
pub(crate) fn map_uri<'a>(inst: &'a Instance, map: &str) -> &'a str {
    content_uri(inst, &format!("{map}Content"), map)
}

/// The uri of Content property `content`, else of its older ContentId spelling `legacy` (`MeshContent`, `MeshId`).
pub(crate) fn content_uri<'a>(inst: &'a Instance, content: &str, legacy: &str) -> &'a str {
    if let Some(Variant::Content(c)) = inst.properties.get(&ustr(content)) {
        return c.as_uri().unwrap_or_default();
    }
    match inst.properties.get(&ustr(legacy)) {
        Some(Variant::ContentId(c)) => c.as_str(),
        Some(Variant::String(s)) => s,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbx_dom_weak::InstanceBuilder;
    use rbx_dom_weak::types::ContentId;

    // The keys must match the `packs.` lines Studio's bakes were captured into byte for byte, or packs silently stop
    // applying; a map in the older ContentId spelling keys as its Content does.
    #[test]
    fn pack_keys() {
        let sa = |content: &[(&str, &str)], id: &[(&str, &str)]| {
            let mut b = InstanceBuilder::new("SurfaceAppearance");
            for &(map, uri) in content {
                b = b.with_property(format!("{map}Content"), Content::from_uri(uri));
            }
            for &(map, uri) in id {
                b = b.with_property(map, ContentId::from(uri));
            }
            let dom = rbx_dom_weak::WeakDom::new(b);
            pack_key(dom.root(), "SurfaceAppearance", &PACKED[0].1)
        };
        assert_eq!(sa(&[], &[]), None);
        assert_eq!(
            sa(&[("ColorMap", "rbxassetid://1")], &[("NormalMap", "rbxassetid://2")]).as_deref(),
            Some("SurfaceAppearance|rbxassetid://1|rbxassetid://2||")
        );
    }

    // what Studio serializes for dev's capture: a surface's key comes from its own maps, written either way, beside the
    // id of the pack it holds; one Studio has not baked yet gives nothing
    #[test]
    fn texture_packs_read_each_baked_surface_key_and_pack() {
        let mut dom = rbx_dom_weak::WeakDom::new(InstanceBuilder::new("Folder"));
        let root = dom.root_ref();
        let baked = InstanceBuilder::new("SurfaceAppearance")
            .with_property("ColorMapContent", Content::from_uri("rbxassetid://11"))
            .with_property("RoughnessMap", ContentId::from("rbxassetid://13"))
            .with_property(PACK, Content::from_uri("rbxassetid://83799176238290"));
        let variant = InstanceBuilder::new("MaterialVariant")
            .with_property("NormalMapContent", Content::from_uri("rbxassetid://22"))
            .with_property(PACK, Content::from_uri("rbxassetid://7"));
        let unbaked = InstanceBuilder::new("SurfaceAppearance")
            .with_property("ColorMapContent", Content::from_uri("rbxassetid://31"))
            .with_property(PACK, Content::none());
        let roots = [dom.insert(root, baked), dom.insert(root, variant), dom.insert(root, unbaked)];
        let mut bytes = Vec::new();
        rbx_binary::to_writer(&mut bytes, &dom, &roots).unwrap();
        assert_eq!(
            texture_packs(&bytes).unwrap(),
            [
                ("SurfaceAppearance|rbxassetid://11||rbxassetid://13|".to_owned(), 83799176238290),
                ("MaterialVariant||rbxassetid://22||".to_owned(), 7),
            ]
        );
    }
}
