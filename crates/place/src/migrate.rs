//! `opossum migrate`: the one-time conversion of a Rojo-era `place/`, in place. `init.meta.json` files become
//! properties files (`place/<service>.toml`, one per service, holding its nested containers), model.json refs written
//! as `Rojo_Id` / `Rojo_Target_<Property>` attributes become relative instance paths (`"Part0": "../hull"`), and the
//! entries of opossum.toml's former `[place]` table join the properties files. The whole conversion is planned before
//! anything is written, so a conflict changes nothing, and a converted project plans no writes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::Diag;
use crate::properties::{PlaceEntry, entries_to_toml, read_properties};
use crate::read::{entries, kebab, service_for_dir};

/// A file the migration writes: its new text, or None to remove it.
pub struct Write {
    pub path: PathBuf,
    pub text: Option<String>,
}

/// The writes, by path, that convert `dir` (a project's `place/`) with `moved` (opossum.toml's `[place]` entries)
/// joining the properties files; Err with every reason it cannot. A path that meta files and `moved` both set merges
/// when they agree; one an existing properties file already sets is a conflict.
pub fn plan_migration(dir: &Path, moved: Vec<PlaceEntry>) -> Result<Vec<Write>, Vec<Diag>> {
    let root = dir.parent().unwrap_or(dir).to_path_buf();
    let mut plan = Plan { root, entries: Vec::new(), writes: Vec::new(), diags: Vec::new() };
    // no place/ yet is nothing to convert; moved entries make it
    let tops = if dir.is_dir() { entries(dir, &mut plan.diags) } else { Vec::new() };
    for (name, path, is_dir) in tops {
        if let Some(service) = service_for_dir(&name).filter(|_| is_dir) {
            plan.dir(&path, &service, true);
        }
    }
    for entry in moved {
        plan.add(entry);
    }
    let existing = read_properties(dir, &mut plan.diags);
    for entry in &plan.entries {
        if let Some(old) = existing.entries.iter().find(|old| old.path == entry.path) {
            let message = format!("{} is already set here; {} sets it too", entry.path, plan.shown(&entry.file));
            plan.diags.push(Diag::error("migrate.clash", &old.file, message));
        }
    }
    if !plan.diags.is_empty() {
        return Err(plan.diags);
    }

    // each entry goes to its service's file, added after what an existing one holds
    let mut files: Vec<(PathBuf, Vec<&PlaceEntry>)> = Vec::new();
    for entry in &plan.entries {
        let service = entry.path.split('/').next().unwrap_or_default();
        let file = dir.join(format!("{}.toml", kebab(service)));
        match files.iter_mut().find(|(f, _)| *f == file) {
            Some((_, list)) => list.push(entry),
            None => files.push((file, vec![entry])),
        }
    }
    let mut writes = plan.writes;
    for (file, list) in files {
        let tables = entries_to_toml(&list).map_err(|e| vec![Diag::error("migrate.meta", &file, e)])?;
        let text = match fs::read_to_string(&file) {
            Ok(old) => format!("{}\n\n{tables}", old.trim_end()),
            Err(_) => tables,
        };
        writes.push(Write { path: file, text: Some(text) });
    }
    writes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(writes)
}

struct Plan {
    /// The project root, which messages name files from.
    root: PathBuf,
    entries: Vec<PlaceEntry>,
    writes: Vec<Write>,
    diags: Vec<Diag>,
}

impl Plan {
    /// A directory of `place/` that makes the instance at `path`; `top` for a service's own directory, whose meta
    /// className is the service's and never kept.
    fn dir(&mut self, dir: &Path, path: &str, top: bool) {
        // the directory's own entry before its children's
        let meta = dir.join("init.meta.json");
        if meta.is_file() {
            self.meta(&meta, path, top);
        }
        for (name, file, is_dir) in entries(dir, &mut self.diags) {
            if name.starts_with('.') || name == "init.meta.json" {
                continue;
            }
            if is_dir {
                self.dir(&file, &format!("{path}/{name}"), false);
            } else if name.ends_with(".meta.json") {
                let message = "only a directory's init.meta.json converts; move its properties by hand";
                self.diags.push(Diag::error("migrate.meta", &file, message));
            } else if name.ends_with(".model.json") {
                self.model(&file);
            }
        }
    }

    /// An `init.meta.json`: its properties, and a className below the service level (where a directory would
    /// otherwise be a Folder), become an entry; the file goes.
    fn meta(&mut self, file: &Path, path: &str, top: bool) {
        let json = fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
        let json = match json {
            Ok(json) => json,
            Err(e) => return self.diags.push(Diag::error("migrate.meta", file, e)),
        };
        if let Value::Object(fields) = &json {
            for key in fields.keys().filter(|k| !matches!(k.as_str(), "className" | "properties")) {
                self.diags.push(Diag::error("migrate.meta", file, format!("{key} has no properties-file form")));
            }
        }
        let class = json["className"].as_str().filter(|&c| !top && c != "Folder");
        let props = match &json["properties"] {
            Value::Object(props) => props.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => Vec::new(),
        };
        if class.is_some() || !props.is_empty() {
            let class = class.map(str::to_string);
            self.add(PlaceEntry { path: path.to_string(), class, props, file: file.to_path_buf() });
        }
        self.writes.push(Write { path: file.to_path_buf(), text: None });
    }

