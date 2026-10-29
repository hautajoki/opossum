//! A universe's resources (DESIGN.md, Resources): passes, products, badges, the experience's name, description and
//! icon, and its thumbnails and their order. [`catalog`] is what opossum.toml declares for an environment, [`plan`]
//! compares it with the lock's records for that universe offline, [`prepare`] reads and checks everything the plan
//! sends before anything is sent, and [`apply`] sends it in phase order, saving the lock after every change. Records
//! hold the values applied, not a fingerprint, so a plan says which fields changed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use cloud::{BadgeChange, Client, Creator, MAX_THUMBNAILS, NewBadge, Sale};
use media::{Fit, ImageOptions, Preset};
use serde_json::{Value, json};

use crate::config::{Config, Environment};
use crate::guard::RemoteGuard;
use crate::lock::{self, Lock, Resource};
use crate::{Outcome, assets};

/// What a resource is, in the order apply goes through them: an order needs the ids this run's thumbnails got.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Pass,
    Product,
    Badge,
    Metadata,
    Icon,
    Thumbnail,
    Order,
}

const KINDS: [Kind; 7] =
    [Kind::Pass, Kind::Product, Kind::Badge, Kind::Metadata, Kind::Icon, Kind::Thumbnail, Kind::Order];

impl Kind {
    /// The kind of a lock key: `pass/vip`, `experience/icon`.
    fn of(key: &str) -> Option<Kind> {
        Some(match key.split_once('/')? {
            ("pass", _) => Kind::Pass,
            ("product", _) => Kind::Product,
            ("badge", _) => Kind::Badge,
            ("thumbnail", _) => Kind::Thumbnail,
            (_, "metadata") => Kind::Metadata,
            (_, "icon") => Kind::Icon,
            (_, _) => Kind::Order,
        })
    }

    /// Whether Roblox gives it an id when it is created; the others are set in place.
    fn has_id(self) -> bool {
        matches!(self, Kind::Pass | Kind::Product | Kind::Badge | Kind::Thumbnail)
    }
}

/// A declared resource: its lock key, the values a provision leaves applied (images as their input hash; an order's
/// ids are only known as it applies), and the image it sends.
pub struct Want {
    pub key: String,
    pub kind: Kind,
    pub values: Resource,
    pub image: Option<Source>,
    /// A badge's fee.
    pub cost: u64,
}

/// An image file as opossum.toml names it: its project-relative path, its lowercase extension and its bytes.
pub struct Source {
    pub path: String,
    pub ext: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Create,
    Update,
    Noop,
    /// In the lock, no longer declared: left alone on Roblox (gameplay ids may still be referenced).
    Retain,
    /// A thumbnail no longer declared: presentational, holding one of ten carousel slots, so it goes.
    Delete,
    /// An earlier create may have succeeded: `opossum adopt` says how it went before anything is sent.
    Blocked,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Noop => "noop",
            Action::Retain => "retain",
            Action::Delete => "delete",
            Action::Blocked => "blocked",
        }
    }
}

pub struct Op {
    pub key: String,
    pub kind: Kind,
    pub action: Action,
    /// The fields whose values differ, by their opossum.toml names. An update sends those still declared: a field
    /// taken out of the config is listed, and Roblox keeps its value.
    pub changed: Vec<&'static str>,
    /// Its declaration in the catalog; None for a record nothing declares.
    pub want: Option<usize>,
}

/// Every resource `cfg` declares (each environment holds them all), in apply order, with each image read and hashed;
/// an unreadable one is the error. Images are hashed as asset inputs under the preset of their purpose, so a hash says
/// the bytes and the pipeline they go through.
pub fn catalog(cfg: &Config) -> Result<Vec<Want>, String> {
    let read = |path: &str, preset: Preset| -> Result<(String, Option<Source>), String> {
        let bytes = std::fs::read(cfg.root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        let ext = path.rfind('.').map_or(String::new(), |dot| path[dot..].to_ascii_lowercase());
        let hash = assets::hash(&ext, ImageOptions { preset, bleed: true }, &bytes);
        Ok((hash, Some(Source { path: path.to_owned(), ext, bytes })))
    };
    let mut wants = Vec::new();
    for (kind, sales, preset) in
        [(Kind::Pass, &cfg.passes, Preset::GamePassIcon), (Kind::Product, &cfg.products, Preset::ProductIcon)]
    {
        for s in sales {
            let (icon, image) = match &s.icon {
                Some(path) => read(path, preset).map(|(hash, image)| (Some(hash), image))?,
                None => (None, None),
            };
            let values = Resource {
                name: Some(s.name.clone()),
                description: s.description.clone(),
                price: s.price,
                for_sale: s.for_sale,
                managed_pricing: s.managed_pricing,
                icon,
                ..Resource::default()
            };
            let key = format!("{}/{}", if kind == Kind::Pass { "pass" } else { "product" }, s.key);
            wants.push(Want { key, kind, values, image, cost: 0 });
        }
    }
    for b in &cfg.badges {
        let (icon, image) = read(&b.icon, Preset::BadgeIcon)?;
        let values = Resource {
            name: Some(b.name.clone()),
            description: Some(b.description.clone()),
            enabled: Some(b.enabled),
            icon: Some(icon),
            ..Resource::default()
        };
        wants.push(Want { key: format!("badge/{}", b.key), kind: Kind::Badge, values, image, cost: b.expected_cost });
    }
    let Some(exp) = &cfg.experience else { return Ok(wants) };
    if exp.name.is_some() || exp.description.is_some() {
        let values = Resource { name: exp.name.clone(), description: exp.description.clone(), ..Resource::default() };
        wants.push(Want { key: "experience/metadata".into(), kind: Kind::Metadata, values, image: None, cost: 0 });
    }
    if let Some(path) = &exp.icon {
        let (icon, image) = read(path, Preset::ExperienceIcon)?;
        let values = Resource { icon: Some(icon), ..Resource::default() };
        wants.push(Want { key: "experience/icon".into(), kind: Kind::Icon, values, image, cost: 0 });
    }
    for t in &exp.thumbnails {
        let (hash, image) = read(&t.image, Preset::Thumbnail)?;
        let values = Resource { image: Some(hash), alt_text: t.alt_text.clone(), ..Resource::default() };
        wants.push(Want { key: format!("thumbnail/{}", t.key), kind: Kind::Thumbnail, values, image, cost: 0 });
    }
    if !exp.thumbnails.is_empty() {
        let want = Want {
            key: "experience/order".into(),
            kind: Kind::Order,
            values: Resource::default(),
            image: None,
            cost: 0,
        };
        wants.push(want);
    }
    Ok(wants)
}

/// The applied value fields of a record, by their config names (an order's `ids` are compared on their own).
fn fields(r: &Resource) -> [(&'static str, Option<String>); 9] {
    let text = |b: Option<bool>| b.map(|b| b.to_string());
    [
        ("name", r.name.clone()),
        ("description", r.description.clone()),
        ("price", r.price.map(|p| p.to_string())),
        ("forSale", text(r.for_sale)),
        ("managedPricing", text(r.managed_pricing)),
        ("enabled", text(r.enabled)),
        ("altText", r.alt_text.clone()),
        ("icon", r.icon.clone()),
        ("image", r.image.clone()),
    ]
}

fn present(r: &Resource) -> Vec<&'static str> {
    fields(r).into_iter().filter(|(_, v)| v.is_some()).map(|(name, _)| name).collect()
}

/// Compares the declaration with the records of its universe, offline: apply's phases in order, each kind's declared
/// resources in config order, then the records nothing declares (thumbnails first, so their slots are free before
/// any upload).
pub fn plan(wants: &[Want], records: &BTreeMap<String, Resource>) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    for kind in KINDS {
        let gone: Vec<Op> = records
            .keys()
            .filter(|key| Kind::of(key) == Some(kind) && !wants.iter().any(|w| &w.key == *key))
            .map(|key| {
                let action = if kind == Kind::Thumbnail { Action::Delete } else { Action::Retain };
                Op { key: key.clone(), kind, action, changed: Vec::new(), want: None }
            })
            .collect();
        let (first, last) = if kind == Kind::Thumbnail { (gone, Vec::new()) } else { (Vec::new(), gone) };
        ops.extend(first);
        for (i, w) in wants.iter().enumerate().filter(|(_, w)| w.kind == kind) {
            let record = records.get(&w.key);
            let (action, changed) = match record {
                Some(r) if r.next.is_some() => (Action::Blocked, Vec::new()),
                _ if kind == Kind::Order => order_plan(&ops, wants, records, record),
                None => (Action::Create, present(&w.values)),
                Some(r) if kind.has_id() && r.id.is_none() => (Action::Create, present(&w.values)),
                // adopted: every declared value goes up
                Some(r) if present(r).is_empty() => (Action::Update, present(&w.values)),
                Some(r) => {
                    let changed: Vec<&'static str> = fields(r)
                        .into_iter()
                        .zip(fields(&w.values))
                        .filter(|((_, a), (_, b))| a != b)
                        .map(|((name, _), _)| name)
                        .collect();
                    (if changed.is_empty() { Action::Noop } else { Action::Update }, changed)
                }
            };
            ops.push(Op { key: w.key.clone(), kind, action, changed, want: Some(i) });
        }
        ops.extend(last);
    }
    ops
}

