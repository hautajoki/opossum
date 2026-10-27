//! The in-memory DataModel and the mounting API. Instances are addressed by `/`-separated paths from the
//! DataModel (`ReplicatedStorage/shared/x`). Everything the mounting API creates is owned by the tool and is
//! the only thing it will replace or remove; content read from `place/` is never touched by mounts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rbx_dom_weak::InstanceBuilder;
use rbx_dom_weak::WeakDom;
use rbx_dom_weak::types::{Enum, Ref, Variant};
use rbx_reflection::ClassTag;

use crate::Diag;
use crate::value::db;

/// Containers that live inside a service rather than at the top of the DataModel.
pub const NESTED: &[(&str, &str)] = &[
    ("StarterPlayer", "StarterPlayerScripts"),
    ("StarterPlayer", "StarterCharacterScripts"),
];

/// Where packages mount: `<PACKAGES>/<scope>/<name>`.
pub const PACKAGES: &str = "ReplicatedStorage/rbxts/node_modules";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    Module,
    /// A Script with RunContext Server.
    Server,
    /// A Script with RunContext Client.
    Client,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owned {
    /// Made to hold mounted children; becomes a script if one is mounted at its path, and goes away once empty.
    Folder,
    Script,
}

/// One Luau file of a package, `rel` relative to the package's tree root (`init.luau`, `hooks/use-x.luau`).
pub struct PackageFile {
    pub rel: String,
    pub source: String,
    pub file: PathBuf,
}

