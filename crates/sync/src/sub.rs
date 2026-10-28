//! One subscriber (a DataModel's socket): the tree it is known to hold, and the frames that take it to the current
//! tree (SYNC.md, Sessions). Its writer thread owns it, so nothing here locks.

use std::collections::hash_map::Entry;
use std::sync::Arc;

use place::props::creatable;
use place::{Id, NESTED, NONE, ROOT, Tree, Ustr};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::frame::{self, AUTHORITATIVE, Blob, RECONCILE, Section, changed_names, copy_names, folder_carried};

/// A frame splits above this many new instances or bytes of new content.
const MAX_INSTANCES: usize = 10_000;
const MAX_BYTES: usize = 16 << 20;

const FORCE_PROPS: u8 = 1;
const FORCE_SOURCE: u8 = 2;
const FORCE_REFS: u8 = 4;
/// Diff the node although its pointer did not change (a failed move or remove, a lost instance).
const FORCE_NODE: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Edit,
    Server,
    Client,
}

/// What a subscriber's writer reads from the server when it builds a frame.
pub struct Context<'a> {
    pub cur: &'a Tree,
    /// The last generations' trees, oldest first.
    pub ring: &'a [Tree],
    pub hot_reload: bool,
    /// Prints a line in the dev terminal; the same text is printed once per server.
    pub notice: &'a dyn Fn(String),
    /// Prints a failure in the dev terminal.
    pub say: &'a dyn Fn(String),
}

/// A frame waiting for its ack.
pub struct Sent {
    generation: u32,
    prev: Tree,
    /// Ids the frame's blob inserted, by `children` op.
    inserted: FxHashSet<Id>,
    /// Ids a reconcile placed, with the root it verified: a failure there verifies the root again.
    reconciled: FxHashMap<Id, Id>,
    consumers: usize,
    /// A connect frame's hello: if the frame never applies, the subscriber connects again.
    hello: Option<Vec<(String, String)>>,
}

pub struct Sub {
    pub role: Role,
    known: Tree,
    /// The last generation sent.
    pub base: u32,
    force: FxHashMap<Id, u8>,
    /// The content hash a node last failed with; failing again with it parks the node.
    fails: FxHashMap<Id, u128>,
    parked: FxHashMap<Id, u128>,
    /// Play server: the roots it hydrated, and nodes that did not pair.
    scope: FxHashSet<Id>,
    dropped: FxHashSet<Id>,
    /// Roots to verify in the next frame.
    verify: Vec<Id>,
    /// A blob threw: one insert or verify per frame until no work waits.
    isolate: bool,
    pub sent: Option<Sent>,
    hello: Option<Vec<(String, String)>>,
    /// Throwaway ids for instances only removed (counting down from below NONE).
    temp: Id,
}

#[derive(Default)]
struct Plan {
    binds: Section,
    hydrates: Section,
    /// Roots verified: a reconcile under their frame of reference.
    reconciles: Vec<Id>,
    /// Subtrees a verify leaves out: the subscriber holds them elsewhere, and they move in.
    foreign: FxHashSet<Id>,
    tops: Vec<Id>,
    moves: Vec<Id>,
    swaps: Vec<Id>,
    props: Vec<(Id, Vec<Ustr>)>,
    sources: Vec<Id>,
    refs: Vec<(Id, Ustr, Id)>,
    removes: Vec<Id>,
    touched: FxHashSet<Id>,
    /// Roots that must stay unstamped, as Studio will not hold their tree after this frame.
    unstamped: FxHashSet<Id>,
    /// Current subtrees left out of the known tree (parked or deferred inserts).
    absent: Vec<Id>,
    /// Known nodes kept as they are (parked).
    keep: Vec<Id>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.binds.count == 0
            && self.hydrates.count == 0
            && self.reconciles.is_empty()
            && self.tops.is_empty()
            && self.moves.is_empty()
            && self.swaps.is_empty()
            && self.props.is_empty()
            && self.sources.is_empty()
            && self.refs.is_empty()
            && self.removes.is_empty()
    }
}

pub fn hex(hash: u128) -> String {
    format!("{hash:032x}")
}

/// One `children` op's worth of blob roots: a subtree inserted, or reconciled (a verified root, or a new root adopting
/// what Studio has at its place).
struct Unit {
    parent: Id,
    top: Id,
    reconcile: bool,
    /// A verified root, whose subtree is everything but the foreign parts.
    verify: bool,
    count: usize,
    bytes: usize,
}

impl Sub {
    pub fn new() -> Sub {
        Sub {
            role: Role::Client,
            known: Tree::empty(),
            base: 0,
            force: FxHashMap::default(),
            fails: FxHashMap::default(),
            parked: FxHashMap::default(),
            scope: FxHashSet::default(),
            dropped: FxHashSet::default(),
            verify: Vec::new(),
            isolate: false,
            sent: None,
            hello: None,
            temp: NONE - 1,
        }
    }

    /// The plugin's hello: starts over, connecting at the next frame when this role receives frames.
    pub fn hello(&mut self, role: Role, roots: Vec<(String, String)>, hot_reload: bool) {
        *self = Sub::new();
        self.role = role;
        if role == Role::Edit || (role == Role::Server && hot_reload) {
            self.hello = Some(roots);
        }
    }

