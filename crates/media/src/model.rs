//! Model files: the part names a glb goes up with. Roblox's import makes a MeshPart of every node holding a mesh,
//! named after it, and the read-back matches those names, so a name the import might change is refused here, before
//! anything is sent. Only the JSON chunk is read.

use serde_json::Value;

/// The part names of a glb (binary glTF 2.0) in node order: every node holding a mesh is a part. Refused: no node
/// with a mesh; a part name outside `[A-Za-z0-9_-]+` or used twice; a node whose mesh is named otherwise (which of
/// the two the import names the MeshPart after was never measured); a mesh of more than one primitive, which the
/// import splits into MeshParts under names nobody measured.
pub fn glb(bytes: &[u8]) -> Result<Vec<String>, String> {
    let gltf: Value =
        serde_json::from_slice(json_chunk(bytes)?).map_err(|e| format!("its JSON chunk does not parse: {e}"))?;
    let meshes = gltf["meshes"].as_array().map_or(&[][..], Vec::as_slice);
    let mut parts: Vec<String> = Vec::new();
    for node in gltf["nodes"].as_array().map_or(&[][..], Vec::as_slice) {
        let Some(m) = node["mesh"].as_u64() else { continue };
        let name = node["name"].as_str().unwrap_or_default();
        let Some(mesh) = meshes.get(m as usize) else {
            return Err(format!("node `{name}` holds mesh {m}, which the file does not have"));
        };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(format!("rename `{name}`: a part is named with letters, digits, `_` and `-`"));
        }
        if parts.iter().any(|p| p == name) {
            return Err(format!("two parts are named `{name}`: name each part once"));
        }
        let mesh_name = mesh["name"].as_str().unwrap_or_default();
        if mesh_name != name {
            return Err(format!("node `{name}` holds mesh `{mesh_name}`: name node and mesh alike"));
        }
        let primitives = mesh["primitives"].as_array().map_or(0, Vec::len);
        if primitives > 1 {
            return Err(format!("{name} has {primitives} materials: split it per material"));
        }
        parts.push(name.to_owned());
    }
    if parts.is_empty() {
        return Err("no node holds a mesh".into());
    }
    Ok(parts)
}

/// The JSON chunk of a glb: a 12-byte header (`glTF`, version, length), then the first chunk's length, its type
/// `JSON` and its bytes.
fn json_chunk(bytes: &[u8]) -> Result<&[u8], String> {
    let word = |at: usize| bytes.get(at..at + 4).map(|w| u32::from_le_bytes(w.try_into().expect("four bytes")));
    if !bytes.starts_with(b"glTF") {
        return Err("not a glb (binary glTF)".into());
    }
    if word(4) != Some(2) {
        return Err("not glTF 2.0: export a glTF 2.0 glb".into());
    }
    if word(16) != Some(u32::from_le_bytes(*b"JSON")) {
        return Err("its first chunk is not JSON".into());
    }
    let len = word(12).expect("read with the type after it") as usize;
    bytes.get(20..20 + len).ok_or_else(|| "the file is cut short".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glb holding `json` and no binary chunk, padded as the format pads it.
    fn file(json: &str) -> Vec<u8> {
        let mut chunk = json.as_bytes().to_vec();
        chunk.resize(chunk.len().next_multiple_of(4), b' ');
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend((20 + chunk.len() as u32).to_le_bytes());
        out.extend((chunk.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(chunk);
        out
    }

    /// One node per `(node, mesh, primitives)`, each holding its own mesh, plus a mesh-less root node.
    fn model(parts: &[(&str, &str, usize)]) -> Vec<u8> {
        let mut nodes = vec![r#"{"name":"Armature"}"#.to_owned()];
        nodes.extend(parts.iter().enumerate().map(|(i, (node, _, _))| format!(r#"{{"name":"{node}","mesh":{i}}}"#)));
        let meshes: Vec<String> = parts
            .iter()
            .map(|(_, mesh, n)| format!(r#"{{"name":"{mesh}","primitives":[{}]}}"#, vec!["{}"; *n].join(",")))
            .collect();
        file(&format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{}],"meshes":[{}]}}"#,
            nodes.join(","),
            meshes.join(",")
        ))
    }

    #[test]
    fn parts_are_the_mesh_nodes_in_node_order() {
        let ok = model(&[("hull", "hull", 1), ("Lens", "Lens", 1), ("lens-cap", "lens-cap", 1)]);
        assert_eq!(glb(&ok).unwrap(), ["hull", "Lens", "lens-cap"]);
    }

    #[test]
    fn names_the_import_might_change_are_refused() {
        let cases = [
            (model(&[("Cube.001", "Cube.001", 1)]), "rename `Cube.001`: a part is named with letters, digits"),
            (model(&[("Lens", "Lens", 1), ("Lens", "Lens", 1)]), "two parts are named `Lens`"),
            (model(&[("node_9", "mesh_9", 1)]), "node `node_9` holds mesh `mesh_9`: name node and mesh alike"),
            (model(&[("Hull", "Hull", 2)]), "Hull has 2 materials: split it per material"),
            (model(&[]), "no node holds a mesh"),
            (b"glTF\x01\0\0\0".to_vec(), "not glTF 2.0"),
            (file("{}")[..22].to_vec(), "cut short"),
        ];
        for (bytes, want) in cases {
            let got = glb(&bytes).unwrap_err();
            assert!(got.contains(want), "{got:?} lacks {want:?}");
        }
    }
}
