//! The DataModel opossum owns: a slab of immutable nodes indexed by stable ids. A fresh tree is built by mounting
//! (`place/` content, then scripts and packages); the long-lived tree takes it with [`Tree::update`], which keeps the
//! ids of what it can pair, and takes a re-rendered module with [`Tree::set_sources`]. Clones share every chunk of
//! the slab, so a published generation costs a few pointer copies and two trees diff in O(changed chunks).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rbx_dom_weak::types::{Enum, Ref, Variant};
use rbx_dom_weak::{InstanceBuilder, Ustr, ustr};
use rustc_hash::{FxHashMap, FxHashSet};
use xxhash_rust::xxh3::{Xxh3, xxh3_128};

use crate::Diag;
use crate::props::{self, Write, creatable, is_service, write_kind};
use crate::read::Place;
use crate::value::db;

pub type Id = u32;
/// Never an id: the DataModel's parent, a nil ref.
pub const NONE: Id = u32::MAX;
/// The DataModel. Never sent: services are bound by class.
pub const ROOT: Id = 0;

/// Containers inside a service that are frames of reference like the services themselves.
pub const NESTED: &[(&str, &str)] = &[
    ("StarterPlayer", "StarterPlayerScripts"),
    ("StarterPlayer", "StarterCharacterScripts"),
    ("Workspace", "Terrain"),
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
}

/// Who made a node, which decides what mounting may replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `place/`, a properties file, or a service or nested container.
    Place,
    /// Made to hold mounted scripts; a script mounted at its path takes it over.
    Folder,
    Script,
}

/// A script's text with its hash and the require strings it holds, made once per rendered or read source and shared
/// by every tree after.
#[derive(Debug, Clone)]
pub struct Source {
    pub text: Arc<str>,
    pub hash: u128,
    pub requires: Arc<[Box<str>]>,
}

impl Source {
    pub fn new(text: Arc<str>) -> Source {
        let hash = xxh3_128(text.as_bytes());
        let requires = require_strings(&text).into();
        Source { text, hash, requires }
    }

    fn same(&self, other: &Source) -> bool {
        Arc::ptr_eq(&self.text, &other.text) || self.hash == other.hash
    }
}