    /// The plugin's `verify` (the Resync button, or a frame it dropped or could not apply): the wait for an ack ends.
    /// Edit verifies every root. The play server, which never verifies, is sent again what the frame in flight
    /// carried; a connect frame that never applied connects again.
    pub fn resync(&mut self, cur: &Tree) {
        let sent = self.sent.take();
        if self.hello.is_some() {
            return;
        }
        if let Some(hello) = sent.as_ref().and_then(|s| s.hello.clone()) {
            self.hello = Some(hello);
        }
        match self.role {
            Role::Edit => {
                self.verify = cur.roots();
                self.parked.clear();
                self.fails.clear();
                for f in cur.frames() {
                    *self.force.entry(f).or_default() |= FORCE_PROPS;
                }
            }
            Role::Server if self.hello.is_none() => {
                if let Some(sent) = sent {
                    self.known = sent.prev;
                }
            }
            _ => {}
        }
    }

    /// Instances someone else removed: they leave the known tree, so the next frame puts them back, and refs into
    /// them are sent again. The play server's game code removes synced instances on purpose, so it never reports any.
    pub fn lost(&mut self, cur: &Tree, ids: &[Id]) {
        if self.role != Role::Edit {
            return;
        }
        let mut gone = FxHashSet::default();
        for &id in ids {
            gone.extend(drop_subtree(&mut self.known, id));
        }
        self.refs_into(cur, &gone);
    }

    /// Marks every node outside `gone` whose refs point into it, so its refs are sent again.
    fn refs_into(&mut self, cur: &Tree, gone: &FxHashSet<Id>) {
        if gone.is_empty() {
            return;
        }
        for id in cur.ids() {
            let node = cur.get(id).expect("a live id");
            if !gone.contains(&id) && node.refs.iter().any(|(_, t)| gone.contains(t)) {
                *self.force.entry(id).or_default() |= FORCE_REFS;
            }
        }
    }

    /// The frame parts that take the subscriber from what it holds to `cx.cur`, or None when nothing differs.
    pub fn frame(&mut self, cx: &Context, generation: u32) -> Option<Vec<Vec<u8>>> {
        let cur = cx.cur;
        let mut plan = Plan::default();
        let hello = self.hello.take();
        let mut known = match &hello {
            Some(roots) => self.connect(cx, roots.clone(), &mut plan),
            None if self.known.ids().next().is_none() => return None,
            None => self.known.clone(),
        };
        let mut verify = std::mem::take(&mut self.verify);
        verify.sort_unstable();
        verify.dedup();
        // still a root, and not parked with what it holds now
        verify.retain(|&r| cur.root_of(r) == Some(r) && self.parked.get(&r) != cur.get(r).map(|n| &n.hash));
        if self.isolate && verify.len() > 1 {
            self.verify = verify.split_off(1);
        }
        for root in verify {
            self.verify_root(cur, root, &mut known, &mut plan);
        }
        self.diff(cx, &known, &mut plan);
        if self.isolate {
            if plan.reconciles.len() + plan.tops.len() > 1 || !self.verify.is_empty() {
                let keep = usize::from(plan.reconciles.is_empty()).min(plan.tops.len());
                plan.absent.extend(plan.tops.drain(keep..));
            } else {
                self.isolate = false;
            }
        }
        // a parked node keeps what made it fail, to be sent when it changes
        self.force.retain(|id, _| self.parked.contains_key(id));
        if plan.is_empty() && hello.is_none() {
            self.known = known;
            return None;
        }
        Some(self.encode(cx, known, plan, generation, hello))
    }

    fn connect(&mut self, cx: &Context, roots: Vec<(String, String)>, plan: &mut Plan) -> Tree {
        let cur = cx.cur;
        let edit = self.role == Role::Edit;
        let mut known = Tree::empty();
        known.put(ROOT, cur.arc(ROOT).cloned());
        for f in cur.frames() {
            let node = cur.get(f).expect("a frame");
            bind(plan, f, node.parent, &node.class, &node.name);
            known.put(f, cur.arc(f).cloned());
            if edit {
                self.force.insert(f, FORCE_PROPS);
            }
        }
        let by_path: FxHashMap<String, Id> = cur.roots().into_iter().map(|r| (cur.path_of(r), r)).collect();
        let mut listed: FxHashSet<Id> = FxHashSet::default();
        for (path, stamp) in roots {
            let hash = u128::from_str_radix(&stamp, 16).ok().filter(|_| stamp.len() == 32);
            let Some(&root) = by_path.get(&path) else {
                if edit {
                    self.orphan(cx, &path, plan);
                }
                continue;
            };
            listed.insert(root);
            let node = cur.get(root).expect("a root");
            let from = match hash {
                Some(h) if h == node.hash => Some((cur, root)),
                Some(h) => cx.ring.iter().rev().find_map(|t| {
                    let old = t.find(&path)?;
                    (t.get(old)?.hash == h && t.root_of(old) == Some(old)).then_some((t, old))
                }),
                None => None,
            };
            match from {
                Some((tree, old)) => {
                    self.hydrate(tree, old, node.parent, &mut known, plan);
                    self.scope.insert(root);
                    if !std::ptr::eq(tree, cur) {
                        plan.touched.insert(root);
                    }
                }
                None if edit => self.verify.push(root),
                None => {}
            }
        }
        if edit {
            // a root the hello lacks is verified too: the reconcile under its frame of reference adopts an unstamped
            // instance with its name and class (left by `opossum build`, an older sync, or made by hand) or inserts it
            self.verify.extend(cur.roots().into_iter().filter(|r| !listed.contains(r)));
        }
        known
    }