pub struct Place {
    pub dom: WeakDom,
    owned: HashMap<Ref, Owned>,
    files: HashMap<Ref, PathBuf>,
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
            owned: HashMap::new(),
            files: HashMap::new(),
        }
    }

    pub fn root(&self) -> Ref {
        self.dom.root_ref()
    }

    /// The first child of `parent` named `name`.
    pub fn child(&self, parent: Ref, name: &str) -> Option<Ref> {
        let parent = self.dom.get_by_ref(parent)?;
        parent
            .children()
            .iter()
            .copied()
            .find(|&c| self.dom.get_by_ref(c).is_some_and(|i| i.name == name))
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
            let Some(inst) = self.dom.get_by_ref(at) else {
                break;
            };
            names.push(inst.name.as_str());
            at = inst.parent();
        }
        names.reverse();
        names.join("/")
    }

    /// The source file behind an instance, for the sourcemap.
    pub fn file_of(&self, r: Ref) -> Option<&Path> {
        self.files.get(&r).map(PathBuf::as_path)
    }

    pub(crate) fn set_file(&mut self, r: Ref, file: PathBuf) {
        self.files.insert(r, file);
    }

    pub fn is_owned(&self, r: Ref) -> bool {
        self.owned.contains_key(&r)
    }

    /// The top-level service `name`, created if it is not there yet.
    pub fn service(&mut self, name: &str) -> Result<Ref, String> {
        if let Some(r) = self.child(self.root(), name) {
            return Ok(r);
        }
        let is_service = db()
            .classes
            .get(name)
            .is_some_and(|c| c.tags.contains(&ClassTag::Service));
        if !is_service {
            return Err(format!("{name} is not a service"));
        }
        Ok(self
            .dom
            .insert(self.root(), InstanceBuilder::new(name).with_name(name)))
    }

    /// Puts a script at `path`, creating folders on the way. A script the tool mounted earlier is replaced in
    /// place (its children stay); anything else at that path is a clash.
    pub fn set_script(
        &mut self,
        path: &str,
        kind: ScriptKind,
        source: String,
        file: Option<PathBuf>,
    ) -> Result<Ref, Diag> {
        let names: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((&last, dirs)) = names.split_last() else {
            return Err(Diag::error(
                "tree.badPath",
                path,
                "a script needs a path inside a service",
            ));
        };
        let parent = self.folder_path(path, dirs)?;
        let (class, run_context) = match kind {
            ScriptKind::Module => ("ModuleScript", None),
            ScriptKind::Server => ("Script", Some("Server")),
            ScriptKind::Client => ("Script", Some("Client")),
            ScriptKind::Local => ("LocalScript", None),
        };
        let r = match self.child(parent, last) {
            Some(r) if self.is_owned(r) => r,
            Some(_) => {
                return Err(Diag::error(
                    "tree.instanceClash",
                    path,
                    "an instance from place/ is already here",
                ));
            }
            None => self
                .dom
                .insert(parent, InstanceBuilder::new("Folder").with_name(last)),
        };
        let inst = self.dom.get_by_ref_mut(r).expect("instance just found");
        inst.class = class.into();
        inst.properties.clear();
        inst.properties
            .insert("Source".into(), Variant::String(source));
        if let Some(context) = run_context {
            let value = db()
                .enums
                .get("RunContext")
                .and_then(|e| e.items.get(context))
                .copied()
                .unwrap_or(0);
            inst.properties
                .insert("RunContext".into(), Variant::Enum(Enum::from_u32(value)));
        }
        self.owned.insert(r, Owned::Script);
        match file {
            Some(file) => self.files.insert(r, file),
            None => self.files.remove(&r),
        };
        Ok(r)
    }

    /// Walks `dirs` from the DataModel, making owned folders (or the service, or a nested container such as
    /// StarterPlayerScripts) where nothing exists yet.
    fn folder_path(&mut self, path: &str, dirs: &[&str]) -> Result<Ref, Diag> {
        let Some((&service, rest)) = dirs.split_first() else {
            return Err(Diag::error(
                "tree.badPath",
                path,
                "a script cannot sit directly in the DataModel",
            ));
        };
        let mut at = self
            .service(service)
            .map_err(|e| Diag::error("tree.badPath", path, e))?;
        for &name in rest {
            at = match self.child(at, name) {
                Some(r) => r,
                None => {
                    let parent_class = self
                        .dom
                        .get_by_ref(at)
                        .map(|i| i.class.as_str())
                        .unwrap_or_default();
                    if NESTED.contains(&(parent_class, name)) {
                        self.dom
                            .insert(at, InstanceBuilder::new(name).with_name(name))
                    } else {
                        let r = self
                            .dom
                            .insert(at, InstanceBuilder::new("Folder").with_name(name));
                        self.owned.insert(r, Owned::Folder);
                        r
                    }
                }
            };
        }
        Ok(at)
    }

    /// Removes a mounted script. One that still holds mounted children turns back into a folder.
    pub fn remove(&mut self, path: &str) -> Result<(), Diag> {
        let r = self.owned_at(path)?;
        let inst = self.dom.get_by_ref_mut(r).expect("owned instance exists");
        self.files.remove(&r);
        if inst.children().is_empty() {
            self.destroy(r);
        } else {
            inst.class = "Folder".into();
            inst.properties.clear();
            self.owned.insert(r, Owned::Folder);
        }
        Ok(())
    }

    /// Removes a mounted subtree whole (a package, a route root).
    pub fn remove_tree(&mut self, path: &str) -> Result<(), Diag> {
        let r = self.owned_at(path)?;
        self.destroy(r);
        Ok(())
    }

    fn owned_at(&self, path: &str) -> Result<Ref, Diag> {
        match self.find(path) {
            Some(r) if self.is_owned(r) => Ok(r),
            Some(_) => Err(Diag::error(
                "tree.notOwned",
                path,
                "not mounted by the tool, so it is not removed",
            )),
            None => Err(Diag::error("tree.badPath", path, "nothing is mounted here")),
        }
    }

    /// Destroys `r` and its subtree, then the owned folders above it that it leaves empty.
    fn destroy(&mut self, r: Ref) {
        let mut gone: Vec<Ref> = self.dom.descendants_of(r).map(|i| i.referent()).collect();
        gone.push(r);
        for g in gone {
            self.owned.remove(&g);
            self.files.remove(&g);
        }
        let mut parent = self.dom.get_by_ref(r).map_or(Ref::none(), |i| i.parent());
        self.dom.destroy(r);
        while self.owned.get(&parent) == Some(&Owned::Folder) {
            let inst = self.dom.get_by_ref(parent).expect("owned instance exists");
            if !inst.children().is_empty() {
                break;
            }
            let up = inst.parent();
            self.owned.remove(&parent);
            self.dom.destroy(parent);
            parent = up;
        }
    }

    /// Mounts a package's Luau files at `ReplicatedStorage/rbxts/node_modules/<scope>/<name>`, replacing an
    /// earlier mount of it. `init.luau` makes its folder the module, as everywhere else.
    pub fn mount_package(
        &mut self,
        scope: &str,
        name: &str,
        files: &[PackageFile],
    ) -> Result<Ref, Diag> {
        let base = format!("{PACKAGES}/{scope}/{name}");
        if self.find(&base).is_some() {
            self.remove_tree(&base)?;
        }
        for file in files {
            let Some(module) = module_path(&file.rel) else {
                return Err(Diag::error(
                    "tree.badPackageFile",
                    &file.file,
                    "package files are .luau or .lua",
                ));
            };
            let path = if module.is_empty() {
                base.clone()
            } else {
                format!("{base}/{module}")
            };
            self.set_script(
                &path,
                ScriptKind::Module,
                file.source.clone(),
                Some(file.file.clone()),
            )?;
        }
        // A package with no init module is still there, as the folder holding its modules.
        match self.find(&base) {
            Some(r) => Ok(r),
            None => {
                let dirs: Vec<&str> = base.split('/').collect();
                self.folder_path(&base, &dirs)
            }
        }
    }
}