/// The order changes when a thumbnail uploads or goes in this run (a new image is a new media id), or when the ids
/// the lock holds now, in config order, are not the ones last posted.
fn order_plan(
    ops: &[Op],
    wants: &[Want],
    records: &BTreeMap<String, Resource>,
    order: Option<&Resource>,
) -> (Action, Vec<&'static str>) {
    let moving = ops.iter().any(|op| {
        op.kind == Kind::Thumbnail
            && (matches!(op.action, Action::Create | Action::Delete) || op.changed.contains(&"image"))
    });
    let ids: Vec<u64> = wants
        .iter()
        .filter(|w| w.kind == Kind::Thumbnail)
        .filter_map(|w| records.get(&w.key).and_then(|r| r.id))
        .collect();
    match order {
        None => (Action::Create, vec!["ids"]),
        Some(r) if moving || r.ids.as_ref() != Some(&ids) => (Action::Update, vec!["ids"]),
        Some(_) => (Action::Noop, Vec::new()),
    }
}

/// Whether an operation uploads its thumbnail: a new one, or a new image for one Roblox has.
fn uploads(op: &Op) -> bool {
    op.kind == Kind::Thumbnail && (op.action == Action::Create || op.changed.contains(&"image"))
}

/// Whether an operation sends its want's image.
fn sends_image(op: &Op) -> bool {
    match (op.kind, op.action) {
        (_, Action::Create) => true,
        (Kind::Thumbnail, Action::Update) => uploads(op),
        (_, Action::Update) => op.changed.contains(&"icon"),
        _ => false,
    }
}