    /// Maps the live root and its subtree to the ids `tree` has for them. An id the known tree already holds (it
    /// moved between roots across generations) maps to a throwaway id that is removed.
    fn hydrate(&mut self, tree: &Tree, root: Id, frame: Id, known: &mut Tree, plan: &mut Plan) {
        let mut rename: FxHashMap<Id, Id> = FxHashMap::default();
        for id in tree.preorder_from(root) {
            if known.get(id).is_some() {
                rename.insert(id, self.temp);
                plan.removes.push(self.temp);
                self.temp -= 1;
            } else {
                known.put(id, tree.arc(id).cloned());
            }
        }
        let out = plan.hydrates.op(frame::HYDRATE);
        frame::u32(out, frame);
        frame::u32(out, 1);
        frame::hydrate_record(out, tree, root, &|id| rename.get(&id).copied().unwrap_or(id));
    }

    /// A stamped instance that is no longer a root: bound by its path and removed. The class is `""`, which binds
    /// only an instance carrying the `opossum` attribute: what the hello listed, never the user's own.
    fn orphan(&mut self, cx: &Context, path: &str, plan: &mut Plan) {
        let parts: Vec<&str> = path.split('/').collect();
        let nested = parts.len() >= 3 && NESTED.contains(&(parts[0], parts[1]));
        let (frames, name) =
            if nested { (&parts[..2], parts[2..].join("/")) } else { (&parts[..1], parts[1..].join("/")) };
        let mut parent = ROOT;
        for (i, &f) in frames.iter().enumerate() {
            parent = match cx.cur.find(&frames[..=i].join("/")) {
                Some(id) => id,
                None => {
                    let id = self.temp;
                    self.temp -= 1;
                    bind(plan, id, parent, f, f);
                    id
                }
            };
        }
        let id = self.temp;
        self.temp -= 1;
        bind(plan, id, parent, "", &name);
        plan.removes.push(id);
        (cx.notice)(format!("removing {path}: it is no longer a root"));
    }

    /// A reconcile of the root under its frame of reference, every node's properties listed. What the subscriber
    /// holds under the root and the tree no longer has is removed; what it holds elsewhere and the tree now has here
    /// is left out of the reconcile and moves in, keeping its identity.
    fn verify_root(&mut self, cur: &Tree, root: Id, known: &mut Tree, plan: &mut Plan) {
        if cur.get(root).is_none() {
            return;
        }
        let inside: FxHashSet<Id> = cur.preorder_from(root).into_iter().collect();
        let held = subtree(known, root);
        let mut gone: FxHashSet<Id> = FxHashSet::default();
        for &id in &held {
            if cur.get(id).is_none() {
                if !gone.contains(&known.get(id).map_or(NONE, |n| n.parent)) {
                    plan.removes.push(id);
                }
                gone.insert(id);
            }
        }
        // what stays under the root is verified again; what the tree moved out of it stays known and moves out, and
        // so does what stays below something that leaves (it moves back in rather than riding out with it)
        let stays = |id: Id| {
            let mut at = id;
            while at != root {
                if !inside.contains(&at) {
                    return false;
                }
                at = known.get(at).map_or(root, |n| n.parent);
            }
            true
        };
        let again: Vec<Id> = held.iter().copied().filter(|&id| gone.contains(&id) || stays(id)).collect();
        for id in again {
            known.put(id, None);
        }
        // anything still known here is held under another root: left out, it moves in
        let reconciled = preorder_where(cur, root, &|c| known.get(c).is_none());
        for &id in &reconciled {
            let children = &cur.get(id).expect("a live id").children;
            plan.foreign.extend(children.iter().copied().filter(|&c| known.get(c).is_some()));
        }
        for id in reconciled {
            known.put(id, cur.arc(id).cloned());
        }
        plan.reconciles.push(root);
        plan.touched.insert(root);
    }

    /// Whether the play server follows this node: frames, and what is under a root it hydrated.
    fn tracked(&self, cur: &Tree, id: Id) -> bool {
        if self.role != Role::Server {
            return true;
        }
        if cur.is_frame(id) || id == ROOT {
            return true;
        }
        let mut at = id;
        while let Some(node) = cur.get(at) {
            if self.dropped.contains(&at) {
                return false;
            }
            if self.scope.contains(&at) {
                return true;
            }
            at = node.parent;
        }
        false
    }

