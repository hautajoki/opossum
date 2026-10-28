//! Frame encoding (SYNC.md, Frames): little-endian ops in fixed order, and the blob, a subset `WeakDom` written by
//! rbx_binary whose roots are the inserted subtrees, then the swapped instances, then the property carriers.

use place::props::{Write, is_a, verified, write_kind};
use place::rbx_dom_weak::types::{Attributes, Color3, EnumItem, Ref, Tags, Variant};
use place::rbx_dom_weak::{InstanceBuilder, WeakDom};
use place::{Id, NONE, Node, Tree, Ustr, Variant as V, descriptor, ustr};
use rbx_reflection::DataType;
use rustc_hash::FxHashMap;

pub const BIND: u8 = 1;
pub const HYDRATE: u8 = 2;
pub const CHILDREN: u8 = 3;
pub const PROPS: u8 = 4;
pub const SOURCE: u8 = 5;
pub const MOVE: u8 = 6;
pub const REF: u8 = 7;
pub const REMOVE: u8 = 8;
pub const STAMP: u8 = 9;
pub const SWAP: u8 = 10;
pub const RELOAD: u8 = 11;

/// Frame header flags: the generation's last part, and a generation that answers a quiet `verify` (an undo's).
pub const LAST: u8 = 1;
pub const QUIET: u8 = 2;

pub const RECONCILE: u8 = 1;
pub const AUTHORITATIVE: u8 = 2;

/// Ops of one kind, already encoded.
#[derive(Default)]
pub struct Section {
    pub bytes: Vec<u8>,
    pub count: u32,
}

impl Section {
    pub fn op(&mut self, tag: u8) -> &mut Vec<u8> {
        self.count += 1;
        self.bytes.push(tag);
        &mut self.bytes
    }
}

