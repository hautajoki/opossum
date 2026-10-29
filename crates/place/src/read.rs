//! The `place/` folder reader. Each top-level directory is a service named in kebab case (`replicated-storage`
//! is ReplicatedStorage; `starter-player-scripts` is StarterPlayer's StarterPlayerScripts). Inside, directories
//! are Folders, `.model.json` files are instance trees, `.rbxm`/`.rbxmx` files are Roblox models and `.txt`
//! files are StringValues. Loose top-level `*.toml` files are properties files ([`crate::properties`]); `place/assets`
//! (tessera's upload root) and other loose top-level files are not content. Rojo's formats, `*.meta.json` files and
//! `Rojo_Id` / `Rojo_Target_<Property>` attributes, are errors that `opossum migrate` converts.
//!
//! `dev` keeps the [`Place`] it read and re-reads only the entry a file event touched ([`Place::replace_entry`]).
//! Entry order is not kept in the dom: a re-read entry lands last among its siblings, so [`Place::entry_name`] gives
//! the tree its directory order back.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rbx_dom_weak::types::{Ref, Variant};
use rbx_dom_weak::ustr;
use rbx_dom_weak::{InstanceBuilder, WeakDom};
use rbx_reflection::ClassTag;
use serde_json::Value;

use crate::properties::{PlaceEntry, is_properties_file, read_properties};
use crate::tree::NESTED;
use crate::value::{self, Resolved, db};
use crate::{Diag, Severity};

/// What `place/` holds, as a DataModel, with the file each instance came from.
pub struct Place {
    pub dom: WeakDom,
    files: HashMap<Ref, PathBuf>,
    /// The directory entry (file or directory name) each entry's instance was read from.
    entries: HashMap<Ref, String>,
    /// Every Ref property read, resolved or not, so a re-read entry can resolve the refs into it again.
    refs: Vec<RefSpec>,
}

/// A Ref property as written in a model file or a properties file: a path from its holder.
struct RefSpec {
    holder: Ref,
    property: &'static str,
    path: String,
    file: PathBuf,
    /// Why it does not resolve, or None when it does.
    error: Option<String>,
}

impl Default for Place {
    fn default() -> Place {
        Place::new()
    }
}

impl Place {
    pub fn new() -> Place {
        Place {
            dom: WeakDom::new(InstanceBuilder::new("DataModel").with_name("Game")),
            files: HashMap::new(),
            entries: HashMap::new(),
            refs: Vec::new(),
        }
    }

    pub fn root(&self) -> Ref {
        self.dom.root_ref()
    }

    /// The first child of `parent` named `name`.
    pub fn child(&self, parent: Ref, name: &str) -> Option<Ref> {
        let parent = self.dom.get_by_ref(parent)?;
        parent.children().iter().copied().find(|&c| self.dom.get_by_ref(c).is_some_and(|i| i.name == name))
    }

    /// The instance at `path` (`""` is the DataModel).
    pub fn find(&self, path: &str) -> Option<Ref> {
        let mut at = self.root();
        for name in path.split('/').filter(|s| !s.is_empty()) {
            at = self.child(at, name)?;
        }
        Some(at)
    }

    pub fn path_of(&self, r: Ref) -> String {
        let mut names = Vec::new();
        let mut at = r;
        while at != self.root() {
            let Some(inst) = self.dom.get_by_ref(at) else { break };
            names.push(inst.name.as_str());
            at = inst.parent();
        }
        names.reverse();
        names.join("/")
    }

    /// The file an instance came from.
    pub fn file_of(&self, r: Ref) -> Option<&Path> {
        self.files.get(&r).map(PathBuf::as_path)
    }

    pub(crate) fn set_file(&mut self, r: Ref, file: PathBuf) {
        self.files.insert(r, file);
    }