    fn diff(&mut self, cx: &Context, known: &Tree, plan: &mut Plan) {
        let cur = cx.cur;
        let mut ids = known.differs(cur);
        ids.extend(self.force.keys().copied());
        ids.sort_unstable();
        ids.dedup();
        let edit = self.role == Role::Edit;
        // a new frame of reference (a service `[place]` started setting) is bound, never inserted
        let bound: FxHashSet<Id> =
            ids.iter().copied().filter(|&id| known.get(id).is_none() && cur.is_frame(id)).collect();
        // a root of `cur` that a removed known node was in, and a moved node's old root, lose their stamps in Studio
        let still_root = |r: &Id| cur.root_of(*r) == Some(*r);
        for id in ids {
            if id == ROOT {
                continue;
            }
            let force = self.force.get(&id).copied().unwrap_or(0);
            let (k, c) = (known.get(id), cur.get(id));
            if let Some(c) = c
                && let Some(&h) = self.parked.get(&id)
            {
                if h == c.hash {
                    match k {
                        Some(_) => plan.keep.push(id),
                        None => plan.absent.push(id),
                    }
                    continue;
                }
                self.parked.remove(&id);
            }
            if c.is_some() && !self.tracked(cur, id) {
                continue;
            }
            match (k, c) {
                (None, None) => {}
                (None, Some(c)) if bound.contains(&id) => {
                    bind(plan, id, c.parent, &c.class, &c.name);
                    let names = copy_names(c);
                    if !names.is_empty() {
                        plan.props.push((id, names));
                    }
                }
                // a root still waiting for its verify (one per frame while isolating) is neither inserted nor moved
                (None, Some(_)) if self.verify.contains(&id) => {}
                (None, Some(c)) => {
                    if known.get(c.parent).is_some() || bound.contains(&c.parent) {
                        plan.tops.push(id);
                    }
                }
                // a frame of reference is never removed, only what the tree had under it
                (Some(_), None) if known.is_frame(id) => {}
                (Some(k), None) => {
                    if known.get(k.parent).is_none() || cur.get(k.parent).is_some() || known.is_frame(k.parent) {
                        plan.removes.push(id);
                        plan.touched.extend(known.root_of(id).filter(still_root));
                    }
                }
                (Some(k), Some(c)) => {
                    let frame = cur.is_frame(id);
                    if !frame && (k.parent != c.parent || k.name != c.name) {
                        plan.moves.push(id);
                        plan.touched.extend(known.root_of(id).filter(still_root));
                    }
                    let (mut names, never) = changed_names(k, c);
                    if force & FORCE_PROPS != 0 {
                        names = copy_names(c);
                    }
                    for name in &never {
                        let why = if frame || !creatable(&c.class) { "build-only" } else { "not writable" };
                        (cx.notice)(format!("{}.{name} is {why}: it reaches Studio through opossum build", c.class));
                    }
                    // the tree replaces a creatable instance whose unwritable property changed (a new id), so this
                    // is only a node it could not pair; Studio keeps the old value, so its root is not certified
                    if !never.is_empty() && !frame && creatable(&c.class) {
                        plan.unstamped.extend(cur.root_of(id));
                    }
                    names.retain(|n| n.as_str() != "Source");
                    if !names.is_empty() {
                        plan.props.push((id, names));
                    }
                    let source_changed = match (&k.source, &c.source) {
                        (Some(a), Some(b)) => !Arc::ptr_eq(&a.text, &b.text) && a.hash != b.hash,
                        (_, Some(_)) => true,
                        _ => false,
                    };
                    if c.source.is_some() && (source_changed || force & FORCE_SOURCE != 0) {
                        if edit || !cx.hot_reload {
                            plan.sources.push(id);
                        } else {
                            plan.swaps.push(id);
                        }
                    }
                    let mut names: Vec<Ustr> = k.refs.iter().chain(&c.refs).map(|(n, _)| *n).collect();
                    names.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
                    names.dedup();
                    for name in names {
                        let target = |refs: &[(Ustr, Id)]| refs.iter().find(|(n, _)| *n == name).map_or(NONE, |r| r.1);
                        let to = target(&c.refs);
                        if to != target(&k.refs) || force & FORCE_REFS != 0 {
                            plan.refs.push((id, name, if cur.get(to).is_some() { to } else { NONE }));
                        }
                    }
                }
            }
        }
        if self.role == Role::Server && !plan.swaps.is_empty() {
            self.importers(cur, plan);
        }
        if edit {
            for &id in plan.tops.iter().chain(&plan.moves).chain(&plan.sources).chain(&plan.reconciles) {
                plan.touched.extend(cur.root_of(id));
            }
            for (id, _) in &plan.props {
                plan.touched.extend(cur.root_of(*id));
            }
            for (id, _, _) in &plan.refs {
                plan.touched.extend(cur.root_of(*id));
            }
        }
    }

    /// Hot reload: every transitive importer of a swapped module is swapped too, up to the Scripts, which restart.
    fn importers(&self, cur: &Tree, plan: &mut Plan) {
        let mut importers: FxHashMap<Id, Vec<Id>> = FxHashMap::default();
        for id in cur.ids() {
            for &target in &cur.get(id).expect("a live id").requires {
                importers.entry(target).or_default().push(id);
            }
        }
        let mut swapped: FxHashSet<Id> = plan.swaps.iter().copied().collect();
        let mut queue = plan.swaps.clone();
        while let Some(id) = queue.pop() {
            for &importer in importers.get(&id).map_or(&[][..], Vec::as_slice) {
                if self.tracked(cur, importer) && swapped.insert(importer) {
                    queue.push(importer);
                }
            }
        }
        let mut swaps: Vec<Id> = swapped.into_iter().collect();
        swaps.sort_by_key(|&id| (cur.depth(id), id));
        plan.props.retain(|(id, _)| !swaps.contains(id));
        plan.sources.retain(|id| !swaps.contains(id));
        for &id in &swaps {
            let node = cur.get(id).expect("a swapped id");
            for &(name, target) in &node.refs {
                plan.refs.push((id, name, if cur.get(target).is_some() { target } else { NONE }));
            }
        }
        plan.swaps = swaps;
    }