pub fn u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn str(out: &mut Vec<u8>, s: &str) {
    u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// Assembles one frame part: header (`flags` of [`LAST`] and [`QUIET`]), blob, then the sections in the order given.
pub fn assemble(generation: u32, base: u32, flags: u8, blob: &[u8], sections: &[&Section]) -> Vec<u8> {
    let size: usize = sections.iter().map(|s| s.bytes.len()).sum();
    let mut out = Vec::with_capacity(17 + blob.len() + size);
    u32(&mut out, generation);
    u32(&mut out, base);
    out.push(flags);
    u32(&mut out, blob.len() as u32);
    out.extend_from_slice(blob);
    u32(&mut out, sections.iter().map(|s| s.count).sum());
    for s in sections {
        out.extend_from_slice(&s.bytes);
    }
    out
}

/// A `hydrate` node and its children, preorder, from `tree`.
pub fn hydrate_record(out: &mut Vec<u8>, tree: &Tree, id: Id, rename: &dyn Fn(Id) -> Id) {
    let node = tree.get(id).expect("a hydrated id");
    u32(out, rename(id));
    str(out, &node.name);
    str(out, &node.class);
    u32(out, node.children.len() as u32);
    for &c in &node.children {
        hydrate_record(out, tree, c, rename);
    }
}

/// The names `copy` takes for all of a node's properties: each group once, `Source` for a script.
pub fn copy_names(node: &Node) -> Vec<Ustr> {
    let mut out: Vec<Ustr> = Vec::new();
    for (name, _) in &node.props {
        match write_kind(&node.class, name) {
            Write::Name => out.push(*name),
            Write::Group(g) if !out.contains(&ustr(g)) => out.push(ustr(g)),
            _ => {}
        }
    }
    if node.source.is_some() {
        out.push(ustr("Source"));
    }
    out
}

/// The `copy` names whose values differ between `old` and `new`, and the changed names no plugin can write.
pub fn changed_names(old: &Node, new: &Node) -> (Vec<Ustr>, Vec<Ustr>) {
    let (mut names, mut never) = (Vec::new(), Vec::new());
    let mut seen: Vec<Ustr> = Vec::new();
    for (name, _) in old.props.iter().chain(&new.props) {
        if seen.contains(name) || old.prop(name) == new.prop(name) {
            continue;
        }
        seen.push(*name);
        match write_kind(&new.class, name) {
            Write::Meta => {}
            Write::Name => names.push(*name),
            Write::Group(g) if !names.contains(&ustr(g)) => names.push(ustr(g)),
            Write::Group(_) => {}
            Write::Never => never.push(*name),
        }
    }
    (names, never)
}

/// A property as an attribute of a carrier Folder (carrier 1): enums as EnumItem, every number as a float64. None
/// when an attribute cannot hold it.
pub fn attribute(class: &str, name: &str, value: &Variant) -> Option<Variant> {
    Some(match value {
        V::Bool(_) | V::String(_) | V::Float64(_) | V::Color3(_) | V::Vector2(_) | V::Vector3(_) | V::UDim(_) => {
            value.clone()
        }
        V::UDim2(_) | V::CFrame(_) | V::NumberSequence(_) | V::ColorSequence(_) | V::NumberRange(_) => value.clone(),
        V::Rect(_) | V::BrickColor(_) | V::Font(_) | V::EnumItem(_) => value.clone(),
        V::Float32(f) => V::Float64(f64::from(*f)),
        V::Int32(i) => V::Float64(f64::from(*i)),
        V::ContentId(id) => V::String(id.as_str().to_owned()),
        V::OptionalCFrame(Some(cf)) => V::CFrame(*cf),
        V::Color3uint8(c) => {
            V::Color3(Color3::new(f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0))
        }
        V::Enum(e) => {
            let DataType::Enum(ty) = descriptor(class, name)?.data_type else { return None };
            V::EnumItem(EnumItem { ty: ty.to_string(), value: e.to_u32() })
        }
        _ => return None,
    })
}

/// The instances a frame carries, as roots in the order their ops consume them.
#[derive(Default)]
pub struct Blob {
    dom: Option<WeakDom>,
    inserts: Vec<Ref>,
    swaps: Vec<Ref>,
    carriers: Vec<Ref>,
    /// Inserted instances by id: refs between them travel in the blob.
    placed: FxHashMap<Id, Ref>,
    held: Vec<(Id, Ref, Ustr, Id)>,
    /// Refs of reconciled instances, which are all `ref` ops: a paired live instance takes nothing from its blob copy.
    reconciled: Vec<(Id, Ustr, Id)>,
    /// [`verified`] by class.
    classes: FxHashMap<Ustr, Vec<Ustr>>,
}

impl Blob {
    fn dom(&mut self) -> &mut WeakDom {
        self.dom.get_or_insert_with(|| WeakDom::new(InstanceBuilder::new("DataModel")))
    }

    /// Places `top` and the descendants `include` admits as one root, writing its `children` node record; returns the
    /// ids it placed. A `reconcile` placement may pair with live instances, so nothing refers to its blob copies: its
    /// refs and the refs into it become `ref` ops, and each record lists every property a verify copies, so the blob
    /// copy's defaults reset what the node leaves unset (a nil ref included).
    pub fn insert(
        &mut self,
        tree: &Tree,
        top: Id,
        include: &dyn Fn(Id) -> bool,
        reconcile: bool,
        record: &mut Vec<u8>,
    ) -> Vec<Id> {
        let mut placed = Vec::new();
        let root = self.dom().root_ref();
        let r = self.place(tree, top, root, include, reconcile, record, &mut placed);
        self.inserts.push(r);
        placed
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        tree: &Tree,
        id: Id,
        parent: Ref,
        include: &dyn Fn(Id) -> bool,
        reconcile: bool,
        record: &mut Vec<u8>,
        placed: &mut Vec<Id>,
    ) -> Ref {
        let node = tree.get(id).expect("an inserted id");
        let r = self.dom().insert(parent, tree.builder(id, false));
        placed.push(id);
        if reconcile {
            self.reconciled.extend(node.refs.iter().map(|&(name, target)| (id, name, target)));
        } else {
            self.placed.insert(id, r);
            self.held.extend(node.refs.iter().map(|&(name, target)| (id, r, name, target)));
        }
        u32(record, id);
        let mut list = copy_names(node);
        if reconcile {
            let all = self.classes.entry(node.class).or_insert_with(|| verified(&node.class));
            let set = |n: &Ustr| node.prop(n).is_some() || node.refs.iter().any(|(r, _)| r == n);
            list.extend(all.iter().filter(|n| !set(n)));
        }
        record.extend_from_slice(&(list.len() as u16).to_le_bytes());
        for name in &list {
            str(record, name);
        }
        let children: Vec<Id> = node.children.iter().copied().filter(|&c| include(c)).collect();
        u32(record, children.len() as u32);
        for c in children {
            self.place(tree, c, r, include, reconcile, record, placed);
        }
        r
    }

    /// The whole new instance for a hot-reload swap: the node alone, a Script carried disabled.
    pub fn swap(&mut self, tree: &Tree, id: Id) {
        let root = self.dom().root_ref();
        let r = self.dom().insert(root, tree.builder(id, true));
        self.swaps.push(r);
    }

    /// Carrier 0: the node alone in its own class, holding its values.
    pub fn carrier(&mut self, tree: &Tree, id: Id) {
        let root = self.dom().root_ref();
        let r = self.dom().insert(root, tree.builder(id, false));
        self.carriers.push(r);
    }

    /// Whether carrier 1 can carry `name` of `node`: not a value an attribute cannot hold, nor a removed property
    /// (an attribute cannot say "the class default").
    pub fn folder_carries(node: &Node, name: &str) -> bool {
        matches!(name, "Attributes" | "Tags") || node.prop(name).and_then(|v| attribute(&node.class, name, v)).is_some()
    }

    /// Carrier 1, for a frame of reference: a Folder whose attributes are the values of `names` (each one
    /// [`Blob::folder_carries`]), with a child Folder `Attributes` holding the node's attributes and tags.
    pub fn folder_carrier(&mut self, node: &Node, names: &[Ustr]) {
        let mut values = Attributes::new();
        let mut own: Option<InstanceBuilder> = None;
        for &name in names {
            match name.as_str() {
                "Attributes" | "Tags" => {
                    let b = own.get_or_insert_with(|| InstanceBuilder::new("Folder").with_name("Attributes"));
                    if let Some(v @ (V::Attributes(_) | V::Tags(_))) = node.prop(&name) {
                        b.add_property(name, v.clone());
                    }
                }
                _ => {
                    if let Some(v) = node.prop(&name).and_then(|v| attribute(&node.class, &name, v)) {
                        values.insert(name.to_string(), v);
                    }
                }
            }
        }
        let mut folder =
            InstanceBuilder::new("Folder").with_name(node.class.as_str()).with_property("Attributes", values);
        if let Some(mut own) = own {
            if !own.has_property("Tags") {
                own.add_property("Tags", Tags::new());
            }
            folder.add_child(own);
        }
        let root = self.dom().root_ref();
        let r = self.dom().insert(root, folder);
        self.carriers.push(r);
    }

    /// The rbx_binary bytes, and the refs that travel as `ref` ops: `(holder, prop, target)`, a target that is gone
    /// as nil.
    pub fn finish(mut self, tree: &Tree) -> (Vec<u8>, Vec<(Id, Ustr, Id)>) {
        let live = |target: Id| if tree.get(target).is_some() { target } else { NONE };
        let mut outside: Vec<(Id, Ustr, Id)> = self.reconciled.iter().map(|&(h, n, t)| (h, n, live(t))).collect();
        let Some(mut dom) = self.dom.take() else { return (Vec::new(), outside) };
        for (holder, r, name, target) in self.held {
            match self.placed.get(&target) {
                Some(&t) => {
                    if let Some(inst) = dom.get_by_ref_mut(r) {
                        inst.properties.insert(name, Variant::Ref(t));
                    }
                }
                None => outside.push((holder, name, live(target))),
            }
        }
        let roots: Vec<Ref> = self.inserts.into_iter().chain(self.swaps).chain(self.carriers).collect();
        let mut out = Vec::new();
        if let Err(e) = rbx_binary::to_writer(&mut out, &dom, &roots) {
            eprintln!("opossum: writing a sync blob: {e}");
        }
        (out, outside)
    }
}

/// Whether `class` is carried by a Folder (carrier 1): a service, or a class no blob can hold.
pub fn folder_carried(tree: &Tree, id: Id) -> bool {
    tree.is_frame(id) || tree.get(id).is_some_and(|n| is_a(&n.class, "ServiceProvider"))
}