    /// The directory entry an instance was read from (`boat.model.json`, `models`), None for anything else: services,
    /// nested containers, what a properties file made and what is inside a model file. Siblings that are entries sort
    /// by it.
    pub fn entry_name(&self, r: Ref) -> Option<&str> {
        self.entries.get(&r).map(String::as_str)
    }

    /// The top-level service `name`, created if it is not there yet.
    pub fn service(&mut self, name: &str) -> Result<Ref, String> {
        if let Some(r) = self.child(self.root(), name) {
            return Ok(r);
        }
        let is_service = db().classes.get(name).is_some_and(|c| c.tags.contains(&ClassTag::Service));
        if !is_service {
            return Err(format!("{name} is not a service"));
        }
        Ok(self.dom.insert(self.root(), InstanceBuilder::new(name).with_name(name)))
    }
}

/// The instance path a top-level `place/` directory stands for, or None when it names no service.
pub fn service_for_dir(dir: &str) -> Option<String> {
    for &(parent, child) in NESTED {
        if kebab(child) == dir {
            return Some(format!("{parent}/{child}"));
        }
    }
    let class = db().classes.values().find(|c| c.tags.contains(&ClassTag::Service) && kebab(c.name) == dir)?;
    Some(class.name.to_string())
}

/// The top-level `place/` directory a frame of reference's children are read from, the inverse of
/// [`service_for_dir`]: `Workspace` -> `workspace`, `StarterPlayer/StarterPlayerScripts` -> `starter-player-scripts`;
/// None when `path` is no service or nested container.
pub fn frame_dir(path: &str) -> Option<String> {
    match path.split_once('/') {
        Some((service, child)) => NESTED.contains(&(service, child)).then(|| kebab(child)),
        None => crate::props::is_service(path).then(|| kebab(path)),
    }
}

/// The class of the one top-level instance an `.rbxm` holds, as the reader would take it.
pub fn rbxm_class(bytes: &[u8]) -> Result<String, String> {
    match &model_classes(bytes, false)?[..] {
        [class] => Ok(class.clone()),
        roots => Err(format!("holds {} top-level instances; a model file holds one", roots.len())),
    }
}

/// The classes of the top-level instances in an `.rbxm` (or, with `xml`, an `.rbxmx`), in file order.
pub fn model_classes(bytes: &[u8], xml: bool) -> Result<Vec<String>, String> {
    let model = match xml {
        true => rbx_xml::from_reader_default(bytes).map_err(|e| e.to_string())?,
        false => rbx_binary::from_reader(bytes).map_err(|e| e.to_string())?,
    };
    Ok(model.root().children().iter().filter_map(|r| model.get_by_ref(*r)).map(|i| i.class.to_string()).collect())
}

