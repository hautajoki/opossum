//! One-time conversions from a Rojo project: `init.meta.json` files into `[place]` entries, and model.json refs
//! written as `Rojo_Id` / `Rojo_Target_<Property>` attributes into relative instance paths (`"Part0": "../hull"`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::Diag;
use crate::config::{PlaceConfig, PlaceEntry};
use crate::read::service_for_dir;

/// The `[place]` entries that a Rojo `place/` tree's `init.meta.json` files describe. A meta file's className
/// is kept only below the service level, where a directory would otherwise be a Folder.
pub fn config_from_meta(dir: &Path) -> (PlaceConfig, Vec<Diag>) {
    let mut config = Vec::new();
    let mut diags = Vec::new();
    let mut tops: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    tops.sort();
    for top in tops.iter().filter(|p| p.is_dir()) {
        let name = top.file_name().unwrap_or_default().to_string_lossy();
        if let Some(service) = service_for_dir(&name) {
            meta_dir(top, &service, true, &mut config, &mut diags);
        }
    }
    (config, diags)
}

fn meta_dir(dir: &Path, path: &str, top: bool, config: &mut PlaceConfig, diags: &mut Vec<Diag>) {
    let meta = dir.join("init.meta.json");
    if meta.is_file() {
        match fs::read_to_string(&meta)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
        {
            Ok(json) => {
                let mut entry = PlaceEntry {
                    path: path.to_string(),
                    class: None,
                    props: Vec::new(),
                };
                let class = json["className"].as_str();
                if !top && class.is_some_and(|c| c != "Folder") {
                    entry.class = class.map(str::to_string);
                }
                if let Value::Object(props) = &json["properties"] {
                    entry.props = props.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                }
                if let Value::Object(fields) = &json {
                    for key in fields
                        .keys()
                        .filter(|k| !matches!(k.as_str(), "className" | "properties"))
                    {
                        diags.push(Diag::error(
                            "migrate.meta",
                            &meta,
                            format!("{key} has no [place] form"),
                        ));
                    }
                }
                if entry.class.is_some() || !entry.props.is_empty() {
                    config.push(entry);
                }
            }
            Err(e) => diags.push(Diag::error("migrate.meta", &meta, e)),
        }
    }
    let mut subdirs: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    for sub in subdirs {
        let name = sub
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        meta_dir(&sub, &format!("{path}/{name}"), false, config, diags);
    }
}

/// Rewrites a Rojo model.json in place: every `Rojo_Target_X = id` attribute becomes property `X`, a path from
/// the holder to the node whose `Rojo_Id` is `id`, and the Rojo attributes are dropped. Returns the number of
/// refs converted. Fails without touching the model when an id is missing or duplicated, a target is only
/// reachable through a name shared by siblings, or the property is already set.
pub fn convert_rojo_refs(model: &mut Value) -> Result<usize, String> {
    let mut ids: HashMap<String, Vec<usize>> = HashMap::new();
    let mut targets: Vec<(Vec<usize>, String, String)> = Vec::new();
    let mut stack = vec![(Vec::new(), &*model)];
    while let Some((at, node)) = stack.pop() {
        if let Value::Object(attributes) = &node["attributes"] {
            for (key, value) in attributes {
                let rojo = key == "Rojo_Id" || key.starts_with("Rojo_Target_");
                let Some(id) = value.as_str().filter(|_| rojo) else {
                    continue;
                };
                match key.strip_prefix("Rojo_Target_") {
                    Some(prop) => targets.push((at.clone(), prop.to_string(), id.to_string())),
                    None if ids.contains_key(id) => {
                        return Err(format!("Rojo_Id {id} is used twice"));
                    }
                    None => {
                        ids.insert(id.to_string(), at.clone());
                    }
                }
            }
        }
        if let Value::Array(children) = &node["children"] {
            for (i, child) in children.iter().enumerate() {
                let mut path = at.clone();
                path.push(i);
                stack.push((path, child));
            }
        }
    }

    let mut paths = Vec::with_capacity(targets.len());
    for (holder, prop, id) in &targets {
        let target = ids
            .get(id)
            .ok_or(format!("{prop} targets Rojo_Id {id}, which nothing has"))?;
        let props = &node_at(model, holder)["properties"];
        if !(props.is_null() || props.is_object()) || !props[prop.as_str()].is_null() {
            return Err(format!(
                "{prop} is set both as a property and as Rojo_Target_{prop}"
            ));
        }
        let common = holder
            .iter()
            .zip(target)
            .take_while(|(a, b)| a == b)
            .count();
        let mut segments = vec![".."; holder.len() - common];
        let mut parent = node_at(model, &target[..common]);
        for &i in &target[common..] {
            let children = parent["children"]
                .as_array()
                .expect("index paths follow children");
            let name = children[i]["name"].as_str().ok_or("a child has no name")?;
            if children
                .iter()
                .filter(|c| c["name"].as_str() == Some(name))
                .count()
                > 1
            {
                return Err(format!(
                    "{prop} targets {name}, a name shared by its siblings"
                ));
            }
            segments.push(name);
            parent = &children[i];
        }
        let path = if segments.is_empty() {
            ".".to_string()
        } else {
            segments.join("/")
        };
        paths.push(path);
    }

    for ((holder, prop, _), path) in targets.iter().zip(&paths) {
        let node = node_at_mut(model, holder)
            .as_object_mut()
            .expect("nodes are objects");
        let props = node
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(props) = props.as_object_mut() {
            props.insert(prop.clone(), Value::String(path.clone()));
        }
    }
    strip_rojo_attributes(model);
    Ok(targets.len())
}