/// The image an operation sends: MIME and bytes.
pub type Sent = Option<(&'static str, Vec<u8>)>;

/// Everything the plan will send, read, compiled and checked before anything is: nothing blocked, a badge create
/// allowed to spend, every image through the pipeline, and the thumbnails within Roblox's ten at every step of the
/// apply order. The images come back in `ops` order. A refusal is its code and why.
pub fn prepare(
    wants: &[Want],
    ops: &[Op],
    records: &BTreeMap<String, Resource>,
    allow_spend: bool,
) -> Result<Vec<Sent>, (&'static str, String)> {
    let blocked: Vec<String> = ops
        .iter()
        .filter(|op| op.action == Action::Blocked)
        .map(|op| format!("{}: {}", op.key, settle(&op.key)))
        .collect();
    if !blocked.is_empty() {
        return Err(("resource.blocked", format!("an earlier create may have succeeded: {}", blocked.join("; "))));
    }
    if !allow_spend && let Some(op) = ops.iter().find(|op| op.kind == Kind::Badge && op.action == Action::Create) {
        let cost = op.want.map_or(0, |i| wants[i].cost);
        let error = format!(
            "creating {} costs its expectedCost ({cost} Robux, paid by the creator, who must own the universe); pass \
             --allow-spend to allow it",
            op.key
        );
        return Err(("resource.spend", error));
    }
    let mut images = Vec::with_capacity(ops.len());
    for op in ops {
        let image = match (op.want.map(|i| &wants[i]), sends_image(op)) {
            (Some(Want { image: Some(Source { path, ext, bytes }), .. }), true) => {
                let fitted = fit(op.kind, ext, bytes).map_err(|e| ("resource.image", format!("{path}: {e}")))?;
                Some(fitted)
            }
            _ => None,
        };
        images.push(image);
    }

    // the carousel, step by step: undeclared thumbnails go first, a replacement at the limit deletes its old image
    // before uploading, and one below it keeps the old as stale until the order no longer shows it
    let mut live: BTreeSet<u64> = BTreeSet::new();
    for (key, r) in records.iter().filter(|(key, _)| Kind::of(key) == Some(Kind::Thumbnail)) {
        live.extend(r.id);
        live.extend(r.stale.iter().flatten());
        if live.len() > MAX_THUMBNAILS {
            let error = format!("the lock holds more than {MAX_THUMBNAILS} thumbnails (at {key}); Roblox keeps 10");
            return Err(("resource.limit", error));
        }
    }
    let mut placeholder = 9_000_000_000_000_000_000u64;
    let mut ordered: BTreeMap<&str, u64> = BTreeMap::new();
    for op in ops.iter().filter(|op| op.kind == Kind::Thumbnail) {
        let record = records.get(&op.key);
        let id = record.and_then(|r| r.id);
        if op.action == Action::Delete {
            for id in id.iter().chain(record.and_then(|r| r.stale.as_ref()).into_iter().flatten()) {
                live.remove(id);
            }
            continue;
        }
        if !uploads(op) {
            ordered.extend(id.map(|id| (op.key.as_str(), id)));
            continue;
        }
        match id {
            Some(old) if live.len() >= MAX_THUMBNAILS => {
                live.remove(&old);
            }
            None if live.len() >= MAX_THUMBNAILS => {
                let error = format!(
                    "{} cannot be uploaded: the universe already has {MAX_THUMBNAILS} thumbnails in opossum.lock, \
                     Roblox's limit",
                    op.key
                );
                return Err(("resource.limit", error));
            }
            _ => {}
        }
        while live.contains(&placeholder) {
            placeholder -= 1;
        }
        live.insert(placeholder);
        ordered.insert(&op.key, placeholder);
    }
    let mut seen = BTreeMap::new();
    for w in wants.iter().filter(|w| w.kind == Kind::Thumbnail) {
        if let Some(&id) = ordered.get(w.key.as_str())
            && let Some(other) = seen.insert(id, &w.key)
        {
            let error = format!("{other} and {} hold the same thumbnail id {id} in opossum.lock", w.key);
            return Err(("resource.limit", error));
        }
    }
    Ok(images)
}

/// An icon (512 by 512, whole) or a thumbnail (1920 by 1080, filled), bled, as the bytes that go up.
fn fit(kind: Kind, ext: &str, bytes: &[u8]) -> Result<(&'static str, Vec<u8>), String> {
    let (w, h, how) = if kind == Kind::Thumbnail { (1920, 1080, Fit::Cover) } else { (512, 512, Fit::Contain) };
    match media::fit(ext, bytes, w, h, how)? {
        media::Image::Unchanged { .. } => Ok((media::mime(ext).unwrap_or("image/png"), bytes.to_vec())),
        media::Image::Png { bytes, .. } => Ok(("image/png", bytes)),
    }
}

/// How a blocked resource is settled: with the id it got if it exists on Roblox, else `--none`.
fn settle(key: &str) -> String {
    let (kind, name) = key.split_once('/').unwrap_or((key, ""));
    format!("opossum adopt {kind} {name} <id>, or --none")
}

/// Where apply sends and saves.
pub struct Target<'a> {
    pub client: &'a Client,
    pub root: &'a Path,
    pub guard: &'a RemoteGuard,
    pub universe: u64,
    /// The root place's id: the experience's name and description are its.
    pub place: u64,
    /// Who pays a badge's fee.
    pub creator: Option<Creator>,
}

impl<'a> Target<'a> {
    /// `env`'s universe and root place, saved into `cfg`'s lock.
    pub fn new(cfg: &'a Config, env: &Environment, client: &'a Client, guard: &'a RemoteGuard) -> Target<'a> {
        let root = cfg.places.iter().find(|p| p.root).map(|p| p.name.as_str());
        let place = env.places.iter().find(|(name, _)| Some(name.as_str()) == root).map_or(0, |&(_, id)| id);
        Target { client, root: &cfg.root, guard, universe: env.universe, place, creator: cfg.creator }
    }
}

#[derive(Default)]
pub struct Tally {
    pub applied: usize,
    pub noop: usize,
    pub retained: usize,
    pub deleted: usize,
}

/// Why apply stopped: a stable code and the error.
pub struct Fail {
    pub code: String,
    pub error: String,
}

impl Fail {
    fn cloud(e: cloud::Error) -> Fail {
        Fail { code: e.diag().to_owned(), error: e.to_string() }
    }
}

/// Sends the prepared plan in its order, each change saved to the lock before the next is sent, then deletes the
/// thumbnails replaced images left behind. Stops at the first failure, with everything before it recorded.
pub fn apply(
    t: &Target,
    lock: &mut Lock,
    wants: &[Want],
    ops: &[Op],
    images: &[Sent],
    stop: &AtomicBool,
) -> (Tally, Option<Fail>) {
    let mut tally = Tally::default();
    let mut deleted = false;
    for (op, image) in ops.iter().zip(images) {
        match op.action {
            Action::Noop => tally.noop += 1,
            Action::Retain => tally.retained += 1,
            Action::Blocked => unreachable!("prepare refuses a blocked plan"),
            _ if stop.load(Ordering::SeqCst) => {
                let error = "opossum was stopped; what it changed is recorded".to_owned();
                return (tally, Some(Fail { code: "provision.stopped".into(), error }));
            }
            // every undeclared thumbnail at once, before the first upload
            Action::Delete if deleted => {}
            Action::Delete => {
                deleted = true;
                let gone: Vec<&Op> = ops.iter().filter(|op| op.action == Action::Delete).collect();
                eprintln!("deleting {} thumbnails no longer declared…", gone.len());
                let records = lock.universes.entry(t.universe).or_default();
                let ids: Vec<u64> = gone
                    .iter()
                    .filter_map(|op| records.get(&op.key))
                    .flat_map(|r| r.id.into_iter().chain(r.stale.iter().flatten().copied()))
                    .collect();
                if let Err(fail) = delete_all(t, &ids) {
                    return (tally, Some(fail));
                }
                for op in &gone {
                    records.remove(&op.key);
                }
                if let Err(fail) = save(t, lock) {
                    return (tally, Some(fail));
                }
                tally.deleted += gone.len();
            }
            Action::Create | Action::Update => {
                eprintln!("{} {}…", op.action.name(), op.key);
                let want = &wants[op.want.expect("a create or an update is declared")];
                let image = image.as_ref().map(|(mime, bytes)| cloud::Image { mime, bytes });
                let done = match op.kind {
                    Kind::Pass | Kind::Product => sale(t, lock, op, want, image),
                    Kind::Badge => badge(t, lock, op, want, image),
                    Kind::Metadata => metadata(t, lock, op, want),
                    Kind::Icon => icon(t, lock, want, image),
                    Kind::Thumbnail => thumbnail(t, lock, want, image),
                    Kind::Order => order(t, lock, wants),
                };
                if let Err(fail) = done {
                    return (tally, Some(fail));
                }
                tally.applied += 1;
            }
        }
    }
    // stale images go once no order shows them: after this run's order, or whatever an earlier run left
    let records = lock.universes.entry(t.universe).or_default();
    let stale: Vec<u64> = records.values().flat_map(|r| r.stale.iter().flatten().copied()).collect();
    if !stale.is_empty() {
        eprintln!("deleting {} replaced thumbnails…", stale.len());
        if let Err(fail) = delete_all(t, &stale) {
            return (tally, Some(fail));
        }
        for r in records.values_mut() {
            r.stale = None;
        }
        if let Err(fail) = save(t, lock) {
            return (tally, Some(fail));
        }
    }
    (tally, None)
}

fn save(t: &Target, lock: &mut Lock) -> Result<(), Fail> {
    match lock::save(lock, t.root, t.guard) {
        Ok(note) => {
            if let Some(note) = note {
                eprintln!("{note}");
            }
            Ok(())
        }
        Err(d) => Err(Fail { code: d.code, error: d.message }),
    }
}

fn record<'a>(t: &Target, lock: &'a mut Lock, key: &str) -> &'a mut Resource {
    lock.universes.entry(t.universe).or_default().entry(key.to_owned()).or_default()
}