    fn encode(
        &mut self,
        cx: &Context,
        known: Tree,
        plan: Plan,
        generation: u32,
        hello: Option<Vec<(String, String)>>,
    ) -> Vec<Vec<u8>> {
        let cur = cx.cur;
        let edit = self.role == Role::Edit;
        let absent: FxHashSet<Id> = plan.absent.iter().flat_map(|&a| cur.preorder_from(a)).collect();
        let new = |id: Id| known.get(id).is_none() && !absent.contains(&id);

        // verified roots, then inserts as (parent, top) units, parents first; an insert over the limits splits into
        // its shell and children. In edit a new root is reconciled under its frame, adopting what is at its place.
        let mut units: Vec<Unit> = Vec::new();
        for &root in &plan.reconciles {
            let subtree = preorder_where(cur, root, &|c| !plan.foreign.contains(&c));
            let bytes = subtree.iter().map(|&id| weight(cur, id)).sum();
            let parent = cur.get(root).expect("a verified root").parent;
            units.push(Unit { parent, top: root, reconcile: true, verify: true, count: subtree.len(), bytes });
        }
        let mut tops = plan.tops.clone();
        tops.sort_by_key(|&t| {
            let parent = cur.get(t).expect("a new id").parent;
            let at = cur.get(parent).map_or(0, |p| p.children.iter().position(|&c| c == t).unwrap_or(0));
            (cur.depth(parent), parent, at)
        });
        let mut shells: FxHashSet<Id> = FxHashSet::default();
        let mut queue: std::collections::VecDeque<Id> = tops.into();
        while let Some(top) = queue.pop_front() {
            let subtree: Vec<Id> = preorder_where(cur, top, &new);
            let bytes: usize = subtree.iter().map(|&id| weight(cur, id)).sum();
            let parent = cur.get(top).expect("a new id").parent;
            let adopt = edit && cur.is_frame(parent);
            if !adopt && (subtree.len() > MAX_INSTANCES || bytes > MAX_BYTES) {
                shells.insert(top);
                units.push(Unit { parent, top, reconcile: false, verify: false, count: 1, bytes: weight(cur, top) });
                queue.extend(cur.get(top).expect("a new id").children.iter().copied().filter(|&c| new(c)));
            } else {
                units.push(Unit { parent, top, reconcile: adopt, verify: false, count: subtree.len(), bytes });
            }
        }
        let include = |id: Id| new(id) && !shells.contains(&cur.get(id).map_or(NONE, |n| n.parent));
        let outside_foreign = |id: Id| !plan.foreign.contains(&id);

        struct Part {
            blob: Blob,
            children: Section,
            count: usize,
            bytes: usize,
        }
        let part = || Part { blob: Blob::default(), children: Section::default(), count: 0, bytes: 0 };
        let mut parts = vec![part()];
        let mut inserted: FxHashSet<Id> = FxHashSet::default();
        let mut reconciled: FxHashMap<Id, Id> = FxHashMap::default();
        // consecutive units under one parent with the same flags share one op
        let mut pending: Option<(Id, u8, Vec<u8>, u32)> = None;
        let flush = |part: &mut Part, pending: &mut Option<(Id, u8, Vec<u8>, u32)>| {
            if let Some((parent, flags, records, n)) = pending.take() {
                let out = part.children.op(frame::CHILDREN);
                frame::u32(out, parent);
                out.push(flags);
                frame::u32(out, n);
                out.extend_from_slice(&records);
            }
        };
        for unit in &units {
            let full = {
                let part = parts.last().expect("a part");
                part.count > 0 && (part.count + unit.count > MAX_INSTANCES || part.bytes + unit.bytes > MAX_BYTES)
            };
            if full {
                flush(parts.last_mut().expect("a part"), &mut pending);
                parts.push(part());
            }
            let part = parts.last_mut().expect("a part");
            let flags = if unit.reconcile { RECONCILE | AUTHORITATIVE } else { 0 };
            if pending.as_ref().is_some_and(|p| (p.0, p.1) != (unit.parent, flags)) {
                flush(part, &mut pending);
            }
            let (_, _, records, n) = pending.get_or_insert_with(|| (unit.parent, flags, Vec::new(), 0));
            let filter: &dyn Fn(Id) -> bool = if unit.verify { &outside_foreign } else { &include };
            let placed = part.blob.insert(cur, unit.top, filter, unit.reconcile, records);
            *n += 1;
            part.count += unit.count;
            part.bytes += unit.bytes;
            if unit.reconcile {
                reconciled.extend(placed.into_iter().map(|id| (id, unit.top)));
            } else {
                inserted.extend(placed);
            }
        }
        flush(parts.last_mut().expect("a part"), &mut pending);

        // everything else rides the last part
        let last = parts.last_mut().expect("a part");
        let mut moves = Section::default();
        let mut ordered = plan.moves.clone();
        ordered.sort_by_key(|&id| (cur.depth(id), id));
        for id in ordered {
            let node = cur.get(id).expect("a moved id");
            let out = moves.op(frame::MOVE);
            frame::u32(out, id);
            frame::u32(out, node.parent);
            frame::str(out, &node.name);
        }
        let mut swaps = Section::default();
        for &id in &plan.swaps {
            last.blob.swap(cur, id);
            frame::u32(swaps.op(frame::SWAP), id);
        }
        let mut props = Section::default();
        for (id, names) in &plan.props {
            let node = cur.get(*id).expect("a node with props");
            let (carrier, names) = if folder_carried(cur, *id) {
                let (names, lost): (Vec<Ustr>, Vec<Ustr>) =
                    names.iter().partition(|n| Blob::folder_carries(node, n.as_str()));
                for name in &lost {
                    (cx.notice)(format!("{}.{name} is build-only: a frame of reference cannot carry it", node.class));
                }
                if names.is_empty() {
                    continue;
                }
                last.blob.folder_carrier(node, &names);
                (1, names)
            } else {
                last.blob.carrier(cur, *id);
                (0, names.clone())
            };
            let out = props.op(frame::PROPS);
            frame::u32(out, *id);
            out.push(carrier);
            out.extend_from_slice(&(names.len() as u16).to_le_bytes());
            for name in &names {
                frame::str(out, name);
            }
        }
        let mut sources = Section::default();
        for &id in &plan.sources {
            let text = &cur.get(id).and_then(|n| n.source.as_ref()).expect("a script").text;
            let out = sources.op(frame::SOURCE);
            frame::u32(out, id);
            frame::str(out, text);
        }
        let mut refs = Section::default();
        let mut removes = Section::default();
        for &id in &plan.removes {
            frame::u32(removes.op(frame::REMOVE), id);
        }
        // a root is stamped when the frame touched it and Studio will hold exactly its tree: nothing under it parked,
        // deferred or left with a value it could not take
        let mut stamps = Section::default();
        if edit {
            let mut unstamped = plan.unstamped.clone();
            let held = plan.absent.iter().chain(&plan.keep).chain(self.parked.keys());
            unstamped.extend(held.filter_map(|&id| cur.root_of(id).or_else(|| known.root_of(id))));
            let mut touched: Vec<Id> = plan
                .touched
                .iter()
                .copied()
                .filter(|&r| cur.root_of(r) == Some(r) && !unstamped.contains(&r))
                .collect();
            touched.sort_unstable();
            for root in touched {
                let out = stamps.op(frame::STAMP);
                frame::u32(out, root);
                frame::str(out, &hex(cur.get(root).expect("a root").hash));
            }
        }
        let mut reload = Section::default();
        if !plan.swaps.is_empty() {
            let scripts: Vec<Id> = plan
                .swaps
                .iter()
                .copied()
                .filter(|&id| {
                    let node = cur.get(id).expect("a swapped id");
                    node.class == "Script" && node.prop("Disabled") != Some(&place::Variant::Bool(true))
                })
                .collect();
            let out = reload.op(frame::RELOAD);
            frame::u32(out, scripts.len() as u32);
            for id in scripts {
                frame::u32(out, id);
            }
        }

        let consumers = units.len() + plan.swaps.len() + props.count as usize;
        let count = parts.len();
        let mut blobs = Vec::with_capacity(count);
        let mut outside = Vec::new();
        let mut childrens = Vec::with_capacity(count);
        for part in parts {
            let (bytes, held) = part.blob.finish(cur);
            blobs.push(bytes);
            outside.extend(held);
            childrens.push(part.children);
        }
        for (id, name, target) in outside.into_iter().chain(plan.refs.iter().copied()) {
            let out = refs.op(frame::REF);
            frame::u32(out, id);
            frame::str(out, &name);
            frame::u32(out, target);
        }
        let empty = Section::default();
        let mut out = Vec::with_capacity(count);
        for (i, (blob, children)) in blobs.iter().zip(&childrens).enumerate() {
            let (binds, hydrates) = if i == 0 { (&plan.binds, &plan.hydrates) } else { (&empty, &empty) };
            if i + 1 < count {
                out.push(frame::assemble(generation, self.base, false, blob, &[binds, hydrates, children]));
            } else {
                let sections =
                    [binds, hydrates, children, &moves, &swaps, &props, &sources, &refs, &removes, &stamps, &reload];
                out.push(frame::assemble(generation, self.base, true, blob, &sections));
            }
        }

        // what the subscriber holds once the frame applies
        let mut next = cur.clone();
        for &id in &absent {
            next.put(id, None);
        }
        for &id in &plan.keep {
            next.put(id, known.arc(id).cloned());
        }
        for &root in &self.verify {
            // a root waiting for its first verify (a connect while isolating) is not held at all; one held already
            // took this frame's diff like any other
            if known.get(root).is_none() {
                for id in subtree(&next, root) {
                    next.put(id, known.arc(id).cloned());
                }
            }
        }
        if self.role == Role::Server {
            for id in cur.ids().collect::<Vec<_>>() {
                if !self.tracked(cur, id) {
                    next.put(id, None);
                }
            }
        }
        self.sent = Some(Sent { generation, prev: known, inserted, reconciled, consumers, hello });
        self.known = next;
        self.base = generation;
        out
    }