/// `ReplicatedStorage` -> `replicated-storage`, `VRService` -> `vr-service`.
pub(crate) fn kebab(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_ascii_lowercase() || prev.is_ascii_digit() || (prev.is_ascii_uppercase() && next_lower) {
                out.push('-');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

struct Reader<'a> {
    place: &'a mut Place,
    diags: Vec<Diag>,
    /// Refs read since the last [`Reader::resolve_refs`].
    refs: Vec<RefSpec>,
}

const ROJO_META: &str = "a Rojo meta file; properties live in place/*.toml, and `opossum migrate` moves them there";

/// Reads `dir` (a project's `place/`) into a fresh DataModel, applies its properties files and resolves every Ref.
/// Problems come back as diagnostics; whatever could be read is in the place.
pub fn read_place(dir: &Path) -> (Place, Vec<Diag>) {
    let mut place = Place::new();
    let mut r = Reader { place: &mut place, diags: Vec::new(), refs: Vec::new() };
    let props = read_properties(dir, &mut r.diags);
    for (name, path, is_dir) in entries(dir, &mut r.diags) {
        if !is_dir && name.ends_with(".meta.json") {
            r.diags.push(Diag::error("place.rojoMeta", &path, ROJO_META));
        }
        if !is_dir || name.starts_with('.') || name == "assets" {
            continue;
        }
        let Some(service_path) = service_for_dir(&name) else {
            r.diags.push(Diag::error("place.unknownService", &path, format!("{name} names no service")));
            continue;
        };
        match r.instance_at(&service_path, None) {
            Ok(service) => r.dir(&path, service),
            Err(e) => r.diags.push(Diag::error("place.unknownService", &path, e)),
        }
    }
    r.apply(&props.entries);
    r.resolve_refs();
    let mut diags = r.diags;
    diags.extend(place.ref_diags());
    settle_pivots(&mut place);
    (place, diags)
}

impl Place {
    /// Re-reads what changed at `path` (a file or directory under `dir`, the project's `place/`): the shallowest entry
    /// on its way that is new, gone or not a directory, together with every sibling entry that makes the same instance
    /// name (the first, by name, wins, as in a full read). An event on a service directory or `place/` itself reads
    /// everything again, and so does one on a properties file (rare: properties may make instances, which only a full
    /// read orders) or a top-level meta file (whose error only a full read reports). Afterwards the properties files
    /// are read and applied again and every ref that pointed into the re-read entries, or did not resolve, is resolved
    /// again.
    ///
    /// Returns the files and directories re-read, whose diagnostics the returned ones replace (the properties files
    /// among them), together with every `place.badRef`; None when `path` is no content.
    pub fn replace_entry(&mut self, dir: &Path, path: &Path) -> Option<(Vec<PathBuf>, Vec<Diag>)> {
        let rel = path.strip_prefix(dir).ok()?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let full = parts.len() == 1 && (is_properties_file(&parts[0]) || parts[0].ends_with(".meta.json"));
        let Some(top) = parts.first().filter(|_| !full) else {
            let (place, diags) = read_place(dir);
            *self = place;
            return Some((vec![dir.to_path_buf()], diags));
        };
        if top.starts_with('.') || top == "assets" {
            return None;
        }
        let Some(service_path) = service_for_dir(top) else {
            // a loose file, there or gone, is no content; only a directory names a service
            if !dir.join(top).is_dir() {
                return None;
            }
            let diags = vec![Diag::error("place.unknownService", dir.join(top), format!("{top} names no service"))];
            return Some((vec![dir.join(top)], diags));
        };
        if !dir.join(top).is_dir() && parts.len() > 1 {
            // a service directory that went away with what was in it
            let (place, diags) = read_place(dir);
            *self = place;
            return Some((vec![dir.to_path_buf()], diags));
        }
        if parts.len() == 1 {
            // a service directory came or went: rare, and only a full read orders services and nested containers
            let (place, diags) = read_place(dir);
            *self = place;
            return Some((vec![dir.to_path_buf()], diags));
        }
        let mut r = Reader { place: self, diags: Vec::new(), refs: Vec::new() };
        let mut parent = match r.instance_at(&service_path, None) {
            Ok(service) => service,
            Err(e) => {
                r.diags.push(Diag::error("place.unknownService", dir.join(top), e));
                return Some((vec![dir.join(top)], r.diags));
            }
        };
        let mut at = dir.join(top);
        let mut name = String::new();
        for (k, part) in parts.iter().enumerate().skip(1) {
            at = at.join(part);
            name = instance_name(part, at.is_dir());
            // descend only into the directory's own instance, never a same-named one another entry made
            match r.place.child(parent, &name) {
                Some(child) if k + 1 < parts.len() && at.is_dir() && r.place.entry_name(child) == Some(part) => {
                    parent = child;
                }
                _ => break,
            }
        }
        let parent_class = r.place.dom.get_by_ref(parent).map(|i| i.class.to_string()).unwrap_or_default();
        if NESTED.contains(&(parent_class.as_str(), name.as_str())) {
            // a nested container's own directory came, went or holds the event: its children may also come from
            // a top-level directory (`place/terrain`) and a properties file may make it, which only a full read orders
            let (place, diags) = read_place(dir);
            *self = place;
            return Some((vec![dir.to_path_buf()], diags));
        }
        let folder = at.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
        // what made `name` here before: entries, or a properties file (made again below)
        let old: Vec<Ref> = r
            .place
            .dom
            .get_by_ref(parent)
            .map(|p| p.children().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|&c| r.place.dom.get_by_ref(c).is_some_and(|i| i.name == name))
            .collect();
        for old in old {
            r.place.forget(old);
        }
        let mut scopes = vec![at.clone()];
        let mut sink = Vec::new();
        for (entry, entry_path, is_dir) in entries(&folder, &mut sink) {
            if !entry.starts_with('.') && instance_name(&entry, is_dir) == name {
                r.entry(&entry_path, parent);
                if entry_path != at {
                    scopes.push(entry_path);
                }
            }
        }
        // what pointed into the forgotten instances now dangles; properties refs are read again with their files
        r.place.refs.retain(|s| s.file.parent() != Some(dir) && r.place.dom.get_by_ref(s.holder).is_some());
        let props = read_properties(dir, &mut r.diags);
        r.apply(&props.entries);
        r.resolve_refs();
        scopes.extend(props.files);
        let mut diags = r.diags;
        self.resolve_again();
        diags.extend(self.ref_diags());
        settle_pivots(self);
        Some((scopes, diags))
    }

    /// Resolves again every ref that did not resolve, or whose target was forgotten.
    fn resolve_again(&mut self) {
        let mut refs = std::mem::take(&mut self.refs);
        for spec in &mut refs {
            let dangling = self.dom.get_by_ref(spec.holder).is_some_and(|inst| {
                matches!(inst.properties.get(&ustr(spec.property)), Some(Variant::Ref(t)) if self.dom.get_by_ref(*t).is_none())
            });
            if spec.error.is_none() && !dangling {
                continue;
            }
            let resolved = resolve_ref(self, spec.holder, &spec.path);
            if let Some(inst) = self.dom.get_by_ref_mut(spec.holder) {
                match &resolved {
                    Ok(t) => inst.properties.insert(ustr(spec.property), Variant::Ref(*t)),
                    Err(_) => inst.properties.remove(&ustr(spec.property)),
                };
            }
            spec.error = resolved.err();
        }
        self.refs = refs;
    }

    /// A `place.badRef` for every ref that does not resolve.
    fn ref_diags(&self) -> Vec<Diag> {
        let unresolved = self.refs.iter().filter_map(|s| Some((s, s.error.as_ref()?)));
        unresolved
            .map(|(s, e)| {
                let message = format!("{}.{} = \"{}\": {e}", self.path_of(s.holder), s.property, s.path);
                Diag::error("place.badRef", &s.file, message)
            })
            .collect()
    }

    /// Destroys an instance and its subtree, with their files.
    fn forget(&mut self, r: Ref) {
        for inst in self.dom.descendants_of(r) {
            self.files.remove(&inst.referent());
            self.entries.remove(&inst.referent());
        }
        self.dom.destroy(r);
    }
}

/// The instance a directory entry makes: a directory's own name, a file's name without its content extension.
pub fn instance_name(entry: &str, is_dir: bool) -> String {
    if is_dir {
        return entry.to_owned();
    }
    [".model.json", ".rbxm", ".rbxmx", ".txt"]
        .iter()
        .find_map(|ext| entry.strip_suffix(ext))
        .unwrap_or(entry)
        .to_owned()
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
        .filter(|i| db.classes.get(i.class.as_str()).is_some_and(|c| db.has_superclass(c, model)))
        .map(|i| i.referent())
        .collect();
    for r in models {
        if let Some(inst) = place.dom.get_by_ref_mut(r) {
            inst.properties.insert(ustr("NeedsPivotMigration"), Variant::Bool(false));
        }
    }
}

/// Directory entries sorted by name: (name, path, is_dir).
pub(crate) fn entries(dir: &Path, diags: &mut Vec<Diag>) -> Vec<(String, PathBuf, bool)> {
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
            (e.file_name().to_string_lossy().into_owned(), path.clone(), path.is_dir())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

impl Reader<'_> {
    fn dir(&mut self, dir: &Path, parent: Ref) {
        for (name, path, _) in entries(dir, &mut self.diags) {
            if !name.starts_with('.') {
                self.entry(&path, parent);
            }
        }
    }

    /// One file or directory of `place/`, read under `parent`.
    fn entry(&mut self, path: &Path, parent: Ref) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let is_dir = path.is_dir();
        // Instances are addressed by path, so two entries may not make one name (`boat/` and `boat.model.json`).
        // Siblings inside one model file may share a name, as Studio content does.
        let instance = instance_name(&name, is_dir);
        let content = is_dir || instance != name;
        if content && self.place.child(parent, &instance).is_some() {
            let message = format!("{instance} is already made by another entry here");
            self.diags.push(Diag::error("tree.instanceClash", path, message));
            return;
        }
        if is_dir {
            // a nested container (Workspace/Terrain) keeps its own class
            let parent_class = self.place.dom.get_by_ref(parent).map(|i| i.class.to_string()).unwrap_or_default();
            let class = if NESTED.contains(&(parent_class.as_str(), name.as_str())) { name.as_str() } else { "Folder" };
            let folder = self.place.dom.insert(parent, InstanceBuilder::new(class).with_name(&name));
            self.place.entries.insert(folder, name.clone());
            self.dir(path, folder);
            return;
        }
        let before = self.place.dom.get_by_ref(parent).map_or(0, |p| p.children().len());
        self.file(path, &name, parent);
        // the file's instance, when it made one, is the parent's last child
        let made = self.place.dom.get_by_ref(parent).and_then(|p| p.children()[before..].first().copied());
        if let Some(made) = made {
            self.place.entries.insert(made, name);
        }
    }

    fn file(&mut self, path: &Path, name: &str, parent: Ref) {
        if let Some(stem) = name.strip_suffix(".model.json") {
            self.model_file(path, stem, parent);
        } else if let Some(stem) = name.strip_suffix(".rbxm") {
            self.roblox_model(path, stem, parent, false);
        } else if let Some(stem) = name.strip_suffix(".rbxmx") {
            self.roblox_model(path, stem, parent, true);
        } else if let Some(stem) = name.strip_suffix(".txt") {
            match fs::read_to_string(path) {
                Ok(text) => {
                    let value = InstanceBuilder::new("StringValue").with_name(stem).with_property("Value", text);
                    let r = self.place.dom.insert(parent, value);
                    self.place.set_file(r, path.to_path_buf());
                }
                Err(e) => self.diags.push(Diag::error("place.io", path, e.to_string())),
            }
        } else if name.ends_with(".meta.json") {
            self.diags.push(Diag::error("place.rojoMeta", path, ROJO_META));
        } else {
            self.diags.push(Diag::note("place.ignored", path, "not place content"));
        }
    }

    fn model_file(&mut self, path: &Path, name: &str, parent: Ref) {
        let json = fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| format!("invalid JSON: {e}")));
        match json {
            Ok(json) => {
                let rojo = crate::migrate::rojo_attributes(&json);
                if rojo > 0 {
                    let message = format!(
                        "{rojo} Rojo_Id / Rojo_Target_ attributes are Rojo refs, which set nothing; \
                         `opossum migrate` turns them into paths"
                    );
                    self.diags.push(Diag::error("place.rojoRef", path, message));
                }
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
            self.diags.push(Diag::error("place.badModel", file, format!("{name} has no className")));
            return None;
        };
        if !value::is_class(class) {
            self.diags.push(Diag::error("place.badModel", file, format!("{name}: unknown class {class}")));
            return None;
        }
        let r = self.place.dom.insert(parent, InstanceBuilder::new(class).with_name(name));
        if let Value::Object(fields) = json {
            for key in fields.keys() {
                if !matches!(key.as_str(), "name" | "className" | "properties" | "attributes" | "children" | "$schema")
                {
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
                    self.diags.push(Diag::error("place.badProperty", file, format!("{name}.{prop}: {e}")));
                }
            }
        }
        if !json["attributes"].is_null() {
            match value::attributes(&json["attributes"]) {
                Ok(attributes) => self.set(r, "Attributes", Variant::Attributes(attributes)),
                Err(e) => self.diags.push(Diag::error("place.badProperty", file, format!("{name}: {e}"))),
            }
        }
        if let Value::Array(children) = &json["children"] {
            for child in children {
                let Some(child_name) = child["name"].as_str() else {
                    self.diags.push(Diag::error("place.badModel", file, format!("a child of {name} has no name")));
                    continue;
                };
                self.model(child, child_name, r, file);
            }
        }
        Some(r)
    }

    fn set_property(&mut self, r: Ref, class: &str, prop: &str, json: &Value, file: &Path) -> Result<(), String> {
        match value::property(class, prop, json)? {
            (name, Resolved::Value(v)) => self.set(r, name, v),
            (name, Resolved::Ref(path)) => {
                self.refs.push(RefSpec { holder: r, property: name, path, file: file.to_path_buf(), error: None })
            }
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
                return self.diags.push(Diag::error("place.io", path, e.to_string()));
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
            let message = format!("holds {} top-level instances; a model file holds one", roots.len());
            return self.diags.push(Diag::error("place.badModel", path, message));
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
        let parent_class = self.place.dom.get_by_ref(parent).map(|i| i.class.as_str()).unwrap_or_default();
        let class = match class {
            Some(class) if !value::is_class(class) => {
                return Err(format!("{class} is not a class"));
            }
            Some(class) => class,
            None if NESTED.contains(&(parent_class, name)) || value::is_class(name) => name,
            None => return Err(format!("{path} does not exist; give its ClassName")),
        };
        Ok(self.place.dom.insert(parent, InstanceBuilder::new(class).with_name(name)))
    }

    /// Applies the properties files' entries. Each re-read applies all of them again (there are few, and setting a
    /// property twice changes nothing), so their diagnostics are always the whole set.
    fn apply(&mut self, entries: &[PlaceEntry]) {
        for entry in entries {
            let toml = &entry.file;
            let r = match self.instance_at(&entry.path, entry.class.as_deref()) {
                Ok(r) => r,
                Err(e) => {
                    self.diags.push(Diag::error("place.unknownInstance", toml, e));
                    continue;
                }
            };
            let inst = self.place.dom.get_by_ref_mut(r).expect("instance just found");
            if let Some(class) = &entry.class
                && inst.class != class.as_str()
            {
                // A directory gives a Folder; a properties file may say what it really is.
                if inst.class == "Folder" && inst.properties.is_empty() && value::is_class(class) {
                    inst.class = ustr(class);
                } else {
                    let message = format!("{} is a {}, not a {class}", entry.path, inst.class);
                    self.diags.push(Diag::error("place.badProperty", toml, message));
                    continue;
                }
            }
            let class = inst.class.to_string();
            for (prop, json) in &entry.props {
                if let Err(e) = self.set_property(r, &class, prop, json, toml) {
                    self.diags.push(Diag::error("place.badProperty", toml, format!("{}.{prop}: {e}", entry.path)));
                }
            }
        }
    }

    /// Resolves the refs read since the last call and keeps them in the place; [`Place::ref_diags`] reports the ones
    /// that do not resolve.
    fn resolve_refs(&mut self) {
        for mut spec in std::mem::take(&mut self.refs) {
            match resolve_ref(self.place, spec.holder, &spec.path) {
                Ok(target) => self.set(spec.holder, spec.property, Variant::Ref(target)),
                Err(e) => spec.error = Some(e),
            }
            self.place.refs.push(spec);
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
                        return Err(format!("{} has several children named {name}", place.path_of(at)));
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
        assert_eq!(service_for_dir("replicated-storage").as_deref(), Some("ReplicatedStorage"));
        assert_eq!(service_for_dir("workspace").as_deref(), Some("Workspace"));
        assert_eq!(service_for_dir("starter-player-scripts").as_deref(), Some("StarterPlayer/StarterPlayerScripts"));
        assert_eq!(service_for_dir("starter-gui").as_deref(), Some("StarterGui"));
        assert_eq!(service_for_dir("vr-service").as_deref(), Some("VRService"));
        // Only the kebab spelling names a service, and only services are directories.
        assert_eq!(service_for_dir("ReplicatedStorage"), None);
        assert_eq!(service_for_dir("folder"), None);
        // frame_dir is its inverse, for services and nested containers only
        for path in ["ReplicatedStorage", "VRService", "StarterPlayer/StarterPlayerScripts", "Workspace/Terrain"] {
            assert_eq!(service_for_dir(&frame_dir(path).unwrap()).as_deref(), Some(path));
        }
        assert_eq!(frame_dir("Workspace/Map"), None);
        assert_eq!(frame_dir("Folder"), None);
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
        let (place, diags) = read_place(&dir);
        fs::remove_dir_all(&dir).unwrap();

        let get = |path: &str| place.find(path).unwrap();
        let prop =
            |path: &str, name: &str| place.dom.get_by_ref(get(path)).unwrap().properties.get(&ustr(name)).cloned();
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

    /// The tree's hash at `path`, as `dev` would mount `place`.
    fn hash_at(place: &Place, path: &str) -> u128 {
        let mut fresh = crate::Tree::new();
        fresh.add_place(place);
        let mut tree = crate::Tree::new();
        tree.update(fresh);
        tree.get(tree.find(path).unwrap()).unwrap().hash
    }

    #[test]
    fn a_re_read_entry_is_what_a_full_read_makes() {
        let dir = std::env::temp_dir().join(format!("opossum-place-reread-{}", std::process::id()));
        let models = dir.join("replicated-storage").join("models");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join("a.txt"), "a").unwrap();
        fs::write(models.join("b.txt"), "b").unwrap();
        let door = r#"{ "className": "Model", "children": [
            { "name": "weld", "className": "Weld", "properties": { "Part1": "../../frame" } }
        ] }"#;
        fs::write(models.join("door.model.json"), door).unwrap();
        let (mut place, diags) = read_place(&dir);
        assert_eq!(diags.iter().map(|d| d.code).collect::<Vec<_>>(), ["place.badRef"]);

        // the ref's target appears in an entry of its own, and an earlier sibling is edited
        fs::write(models.join("frame.model.json"), r#"{ "className": "Part" }"#).unwrap();
        fs::write(models.join("a.txt"), "a2").unwrap();
        let (_, diags) = place.replace_entry(&dir, &models.join("frame.model.json")).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        place.replace_entry(&dir, &models.join("a.txt")).unwrap();
        let (full, _) = read_place(&dir);
        fs::remove_dir_all(&dir).unwrap();

        let weld = place.find("ReplicatedStorage/models/door/weld").unwrap();
        let part1 = place.dom.get_by_ref(weld).unwrap().properties.get(&ustr("Part1")).cloned();
        assert_eq!(part1, Some(Variant::Ref(place.find("ReplicatedStorage/models/frame").unwrap())));
        // same order, same refs: the root's stamp is the one a restarted session computes
        assert_eq!(hash_at(&place, "ReplicatedStorage/models"), hash_at(&full, "ReplicatedStorage/models"));
    }
}
