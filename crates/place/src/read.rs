//! The `place/` folder reader. Each top-level directory is a service named in kebab case (`replicated-storage`
//! is ReplicatedStorage; `starter-player-scripts` is StarterPlayer's StarterPlayerScripts). Inside, directories
//! are Folders, `.model.json` files are instance trees, `.rbxm`/`.rbxmx` files are Roblox models and `.txt`
//! files are StringValues. `place/assets` (tessera's upload root) and loose top-level files are not content.

use std::fs;
use std::path::{Path, PathBuf};

use rbx_dom_weak::InstanceBuilder;
use rbx_dom_weak::types::{Ref, Variant};
use rbx_dom_weak::ustr;
use rbx_reflection::ClassTag;
use serde_json::Value;

use crate::config::PlaceConfig;
use crate::tree::{NESTED, Place};
use crate::value::{self, Resolved, db};
use crate::{Diag, Severity};

/// The instance path a top-level `place/` directory stands for, or None when it names no service.
pub fn service_for_dir(dir: &str) -> Option<String> {
    for &(parent, child) in NESTED {
        if kebab(child) == dir {
            return Some(format!("{parent}/{child}"));
        }
    }
    let class = db()
        .classes
        .values()
        .find(|c| c.tags.contains(&ClassTag::Service) && kebab(c.name) == dir)?;
    Some(class.name.to_string())
}