    /// An ack: each failure updates the known tree so the next frame repairs it. An ack for another frame than the
    /// one in flight (the plugin asked for a verify meanwhile, which covers it) only prints its failures. Returns
    /// whether anything failed.
    pub fn ack(&mut self, cx: &Context, generation: u32, failed: &[(Id, u8, String)]) -> bool {
        let cur = cx.cur;
        let Some(sent) = self.sent.take_if(|s| s.generation == generation) else {
            for (id, _, message) in failed {
                (cx.say)(format!("{}: {message}", locate(&[cur, &self.known], *id)));
            }
            return !failed.is_empty();
        };
        if failed.is_empty() {
            return false;
        }
        let mut blob_failures = 0;
        // ids failing in this ack: a second failure of one node in the same frame does not park it
        let mut now: FxHashSet<Id> = FxHashSet::default();
        // subtrees Studio no longer holds, whose inbound refs are sent again
        let mut gone: FxHashSet<Id> = FxHashSet::default();
        for (id, tag, message) in failed {
            let id = *id;
            let at = locate(&[cur, &self.known, &sent.prev], id);
            let hash = cur.get(id).map_or(0, |n| n.hash);
            let before = self.fails.insert(id, hash);
            let again = now.insert(id) && before == Some(hash);
            let parked = match self.parked.entry(id) {
                // printed when it was parked
                Entry::Occupied(_) => true,
                Entry::Vacant(slot) if again => {
                    slot.insert(hash);
                    (cx.say)(format!("{at}: {message} (failed again unchanged; parked until it changes or Resync)"));
                    true
                }
                Entry::Vacant(_) => {
                    (cx.say)(format!("{at}: {message}"));
                    false
                }
            };
            match *tag {
                frame::BIND => {
                    drop_subtree(&mut self.known, id);
                }
                frame::HYDRATE if self.role == Role::Edit => {
                    // the failed node's root, or every root hydrated under a failed frame
                    let roots: Vec<Id> = match sent.prev.root_of(id) {
                        Some(r) => cur.find(&sent.prev.path_of(r)).into_iter().collect(),
                        None => {
                            cur.roots().into_iter().filter(|&r| cur.get(r).is_some_and(|n| n.parent == id)).collect()
                        }
                    };
                    for root in roots {
                        drop_subtree(&mut self.known, root);
                        if !self.verify.contains(&root) {
                            self.verify.push(root);
                        }
                    }
                }
                frame::HYDRATE => {
                    self.dropped.insert(id);
                    drop_subtree(&mut self.known, id);
                }
                frame::CHILDREN => {
                    blob_failures += 1;
                    match sent.reconciled.get(&id) {
                        // a reconcile pairs live content: inserting it again would duplicate it, so the root is
                        // verified again, unless the node is parked (then it is left out until it changes)
                        Some(_) if parked => {
                            drop_subtree(&mut self.known, id);
                        }
                        Some(&root) => {
                            if cur.root_of(root) == Some(root) && !self.verify.contains(&root) {
                                self.verify.push(root);
                            }
                        }
                        // the blob-made part of the subtree leaves the known tree and is inserted again
                        None => {
                            for d in subtree(&self.known, id) {
                                if d == id || sent.inserted.contains(&d) {
                                    self.known.put(d, None);
                                }
                            }
                        }
                    }
                }
                frame::MOVE | frame::REMOVE => {
                    let from = sent.prev.get(id).map_or(NONE, |n| n.parent);
                    if *tag == frame::MOVE && cur.get(from).is_none() {
                        // its old parent was removed in the same frame, and it with it: inserted again
                        gone.extend(drop_subtree(&mut self.known, id));
                    } else {
                        for d in subtree(&sent.prev, id) {
                            if self.known.get(d).is_none() || d == id {
                                self.known.put(d, sent.prev.arc(d).cloned());
                            }
                        }
                        *self.force.entry(id).or_default() |= FORCE_NODE;
                    }
                }
                frame::PROPS => {
                    blob_failures += 1;
                    *self.force.entry(id).or_default() |= FORCE_PROPS;
                }
                frame::SOURCE => *self.force.entry(id).or_default() |= FORCE_SOURCE,
                frame::SWAP => {
                    blob_failures += 1;
                    *self.force.entry(id).or_default() |= FORCE_SOURCE;
                }
                frame::REF => *self.force.entry(id).or_default() |= FORCE_REFS,
                _ => {}
            }
        }
        self.refs_into(cur, &gone);
        if sent.consumers > 1 && blob_failures >= sent.consumers {
            self.isolate = true;
        }
        true
    }
}