/// `require("…")` string arguments, in order: what a module requires at run time when it names modules by path.
fn require_strings(text: &str) -> Vec<Box<str>> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("require(") {
        let rest = text[at + 8..].trim_start();
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
        if let Some(end) = rest[1..].find(quote) {
            out.push(rest[1..1 + end].into());
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Node {
    pub parent: Id,
    pub name: String,
    pub class: Ustr,
    /// Sorted by name: every property except refs and `Source`, under the names rbx_binary serializes.
    pub props: Vec<(Ustr, Variant)>,
    /// Ref-valued properties by target id, sorted by name.
    pub refs: Vec<(Ustr, Id)>,
    pub source: Option<Source>,
    /// The modules this script requires by path.
    pub requires: Vec<Id>,
    pub children: Vec<Id>,
    pub hash: u128,
    /// The file behind it, for the sourcemap and for messages.
    pub file: Option<Arc<Path>>,
    pub origin: Origin,
}

impl Node {
    fn new(parent: Id, name: &str, class: &str, origin: Origin) -> Node {
        Node {
            parent,
            name: name.to_owned(),
            class: ustr(class),
            props: Vec::new(),
            refs: Vec::new(),
            source: None,
            requires: Vec::new(),
            children: Vec::new(),
            hash: 0,
            file: None,
            origin,
        }
    }

    pub fn prop(&self, name: &str) -> Option<&Variant> {
        self.props.iter().find(|(n, _)| n.as_str() == name).map(|(_, v)| v)
    }

    fn set_prop(&mut self, name: Ustr, value: Variant) {
        match self.props.binary_search_by(|(n, _)| n.as_str().cmp(name.as_str())) {
            Ok(i) => self.props[i].1 = value,
            Err(i) => self.props.insert(i, (name, value)),
        }
    }
}

const CHUNK: usize = 64;
type Chunk = [Option<Arc<Node>>; CHUNK];

/// See the module docs. Id 0 is the DataModel.
#[derive(Clone)]
pub struct Tree {
    chunks: Vec<Arc<Chunk>>,
    next: Id,
}

impl Default for Tree {
    fn default() -> Tree {
        Tree::new()
    }
}

impl Tree {
    pub fn new() -> Tree {
        let mut tree = Tree::empty();
        tree.put(ROOT, Some(Arc::new(Node::new(NONE, "Game", "DataModel", Origin::Place))));
        tree.next = 1;
        tree
    }

    /// No nodes at all, not even the DataModel: a subscriber's known tree starts here.
    pub fn empty() -> Tree {
        Tree { chunks: Vec::new(), next: 0 }
    }

    pub fn get(&self, id: Id) -> Option<&Node> {
        self.arc(id).map(|n| &**n)
    }

    pub fn arc(&self, id: Id) -> Option<&Arc<Node>> {
        self.chunks.get(id as usize / CHUNK)?[id as usize % CHUNK].as_ref()
    }

    fn node(&self, id: Id) -> &Node {
        self.get(id).expect("a live id")
    }

    /// Sets or clears a slot; for trees assembled from other trees' nodes.
    pub fn put(&mut self, id: Id, node: Option<Arc<Node>>) {
        let (c, i) = (id as usize / CHUNK, id as usize % CHUNK);
        while self.chunks.len() <= c {
            self.chunks.push(Arc::new(std::array::from_fn(|_| None)));
        }
        Arc::make_mut(&mut self.chunks[c])[i] = node;
        self.next = self.next.max(id + 1);
    }

    fn node_mut(&mut self, id: Id) -> &mut Node {
        let (c, i) = (id as usize / CHUNK, id as usize % CHUNK);
        Arc::make_mut(Arc::make_mut(&mut self.chunks[c])[i].as_mut().expect("a live id"))
    }

    fn add(&mut self, parent: Id, mut node: Node) -> Id {
        let id = self.next;
        node.parent = parent;
        self.put(id, Some(Arc::new(node)));
        self.node_mut(parent).children.push(id);
        id
    }

    /// Every id in use, ascending.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.chunks.iter().enumerate().flat_map(|(c, chunk)| {
            chunk.iter().enumerate().filter(|(_, n)| n.is_some()).map(move |(i, _)| (c * CHUNK + i) as Id)
        })
    }

    /// The ids whose node differs between the two trees (by pointer): what a subscriber's diff walks.
    pub fn differs(&self, other: &Tree) -> Vec<Id> {
        let mut out = Vec::new();
        for c in 0..self.chunks.len().max(other.chunks.len()) {
            let (a, b) = (self.chunks.get(c), other.chunks.get(c));
            if let (Some(a), Some(b)) = (a, b)
                && Arc::ptr_eq(a, b)
            {
                continue;
            }
            for i in 0..CHUNK {
                let x = a.and_then(|a| a[i].as_ref());
                let y = b.and_then(|b| b[i].as_ref());
                let same = match (x, y) {
                    (Some(x), Some(y)) => Arc::ptr_eq(x, y),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    out.push((c * CHUNK + i) as Id);
                }
            }
        }
        out
    }

    /// The first child of `parent` named `name`.
    pub fn child(&self, parent: Id, name: &str) -> Option<Id> {
        self.get(parent)?.children.iter().copied().find(|&c| self.node(c).name == name)
    }

    /// The node at `path` (`/`-separated from the DataModel; `""` is the DataModel).
    pub fn find(&self, path: &str) -> Option<Id> {
        let mut at = ROOT;
        for name in path.split('/').filter(|s| !s.is_empty()) {
            at = self.child(at, name)?;
        }
        Some(at)
    }

    /// Ids from the DataModel's child down to `id`.
    fn ancestry(&self, id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        let mut at = id;
        while at != ROOT && at != NONE {
            out.push(at);
            at = self.get(at).map_or(NONE, |n| n.parent);
        }
        out.reverse();
        out
    }

    pub fn path_of(&self, id: Id) -> String {
        self.ancestry(id).iter().map(|&a| self.node(a).name.as_str()).collect::<Vec<_>>().join("/")
    }

    /// The name Studio prints: `ReplicatedStorage.shared.x`.
    pub fn full_name(&self, id: Id) -> String {
        self.ancestry(id).iter().map(|&a| self.node(a).name.as_str()).collect::<Vec<_>>().join(".")
    }

    pub fn depth(&self, id: Id) -> usize {
        self.ancestry(id).len()
    }

    /// A frame of reference: a service, or a nested container inside one.
    pub fn is_frame(&self, id: Id) -> bool {
        let Some(node) = self.get(id) else { return false };
        node.parent == ROOT
            || self
                .get(node.parent)
                .is_some_and(|p| p.parent == ROOT && NESTED.contains(&(p.class.as_str(), node.name.as_str())))
    }

    /// Frames, parents first.
    pub fn frames(&self) -> Vec<Id> {
        let mut out: Vec<Id> = self.node(ROOT).children.clone();
        for i in 0..out.len() {
            out.extend(self.node(out[i]).children.iter().copied().filter(|&c| self.is_frame(c)));
        }
        out
    }

    /// The roots: every child of a frame that is not a frame itself.
    pub fn roots(&self) -> Vec<Id> {
        let mut out = Vec::new();
        for frame in self.frames() {
            out.extend(self.node(frame).children.iter().copied().filter(|&c| !self.is_frame(c)));
        }
        out
    }

    /// The root `id` is in (itself for a root), or None for frames and the DataModel.
    pub fn root_of(&self, id: Id) -> Option<Id> {
        let mut at = id;
        loop {
            let node = self.get(at)?;
            if self.is_frame(node.parent) && !self.is_frame(at) {
                return Some(at);
            }
            if node.parent == ROOT || node.parent == NONE {
                return None;
            }
            at = node.parent;
        }
    }

    /// The node `spec` names from `from`: `@game/...` from the DataModel, `@self/...` from the module, `./` and `../`
    /// from its parent, as requires are spelled.
    pub fn resolve(&self, from: Id, spec: &str) -> Option<Id> {
        let parent = self.get(from)?.parent;
        let (mut at, rest) = if let Some(rest) = spec.strip_prefix("@game/") {
            (ROOT, rest)
        } else if let Some(rest) = spec.strip_prefix("@self/") {
            (from, rest)
        } else if spec.starts_with("./") || spec.starts_with("../") {
            (parent, spec)
        } else {
            return None;
        };
        for segment in rest.split('/') {
            at = match segment {
                "" | "." => at,
                ".." => self.get(at)?.parent,
                name => self.child(at, name)?,
            };
        }
        Some(at)
    }

    // --- mounting, on a fresh tree --------------------------------------------------------------------------------

    /// The top-level service `name`, made if it is not there yet.
    pub fn service(&mut self, name: &str) -> Result<Id, String> {
        if let Some(id) = self.child(ROOT, name) {
            return Ok(id);
        }
        if !is_service(name) {
            return Err(format!("{name} is not a service"));
        }
        Ok(self.add(ROOT, Node::new(ROOT, name, name, Origin::Place)))
    }

    /// Puts a script at `path`, making folders on the way. A folder made for scripts becomes the script (its children
    /// stay); anything else already there is a clash.
    pub fn set_script(
        &mut self,
        path: &str,
        kind: ScriptKind,
        source: Source,
        file: Option<Arc<Path>>,
    ) -> Result<Id, Diag> {
        let names: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((&last, dirs)) = names.split_last() else {
            return Err(Diag::error("tree.badPath", path, "a script needs a path inside a service"));
        };
        let parent = self.folder_path(path, dirs)?;
        let id = match self.child(parent, last) {
            Some(id) if self.node(id).origin != Origin::Place => id,
            Some(_) => return Err(Diag::error("tree.instanceClash", path, "an instance from place/ is already here")),
            None => self.add(parent, Node::new(parent, last, "Folder", Origin::Folder)),
        };
        let (class, context) = match kind {
            ScriptKind::Module => ("ModuleScript", None),
            ScriptKind::Server => ("Script", Some("Server")),
            ScriptKind::Client => ("Script", Some("Client")),
        };
        let node = self.node_mut(id);
        node.class = ustr(class);
        node.props.clear();
        if let Some(context) = context {
            let value = db().enums.get("RunContext").and_then(|e| e.items.get(context)).copied().unwrap_or(0);
            node.set_prop(ustr("RunContext"), Variant::Enum(Enum::from_u32(value)));
        }
        node.source = Some(source);
        node.origin = Origin::Script;
        node.file = file;
        Ok(id)
    }

    /// Mounts a script template disabled, for the module that clones it to enable the copy. `Disabled` is what a
    /// place file stores; `Enabled` does not serialize.
    pub fn disable(&mut self, id: Id) {
        self.node_mut(id).set_prop(ustr("Disabled"), Variant::Bool(true));
    }

    /// Walks `dirs` from the DataModel, making folders (or the service, or a nested container) where nothing is.
    fn folder_path(&mut self, path: &str, dirs: &[&str]) -> Result<Id, Diag> {
        let Some((&service, rest)) = dirs.split_first() else {
            return Err(Diag::error("tree.badPath", path, "a script cannot sit directly in the DataModel"));
        };
        let mut at = self.service(service).map_err(|e| Diag::error("tree.badPath", path, e))?;
        for &name in rest {
            at = match self.child(at, name) {
                Some(id) => id,
                None if NESTED.contains(&(self.node(at).class.as_str(), name)) && self.node(at).parent == ROOT => {
                    self.add(at, Node::new(at, name, name, Origin::Place))
                }
                None => self.add(at, Node::new(at, name, "Folder", Origin::Folder)),
            };
        }
        Ok(at)
    }

    /// Mounts a package's Luau files at `<PACKAGES>/<scope>/<name>`; `init.luau` makes its folder the module. A file
    /// that is not Luau fails the mount before anything changes.
    pub fn mount_package(&mut self, scope: &str, name: &str, files: &[PackageFile]) -> Result<Id, Diag> {
        let base = format!("{PACKAGES}/{scope}/{name}");
        let mut modules = Vec::with_capacity(files.len());
        for file in files {
            let Some(module) = module_path(&file.rel) else {
                return Err(Diag::error("tree.badPackageFile", &file.file, "package files are .luau or .lua"));
            };
            let path = if module.is_empty() { base.clone() } else { format!("{base}/{module}") };
            modules.push((path, file));
        }
        for (path, file) in modules {
            self.set_script(&path, ScriptKind::Module, file.source.clone(), Some(Arc::from(file.file.as_path())))?;
        }
        // a package with no init module is still there, as the folder holding its modules
        match self.find(&base) {
            Some(id) => Ok(id),
            None => {
                let dirs: Vec<&str> = base.split('/').collect();
                self.folder_path(&base, &dirs)
            }
        }
    }

    /// Copies `place/` content (the reader's DataModel) into a fresh tree.
    pub fn add_place(&mut self, place: &Place) {
        let mut ids: FxHashMap<Ref, Id> = FxHashMap::default();
        let mut refs: Vec<(Id, Ustr, Ref)> = Vec::new();
        let mut stack: Vec<(Ref, Id)> = place.dom.root().children().iter().rev().map(|&r| (r, ROOT)).collect();
        while let Some((r, parent)) = stack.pop() {
            let inst = place.dom.get_by_ref(r).expect("children are live");
            let id = match self.child(parent, &inst.name) {
                // services and nested containers are shared with mounts
                Some(id) if parent == ROOT || self.is_frame(id) => id,
                _ => self.add(parent, Node::new(parent, &inst.name, &inst.class, Origin::Place)),
            };
            ids.insert(r, id);
            let class = inst.class.as_str();
            let node = self.node_mut(id);
            node.file = place.file_of(r).map(Arc::from);
            let mut names: Vec<&Ustr> = inst.properties.keys().collect();
            names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            for name in names {
                match &inst.properties[name] {
                    Variant::Ref(target) if target.is_some() => refs.push((id, *name, *target)),
                    Variant::Ref(_) => {}
                    Variant::String(text) if name.as_str() == "Source" && props::is_a(class, "LuaSourceContainer") => {
                        node.source = Some(Source::new(Arc::from(text.as_str())));
                    }
                    value => {
                        if let Some((name, value)) = props::node_prop(class, *name, value.clone()) {
                            node.set_prop(name, value);
                        }
                    }
                }
            }
            // entries in directory order, as a full read makes them (a re-read entry is the last child in the dom);
            // anything else after them, in dom order
            let mut kids = inst.children().to_vec();
            kids.sort_by(|&a, &b| match (place.entry_name(a), place.entry_name(b)) {
                (Some(a), Some(b)) => a.cmp(b),
                (a, b) => b.is_some().cmp(&a.is_some()),
            });
            stack.extend(kids.into_iter().rev().map(|c| (c, id)));
        }
        for (holder, name, target) in refs {
            if let Some(&target) = ids.get(&target) {
                let node = self.node_mut(holder);
                node.refs.push((name, target));
                node.refs.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
            }
        }
    }

    // --- the long-lived tree --------------------------------------------------------------------------------------

    /// Makes this tree `fresh`, keeping the ids of what pairs (see SYNC.md, Identity). Nodes whose content did not
    /// change keep their `Arc`, so the chunks they share with older generations stay shared. Returns whether the
    /// shape changed (a node added, removed, moved or renamed).
    pub fn update(&mut self, mut fresh: Tree) -> bool {
        let n = fresh.next as usize;
        let mut to_live = vec![NONE; n];
        to_live[ROOT as usize] = ROOT;
        let mut taken: FxHashSet<Id> = FxHashSet::default();
        taken.insert(ROOT);

        // 1. children of a matched parent pair by (name, class, ordinal); a node whose class changed, or that changed a
        // property no plugin can write, takes a new id and its children pair with the old node's.
        let mut queue: Vec<(Id, Id)> = vec![(ROOT, ROOT)];
        let mut order: Vec<Id> = vec![ROOT];
        let mut at = 0;
        while at < queue.len() {
            let (f, l) = queue[at];
            at += 1;
            let fresh_children = fresh.node(f).children.clone();
            let live_children = self.node(l).children.clone();
            let mut groups: FxHashMap<(&str, Ustr), Vec<Id>> = FxHashMap::default();
            for &c in &live_children {
                let node = self.node(c);
                groups.entry((node.name.as_str(), node.class)).or_default().push(c);
            }
            let mut fresh_groups: FxHashMap<(&str, Ustr), usize> = FxHashMap::default();
            for &c in &fresh_children {
                let node = fresh.node(c);
                *fresh_groups.entry((node.name.as_str(), node.class)).or_default() += 1;
            }
            let mut seen: FxHashMap<(&str, Ustr), usize> = FxHashMap::default();
            let mut paired: Vec<(Id, Option<Id>, bool)> = Vec::with_capacity(fresh_children.len());
            // a duplicate group that changed size, or lost a member to a replacement, is inserted whole (new ids, in
            // tree order), since Studio appends what it inserts and ordinals must stay true
            let mut whole: FxHashSet<(&str, Ustr)> = FxHashSet::default();
            for &c in &fresh_children {
                let node = fresh.node(c);
                let key = (node.name.as_str(), node.class);
                let ordinal = seen.entry(key).or_default();
                let live = groups.get(&key).map_or(&[][..], Vec::as_slice);
                let dup = live.len() > 1 || fresh_groups[&key] > 1;
                if dup && fresh_groups[&key] != live.len() {
                    whole.insert(key);
                }
                let partner = live.get(*ordinal).copied().filter(|&p| !taken.contains(&p));
                let replaced = partner.is_some_and(|p| self.unwritable_change(node, self.node(p)));
                if dup && replaced {
                    whole.insert(key);
                }
                *ordinal += 1;
                paired.push((c, partner, replaced));
            }
            for (c, partner, replaced) in paired {
                let node = fresh.node(c);
                if whole.contains(&(node.name.as_str(), node.class)) {
                    order.push(c);
                    continue;
                }
                match partner {
                    Some(p) if !replaced => {
                        to_live[c as usize] = p;
                        taken.insert(p);
                        queue.push((c, p));
                    }
                    _ => {
                        let shadow = partner.or_else(|| {
                            live_children.iter().copied().find(|&s| {
                                let s = self.node(s);
                                s.name == node.name && s.class != node.class
                            })
                        });
                        if let Some(s) = shadow {
                            queue.push((c, s));
                        }
                    }
                }
                order.push(c);
            }
        }
        // nodes below an unpaired parent were not queued
        let queued: FxHashSet<Id> = order.iter().copied().collect();
        order.extend(fresh.preorder().into_iter().filter(|id| !queued.contains(id)));

        // hashes of the fresh tree, children first; a ref's path is read in the fresh tree
        for &id in order.iter().rev() {
            let hash = fresh.compute_hash(id);
            fresh.node_mut(id).hash = hash;
        }

        // 2. unpartnered nodes, shallowest first, claim a vanished node with an equal hash and its whole subtree
        {
            let mut vanished: FxHashMap<u128, Vec<Id>> = FxHashMap::default();
            for id in self.ids() {
                if !taken.contains(&id) {
                    vanished.entry(self.node(id).hash).or_default().push(id);
                }
            }
            let mut by_depth: Vec<Id> = order.iter().copied().filter(|&f| to_live[f as usize] == NONE).collect();
            by_depth.sort_by_key(|&f| fresh.depth(f));
            // a member of a duplicate (name, class) group never claims: its old id would sit at another ordinal than
            // Studio gives it (a claim carrying a whole subtree keeps the order inside it)
            let mut dup: FxHashSet<Id> = FxHashSet::default();
            let parents: FxHashSet<Id> = by_depth.iter().map(|&f| fresh.node(f).parent).collect();
            for parent in parents {
                let Some(p) = fresh.get(parent) else { continue };
                let mut count: FxHashMap<(&str, Ustr), u32> = FxHashMap::default();
                for &c in &p.children {
                    let n = fresh.node(c);
                    *count.entry((n.name.as_str(), n.class)).or_default() += 1;
                }
                dup.extend(p.children.iter().copied().filter(|&c| {
                    let n = fresh.node(c);
                    count[&(n.name.as_str(), n.class)] > 1
                }));
            }
            for f in by_depth {
                if to_live[f as usize] != NONE || dup.contains(&f) {
                    continue;
                }
                let Some(candidates) = vanished.get(&fresh.node(f).hash) else { continue };
                for &old in candidates {
                    let (a, b) = (fresh.preorder_from(f), self.preorder_from(old));
                    let free = a.len() == b.len()
                        && a.iter().all(|&x| to_live[x as usize] == NONE)
                        && b.iter().all(|y| !taken.contains(y));
                    if free {
                        for (x, y) in a.into_iter().zip(b) {
                            to_live[x as usize] = y;
                            taken.insert(y);
                        }
                        break;
                    }
                }
            }
        }

        // 3. anything left is new
        let mut next = self.next.max(1);
        for &f in &order {
            if to_live[f as usize] == NONE {
                to_live[f as usize] = next;
                next += 1;
            }
        }
        self.next = next;

        let map = |id: Id| if id == NONE { NONE } else { to_live[id as usize] };
        let mut structural = false;
        let mut kept: FxHashSet<Id> = FxHashSet::default();
        for &f in &order {
            let node = fresh.node(f);
            let id = map(f);
            kept.insert(id);
            let mut built = node.clone();
            built.parent = map(node.parent);
            built.children = node.children.iter().map(|&c| map(c)).collect();
            built.refs = node.refs.iter().map(|&(name, t)| (name, map(t))).collect();
            built.requires = Vec::new();
            match self.get(id) {
                Some(old) if same(old, &built) => {}
                old => {
                    structural |= old.is_none_or(|o| {
                        o.parent != built.parent || o.name != built.name || o.children != built.children
                    });
                    // requires are resolved below, against the finished tree
                    built.requires = old.map_or_else(Vec::new, |o| o.requires.clone());
                    self.put(id, Some(Arc::new(built)));
                }
            }
        }
        let gone: Vec<Id> = self.ids().filter(|id| !kept.contains(id)).collect();
        structural |= !gone.is_empty();
        for id in gone {
            self.put(id, None);
        }
        self.link_requires(None);
        structural
    }

    /// The content of `new` would need a property no plugin can write on a creatable class.
    fn unwritable_change(&self, new: &Node, old: &Node) -> bool {
        if !creatable(&new.class) {
            return false;
        }
        let differs = |name: &Ustr| new.prop(name) != old.prop(name);
        let names = new.props.iter().chain(&old.props).map(|(n, _)| n);
        names.filter(|n| write_kind(&new.class, n) == Write::Never).any(differs)
    }

    /// Sets re-rendered or re-read sources, then rehashes the ancestors they dirtied. Returns the ids that changed.
    pub fn set_sources(&mut self, edits: Vec<(Id, Source)>) -> Vec<Id> {
        let mut changed = Vec::new();
        for (id, source) in edits {
            let Some(node) = self.get(id) else { continue };
            if node.source.as_ref().is_some_and(|s| s.same(&source)) {
                continue;
            }
            self.node_mut(id).source = Some(source);
            changed.push(id);
        }
        self.link_requires(Some(&changed));
        let mut dirty: Vec<(usize, Id)> = if changed.is_empty() { Vec::new() } else { vec![(0, ROOT)] };
        let mut seen = FxHashSet::default();
        for &id in &changed {
            for a in self.ancestry(id) {
                if seen.insert(a) {
                    dirty.push((self.depth(a), a));
                }
            }
        }
        dirty.sort_by_key(|d| std::cmp::Reverse(d.0));
        for (_, id) in dirty {
            let hash = self.compute_hash(id);
            self.node_mut(id).hash = hash;
        }
        changed
    }

    /// Resolves the require strings of `only` (or of every script) to the modules they name now.
    fn link_requires(&mut self, only: Option<&[Id]>) {
        let ids: Vec<Id> = match only {
            Some(ids) => ids.to_vec(),
            None => self.ids().filter(|&id| self.node(id).source.is_some()).collect(),
        };
        for id in ids {
            let Some(source) = &self.node(id).source else { continue };
            let requires: Vec<Id> = source.requires.iter().filter_map(|spec| self.resolve(id, spec)).collect();
            if self.node(id).requires != requires {
                self.node_mut(id).requires = requires;
            }
        }
    }

    pub fn preorder(&self) -> Vec<Id> {
        if self.get(ROOT).is_none() {
            return Vec::new();
        }
        self.preorder_from(ROOT)
    }

    pub fn preorder_from(&self, id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            out.push(at);
            stack.extend(self.node(at).children.iter().rev());
        }
        out
    }

    /// xxh3-128 over class, props by name (metadata left out), refs as paths from the holder, the source, then each
    /// child's name and hash. A node's own name and parent are left out, so a moved subtree keeps its hash.
    fn compute_hash(&self, id: Id) -> u128 {
        let node = self.node(id);
        let mut h = Xxh3::new();
        put_str(&mut h, &node.class);
        for (name, value) in &node.props {
            if write_kind(&node.class, name) != Write::Meta {
                put_str(&mut h, name);
                hash_variant(&mut h, value);
            }
        }
        for (name, target) in &node.refs {
            put_str(&mut h, name);
            self.hash_ref_path(&mut h, id, *target);
        }
        if let Some(source) = &node.source {
            h.update(&[1]);
            h.update(&source.hash.to_le_bytes());
        }
        for &c in &node.children {
            let child = self.node(c);
            put_str(&mut h, &child.name);
            h.update(&child.hash.to_le_bytes());
        }
        h.digest128()
    }

    /// The path from `holder` to `target`: steps up to the common ancestor, then (name, class, ordinal) steps down.
    fn hash_ref_path(&self, h: &mut Xxh3, holder: Id, target: Id) {
        if self.get(target).is_none() {
            h.update(&u32::MAX.to_le_bytes());
            return;
        }
        let (from, to) = (self.ancestry(holder), self.ancestry(target));
        let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
        h.update(&((from.len() - common) as u32).to_le_bytes());
        for &step in &to[common..] {
            let node = self.node(step);
            put_str(h, &node.name);
            put_str(h, &node.class);
            h.update(&self.ordinal(step).to_le_bytes());
        }
    }

    /// The node's place among its siblings with the same name and class.
    pub fn ordinal(&self, id: Id) -> u32 {
        let node = self.node(id);
        let Some(parent) = self.get(node.parent) else { return 0 };
        let same = |&&c: &&Id| {
            let s = self.node(c);
            s.name == node.name && s.class == node.class
        };
        parent.children.iter().take_while(|&&c| c != id).filter(same).count() as u32
    }

    // --- outputs --------------------------------------------------------------------------------------------------

    /// The whole DataModel for `opossum build`.
    pub fn to_place(&self) -> Place {
        let mut place = Place::new();
        let root = place.dom.root_ref();
        let mut refs: FxHashMap<Id, Ref> = FxHashMap::default();
        refs.insert(ROOT, root);
        for id in self.preorder().into_iter().skip(1) {
            let node = self.node(id);
            let r = place.dom.insert(refs[&node.parent], self.builder(id, false));
            refs.insert(id, r);
            if let Some(file) = &node.file {
                place.set_file(r, file.to_path_buf());
            }
        }
        for (&id, &r) in &refs {
            let node = self.node(id);
            if let Some(inst) = place.dom.get_by_ref_mut(r) {
                for (name, target) in &node.refs {
                    if let Some(&t) = refs.get(target) {
                        inst.properties.insert(*name, Variant::Ref(t));
                    }
                }
            }
        }
        place
    }

    /// One instance with its props and source, without refs or children.
    pub fn builder(&self, id: Id, disabled: bool) -> InstanceBuilder {
        let node = self.node(id);
        let mut b = InstanceBuilder::with_property_capacity(node.class, node.props.len() + 1).with_name(&node.name);
        for (name, value) in &node.props {
            b.add_property(*name, value.clone());
        }
        if let Some(source) = &node.source {
            b.add_property("Source", source.text.to_string());
        }
        if disabled && node.class == "Script" {
            b.add_property("Disabled", true);
        }
        b
    }

    /// The luau-lsp sourcemap: `{name, className, filePaths, children}`, file paths relative to `base`.
    pub fn sourcemap_json(&self, base: Option<&Path>) -> String {
        self.sourcemap_node(ROOT, base).to_string()
    }

    fn sourcemap_node(&self, id: Id, base: Option<&Path>) -> serde_json::Value {
        let node = self.node(id);
        let mut map = serde_json::Map::new();
        map.insert("name".into(), node.name.clone().into());
        map.insert("className".into(), node.class.to_string().into());
        if let Some(file) = &node.file {
            let file = base.and_then(|b| file.strip_prefix(b).ok()).unwrap_or(file);
            map.insert("filePaths".into(), serde_json::json!([file.to_string_lossy()]));
        }
        if !node.children.is_empty() {
            let children = node.children.iter().map(|&c| self.sourcemap_node(c, base)).collect();
            map.insert("children".into(), serde_json::Value::Array(children));
        }
        serde_json::Value::Object(map)
    }
}