    /// A model.json with Rojo refs, rewritten with paths.
    fn model(&mut self, file: &Path) {
        // a model that does not parse is the reader's to report, and holds nothing to convert
        let Ok(mut json) =
            fs::read_to_string(file).map_err(drop).and_then(|t| serde_json::from_str::<Value>(&t).map_err(drop))
        else {
            return;
        };
        if rojo_attributes(&json) == 0 {
            return;
        }
        match convert_rojo_refs(&mut json) {
            Ok(_) => self.writes.push(Write { path: file.to_path_buf(), text: Some(model_json_text(&json)) }),
            Err(e) => self.diags.push(Diag::error("migrate.ref", file, e)),
        }
    }

    fn shown(&self, file: &Path) -> String {
        file.strip_prefix(&self.root).unwrap_or(file).display().to_string()
    }

    /// Adds an entry, merging it into one for the same path when the two agree.
    fn add(&mut self, entry: PlaceEntry) {
        let Some(at) = self.entries.iter().position(|e| e.path == entry.path) else {
            return self.entries.push(entry);
        };
        let there = self.shown(&self.entries[at].file);
        let have = &mut self.entries[at];
        let mut clashes = Vec::new();
        if let (Some(a), Some(b)) = (&have.class, &entry.class)
            && a != b
        {
            clashes.push(format!("{} is a {b} here but a {a} in {there}", entry.path));
        }
        for (name, value) in &entry.props {
            match have.props.iter().find(|(n, _)| n == name) {
                Some((_, v)) if v != value => {
                    clashes.push(format!("{}.{name} is {value} here but {v} in {there}", entry.path));
                }
                Some(_) => {}
                None => have.props.push((name.clone(), value.clone())),
            }
        }
        if have.class.is_none() {
            have.class = entry.class.clone();
        }
        self.diags.extend(clashes.into_iter().map(|m| Diag::error("migrate.clash", &entry.file, m)));
    }
}

/// Whether a model.json attribute is one of Rojo's ref attributes.
fn is_rojo(key: &str) -> bool {
    key == "Rojo_Id" || key.starts_with("Rojo_Target_")
}

/// The Rojo ref attributes in a model.json, its children's included.
pub(crate) fn rojo_attributes(node: &Value) -> usize {
    let own = node["attributes"].as_object().map_or(0, |a| a.keys().filter(|k| is_rojo(k)).count());
    let children = node["children"].as_array().map_or(0, |c| c.iter().map(rojo_attributes).sum());
    own + children
}

/// Rewrites a Rojo model.json in place: every `Rojo_Target_X = id` attribute becomes property `X`, a path from
/// the holder to the node whose `Rojo_Id` is `id`, and the Rojo attributes are dropped. Returns the number of
/// refs converted. Fails without touching the model when an id is missing or duplicated, a target is only
/// reachable through a name shared by siblings, or the property is already set.
fn convert_rojo_refs(model: &mut Value) -> Result<usize, String> {
    let mut ids: HashMap<String, Vec<usize>> = HashMap::new();
    let mut targets: Vec<(Vec<usize>, String, String)> = Vec::new();
    let mut stack = vec![(Vec::new(), &*model)];
    while let Some((at, node)) = stack.pop() {
        if let Value::Object(attributes) = &node["attributes"] {
            for (key, value) in attributes {
                let Some(id) = value.as_str().filter(|_| is_rojo(key)) else {
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
        let target = ids.get(id).ok_or(format!("{prop} targets Rojo_Id {id}, which nothing has"))?;
        let props = &node_at(model, holder)["properties"];
        if !(props.is_null() || props.is_object()) || !props[prop.as_str()].is_null() {
            return Err(format!("{prop} is set both as a property and as Rojo_Target_{prop}"));
        }
        let common = holder.iter().zip(target).take_while(|(a, b)| a == b).count();
        let mut segments = vec![".."; holder.len() - common];
        let mut parent = node_at(model, &target[..common]);
        for &i in &target[common..] {
            let children = parent["children"].as_array().expect("index paths follow children");
            let name = children[i]["name"].as_str().ok_or("a child has no name")?;
            if children.iter().filter(|c| c["name"].as_str() == Some(name)).count() > 1 {
                return Err(format!("{prop} targets {name}, a name shared by its siblings"));
            }
            segments.push(name);
            parent = &children[i];
        }
        let path = if segments.is_empty() { ".".to_string() } else { segments.join("/") };
        paths.push(path);
    }

    for ((holder, prop, _), path) in targets.iter().zip(&paths) {
        let node = node_at_mut(model, holder).as_object_mut().expect("nodes are objects");
        let props = node.entry("properties").or_insert_with(|| Value::Object(Map::new()));
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
        attributes.retain(|k, _| !is_rojo(k));
        if attributes.is_empty() {
            fields.shift_remove("attributes");
        }
    }
    if let Some(Value::Array(children)) = fields.get_mut("children") {
        children.iter_mut().for_each(strip_rojo_attributes);
    }
}

/// model.json text the way the art tools write it: tab indents, arrays of plain values on one line.
fn model_json_text(model: &Value) -> String {
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
        assert_eq!(model["children"][0]["attributes"], serde_json::json!({ "keep": 1 }));
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