fn node_at<'a>(model: &'a Value, path: &[usize]) -> &'a Value {
    path.iter().fold(model, |node, &i| &node["children"][i])
}

fn node_at_mut<'a>(model: &'a mut Value, path: &[usize]) -> &'a mut Value {
    path.iter().fold(model, |node, &i| &mut node["children"][i])
}

fn strip_rojo_attributes(node: &mut Value) {
    let Value::Object(fields) = node else { return };
    if let Some(Value::Object(attributes)) = fields.get_mut("attributes") {
        attributes.retain(|k, _| k != "Rojo_Id" && !k.starts_with("Rojo_Target_"));
        if attributes.is_empty() {
            fields.shift_remove("attributes");
        }
    }
    if let Some(Value::Array(children)) = fields.get_mut("children") {
        children.iter_mut().for_each(strip_rojo_attributes);
    }
}

/// model.json text the way the art tools write it: tab indents, arrays of plain values on one line.
pub fn model_json_text(model: &Value) -> String {
    let mut out = String::new();
    write_json(model, 0, &mut out);
    out.push('\n');
    out
}

fn write_json(value: &Value, depth: usize, out: &mut String) {
    let indent = |out: &mut String, depth: usize| out.extend(std::iter::repeat_n('\t', depth));
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push_str("{\n");
            for (i, (key, v)) in map.iter().enumerate() {
                indent(out, depth + 1);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write_json(v, depth + 1, out);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push('}');
        }
        Value::Array(items) if items.iter().any(|v| v.is_object() || v.is_array()) => {
            out.push_str("[\n");
            for (i, v) in items.iter().enumerate() {
                indent(out, depth + 1);
                write_json(v, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push(']');
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(Value::to_string).collect();
            out.push('[');
            out.push_str(&items.join(", "));
            out.push(']');
        }
        v => out.push_str(&v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rojo_refs_become_paths_from_the_holder() {
        let mut model: Value = serde_json::from_str(
            r#"{
                "className": "Model",
                "attributes": { "Rojo_Id": "box" },
                "children": [
                    { "name": "hull", "className": "Part", "attributes": { "Rojo_Id": "box.hull", "keep": 1 }, "children": [
                        { "name": "Lid", "className": "Part", "attributes": { "Rojo_Id": "box.lid" } },
                        { "name": "weld", "className": "Weld", "attributes": {
                            "Rojo_Target_Part0": "box.hull", "Rojo_Target_Part1": "box.lid" } }
                    ] },
                    { "name": "hinge", "className": "Weld", "attributes": {
                        "Rojo_Target_Part0": "box", "Rojo_Target_Part1": "box.lid" } }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(convert_rojo_refs(&mut model), Ok(4));
        let weld = &model["children"][0]["children"][1];
        assert_eq!(weld["properties"]["Part0"], "..");
        assert_eq!(weld["properties"]["Part1"], "../Lid");
        assert!(weld.get("attributes").is_none());
        let hinge = &model["children"][1];
        assert_eq!(hinge["properties"]["Part0"], "..");
        assert_eq!(hinge["properties"]["Part1"], "../hull/Lid");
        assert_eq!(
            model["children"][0]["attributes"],
            serde_json::json!({ "keep": 1 })
        );
        assert!(model.get("attributes").is_none());
    }

    #[test]
    fn a_ref_through_a_shared_name_is_refused() {
        let mut model: Value = serde_json::from_str(
            r#"{ "className": "Model", "children": [
                { "name": "a", "className": "Part", "attributes": { "Rojo_Id": "first" } },
                { "name": "a", "className": "Part" },
                { "name": "w", "className": "Weld", "attributes": { "Rojo_Target_Part0": "first" } }
            ] }"#,
        )
        .unwrap();
        let before = model.clone();
        assert!(convert_rojo_refs(&mut model).is_err());
        assert_eq!(model, before);
    }
}