fn same(a: &Node, b: &Node) -> bool {
    a.parent == b.parent
        && a.name == b.name
        && a.class == b.class
        && a.hash == b.hash
        && a.children == b.children
        && a.refs == b.refs
        && a.props == b.props
        && a.origin == b.origin
        && a.file == b.file
        && match (&a.source, &b.source) {
            (Some(x), Some(y)) => x.same(y),
            (None, None) => true,
            _ => false,
        }
}

fn put_str(h: &mut Xxh3, s: &str) {
    h.update(&(s.len() as u32).to_le_bytes());
    h.update(s.as_bytes());
}

/// Feeds the hasher; `serde_json` writes straight into it.
struct Feed<'a>(&'a mut Xxh3);

impl std::io::Write for Feed<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A value in opossum's byte encoding: floats by their bits (serde_json writes the shortest exact form), strings and
/// binary data as bytes, a SharedString by its hash.
fn hash_variant(h: &mut Xxh3, value: &Variant) {
    match value {
        Variant::String(s) => {
            h.update(&[1]);
            put_str(h, s);
        }
        Variant::BinaryString(b) => {
            let bytes: &[u8] = b.as_ref();
            h.update(&[2]);
            h.update(&(bytes.len() as u32).to_le_bytes());
            h.update(bytes);
        }
        Variant::SharedString(s) => {
            h.update(&[3]);
            h.update(s.hash().as_bytes());
        }
        Variant::Float32(f) => {
            h.update(&[4]);
            h.update(&f.to_bits().to_le_bytes());
        }
        Variant::Float64(f) => {
            h.update(&[5]);
            h.update(&f.to_bits().to_le_bytes());
        }
        _ => {
            h.update(&[0]);
            let _ = serde_json::to_writer(Feed(h), value);
        }
    }
}

/// One Luau file of a package, `rel` relative to the package's tree root (`init.luau`, `hooks/use-x.luau`).
pub struct PackageFile {
    pub rel: String,
    pub source: Source,
    pub file: PathBuf,
}

/// `hooks/use-x.luau` -> `hooks/use-x`, `hooks/init.luau` -> `hooks`, `init.luau` -> ``.
fn module_path(rel: &str) -> Option<String> {
    let stem = rel.strip_suffix(".luau").or_else(|| rel.strip_suffix(".lua"))?;
    let stem = stem.strip_suffix("init").filter(|s| s.is_empty() || s.ends_with('/')).unwrap_or(stem);
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
mod tests;