/// Saves a create's intent, so `next` is on disk before its request leaves.
fn intend(t: &Target, lock: &mut Lock, want: &Want) -> Result<(), Fail> {
    record(t, lock, &want.key).next = Some(lock::intent(&want.values));
    save(t, lock)
}

/// What a create's answer leaves of its intent. A refusal that certainly changed nothing clears it; any other failure
/// keeps it, and the resource is blocked until `opossum adopt` says whether it was created. On success the caller
/// records the id, clearing `next`, and saves.
fn created(t: &Target, lock: &mut Lock, want: &Want, sent: Result<u64, cloud::Error>) -> Result<u64, Fail> {
    match sent {
        Ok(id) => Ok(id),
        Err(e) if e.definite() => {
            let records = lock.universes.entry(t.universe).or_default();
            if let Some(r) = records.get_mut(&want.key) {
                r.next = None;
                if r.id.is_none() && present(r).is_empty() {
                    records.remove(&want.key);
                }
            }
            save(t, lock)?;
            Err(Fail::cloud(e))
        }
        Err(e) => Err(Fail {
            code: "resource.pending".into(),
            error: format!("{e}; it may have been created: {}", settle(&want.key)),
        }),
    }
}

/// Whether an operation sends `field`: a create sends every declared field, an update the changed ones.
fn sends(op: &Op, field: &str) -> bool {
    op.action == Action::Create || op.changed.contains(&field)
}

fn sale(t: &Target, lock: &mut Lock, op: &Op, want: &Want, icon: Option<cloud::Image>) -> Result<(), Fail> {
    let v = &want.values;
    let s = Sale {
        name: v.name.as_deref().filter(|_| sends(op, "name")),
        description: v.description.as_deref().filter(|_| sends(op, "description")),
        price: v.price.filter(|_| sends(op, "price")),
        for_sale: v.for_sale.filter(|_| sends(op, "forSale")),
        managed_pricing: v.managed_pricing.filter(|_| sends(op, "managedPricing")),
        icon,
    };
    let (c, u) = (t.client, t.universe);
    let id = match (op.action, op.kind) {
        (Action::Create, Kind::Pass) => {
            intend(t, lock, want)?;
            created(t, lock, want, c.create_pass(u, &s))?
        }
        (Action::Create, _) => {
            intend(t, lock, want)?;
            created(t, lock, want, c.create_product(u, &s))?
        }
        _ => {
            let id = record(t, lock, &want.key).id.expect("an update has an id");
            let empty = s.name.is_none()
                && s.description.is_none()
                && s.price.is_none()
                && s.for_sale.is_none()
                && s.managed_pricing.is_none()
                && s.icon.is_none();
            let sent = match op.kind {
                _ if empty => Ok(()),
                Kind::Pass => c.update_pass(u, id, &s),
                _ => c.update_product(u, id, &s),
            };
            sent.map_err(Fail::cloud)?;
            id
        }
    };
    *record(t, lock, &want.key) = Resource { id: Some(id), ..v.clone() };
    save(t, lock)
}

fn badge(t: &Target, lock: &mut Lock, op: &Op, want: &Want, icon: Option<cloud::Image>) -> Result<(), Fail> {
    let v = &want.values;
    let (name, description) = (v.name.as_deref().unwrap_or(""), v.description.as_deref().unwrap_or(""));
    if op.action == Action::Create {
        let Some(payer) = t.creator else {
            return Err(Fail { code: "config".into(), error: "a badge create needs `creator`".into() });
        };
        let icon = icon.expect("a badge create sends its icon");
        let enabled = v.enabled.unwrap_or(true);
        let badge = NewBadge { name, description, payer, expected_cost: want.cost, enabled, icon };
        intend(t, lock, want)?;
        let id = created(t, lock, want, t.client.create_badge(t.universe, &badge))?;
        *record(t, lock, &want.key) = Resource { id: Some(id), ..v.clone() };
        return save(t, lock);
    }
    let r = record(t, lock, &want.key).clone();
    let id = r.id.expect("an update has an id");
    let change = BadgeChange {
        name: sends(op, "name").then_some(name),
        description: sends(op, "description").then_some(description),
        enabled: v.enabled.filter(|_| sends(op, "enabled")),
    };
    if change.name.is_some() || change.description.is_some() || change.enabled.is_some() {
        t.client.update_badge(id, &change).map_err(Fail::cloud)?;
    }
    *record(t, lock, &want.key) = Resource { id: Some(id), icon: r.icon, ..v.clone() };
    save(t, lock)?;
    if let Some(icon) = icon {
        t.client.badge_icon(id, icon).map_err(Fail::cloud)?;
        record(t, lock, &want.key).icon = v.icon.clone();
        save(t, lock)?;
    }
    Ok(())
}

fn metadata(t: &Target, lock: &mut Lock, op: &Op, want: &Want) -> Result<(), Fail> {
    let v = &want.values;
    let name = v.name.as_deref().filter(|_| sends(op, "name"));
    let description = v.description.as_deref().filter(|_| sends(op, "description"));
    if name.is_some() || description.is_some() {
        t.client.update_experience(t.universe, t.place, name, description).map_err(Fail::cloud)?;
    }
    *record(t, lock, &want.key) = v.clone();
    save(t, lock)
}

fn icon(t: &Target, lock: &mut Lock, want: &Want, icon: Option<cloud::Image>) -> Result<(), Fail> {
    t.client.experience_icon(t.universe, icon.expect("an icon update sends it")).map_err(Fail::cloud)?;
    *record(t, lock, &want.key) = want.values.clone();
    save(t, lock)
}