/// `hooks/use-x.luau` -> `hooks/use-x`, `hooks/init.luau` -> `hooks`, `init.luau` -> ``.
fn module_path(rel: &str) -> Option<String> {
    let stem = rel
        .strip_suffix(".luau")
        .or_else(|| rel.strip_suffix(".lua"))?;
    let stem = stem
        .strip_suffix("init")
        .filter(|s| s.is_empty() || s.ends_with('/'))
        .unwrap_or(stem);
    Some(stem.trim_end_matches('/').to_string())
}

/// The directory a package's instance tree is made from: its `default.project.json` tree `$path` when it has
/// one (every package in use has only that shape), else the package directory itself.
pub fn package_tree_root(dir: &Path) -> PathBuf {
    let project = std::fs::read_to_string(dir.join("default.project.json")).ok();
    let json = project.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    match json.as_ref().and_then(|j| j["tree"]["$path"].as_str()) {
        Some(sub) => dir.join(sub),
        None => dir.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_paths() {
        assert_eq!(module_path("init.luau").as_deref(), Some(""));
        assert_eq!(module_path("hooks/init.lua").as_deref(), Some("hooks"));
        assert_eq!(
            module_path("hooks/use-init.luau").as_deref(),
            Some("hooks/use-init")
        );
        assert_eq!(module_path("init.d.ts"), None);
    }

    #[test]
    fn mounted_init_keeps_children_and_unmounts_cleanly() {
        let mut place = Place::new();
        let child = place
            .set_script(
                "ReplicatedStorage/shared/a/b",
                ScriptKind::Module,
                "b".into(),
                None,
            )
            .unwrap();
        // The folder made for `b` becomes the init module, keeping `b`.
        let a = place
            .set_script(
                "ReplicatedStorage/shared/a",
                ScriptKind::Module,
                "a".into(),
                None,
            )
            .unwrap();
        assert_eq!(place.dom.get_by_ref(a).unwrap().children(), &[child]);
        place.remove("ReplicatedStorage/shared/a").unwrap();
        assert_eq!(place.dom.get_by_ref(a).unwrap().class, "Folder");
        // Removing the last module takes every folder made for it, up to the service.
        place.remove("ReplicatedStorage/shared/a/b").unwrap();
        assert!(place.find("ReplicatedStorage/shared").is_none());
        assert!(place.find("ReplicatedStorage").is_some());
    }
}