impl Default for Sub {
    fn default() -> Sub {
        Sub::new()
    }
}

fn bind(plan: &mut Plan, id: Id, parent: Id, class: &str, name: &str) {
    let out = plan.binds.op(frame::BIND);
    frame::u32(out, id);
    frame::u32(out, if parent == ROOT { NONE } else { parent });
    frame::str(out, class);
    frame::str(out, name);
}

/// Where a failure is, for the terminal: the nearest file and the instance path, from the first tree that has the
/// id (an id the plugin sent back may be in none: a throwaway, or one the tree has dropped since).
fn locate(trees: &[&Tree], id: Id) -> String {
    let Some(tree) = trees.iter().find(|t| t.get(id).is_some()) else { return format!("instance {id}") };
    let path = tree.path_of(id);
    let mut at = id;
    while let Some(node) = tree.get(at) {
        if let Some(file) = &node.file {
            return format!("{} ({path})", file.display());
        }
        at = node.parent;
    }
    path
}

/// `id` and its descendants that `tree` holds, preorder; empty when it does not hold `id`.
fn subtree(tree: &Tree, id: Id) -> Vec<Id> {
    let mut out = Vec::new();
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        if let Some(node) = tree.get(at) {
            out.push(at);
            stack.extend(node.children.iter().rev());
        }
    }
    out
}