/// The number of thumbnails Roblox holds for the universe as the lock knows them: live and stale.
fn live(t: &Target, lock: &mut Lock) -> usize {
    let records = lock.universes.entry(t.universe).or_default();
    let thumbnails = records.iter().filter(|(key, _)| Kind::of(key) == Some(Kind::Thumbnail)).map(|(_, r)| r);
    thumbnails.flat_map(|r| r.id.into_iter().chain(r.stale.iter().flatten().copied())).collect::<BTreeSet<_>>().len()
}

/// A thumbnail: a new image is uploaded (Roblox replaces none in place), keeping the old one as stale until the order
/// no longer shows it, unless the carousel is full, when the old one goes first. Then its alt text.
fn thumbnail(t: &Target, lock: &mut Lock, want: &Want, image: Option<cloud::Image>) -> Result<(), Fail> {
    let (c, u) = (t.client, t.universe);
    if let Some(image) = image {
        let mut old = record(t, lock, &want.key).id;
        if let Some(id) = old
            && live(t, lock) >= MAX_THUMBNAILS
        {
            c.delete_thumbnail(u, id).map_err(Fail::cloud)?;
            record(t, lock, &want.key).id = None;
            save(t, lock)?;
            old = None;
        }
        intend(t, lock, want)?;
        let id = created(t, lock, want, c.upload_thumbnail(u, image))?;
        let r = record(t, lock, &want.key);
        // its alt text is not applied yet: a failure there retries only the alt text
        (r.id, r.image, r.alt_text, r.next) = (Some(id), want.values.image.clone(), None, None);
        if let Some(old) = old.filter(|&old| old != id) {
            r.stale.get_or_insert_default().push(old);
        }
        save(t, lock)?;
    }
    let r = record(t, lock, &want.key);
    let id = r.id.expect("a thumbnail has its id here");
    match &want.values.alt_text {
        Some(alt) if r.alt_text.as_ref() != Some(alt) => {
            c.thumbnail_alt_text(u, id, alt).map_err(Fail::cloud)?;
            record(t, lock, &want.key).alt_text = Some(alt.clone());
        }
        Some(_) => {}
        // no longer managed: Roblox keeps it
        None => r.alt_text = None,
    }
    save(t, lock)
}

/// Posts the declared thumbnails' ids in config order, as the lock holds them after this run's uploads.
fn order(t: &Target, lock: &mut Lock, wants: &[Want]) -> Result<(), Fail> {
    let records = lock.universes.entry(t.universe).or_default();
    let mut ids = Vec::new();
    for w in wants.iter().filter(|w| w.kind == Kind::Thumbnail) {
        match records.get(&w.key).and_then(|r| r.id) {
            Some(id) => ids.push(id),
            None => {
                return Err(Fail { code: "resource.notProvisioned".into(), error: format!("{} has no id", w.key) });
            }
        }
    }
    let applied = records.get("experience/order").and_then(|r| r.ids.as_ref());
    if applied != Some(&ids) {
        t.client.order_thumbnails(t.universe, &ids).map_err(Fail::cloud)?;
    }
    record(t, lock, "experience/order").ids = Some(ids);
    save(t, lock)
}

/// Deletes thumbnails ten at a time; one already gone counts as deleted.
fn delete_all(t: &Target, ids: &[u64]) -> Result<(), Fail> {
    for chunk in ids.chunks(MAX_THUMBNAILS) {
        let delete = |id| t.client.delete_thumbnail(t.universe, id).map_err(Fail::cloud);
        let results: Vec<Result<(), Fail>> = std::thread::scope(|s| {
            let deletes: Vec<_> = chunk.iter().map(|&id| s.spawn(move || delete(id))).collect();
            deletes.into_iter().map(|d| d.join().expect("a thumbnail delete panicked")).collect()
        });
        results.into_iter().collect::<Result<(), Fail>>()?;
    }
    Ok(())
}

/// The catalog and its plan against the lock's records for `env`'s universe, refused when a merge left a resource of
/// that universe twice or an image cannot be read.
pub fn survey(cfg: &Config, env: &Environment, lock: &Lock) -> Result<(Vec<Want>, Vec<Op>), Outcome> {
    let conflicts: Vec<String> =
        lock.conflicts.iter().filter(|(u, _)| *u == env.universe).map(|(_, key)| key.clone()).collect();
    if !conflicts.is_empty() {
        let error = format!(
            "two records for {} in universe {} after a merge; keep the line whose id is live and delete the other",
            conflicts.join(", "),
            env.universe
        );
        return Err(Outcome::refused("lock.conflict", error));
    }
    let wants = catalog(cfg).map_err(|e| Outcome::refused("resource.image", e))?;
    let records = lock.universes.get(&env.universe).cloned().unwrap_or_default();
    let ops = plan(&wants, &records);
    Ok((wants, ops))
}

/// The plan's lines (an 8-wide action, the resource, what changed) and its tally.
pub fn table(ops: &[Op]) -> String {
    let mut out = String::new();
    let mut counts = [0usize; 6];
    for op in ops {
        counts[op.action as usize] += 1;
        let _ = write!(out, "{:<8} {}", op.action.name(), op.key);
        if op.action == Action::Blocked {
            let _ = write!(out, " (an earlier create may have succeeded: {})", settle(&op.key));
        } else if !op.changed.is_empty() && op.action != Action::Noop {
            let _ = write!(out, " ({})", op.changed.join(", "));
        }
        out.push('\n');
    }
    let [create, update, noop, retain, delete, blocked] = counts;
    let _ = writeln!(
        out,
        "{create} create, {update} update, {noop} noop, {delete} delete, {retain} retain, {blocked} blocked"
    );
    out
}

pub fn ops_json(ops: &[Op]) -> Value {
    ops.iter().map(|op| json!({ "action": op.action.name(), "resource": op.key, "changed": op.changed })).collect()
}

pub fn blocked(ops: &[Op]) -> usize {
    ops.iter().filter(|op| op.action == Action::Blocked).count()
}

