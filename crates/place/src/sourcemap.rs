//! The luau-lsp sourcemap: the instance tree as `{name, className, filePaths, children}`. An instance's file is
//! what was recorded for it: a Luau source's own file, the path the compiler supplied for a compiled module, the
//! model or text file a `place/` instance came from.

use std::path::Path;

use rbx_dom_weak::types::Ref;
use serde_json::{Map, Value};

use crate::tree::Place;

/// The sourcemap of the whole DataModel. With `base`, file paths under it are written relative to it.
pub fn sourcemap_json(place: &Place, base: Option<&Path>) -> String {
    node(place, place.root(), base).to_string()
}

fn node(place: &Place, r: Ref, base: Option<&Path>) -> Value {
    let inst = place
        .dom
        .get_by_ref(r)
        .expect("sourcemap walks live instances");
    let mut map = Map::new();
    map.insert("name".into(), Value::String(inst.name.clone()));
    map.insert("className".into(), Value::String(inst.class.to_string()));
    if let Some(file) = place.file_of(r) {
        let file = base.and_then(|b| file.strip_prefix(b).ok()).unwrap_or(file);
        map.insert(
            "filePaths".into(),
            Value::Array(vec![Value::String(file.to_string_lossy().into_owned())]),
        );
    }
    if !inst.children().is_empty() {
        let children = inst
            .children()
            .iter()
            .map(|&c| node(place, c, base))
            .collect();
        map.insert("children".into(), Value::Array(children));
    }
    Value::Object(map)
}