/// Clears `id` and the descendants the known tree holds under it; returns what it cleared.
fn drop_subtree(known: &mut Tree, id: Id) -> Vec<Id> {
    let gone = subtree(known, id);
    for &at in &gone {
        known.put(at, None);
    }
    gone
}

fn preorder_where(tree: &Tree, top: Id, include: &dyn Fn(Id) -> bool) -> Vec<Id> {
    let mut out = Vec::new();
    let mut stack = vec![top];
    while let Some(at) = stack.pop() {
        out.push(at);
        stack.extend(tree.get(at).expect("a live id").children.iter().rev().copied().filter(|&c| include(c)));
    }
    out
}

/// Roughly what a node adds to a blob.
fn weight(tree: &Tree, id: Id) -> usize {
    tree.get(id).map_or(0, |n| 64 + n.props.len() * 16 + n.source.as_ref().map_or(0, |s| s.text.len()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use place::{ScriptKind, Source, Tree};

    use super::*;

    fn tree(scripts: &[(&str, ScriptKind, &str)]) -> Tree {
        let mut fresh = Tree::new();
        for &(path, kind, text) in scripts {
            fresh.set_script(path, kind, Source::new(Arc::from(text)), None).unwrap();
        }
        let mut tree = Tree::new();
        tree.update(fresh);
        tree
    }

    fn plan(sub: &mut Sub, known: &Tree, cur: &Tree, hot_reload: bool) -> Plan {
        let quiet = |_: String| {};
        let cx = Context { cur, ring: &[], hot_reload, notice: &quiet, say: &quiet };
        let mut plan = Plan::default();
        sub.diff(&cx, known, &mut plan);
        plan
    }

    #[test]
    fn a_renamed_subtree_is_one_move() {
        let known = tree(&[("ReplicatedStorage/shared/a/m", ScriptKind::Module, "return 1")]);
        let mut cur = known.clone();
        cur.update(tree(&[("ReplicatedStorage/shared/b/m", ScriptKind::Module, "return 1")]));
        let mut sub = Sub::new();
        sub.role = Role::Edit;
        let plan = plan(&mut sub, &known, &cur, false);
        assert_eq!(plan.moves, [cur.find("ReplicatedStorage/shared/b").unwrap()]);
        assert!(plan.tops.is_empty() && plan.removes.is_empty() && plan.sources.is_empty());
    }

    #[test]
    fn a_new_service_is_bound_and_a_vanished_one_kept() {
        let script = [("ReplicatedStorage/shared/m", ScriptKind::Module, "return 1")];
        let known = tree(&script);
        let mut fresh = tree(&script);
        fresh.service("Lighting").unwrap();
        let mut cur = known.clone();
        cur.update(fresh);
        let mut sub = Sub::new();
        sub.role = Role::Edit;
        let added = plan(&mut sub, &known, &cur, false);
        assert_eq!(added.binds.count, 1);
        assert!(added.tops.is_empty());
        let removed = plan(&mut sub, &cur, &known, false);
        assert!(removed.is_empty());
    }

    #[test]
    fn hot_reload_swaps_every_importer_up_to_the_scripts() {
        let scripts = |leaf: &'static str| {
            [
                ("ReplicatedStorage/shared/leaf", ScriptKind::Module, leaf),
                ("ReplicatedStorage/shared/mid", ScriptKind::Module, r#"return require("./leaf")"#),
                (
                    "ServerScriptService/server/main",
                    ScriptKind::Server,
                    r#"require("@game/ReplicatedStorage/shared/mid")"#,
                ),
                ("ReplicatedStorage/shared/other", ScriptKind::Module, "return 0"),
            ]
        };
        let known = tree(&scripts("return 1"));
        let mut cur = known.clone();
        cur.update(tree(&scripts("return 2")));
        let mut sub = Sub::new();
        sub.role = Role::Server;
        sub.scope.extend(cur.roots());
        let plan = plan(&mut sub, &known, &cur, true);
        let ids: Vec<Id> =
            ["ReplicatedStorage/shared/leaf", "ReplicatedStorage/shared/mid", "ServerScriptService/server/main"]
                .iter()
                .map(|p| cur.find(p).unwrap())
                .collect();
        let mut swaps = plan.swaps.clone();
        swaps.sort_unstable();
        let mut expected = ids.clone();
        expected.sort_unstable();
        assert_eq!(swaps, expected);
        assert!(plan.sources.is_empty());
    }
}