/// `opossum provision [--environment e] [--allow-spend]`: plans the environment's resources, prepares every request,
/// and applies them. Exit 2 when nothing was sent (a blocked plan, a badge create without `--allow-spend`, an image or
/// the config or the lock that is wrong), 1 when it stopped part way, with what it changed recorded.
pub fn provision(
    cfg: &Config,
    client: &Client,
    guard: &RemoteGuard,
    environment: Option<&str>,
    allow_spend: bool,
    stop: &AtomicBool,
) -> Outcome {
    let env = match cfg.environment(environment) {
        Ok(env) => env,
        Err(error) => return Outcome::refused("config", error),
    };
    let mut lock = match read(cfg) {
        Ok(lock) => lock,
        Err(refusal) => return refusal,
    };
    let (wants, ops) = match survey(cfg, env, &lock) {
        Ok(planned) => planned,
        Err(refusal) => return refusal,
    };
    let records = lock.universes.get(&env.universe).cloned().unwrap_or_default();
    let images = match prepare(&wants, &ops, &records, allow_spend) {
        Ok(images) => images,
        Err((code, error)) => return Outcome::refused(code, error),
    };
    let (tally, fail) = apply(&Target::new(cfg, env, client, guard), &mut lock, &wants, &ops, &images, stop);
    outcome(env, &ops, &tally, fail)
}

/// What provision did, as `provision` and `deploy` report it: exit 1 with the failure when it stopped part way.
pub fn outcome(env: &Environment, ops: &[Op], tally: &Tally, fail: Option<Fail>) -> Outcome {
    let mut json = json!({
        "ok": fail.is_none(), "environment": env.name, "universeId": env.universe, "applied": tally.applied,
        "noop": tally.noop, "retained": tally.retained, "deleted": tally.deleted, "operations": ops_json(ops),
    });
    let mut text = format!(
        "{}: {} applied, {} noop, {} retained, {} deleted\n",
        env.name, tally.applied, tally.noop, tally.retained, tally.deleted
    );
    let Some(fail) = fail else { return Outcome { exit: 0, json, text } };
    let _ = writeln!(text, "failed: {}", fail.error);
    json["ran"] = true.into();
    json["code"] = fail.code.into();
    json["error"] = fail.error.into();
    Outcome { exit: 1, json, text }
}

/// The lock, refused when it cannot be read or has errors.
pub fn read(cfg: &Config) -> Result<Lock, Outcome> {
    let (lock, diags) = lock::read(&cfg.root).map_err(|error| Outcome::refused("lock.io", error))?;
    match diags.iter().find(|d| d.is_error()) {
        Some(d) => Err(Outcome::refused(&d.code, d.human(&cfg.root))),
        None => Ok(lock),
    }
}