/// `ReplicatedStorage` -> `replicated-storage`, `VRService` -> `vr-service`.
fn kebab(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_lower)
            {
                out.push('-');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

struct PendingRef {
    holder: Ref,
    property: &'static str,
    path: String,
    file: PathBuf,
}

struct Reader {
    place: Place,
    diags: Vec<Diag>,
    refs: Vec<PendingRef>,
}

/// Reads `dir` (a project's `place/`) into a fresh DataModel, applies the `[place]` properties and resolves
/// every Ref. Problems come back as diagnostics; whatever could be read is in the place.
pub fn read_place(dir: &Path, config: &PlaceConfig) -> (Place, Vec<Diag>) {
    let mut r = Reader {
        place: Place::new(),
        diags: Vec::new(),
        refs: Vec::new(),
    };
    for (name, path, is_dir) in entries(dir, &mut r.diags) {
        if !is_dir || name.starts_with('.') || name == "assets" {
            continue;
        }
        let Some(service_path) = service_for_dir(&name) else {
            r.diags.push(Diag::error(
                "place.unknownService",
                &path,
                format!("{name} names no service"),
            ));
            continue;
        };
        match r.instance_at(&service_path, None) {
            Ok(service) => r.dir(&path, service),
            Err(e) => r.diags.push(Diag::error("place.unknownService", &path, e)),
        }
    }
    r.apply(config);
    r.resolve_refs();
    settle_pivots(&mut r.place);
    (r.place, r.diags)
}

/// Models authored outside Studio already have their pivots where they belong; without NeedsPivotMigration
/// = false Studio would move them (Workspace included) on load.
fn settle_pivots(place: &mut Place) {
    let db = db();
    let Some(model) = db.classes.get("Model") else {
        return;
    };
    let models: Vec<Ref> = place
        .dom
        .descendants()
        .filter(|i| !i.properties.contains_key(&ustr("NeedsPivotMigration")))
        .filter(|i| {
            db.classes
                .get(i.class.as_str())
                .is_some_and(|c| db.has_superclass(c, model))
        })
        .map(|i| i.referent())
        .collect();
    for r in models {
        if let Some(inst) = place.dom.get_by_ref_mut(r) {
            inst.properties
                .insert(ustr("NeedsPivotMigration"), Variant::Bool(false));
        }
    }
}

/// Directory entries sorted by name: (name, path, is_dir).
fn entries(dir: &Path, diags: &mut Vec<Diag>) -> Vec<(String, PathBuf, bool)> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) => {
            diags.push(Diag::error("place.io", dir, e.to_string()));
            return Vec::new();
        }
    };
    let mut out: Vec<(String, PathBuf, bool)> = read
        .filter_map(Result::ok)
        .map(|e| {
            let path = e.path();
            (
                e.file_name().to_string_lossy().into_owned(),
                path.clone(),
                path.is_dir(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

impl Reader {
    fn dir(&mut self, dir: &Path, parent: Ref) {
        for (name, path, is_dir) in entries(dir, &mut self.diags) {
            if name.starts_with('.') {
                continue;
            }
            if is_dir {
                let folder = self
                    .place
                    .dom
                    .insert(parent, InstanceBuilder::new("Folder").with_name(&name));
                self.dir(&path, folder);
            } else if let Some(stem) = name.strip_suffix(".model.json") {
                self.model_file(&path, stem, parent);
            } else if let Some(stem) = name.strip_suffix(".rbxm") {
                self.roblox_model(&path, stem, parent, false);
            } else if let Some(stem) = name.strip_suffix(".rbxmx") {
                self.roblox_model(&path, stem, parent, true);
            } else if let Some(stem) = name.strip_suffix(".txt") {
                match fs::read_to_string(&path) {
                    Ok(text) => {
                        let value = InstanceBuilder::new("StringValue")
                            .with_name(stem)
                            .with_property("Value", text);
                        let r = self.place.dom.insert(parent, value);
                        self.place.set_file(r, path);
                    }
                    Err(e) => self
                        .diags
                        .push(Diag::error("place.io", &path, e.to_string())),
                }
            } else if name.ends_with(".meta.json") {
                self.diags.push(Diag::note(
                    "place.ignored",
                    &path,
                    "properties come from [place] in opossum.toml",
                ));
            } else {
                self.diags
                    .push(Diag::note("place.ignored", &path, "not place content"));
            }
        }
    }

    fn model_file(&mut self, path: &Path, name: &str, parent: Ref) {
        let json = fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                serde_json::from_str::<Value>(&text).map_err(|e| format!("invalid JSON: {e}"))
            });
        match json {
            Ok(json) => {
                if let Some(r) = self.model(&json, name, parent, path) {
                    self.place.set_file(r, path.to_path_buf());
                }
            }
            Err(e) => self.diags.push(Diag::error("place.badModel", path, e)),
        }
    }

    /// One model.json node and its children, inserted under `parent`.
    fn model(&mut self, json: &Value, name: &str, parent: Ref, file: &Path) -> Option<Ref> {
        let Some(class) = json["className"].as_str() else {
            self.diags.push(Diag::error(
                "place.badModel",
                file,
                format!("{name} has no className"),
            ));
            return None;
        };
        if !value::is_class(class) {
            self.diags.push(Diag::error(
                "place.badModel",
                file,
                format!("{name}: unknown class {class}"),
            ));
            return None;
        }
        let r = self
            .place
            .dom
            .insert(parent, InstanceBuilder::new(class).with_name(name));
        if let Value::Object(fields) = json {
            for key in fields.keys() {
                if !matches!(
                    key.as_str(),
                    "name" | "className" | "properties" | "attributes" | "children" | "$schema"
                ) {
                    self.diags.push(Diag {
                        code: "place.unknownKey",
                        severity: Severity::Warning,
                        path: file.to_path_buf(),
                        message: format!("{name}: unknown key {key}"),
                    });
                }
            }
        }
        if let Value::Object(props) = &json["properties"] {
            for (prop, value) in props {
                if let Err(e) = self.set_property(r, class, prop, value, file) {
                    self.diags.push(Diag::error(
                        "place.badProperty",
                        file,
                        format!("{name}.{prop}: {e}"),
                    ));
                }
            }
        }
        if !json["attributes"].is_null() {
            match value::attributes(&json["attributes"]) {
                Ok(attributes) => self.set(r, "Attributes", Variant::Attributes(attributes)),
                Err(e) => self.diags.push(Diag::error(
                    "place.badProperty",
                    file,
                    format!("{name}: {e}"),
                )),
            }
        }
        if let Value::Array(children) = &json["children"] {
            for child in children {
                let Some(child_name) = child["name"].as_str() else {
                    self.diags.push(Diag::error(
                        "place.badModel",
                        file,
                        format!("a child of {name} has no name"),
                    ));
                    continue;
                };
                self.model(child, child_name, r, file);
            }
        }
        Some(r)
    }

    fn set_property(
        &mut self,
        r: Ref,
        class: &str,
        prop: &str,
        json: &Value,
        file: &Path,
    ) -> Result<(), String> {
        match value::property(class, prop, json)? {
            (name, Resolved::Value(v)) => self.set(r, name, v),
            (name, Resolved::Ref(path)) => self.refs.push(PendingRef {
                holder: r,
                property: name,
                path,
                file: file.to_path_buf(),
            }),
        }
        Ok(())
    }

    fn set(&mut self, r: Ref, name: &str, value: Variant) {
        if let Some(inst) = self.place.dom.get_by_ref_mut(r) {
            inst.properties.insert(ustr(name), value);
        }
    }

    /// An `.rbxm`/`.rbxmx` holds one top-level instance, which takes the file's name.
    fn roblox_model(&mut self, path: &Path, name: &str, parent: Ref, xml: bool) {
        let file = match fs::File::open(path) {
            Ok(file) => std::io::BufReader::new(file),
            Err(e) => {
                return self
                    .diags
                    .push(Diag::error("place.io", path, e.to_string()));
            }
        };
        let decoded = if xml {
            rbx_xml::from_reader_default(file).map_err(|e| e.to_string())
        } else {
            rbx_binary::from_reader(file).map_err(|e| e.to_string())
        };
        let mut model = match decoded {
            Ok(model) => model,
            Err(e) => return self.diags.push(Diag::error("place.badModel", path, e)),
        };
        let roots = model.root().children().to_vec();
        let [root] = roots[..] else {
            let message = format!(
                "holds {} top-level instances; a model file holds one",
                roots.len()
            );
            return self
                .diags
                .push(Diag::error("place.badModel", path, message));
        };
        model.transfer(root, &mut self.place.dom, parent);
        if let Some(inst) = self.place.dom.get_by_ref_mut(root) {
            inst.name = name.to_string();
        }
        self.place.set_file(root, path.to_path_buf());
    }

    /// The instance at `path`, creating it when missing: a service at the top, a NESTED container inside one,
    /// else `class` (or the name itself when that is a class, as for Workspace/Terrain).
    fn instance_at(&mut self, path: &str, class: Option<&str>) -> Result<Ref, String> {
        if let Some(r) = self.place.find(path) {
            return Ok(r);
        }
        let (parent_path, name) = path.rsplit_once('/').unwrap_or(("", path));
        if parent_path.is_empty() {
            return self.place.service(name);
        }
        let parent = self.instance_at(parent_path, None)?;
        let parent_class = self
            .place
            .dom
            .get_by_ref(parent)
            .map(|i| i.class.as_str())
            .unwrap_or_default();
        let class = match class {
            Some(class) => class,
            None if NESTED.contains(&(parent_class, name)) || value::is_class(name) => name,
            None => return Err(format!("{path} does not exist; give its ClassName")),
        };
        Ok(self
            .place
            .dom
            .insert(parent, InstanceBuilder::new(class).with_name(name)))
    }

    fn apply(&mut self, config: &PlaceConfig) {
        let toml = PathBuf::from("opossum.toml");
        for entry in config {
            let r = match self.instance_at(&entry.path, entry.class.as_deref()) {
                Ok(r) => r,
                Err(e) => {
                    self.diags.push(Diag::error(
                        "place.unknownInstance",
                        &toml,
                        format!("[place] {e}"),
                    ));
                    continue;
                }
            };
            let inst = self
                .place
                .dom
                .get_by_ref_mut(r)
                .expect("instance just found");
            if let Some(class) = &entry.class
                && inst.class != class.as_str()
            {
                // A directory gives a Folder; [place] may say what it really is.
                if inst.class == "Folder" && inst.properties.is_empty() && value::is_class(class) {
                    inst.class = ustr(class);
                } else {
                    let message =
                        format!("[place] {} is a {}, not a {class}", entry.path, inst.class);
                    self.diags
                        .push(Diag::error("place.badProperty", &toml, message));
                    continue;
                }
            }
            let class = inst.class.to_string();
            for (prop, json) in &entry.props {
                if let Err(e) = self.set_property(r, &class, prop, json, &toml) {
                    self.diags.push(Diag::error(
                        "place.badProperty",
                        &toml,
                        format!("[place] {}.{prop}: {e}", entry.path),
                    ));
                }
            }
        }
    }

    fn resolve_refs(&mut self) {
        for pending in std::mem::take(&mut self.refs) {
            match resolve_ref(&self.place, pending.holder, &pending.path) {
                Ok(target) => self.set(pending.holder, pending.property, Variant::Ref(target)),
                Err(e) => {
                    let holder = self.place.path_of(pending.holder);
                    let message =
                        format!("{holder}.{} = \"{}\": {e}", pending.property, pending.path);
                    self.diags
                        .push(Diag::error("place.badRef", &pending.file, message));
                }
            }
        }
    }
}

/// Follows a Ref path from the instance holding it: `..` is its parent, `.` itself, a name a child by that name.
pub fn resolve_ref(place: &Place, holder: Ref, path: &str) -> Result<Ref, String> {
    let mut at = holder;
    for segment in path.split('/') {
        let inst = place.dom.get_by_ref(at).ok_or("dangling instance")?;
        at = match segment {
            "" | "." => at,
            ".." if at == place.root() => return Err("climbs above the DataModel".into()),
            ".." => inst.parent(),
            name => {
                let mut named = inst
                    .children()
                    .iter()
                    .copied()
                    .filter(|&c| place.dom.get_by_ref(c).is_some_and(|i| i.name == name));
                match (named.next(), named.next()) {
                    (Some(child), None) => child,
                    (None, _) => return Err(format!("{} has no child {name}", place.path_of(at))),
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "{} has several children named {name}",
                            place.path_of(at)
                        ));
                    }
                }
            }
        };
    }
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_dirs() {
        assert_eq!(
            service_for_dir("replicated-storage").as_deref(),
            Some("ReplicatedStorage")
        );
        assert_eq!(service_for_dir("workspace").as_deref(), Some("Workspace"));
        assert_eq!(
            service_for_dir("starter-player-scripts").as_deref(),
            Some("StarterPlayer/StarterPlayerScripts")
        );
        assert_eq!(
            service_for_dir("starter-gui").as_deref(),
            Some("StarterGui")
        );
        assert_eq!(service_for_dir("vr-service").as_deref(), Some("VRService"));
        // Only the kebab spelling names a service, and only services are directories.
        assert_eq!(service_for_dir("ReplicatedStorage"), None);
        assert_eq!(service_for_dir("folder"), None);
    }

    #[test]
    fn refs_resolve_relative_to_their_holder() {
        let dir = std::env::temp_dir().join(format!("opossum-place-refs-{}", std::process::id()));
        let models = dir.join("replicated-storage");
        fs::create_dir_all(&models).unwrap();
        let boat = r#"{
            "className": "Model",
            "children": [
                { "name": "hull", "className": "Part", "children": [
                    { "name": "weld", "className": "Weld", "properties": { "Part0": "..", "Part1": "../../lid" } }
                ] },
                { "name": "lid", "className": "Part" },
                { "name": "hinge", "className": "Weld", "properties": { "Part0": "../hull", "Part1": "../nowhere" } }
            ]
        }"#;
        fs::write(models.join("boat.model.json"), boat).unwrap();
        let (place, diags) = read_place(&dir, &Vec::new());
        fs::remove_dir_all(&dir).unwrap();

        let get = |path: &str| place.find(path).unwrap();
        let prop = |path: &str, name: &str| {
            place
                .dom
                .get_by_ref(get(path))
                .unwrap()
                .properties
                .get(&ustr(name))
                .cloned()
        };
        assert_eq!(
            prop("ReplicatedStorage/boat/hull/weld", "Part0"),
            Some(Variant::Ref(get("ReplicatedStorage/boat/hull")))
        );
        assert_eq!(
            prop("ReplicatedStorage/boat/hull/weld", "Part1"),
            Some(Variant::Ref(get("ReplicatedStorage/boat/lid")))
        );
        assert_eq!(
            prop("ReplicatedStorage/boat/hinge", "Part0"),
            Some(Variant::Ref(get("ReplicatedStorage/boat/hull")))
        );
        assert_eq!(prop("ReplicatedStorage/boat/hinge", "Part1"), None);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "place.badRef");
    }
}