/// `opossum adopt pass|product|badge|thumbnail <key> (<id> | --none)`: says how a create that may or may not have
/// happened went, or binds a resource made elsewhere. With an id, the record takes it with no values applied, so the
/// next provision sets every declared field (a thumbnail's image is the one its upload sent, which must still be the
/// one declared); with `--none` (`id` None) the create is forgotten and the next provision sends it again.
pub fn adopt(
    cfg: &Config,
    guard: &RemoteGuard,
    environment: Option<&str>,
    kind: &str,
    name: &str,
    id: Option<u64>,
) -> Outcome {
    let env = match cfg.environment(environment) {
        Ok(env) => env,
        Err(error) => return Outcome::refused("config", error),
    };
    let mut lock = match read(cfg) {
        Ok(lock) => lock,
        Err(refusal) => return refusal,
    };
    let key = format!("{kind}/{name}");
    let refuse = |error: String| Outcome::refused("resource.adopt", error);
    if lock.conflicts.contains(&(env.universe, key.clone())) {
        let error = format!("two records for {key} after a merge; keep the line whose id is live and delete the other");
        return Outcome::refused("lock.conflict", error);
    }
    let wants = match catalog(cfg) {
        Ok(wants) => wants,
        Err(error) => return Outcome::refused("resource.image", error),
    };
    let records = lock.universes.entry(env.universe).or_default();
    let record = records.get(&key).cloned().unwrap_or_default();
    let text = match id {
        None => {
            if record.next.is_none() {
                return refuse(format!("{key} has no create in flight in universe {}", env.universe));
            }
            let r = records.entry(key.clone()).or_default();
            r.next = None;
            if r.id.is_none() && present(r).is_empty() {
                records.remove(&key);
            }
            format!("forgot the create of {key}; `opossum provision` sends it again\n")
        }
        Some(id) => {
            let Some(want) = wants.iter().find(|w| w.key == key) else {
                return refuse(format!("{key} is not declared in opossum.toml"));
            };
            let thumbnail = want.kind == Kind::Thumbnail;
            if thumbnail && record.next.is_some() && record.id != Some(id) {
                let next = record.next.as_deref().unwrap_or("");
                if next.bytes().all(|b| b == b'0') || next != lock::intent(&want.values) {
                    return refuse(format!(
                        "the thumbnail {name} changed since its upload that may have happened, so {id} may hold \
                         another image; `opossum adopt thumbnail {name} --none` and let provision upload it"
                    ));
                }
            }
            let adopted = match record.id {
                // the upload did not happen: the thumbnail keeps the image it had
                Some(old) if old == id => Resource { next: None, ..record },
                Some(old) if !thumbnail || record.next.is_none() => {
                    return refuse(format!("{key} already has the id {old}"));
                }
                // a replacement's upload: the old image waits for the order to move off it
                Some(old) => {
                    let mut stale = record.stale.unwrap_or_default();
                    stale.push(old);
                    let (image, stale) = (want.values.image.clone(), Some(stale));
                    Resource { id: Some(id), image, stale, ..Resource::default() }
                }
                _ if thumbnail => Resource { id: Some(id), image: want.values.image.clone(), ..Resource::default() },
                _ => Resource { id: Some(id), ..Resource::default() },
            };
            records.insert(key.clone(), adopted);
            format!("adopted {key} as {id} in {}; the next provision sets its declared fields\n", env.name)
        }
    };
    if let Err(d) = lock::save(&mut lock, &cfg.root, guard) {
        return Outcome::refused(&d.code, d.message);
    }
    let mut json = json!({ "ok": true, "resource": key });
    if let Some(id) = id {
        json["id"] = id.into();
    }
    Outcome { exit: 0, json, text }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    use cloud::mock::{Mock, Reply, auto};
    use cloud::{Clock, Fake};

    use super::*;
    use crate::guard::Cache;

    /// A project in universe 7 (root place 8) declaring `toml`, with these files and these records of universe 7 in its
    /// lock.
    fn project(name: &str, toml: &str, files: &[(&str, &str)], records: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("opossum-resources-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let base = "name = \"r\"\ncreator = { user = 1 }\n[routes]\nshared = \"ReplicatedStorage/shared\"\n\
                    [environments.dev]\nuniverseId = 7\nplaces = { main = 8 }\n";
        std::fs::write(root.join("opossum.toml"), format!("{base}{toml}")).unwrap();
        for (file, text) in files {
            std::fs::write(root.join(file), text).unwrap();
        }
        std::fs::write(root.join(lock::FILE), universe7(records)).unwrap();
        std::fs::canonicalize(root).unwrap()
    }

    /// A lock holding `records` (`"<kind>/<key>" = { … }` lines) in universe 7.
    fn universe7(records: &str) -> String {
        records.lines().map(|l| format!("universes.7.{l}\n")).fold("version = 1\n".to_owned(), |text, l| text + &l)
    }

    /// A square of one colour: enough for the image pipeline.
    fn svg(color: &str) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 2 2\"><rect width=\"2\" height=\"2\" fill=\"#{color}\"/></svg>"
        )
    }

    fn hash(text: &str, preset: Preset) -> String {
        assets::hash(".svg", ImageOptions { preset, bleed: true }, text.as_bytes())
    }

    fn provision_at(root: &Path, mock: &Mock, allow_spend: bool) -> Outcome {
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let client = Client::new("test-key", &mock.base, Clock::Fake(Fake::new())).unwrap();
        let guard = RemoteGuard::acquire(&Cache { dir: root.join(".cache") }).unwrap();
        provision(&cfg, &client, &guard, None, allow_spend, &AtomicBool::new(false))
    }

    fn adopt_at(root: &Path, kind: &str, name: &str, id: Option<u64>) -> Outcome {
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let guard = RemoteGuard::acquire(&Cache { dir: root.join(".cache") }).unwrap();
        adopt(&cfg, &guard, None, kind, name, id)
    }

    fn records(root: &Path) -> BTreeMap<String, Resource> {
        lock::read(root).unwrap().0.universes.remove(&7).unwrap_or_default()
    }

    /// Each request's method and path, the universe's thumbnail prefix cut.
    fn sent(mock: &Mock) -> Vec<String> {
        let prefix = "legacy-game-internationalization/v1/game-thumbnails/games/7/language-codes/en_us/";
        mock.requests().iter().map(|r| format!("{} {}", r.method, r.path.trim_start_matches(prefix))).collect()
    }

    #[test]
    fn the_plan_says_what_each_resource_needs_and_a_field_taken_out_is_not_sent() {
        let (a, b) = (svg("ff0000"), svg("00ff00"));
        let toml = "[passes.vip]\nname = \"VIP\"\nprice = 100\n[products.coins]\nname = \"Coins\"\n\
                    [badges.welcome]\nname = \"Welcome\"\ndescription = \"Hi\"\nicon = \"a.svg\"\n\
                    [experience]\nname = \"Demo\"\n[[experience.thumbnails]]\nkey = \"a\"\nimage = \"a.svg\"\n\
                    [[experience.thumbnails]]\nkey = \"b\"\nimage = \"b.svg\"\n";
        let vip = "\"pass/vip\" = { id = 1, name = \"VIP\", description = \"old\", price = 100 }\n";
        let lines = format!(
            "{vip}\"pass/gone\" = {{ id = 2, name = \"Gone\" }}\n\
             \"product/coins\" = {{ next = \"{}\" }}\n\
             \"experience/metadata\" = {{ name = \"Demo\" }}\n\
             \"thumbnail/a\" = {{ id = 55, image = \"{}\" }}\n\
             \"thumbnail/b\" = {{ id = 56, image = \"{}\" }}\n\
             \"thumbnail/old\" = {{ id = 57 }}\n\
             \"experience/order\" = {{ ids = [55, 56, 57] }}\n",
            "1".repeat(32),
            hash(&a, Preset::Thumbnail),
            "0".repeat(32),
        );
        let root = project("plan", toml, &[("a.svg", &a), ("b.svg", &b)], &lines);
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let (lock, _) = lock::read(&root).unwrap();
        let (_, ops) = survey(&cfg, &cfg.environments[0], &lock).unwrap_or_else(|o| panic!("{}", o.text));
        let got: Vec<String> =
            ops.iter().map(|op| format!("{} {} {:?}", op.action.name(), op.key, op.changed)).collect();
        assert_eq!(
            got,
            [
                "update pass/vip [\"description\"]",
                "retain pass/gone []",
                "blocked product/coins []",
                "create badge/welcome [\"name\", \"description\", \"enabled\", \"icon\"]",
                "noop experience/metadata []",
                "delete thumbnail/old []",
                "noop thumbnail/a []",
                "update thumbnail/b [\"image\"]",
                "update experience/order [\"ids\"]",
            ]
        );
        assert!(table(&ops).ends_with("1 create, 3 update, 2 noop, 1 delete, 1 retain, 1 blocked\n"));

        // a merge that left a resource twice is the user's to settle before anything plans it
        let twice = format!("{lines}\"pass/vip\" = {{ id = 9, name = \"VIP\" }}\n");
        std::fs::write(root.join(lock::FILE), universe7(&twice)).unwrap();
        let (lock, _) = lock::read(&root).unwrap();
        let refused = survey(&cfg, &cfg.environments[0], &lock).err().unwrap();
        assert_eq!(refused.json["code"], "lock.conflict");

        // the description is no longer managed: nothing is sent, and the record stops holding it
        let root = project("unmanaged", "[passes.vip]\nname = \"VIP\"\nprice = 100\n", &[], vip);
        let mock = Mock::start();
        let outcome = provision_at(&root, &mock, false);
        assert_eq!((outcome.exit, &outcome.json["applied"]), (0, &json!(1)), "{}", outcome.text);
        assert!(mock.requests().is_empty());
        assert_eq!(
            records(&root)["pass/vip"],
            Resource { id: Some(1), name: Some("VIP".into()), price: Some(100), ..Resource::default() }
        );
    }

    #[test]
    fn nothing_is_sent_while_a_badge_would_spend_unasked_or_a_lost_create_waits_for_adopt() {
        let icon = svg("ff0000");
        let toml = "[passes.vip]\nname = \"VIP\"\nprice = 100\nicon = \"i.svg\"\n\
                    [badges.welcome]\nname = \"Welcome\"\ndescription = \"Hi\"\nicon = \"i.svg\"\nexpectedCost = 100\n";
        let root = project("spend", toml, &[("i.svg", &icon)], "");
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let refused = provision_at(&root, &mock, false);
        assert_eq!((refused.exit, &refused.json["code"]), (2, &json!("resource.spend")));
        assert!(refused.text.contains("badge/welcome costs its expectedCost (100 Robux"), "{}", refused.text);

        // the pass's intent is on disk while its create is in flight, and the answer is lost
        let lock_file = root.join(lock::FILE);
        mock.on(
            "POST",
            "game-passes/",
            Reply::Run(Box::new(move |_| {
                let text = std::fs::read_to_string(&lock_file).unwrap();
                assert!(text.contains("\"pass/vip\" = { next = "), "{text}");
                Reply::Reset
            })),
        );
        let lost = provision_at(&root, &mock, true);
        assert_eq!((lost.exit, &lost.json["code"]), (1, &json!("resource.pending")), "{}", lost.text);
        assert!(lost.text.contains("opossum adopt pass vip <id>"), "{}", lost.text);
        let blocked = provision_at(&root, &mock, true);
        assert_eq!((blocked.exit, &blocked.json["code"]), (2, &json!("resource.blocked")));
        assert_eq!(sent(&mock), ["POST game-passes/v1/universes/7/game-passes"]);

        // it was created after all: the adopted pass gets every declared field, and then nothing changes
        assert_eq!(adopt_at(&root, "pass", "vip", Some(900)).exit, 0);
        assert_eq!(records(&root)["pass/vip"], Resource { id: Some(900), ..Resource::default() });
        let done = provision_at(&root, &mock, true);
        assert_eq!((done.exit, &done.json["applied"]), (0, &json!(2)), "{}", done.text);
        let requests = mock.requests();
        assert_eq!(
            sent(&mock)[1..],
            ["PATCH game-passes/v1/universes/7/game-passes/900", "POST legacy-badges/v1/universes/7/badges"]
        );
        let patch = String::from_utf8_lossy(&requests[1].body);
        assert!(["name=\"name\"", "name=\"price\"", "name=\"imageFile\""].iter().all(|p| patch.contains(p)), "{patch}");
        let badge = String::from_utf8_lossy(&requests[2].body);
        assert!(
            badge.contains("name=\"expectedCost\"\r\n\r\n100\r\n")
                && badge.contains("name=\"paymentSourceType\"\r\n\r\n1\r\n")
        );
        let again = provision_at(&root, &mock, false);
        assert_eq!((again.exit, &again.json["noop"], mock.requests().len()), (0, &json!(2), 3), "{}", again.text);
    }

    #[test]
    fn a_new_thumbnail_image_goes_up_before_the_old_one_goes_unless_the_carousel_is_full() {
        let thumbnails = |keys: &[&str]| {
            keys.iter()
                .map(|k| format!("[[experience.thumbnails]]\nkey = \"{k}\"\nimage = \"{k}.svg\"\n"))
                .collect::<String>()
        };
        let (old, new) = (svg("ff0000"), svg("00ff00"));
        let old_hash = hash(&old, Preset::Thumbnail);

        // below the limit: upload, order, then delete the old image
        let lines = format!(
            "\"thumbnail/a\" = {{ id = 41, image = \"{old_hash}\" }}\n\"experience/order\" = {{ ids = [41] }}\n"
        );
        let root = project("below", &thumbnails(&["a"]), &[("a.svg", &new)], &lines);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        assert_eq!(provision_at(&root, &mock, false).exit, 0);
        assert_eq!(sent(&mock), ["POST image", "POST images/order", "DELETE images/41"]);
        assert_eq!(mock.requests()[1].json(), json!({ "mediaAssetIds": [2_000_001] }));
        let a = Resource { id: Some(2_000_001), image: Some(hash(&new, Preset::Thumbnail)), ..Resource::default() };
        assert_eq!(records(&root)["thumbnail/a"], a);

        // at the limit: the old image goes first
        let keys: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let colors: Vec<String> = (0..10).map(|i| svg(&format!("0000{i:02}"))).collect();
        let mut files: Vec<(String, &str)> =
            keys.iter().zip(&colors).map(|(k, c)| (format!("{k}.svg"), c.as_str())).collect();
        let mut lines: String = keys
            .iter()
            .zip(&colors)
            .enumerate()
            .map(|(i, (k, c))| {
                format!("\"thumbnail/{k}\" = {{ id = {}, image = \"{}\" }}\n", 40 + i, hash(c, Preset::Thumbnail))
            })
            .collect();
        lines += "\"experience/order\" = { ids = [40, 41, 42, 43, 44, 45, 46, 47, 48, 49] }\n";
        files[0].1 = &new;
        let files: Vec<(&str, &str)> = files.iter().map(|(f, t)| (f.as_str(), *t)).collect();
        let root = project("full", &thumbnails(&keys), &files, &lines);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        assert_eq!(provision_at(&root, &mock, false).exit, 0);
        assert_eq!(sent(&mock), ["DELETE images/40", "POST image", "POST images/order"]);

        // stale images count against the limit: no new thumbnail fits, and nothing is sent
        let stale =
            format!("\"thumbnail/a\" = {{ id = 41, image = \"{old_hash}\", stale = [1, 2, 3, 4, 5, 6, 7, 8, 9] }}\n");
        let root = project("limit", &thumbnails(&["a", "b"]), &[("a.svg", &old), ("b.svg", &new)], &stale);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let refused = provision_at(&root, &mock, false);
        assert_eq!((refused.exit, &refused.json["code"]), (2, &json!("resource.limit")), "{}", refused.text);
        assert!(mock.requests().is_empty());

        // an earlier run's stale image goes even when the order has nothing to change
        let lines =
            format!("{}\"experience/order\" = {{ ids = [41] }}\n", stale.replace("1, 2, 3, 4, 5, 6, 7, 8, 9", "30"));
        let root = project("stale", &thumbnails(&["a"]), &[("a.svg", &old)], &lines);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        assert_eq!(provision_at(&root, &mock, false).exit, 0);
        assert_eq!(sent(&mock), ["DELETE images/30"]);
        assert_eq!(records(&root)["thumbnail/a"].stale, None);

        // a lost upload is adopted only while the declaration is the one it sent
        let sending = lock::intent(&Resource { image: Some(hash(&new, Preset::Thumbnail)), ..Resource::default() });
        let lines = format!("\"thumbnail/a\" = {{ id = 41, image = \"{old_hash}\", next = \"{sending}\" }}\n");
        let root = project("adopt", &thumbnails(&["a"]), &[("a.svg", &svg("0000ff"))], &lines);
        let refused = adopt_at(&root, "thumbnail", "a", Some(77));
        assert_eq!((refused.exit, &refused.json["code"]), (2, &json!("resource.adopt")));
        std::fs::write(root.join("a.svg"), &new).unwrap();
        assert_eq!(adopt_at(&root, "thumbnail", "a", Some(77)).exit, 0);
        let a = Resource {
            id: Some(77),
            image: Some(hash(&new, Preset::Thumbnail)),
            stale: Some(vec![41]),
            ..Resource::default()
        };
        assert_eq!(records(&root)["thumbnail/a"], a);
    }
}
