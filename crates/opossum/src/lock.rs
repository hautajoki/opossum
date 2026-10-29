//! `opossum.lock`: the ids opossum created on Roblox, committed beside `opossum.toml`. TOML with one record per line,
//! sorted, each naming its own table (`assets."<file>" = …`, `packs."<key>" = …`, `universes.<id>."<kind>/<key>" = …`)
//! with no section headers, so every record stands on its own wherever a merge puts it: the first write adds
//! `opossum.lock merge=union` to `.gitattributes`, and [`parse`] reads what any merge leaves (both sides of a conflict,
//! a key twice, lines in any order) without a hand edit. Builds only read it; the commands that change it write it
//! through [`save`], which takes the [`RemoteGuard`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::SystemTime;

use cloud::Creator;
pub use place::Part;

use crate::diag::{Diag, Severity};
use crate::guard::RemoteGuard;

pub const FILE: &str = "opossum.lock";
/// Where [`save`] puts the state it could not write, under `.opossum/`.
const RECOVERED: &str = "opossum.lock.recovered";
const HEADER: &str =
    "# opossum.lock: ids opossum created on Roblox. Commit it. opossum keeps it; a merge needs no hand edits.";
const VERSION: i64 = 1;

/// One source file's record. `hash` is the input hash of what `id` holds; `next` is the input hash of a create in
/// flight and `op` its operation, so `next` without `op` is a create that may or may not have happened. `rejected` is
/// the input hash of content Roblox refused (moderation, processing): the same bytes are never sent again. `parts` is
/// what the import of a `.glb`'s `id` made, by part name, read back once; a glb's record with `id` and no `parts` is
/// unread.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Asset {
    pub id: Option<u64>,
    pub hash: Option<String>,
    pub next: Option<String>,
    pub op: Option<String>,
    pub rejected: Option<String>,
    pub parts: Option<BTreeMap<String, Part>>,
}

impl Asset {
    /// The same upload state, read back or not: a merge of a branch that read the file with one that didn't leaves
    /// these two, which are one record.
    fn same(&self, other: &Asset) -> bool {
        (self.id, &self.hash, &self.next, &self.op, &self.rejected)
            == (other.id, &other.hash, &other.next, &other.op, &other.rejected)
    }
}

/// One resource's applied values: what Roblox holds, not a fingerprint of it, so a plan diffs field by field. Image
/// fields hold the image's input hash. Fields are written in this order.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Resource {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub price: Option<u64>,
    pub for_sale: Option<bool>,
    pub managed_pricing: Option<bool>,
    pub enabled: Option<bool>,
    pub alt_text: Option<String>,
    pub icon: Option<String>,
    pub image: Option<String>,
    /// The thumbnail order applied.
    pub ids: Option<Vec<u64>>,
    /// Replaced thumbnails' media ids awaiting deletion.
    pub stale: Option<Vec<u64>>,
    /// The hash of the values an ambiguous create asked for.
    pub next: Option<String>,
}

#[derive(Default)]
pub struct Lock {
    /// Who the `assets.` records' ids belong to, set by the first upload.
    pub creator: Option<Creator>,
    /// By source file path relative to the project root, with its extension, so renaming a set orphans nothing.
    pub assets: BTreeMap<String, Asset>,
    /// Every different record a merge left for one file, each written back until a command that hashed the file
    /// [`Lock::settle`]s it; `assets` holds the build's pick meanwhile.
    pub merged: BTreeMap<String, Vec<Asset>>,
    /// Texture packs Studio baked, by `place`'s pack key (`<class>|<color>|<normal>|<roughness>|<metalness>`, the map
    /// uris): the pack's id.
    pub packs: BTreeMap<String, u64>,
    /// By universe id, then `<kind>/<key>` (`pass/vip`, `experience/icon`).
    pub universes: BTreeMap<u64, BTreeMap<String, Resource>>,
    /// Every different record a merge left for one resource; `universes` holds the first. Each is written back until
    /// the user keeps one: no command decides which is live.
    pub conflicts: BTreeMap<(u64, String), Vec<Resource>>,
    /// The other `creator` lines a merge left, written back like `conflicts`; `creator` is the first.
    pub other_creators: Vec<Creator>,
    /// The file as this process last read or wrote it; `None` while there is none.
    disk: Option<Disk>,
}

struct Disk {
    text: String,
    len: u64,
    mtime: Option<SystemTime>,
}

/// Reads `<root>/opossum.lock`; a missing file is an empty lock. Problems in the text come back as diagnostics, an
/// unreadable file as the error.
pub fn read(root: &Path) -> Result<(Lock, Vec<Diag>), String> {
    let path = root.join(FILE);
    let mut file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Lock::default(), Vec::new())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut text = String::new();
    let meta = file.metadata().and_then(|m| file.read_to_string(&mut text).map(|_| m));
    let meta = meta.map_err(|e| format!("{}: {e}", path.display()))?;
    let (mut lock, diags) = parse(&text, &path);
    lock.disk = Some(Disk { text, len: meta.len(), mtime: meta.modified().ok() });
    Ok((lock, diags))
}

/// Reads the lock's text line by line, each record on its own, so one bad line costs only that record. Git conflict
/// markers are skipped, keeping both sides' records; a key twice with different records is kept for [`Lock::settle`]
/// (assets) or marked in `conflicts` (resources). An asset record and the same one with `parts` are one record, and
/// two ids for one pack are both good packs of its maps, so the highest is kept: neither is a conflict.
pub fn parse(text: &str, file: &Path) -> (Lock, Vec<Diag>) {
    let mut lock = Lock::default();
    let mut records: BTreeMap<String, Vec<Asset>> = BTreeMap::new();
    let mut diags = Vec::new();
    let mut version = false;
    let mut content = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        let mut fail = |code: &str, message: String| {
            let mut d = Diag::new(code, Severity::Error, Some(file), message);
            (d.line, d.col) = (n as u32 + 1, 1);
            diags.push(d);
        };
        if line.is_empty()
            || line.starts_with('#')
            || ["<<<<<<<", "|||||||", "=======", ">>>>>>>"].iter().any(|m| line.starts_with(m))
        {
            continue;
        }
        if line.starts_with('[') {
            fail("lock.syntax", format!("`{line}`: the lock has no sections; each record names its table"));
            continue;
        }
        let entry = line.parse::<toml::Table>().map_err(|e| e.message().to_owned());
        let entry = entry.and_then(|t| t.into_iter().next().ok_or_else(|| "expected `key = value`".to_owned()));
        let (key, value) = match entry {
            Ok(entry) => entry,
            Err(e) => {
                fail("lock.syntax", e);
                continue;
            }
        };
        content = true;
        let read = match key.as_str() {
            "version" => match value.as_integer() {
                Some(VERSION) => {
                    version = true;
                    Ok(())
                }
                Some(v) => {
                    let message = format!("opossum.lock is version {v}; this opossum reads {VERSION}");
                    let mut d = Diag::new("lock.version", Severity::Error, Some(file), message);
                    d.line = n as u32 + 1;
                    return (Lock::default(), vec![d]);
                }
                None => Err("`version` must be an integer".to_owned()),
            },
            "creator" => crate::config::creator(&value).map(|c| match lock.creator {
                Some(old) if old != c && !lock.other_creators.contains(&c) => lock.other_creators.push(c),
                Some(_) => {}
                _ => lock.creator = Some(c),
            }),
            "assets" => one(&value, "assets.\"<file>\"").and_then(|(key, value)| {
                let a = asset(value)?;
                let all = records.entry(key.clone()).or_default();
                match all.iter_mut().find(|r| r.same(&a)) {
                    Some(r) => {
                        if r.parts.is_none() {
                            r.parts = a.parts;
                        }
                    }
                    None => all.push(a),
                }
                Ok(())
            }),
            "packs" => one(&value, "packs.\"<key>\"").and_then(|(key, value)| {
                if key.split('|').count() != 5 {
                    return Err(format!("`{key}` is not <class>|<color>|<normal>|<roughness>|<metalness>"));
                }
                let id = value.as_integer().filter(|&n| n > 0).ok_or("a pack's value is its id, a positive integer")? as u64;
                let old = lock.packs.entry(key.clone()).or_default();
                *old = id.max(*old);
                Ok(())
            }),
            "universes" => one(&value, "universes.<id>").and_then(|(u, value)| {
                let u = u.parse().ok().filter(|&u: &u64| u > 0).ok_or(format!("`{u}` is not a universe id"))?;
                let (key, value) = one(value, &format!("universes.{u}.\"<kind>/<key>\""))?;
                if !resource_key(key) {
                    return Err(format!(
                        "`{key}` is not pass/, product/, badge/ or thumbnail/<key>, or experience/metadata, icon or order"
                    ));
                }
                let r = resource(value)?;
                let records = lock.universes.entry(u).or_default();
                match records.get(key) {
                    None => {
                        records.insert(key.clone(), r);
                    }
                    Some(old) if *old == r => {}
                    Some(old) => {
                        let all = lock.conflicts.entry((u, key.clone())).or_insert_with(|| vec![old.clone()]);
                        if !all.contains(&r) {
                            all.push(r);
                        }
                    }
                }
                Ok(())
            }),
            _ => Err(format!(
                "`{key}` is not version, creator, assets.\"<file>\", packs.\"<key>\" or universes.<id>.\"<kind>/<key>\""
            )),
        };
        if let Err(e) = read {
            fail("lock.syntax", e);
        }
    }
    if content && !version {
        let message = "opossum.lock has no `version = 1` line".to_owned();
        diags.push(Diag::new("lock.version", Severity::Error, Some(file), message));
    }
    for (key, mut all) in records {
        if all.len() > 1 {
            lock.assets.insert(key.clone(), pick(&all, None).clone());
            lock.merged.insert(key, all);
        } else {
            lock.assets.insert(key, all.pop().expect("a record"));
        }
    }
    (lock, diags)
}

/// Which of a merge's records for one file holds (§2.4): with the file's current hash, one whose id holds that
/// content, then one whose create in flight sends it (resumed or settled, never sent again); then the highest id with
/// no create in flight, since Roblox ids grow; then the first create in flight.
fn pick<'a>(all: &'a [Asset], hash: Option<&str>) -> &'a Asset {
    let holds = |a: &&Asset| a.id.is_some() && hash.is_some() && a.hash.as_deref() == hash;
    all.iter()
        .find(holds)
        .or_else(|| all.iter().find(|a| hash.is_some() && a.next.as_deref() == hash))
        .or_else(|| all.iter().filter(|a| a.id.is_some() && a.next.is_none()).max_by_key(|a| a.id))
        .or_else(|| all.iter().find(|a| a.next.is_some()))
        .unwrap_or(&all[0])
}

/// `lock.merged` for a file with several records after a merge: what builds use meanwhile, or (`settled`) the one a
/// command that hashed the file keeps.
fn merged_warning(root: &Path, file: &str, a: &Asset, settled: bool) -> Diag {
    let used = match a.id {
        Some(id) if !(settled && a.next.is_some()) => format!("id {id}"),
        _ => "the upload in flight".to_owned(),
    };
    let message = match settled {
        true => format!("{file} had several records after a merge; keeping {used}"),
        false => {
            format!("{file} has several records after a merge; builds use {used} until `opossum upload` settles it")
        }
    };
    Diag::new("lock.merged", Severity::Warning, Some(&root.join(FILE)), message)
}

impl Lock {
    /// For a command that hashed `file`: picks its record among a merge's with the file's current `hash`, so the
    /// next save writes only that one, and says which id it took.
    pub fn settle(&mut self, root: &Path, file: &str, hash: &str) -> Option<Diag> {
        let all = self.merged.remove(file)?;
        let a = pick(&all, Some(hash)).clone();
        let warning = merged_warning(root, file, &a, true);
        self.assets.insert(file.to_owned(), a);
        Some(warning)
    }

    /// `lock.merged` for every file a merge left several records for, naming the id a build uses.
    pub fn merge_warnings(&self, root: &Path) -> Vec<Diag> {
        self.merged.keys().map(|file| merged_warning(root, file, &self.assets[file], false)).collect()
    }

    /// `lock.conflict` for every resource a merge left twice: a warning in builds (the resource is left out of
    /// `opossum/resources`), an error where a double create would be a real decision.
    pub fn conflicts(&self, root: &Path, severity: Severity) -> Vec<Diag> {
        let file = root.join(FILE);
        let message = |u, key| format!("two records for {key} in universe {u} after a merge; keep the live one");
        self.conflicts
            .keys()
            .map(|(u, key)| Diag::new("lock.conflict", severity, Some(&file), message(u, key)))
            .collect()
    }

    /// Before an upload as `creator`: the lock's assets must belong to it, and a merge must not have left two.
    pub fn check_creator(&self, root: &Path, creator: Creator) -> Option<Diag> {
        let file = root.join(FILE);
        if !self.other_creators.is_empty() {
            let message = "two different `creator` lines after a merge; keep the one the `assets.` ids belong to";
            return Some(Diag::new("lock.conflict", Severity::Error, Some(&file), message.into()));
        }
        let ours = self.creator.filter(|&c| c != creator)?;
        let message = format!(
            "opossum.lock's assets were uploaded as {} but opossum.toml says {}. Their ids keep working. To upload \
             everything again as the new creator, delete `creator` and every `assets.` line from opossum.lock",
            creator_name(ours),
            creator_name(creator)
        );
        Some(Diag::new("lock.creator", Severity::Error, Some(&file), message))
    }

    fn is_empty(&self) -> bool {
        self.creator.is_none()
            && self.assets.is_empty()
            && self.packs.is_empty()
            && self.universes.values().all(BTreeMap::is_empty)
    }
}

/// `user:7`, as messages name a creator.
pub fn creator_name(c: Creator) -> String {
    match c {
        Creator::User(id) => format!("user:{id}"),
        Creator::Group(id) => format!("group:{id}"),
    }
}

/// The one entry of a dotted key's table: a line holds one record, `what` spelling where it goes.
fn one<'a>(v: &'a toml::Value, what: &str) -> Result<(&'a String, &'a toml::Value), String> {
    let table = v.as_table().filter(|t| t.len() == 1).ok_or(format!("expected one record per line, as {what} = …"))?;
    Ok(table.iter().next().expect("one entry"))
}

fn asset(v: &toml::Value) -> Result<Asset, String> {
    let mut a = Asset::default();
    for (k, v) in v.as_table().ok_or("a record must be an inline table")? {
        match k.as_str() {
            "id" => a.id = Some(positive(v, k)?),
            "hash" => a.hash = Some(hash(v, k)?),
            "next" => a.next = Some(hash(v, k)?),
            "rejected" => a.rejected = Some(hash(v, k)?),
            "parts" => a.parts = Some(parts(v)?),
            "op" => {
                let op = v.as_str().filter(|op| {
                    op.strip_prefix("operations/").is_some_and(|id| {
                        !id.is_empty() && !id.contains(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
                    })
                });
                a.op = Some(op.ok_or("`op` must be operations/<id>")?.to_owned());
            }
            _ => return Err(format!("unknown field `{k}`")),
        }
    }
    if a.id.is_some() != a.hash.is_some() {
        return Err("`id` and `hash` come together".into());
    }
    if a.id.is_none() && a.next.is_none() && a.rejected.is_none() {
        return Err("a record needs `id`, `next` or `rejected`".into());
    }
    if a.op.is_some() && a.next.is_none() {
        return Err("`op` needs `next`".into());
    }
    if a.parts.is_some() && a.id.is_none() {
        return Err("`parts` needs `id`".into());
    }
    Ok(a)
}

/// `{ <name> = { mesh, size, color, normal, roughness, metalness }, … }`; a size component may be written as an
/// integer.
fn parts(v: &toml::Value) -> Result<BTreeMap<String, Part>, String> {
    let table = v.as_table().ok_or("`parts` must be a table of parts by name")?;
    let mut parts = BTreeMap::new();
    for (name, v) in table {
        let (mut mesh, mut size, mut maps) = (None, None, [None; 4]);
        for (k, v) in v.as_table().ok_or(format!("part `{name}` must be a table"))? {
            match k.as_str() {
                "mesh" => mesh = Some(positive(v, k)?),
                "size" => {
                    let bad = || format!("part `{name}`'s `size` must be three numbers");
                    let items = v.as_array().filter(|a| a.len() == 3).ok_or_else(bad)?;
                    let mut xyz = [0.0; 3];
                    for (n, item) in xyz.iter_mut().zip(items) {
                        *n = item.as_float().or(item.as_integer().map(|i| i as f64)).ok_or_else(bad)? as f32;
                    }
                    size = Some(xyz);
                }
                "color" => maps[0] = Some(positive(v, k)?),
                "normal" => maps[1] = Some(positive(v, k)?),
                "roughness" => maps[2] = Some(positive(v, k)?),
                "metalness" => maps[3] = Some(positive(v, k)?),
                _ => return Err(format!("part `{name}` has an unknown field `{k}`")),
            }
        }
        let (Some(mesh), Some(size)) = (mesh, size) else {
            return Err(format!("part `{name}` needs `mesh` and `size`"));
        };
        let [color, normal, roughness, metalness] = maps;
        parts.insert(name.clone(), Part { mesh, size, color, normal, roughness, metalness });
    }
    Ok(parts)
}

fn resource(v: &toml::Value) -> Result<Resource, String> {
    let mut r = Resource::default();
    let string = |v: &toml::Value, k: &str| v.as_str().map(str::to_owned).ok_or(format!("`{k}` must be a string"));
    let boolean = |v: &toml::Value, k: &str| v.as_bool().ok_or(format!("`{k}` must be true or false"));
    let ids = |v: &toml::Value, k: &str| {
        let items = v.as_array().ok_or(format!("`{k}` must be an array of ids"))?;
        items.iter().map(|id| positive(id, k)).collect::<Result<Vec<_>, _>>()
    };
    for (k, v) in v.as_table().ok_or("a record must be an inline table")? {
        match k.as_str() {
            "id" => r.id = Some(positive(v, k)?),
            "name" => r.name = Some(string(v, k)?),
            "description" => r.description = Some(string(v, k)?),
            "price" => r.price = Some(positive(v, k)?),
            "forSale" => r.for_sale = Some(boolean(v, k)?),
            "managedPricing" => r.managed_pricing = Some(boolean(v, k)?),
            "enabled" => r.enabled = Some(boolean(v, k)?),
            "altText" => r.alt_text = Some(string(v, k)?),
            "icon" => r.icon = Some(hash(v, k)?),
            "image" => r.image = Some(hash(v, k)?),
            "ids" => r.ids = Some(ids(v, k)?),
            "stale" => r.stale = Some(ids(v, k)?),
            "next" => r.next = Some(hash(v, k)?),
            _ => return Err(format!("unknown field `{k}`")),
        }
    }
    Ok(r)
}

fn resource_key(key: &str) -> bool {
    match key.split_once('/') {
        Some(("pass" | "product" | "badge" | "thumbnail", key)) => {
            key.starts_with(|c: char| c.is_ascii_alphabetic())
                && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        }
        Some(("experience", "metadata" | "icon" | "order")) => true,
        _ => false,
    }
}

fn positive(v: &toml::Value, k: &str) -> Result<u64, String> {
    v.as_integer().filter(|&n| n > 0).map(|n| n as u64).ok_or(format!("`{k}` must be a positive integer"))
}

fn hash(v: &toml::Value, k: &str) -> Result<String, String> {
    v.as_str()
        .filter(|h| h.len() == 32 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .map(str::to_owned)
        .ok_or(format!("`{k}` must be 32 lowercase hex characters"))
}

/// The lock's only form: the header, `version` and `creator`, then the asset records, the packs and each universe's
/// records (by number), a blank line before each group, records sorted bytewise by key, each record's fields in a fixed
/// order.
pub fn render(lock: &Lock) -> String {
    let mut out = format!("{HEADER}\nversion = {VERSION}\n");
    for c in lock.creator.iter().chain(&lock.other_creators) {
        let (kind, id) = match c {
            Creator::User(id) => ("user", id),
            Creator::Group(id) => ("group", id),
        };
        let _ = writeln!(out, "creator = {{ {kind} = {id} }}");
    }
    if !lock.assets.is_empty() {
        out.push('\n');
    }
    for (key, a) in &lock.assets {
        let mut lines: Vec<String> = match lock.merged.get(key) {
            Some(all) => all.iter().map(asset_fields).collect(),
            None => vec![asset_fields(a)],
        };
        lines.sort();
        for fields in lines {
            let _ = writeln!(out, "assets.{} = {fields}", self::key(key));
        }
    }
    if !lock.packs.is_empty() {
        out.push('\n');
    }
    for (key, id) in &lock.packs {
        let _ = writeln!(out, "packs.{} = {id}", self::key(key));
    }
    for (u, records) in lock.universes.iter().filter(|(_, r)| !r.is_empty()) {
        out.push('\n');
        for (key, r) in records {
            let all = lock.conflicts.get(&(*u, key.clone())).map_or(std::slice::from_ref(r), Vec::as_slice);
            for r in all {
                record(&mut out, &format!("universes.{u}.{}", self::key(key)), &resource_fields(r));
            }
        }
    }
    out
}

/// An asset record's inline table.
fn asset_fields(a: &Asset) -> String {
    let mut fields = Vec::new();
    if let Some(id) = a.id {
        fields.push(("id", id.to_string()));
    }
    for (name, value) in [("hash", &a.hash), ("next", &a.next), ("op", &a.op), ("rejected", &a.rejected)] {
        if let Some(s) = value {
            fields.push((name, string(s)));
        }
    }
    if let Some(parts) = &a.parts {
        let parts: Vec<(String, String)> = parts.iter().map(|(name, p)| (key(name), part_fields(p))).collect();
        fields.push(("parts", inline(&parts)));
    }
    inline(&fields)
}

/// A part's inline table. Sizes are written as Rust's `{:?}` writes an f32, the shortest text that reads back as the
/// same number, and always a TOML float (`8.0`, `1e-7`).
fn part_fields(p: &Part) -> String {
    let [x, y, z] = p.size;
    let mut fields = vec![("mesh", p.mesh.to_string()), ("size", format!("[{x:?}, {y:?}, {z:?}]"))];
    for (name, map) in
        [("color", p.color), ("normal", p.normal), ("roughness", p.roughness), ("metalness", p.metalness)]
    {
        if let Some(id) = map {
            fields.push((name, id.to_string()));
        }
    }
    inline(&fields)
}

fn resource_fields(r: &Resource) -> Vec<(&'static str, String)> {
    let mut f = Vec::new();
    let ids = |ids: &[u64]| format!("[{}]", ids.iter().map(u64::to_string).collect::<Vec<_>>().join(", "));
    if let Some(id) = r.id {
        f.push(("id", id.to_string()));
    }
    if let Some(s) = &r.name {
        f.push(("name", string(s)));
    }
    if let Some(s) = &r.description {
        f.push(("description", string(s)));
    }
    if let Some(price) = r.price {
        f.push(("price", price.to_string()));
    }
    for (name, value) in [("forSale", r.for_sale), ("managedPricing", r.managed_pricing), ("enabled", r.enabled)] {
        if let Some(b) = value {
            f.push((name, b.to_string()));
        }
    }
    for (name, value) in [("altText", &r.alt_text), ("icon", &r.icon), ("image", &r.image)] {
        if let Some(s) = value {
            f.push((name, string(s)));
        }
    }
    for (name, value) in [("ids", &r.ids), ("stale", &r.stale)] {
        if let Some(v) = value {
            f.push((name, ids(v)));
        }
    }
    if let Some(s) = &r.next {
        f.push(("next", string(s)));
    }
    f
}

/// The hash a resource create saves as `next` before its request: of the values it asks for, rendered as the record
/// renders them without `id`, `ids`, `stale` and `next`, so `adopt` can tell whether the declaration changed since.
pub fn intent(desired: &Resource) -> String {
    let values = Resource { id: None, ids: None, stale: None, next: None, ..desired.clone() };
    let mut h = blake3::Hasher::new();
    h.update(b"opossum-resource-1\0");
    h.update(inline(&resource_fields(&values)).as_bytes());
    h.finalize().as_bytes()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// `<dotted key> = { … }`.
fn record(out: &mut String, key: &str, fields: &[(&str, String)]) {
    let _ = writeln!(out, "{key} = {}", inline(fields));
}

/// A TOML key: bare when it is only `[A-Za-z0-9_-]`, else a basic string.
pub(crate) fn key(k: &str) -> String {
    let bare = !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare { k.to_owned() } else { string(k) }
}

/// `{ a = 1, b = "x" }`, or `{}`.
fn inline(fields: &[(impl std::fmt::Display, String)]) -> String {
    if fields.is_empty() {
        return "{}".into();
    }
    let fields: Vec<String> = fields.iter().map(|(k, v)| format!("{k} = {v}")).collect();
    format!("{{ {} }}", fields.join(", "))
}

/// A TOML basic string: `"`, `\`, `\n` and `\t` escaped, every other C0 control and U+007F as `\uXXXX`.
pub(crate) fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes the lock when its text changed: to a temporary file, synced, renamed over `opossum.lock`, then the directory
/// synced, so a crash leaves the old lock or the new one. Something else having changed the file since this process
/// read or wrote it (git, an editor) is `lock.changedOnDisk`: nothing is overwritten, and the state goes to
/// `.opossum/opossum.lock.recovered`. The write that creates the lock also gives it `merge=union` in `.gitattributes`,
/// and says so in the returned note.
pub fn save(lock: &mut Lock, root: &Path, _guard: &RemoteGuard) -> Result<Option<String>, Diag> {
    let path = root.join(FILE);
    let fail = |code: &str, message: String| Diag::new(code, Severity::Error, Some(&path), message);
    match write(lock, root)? {
        Written::Saved(note) => Ok(note),
        Written::Changed(text) => {
            let dir = crate::editor::create(root).map_err(|e| fail("lock.io", e))?;
            fs::write(dir.join(RECOVERED), &text).map_err(|e| fail("lock.io", e.to_string()))?;
            let message = format!(
                "opossum.lock changed while opossum was writing it (git?), so nothing was overwritten: this run's ids \
                 are in .opossum/{RECOVERED}, and no command writes the lock until they are folded back \
                 (lock.recovered)"
            );
            Err(fail("lock.changedOnDisk", message))
        }
    }
}

/// [`save`] for a writer whose state is read again from the lock each time (dev's pack capture): a lock something
/// else changed is left as it is and nothing goes to `.opossum/`, so the writer never leaves a `lock.recovered` that
/// would refuse the user's own commands. False when nothing was written for that reason.
pub fn save_or_drop(lock: &mut Lock, root: &Path, _guard: &RemoteGuard) -> Result<bool, Diag> {
    Ok(matches!(write(lock, root)?, Written::Saved(_)))
}

enum Written {
    /// Written, or unchanged; the note says the lock was created.
    Saved(Option<String>),
    /// The file changed on disk since this process read or wrote it: nothing was written, and this is the text.
    Changed(String),
}

fn write(lock: &mut Lock, root: &Path) -> Result<Written, Diag> {
    let path = root.join(FILE);
    let text = render(lock);
    let unchanged = match &lock.disk {
        Some(disk) => disk.text == text,
        None => lock.is_empty(),
    };
    if unchanged {
        return Ok(Written::Saved(None));
    }
    let fail = |code: &str, message: String| Diag::new(code, Severity::Error, Some(&path), message);
    let io = |e: std::io::Error| fail("lock.io", e.to_string());
    let now = fs::metadata(&path).ok().map(|m| (m.len(), m.modified().ok()));
    if now != lock.disk.as_ref().map(|d| (d.len, d.mtime)) {
        return Ok(Written::Changed(text));
    }
    let tmp = root.join(format!(".{FILE}.tmp-{}", std::process::id()));
    let mut file = fs::File::create(&tmp).map_err(io)?;
    file.write_all(text.as_bytes()).and_then(|_| file.sync_all()).map_err(io)?;
    // the stamp of what this process wrote, taken before the rename (which keeps it): a stat after it could take the
    // stamp of a lock git wrote in between, and the next save would overwrite that lock
    let meta = file.metadata().map_err(io)?;
    fs::rename(&tmp, &path).map_err(io)?;
    fs::File::open(root).and_then(|d| d.sync_all()).map_err(io)?;
    let created = lock.disk.is_none();
    lock.disk = Some(Disk { text, len: meta.len(), mtime: meta.modified().ok() });
    if !created {
        return Ok(Written::Saved(None));
    }
    let attributes = root.join(".gitattributes");
    let old = match fs::read_to_string(&attributes) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail("lock.io", format!("{}: {e}", attributes.display()))),
    };
    if old.lines().any(|l| l.contains(FILE)) {
        return Ok(Written::Saved(None));
    }
    let line = format!("{}{FILE} merge=union\n", if old.is_empty() || old.ends_with('\n') { "" } else { "\n" });
    let append = fs::OpenOptions::new().append(true).create(true).open(&attributes);
    append
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| fail("lock.io", format!("{}: {e}", attributes.display())))?;
    Ok(Written::Saved(Some(format!("wrote .gitattributes ({FILE} merge=union)"))))
}

/// `lock.recovered` while a [`save`] that found the lock changed on disk left its state aside: every command that writes
/// the lock refuses until it is folded back, since a run from the lock git left would send again what that state holds.
pub fn recovered(root: &Path) -> Option<Diag> {
    let path = crate::editor::dir(root).join(RECOVERED);
    if !path.exists() {
        return None;
    }
    let message = format!(
        "a run could not save opossum.lock and left its ids in .opossum/{RECOVERED}. On the branch it ran on, fold them \
         back with `cat .opossum/{RECOVERED} >> {FILE} && rm .opossum/{RECOVERED}` (lock.merged and lock.conflict \
         then say what to settle), or `rm` the file to drop them"
    );
    Some(Diag::new("lock.recovered", Severity::Error, Some(&path), message))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::guard::{Cache, RemoteGuard};

    const CANONICAL: &str = r#"# opossum.lock: ids opossum created on Roblox. Commit it. opossum keeps it; a merge needs no hand edits.
version = 1
creator = { user = 988525232 }

assets."place/assets/animations/idle.rbxmx" = { id = 154467536451246, hash = "6d76e3e3a1c94f0b2e8d7c51a0b3f9e2" }
assets."place/assets/models/lantern.glb" = { id = 156800916240477, hash = "0bc03d3cf5b65244b3aa6352c0a80671", parts = { Lens = { mesh = 83567644535122, size = [0.4272, 0.3491, 0.9131], color = 199676980884141, normal = 147280647780558, roughness = 131313702417440, metalness = 86729430079521 }, hull = { mesh = 87525364898392, size = [8.0, 1e-7, 4.99756], color = 199676980884141 } } }
assets."place/assets/sfx/new.ogg" = { next = "9b1f0c2d4e5a6b7c8d9e0f1a2b3c4d5e", op = "operations/9f2c41" }
assets."place/assets/sfx/odd.ogg" = { id = 140112233445566, hash = "00000000000000000000000000000001", next = "00000000000000000000000000000002" }

packs."MaterialVariant|rbxassetid://5|||" = 7
packs."SurfaceAppearance|rbxassetid://199676980884141|rbxassetid://147280647780558|rbxassetid://131313702417440|rbxassetid://86729430079521" = 83799176238290

universes.42."pass/vip" = { id = 9 }

universes.15302144394."badge/welcome" = { id = 2124, name = "Welcome", description = "Joined\tthe \"game\"\n\u0001\u007F", enabled = true, icon = "0123456789abcdef0123456789abcdef" }
universes.15302144394."experience/order" = { ids = [55, 56] }
universes.15302144394."pass/vip" = { id = 123, name = "VIP", price = 100, forSale = true, managedPricing = false, icon = "0123456789abcdef0123456789abcdef" }
universes.15302144394."thumbnail/main" = { id = 55, altText = "Lobby", image = "0123456789abcdef0123456789abcdef", stale = [41], next = "00000000000000000000000000000000" }
"#;

    fn temp(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("opossum-lock-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn parsed(text: &str) -> Lock {
        let (lock, diags) = parse(text, Path::new(FILE));
        assert!(diags.is_empty(), "{:?}", diags.iter().map(|d| (d.line, &d.message)).collect::<Vec<_>>());
        lock
    }

    #[test]
    fn rendering_is_the_one_form_whatever_order_the_lines_came_in() {
        assert_eq!(render(&parsed(CANONICAL)), CANONICAL);
        // as a hand edit or a merge leaves it: lines in any order, comments and blank lines anywhere
        let mut lines: Vec<&str> = CANONICAL.lines().skip(1).filter(|l| !l.is_empty()).collect();
        lines.reverse();
        let shuffled = lines.join("\n# hi\n\n");
        assert_eq!(render(&parsed(&shuffled)), CANONICAL);
    }

    #[test]
    fn a_merge_without_the_union_driver_keeps_both_sides_and_picks_one_record_per_file() {
        let a = |id: u64, hash: char| format!("{{ id = {id}, hash = \"{}\" }}", hash.to_string().repeat(32));
        let text = format!(
            "version = 1\n<<<<<<< ours\ncreator = {{ user = 1 }}\n=======\ncreator = {{ user = 2 }}\n>>>>>>> theirs\n\
             <<<<<<< ours\nassets.\"x.png\" = {}\nassets.\"same.png\" = {}\nassets.\"y.png\" = {}\n=======\n\
             assets.\"x.png\" = {}\nassets.\"same.png\" = {}\nassets.\"y.png\" = {{ next = \"{}\" }}\n>>>>>>> theirs\n\
             assets.\"x.png\" = {}\nuniverses.7.\"pass/vip\" = {{ id = 1 }}\nuniverses.7.\"pass/vip\" = {{ id = 2 }}\n\
             universes.7.\"pass/vip\" = {{ id = 1 }}\nuniverses.7.\"badge/b\" = {{ id = 3 }}\n\
             universes.7.\"badge/b\" = {{ id = 3 }}\n",
            a(5, 'a'),
            a(1, 'f'),
            a(8, 'b'),
            a(9, 'c'),
            a(1, 'f'),
            "d".repeat(32),
            a(7, 'e'),
        );
        let mut lock = parsed(&text);
        assert_eq!((lock.creator, &lock.other_creators[..]), (Some(Creator::User(1)), &[Creator::User(2)][..]));
        // a build takes the highest id with nothing in flight; identical duplicates are one record
        assert_eq!(lock.assets["x.png"].id, Some(9));
        assert_eq!(lock.assets["y.png"].id, Some(8));
        assert!(!lock.merged.contains_key("same.png"));
        assert_eq!(lock.merge_warnings(Path::new("/p")).len(), 2);
        // a write that hashed nothing keeps every candidate: the highest id need not be the one holding the file
        assert_eq!(render(&lock).matches("assets.\"x.png\"").count(), 3);
        // a command that hashed the file takes the id holding its content, and writes only that record
        let warning = lock.settle(Path::new("/p"), "x.png", &"a".repeat(32)).unwrap();
        assert!(warning.message.contains("keeping id 5"), "{}", warning.message);
        assert!(lock.settle(Path::new("/p"), "x.png", &"a".repeat(32)).is_none());
        assert_eq!(lock.assets["x.png"].id, Some(5));
        assert!(render(&lock).contains(&format!("assets.\"x.png\" = {}\n", a(5, 'a'))));
        assert!(lock.check_creator(Path::new("/p"), Creator::User(1)).is_some_and(|d| d.code == "lock.conflict"));
        assert_eq!(lock.conflicts.keys().collect::<Vec<_>>(), [&(7, "pass/vip".to_owned())]);
        assert_eq!(lock.universes[&7]["pass/vip"].id, Some(1));
        // a write that settles nothing keeps every side of a conflict: only the user decides which is live
        let text = render(&lock);
        for line in [
            "creator = { user = 1 }",
            "creator = { user = 2 }",
            "\"pass/vip\" = { id = 1 }",
            "\"pass/vip\" = { id = 2 }",
        ] {
            assert!(text.contains(line), "{line}: {text}");
        }
        let again = parsed(&text);
        assert_eq!((again.conflicts.len(), again.other_creators.len()), (1, 1));
    }

    #[test]
    fn a_merge_of_a_read_record_and_its_unread_self_is_one_record_and_of_two_packs_keeps_the_highest() {
        let hash = "a".repeat(32);
        let unread = format!("assets.\"m.glb\" = {{ id = 5, hash = \"{hash}\" }}");
        let read = format!(
            "assets.\"m.glb\" = {{ id = 5, hash = \"{hash}\", parts = {{ Box = {{ mesh = 6, size = [1.5, 2, 0.5] }} }} }}"
        );
        let (low, high) = ("packs.\"SurfaceAppearance|a|b|c|d\" = 3", "packs.\"SurfaceAppearance|a|b|c|d\" = 9");
        for lines in [[&unread, &read, low, high], [&read, &unread, high, low]] {
            let lock = parsed(&format!("version = 1\n{}\n", lines.join("\n")));
            assert!(lock.merged.is_empty() && lock.merge_warnings(Path::new("/p")).is_empty());
            assert_eq!(lock.assets["m.glb"].parts.as_ref().map(|p| p["Box"].size), Some([1.5, 2.0, 0.5]));
            assert_eq!(lock.packs["SurfaceAppearance|a|b|c|d"], 9);
            let text = render(&lock);
            assert!(text.contains("parts = { Box = { mesh = 6, size = [1.5, 2.0, 0.5] } }"), "{text}");
            assert_eq!((text.matches("m.glb").count(), text.matches("packs.").count()), (1, 1), "{text}");
        }
    }

    #[test]
    fn every_line_is_checked_on_its_own() {
        let hash = "0".repeat(32);
        let cases = [
            ("assets.\"a\" = { id = 1, hash = \"x\" }", "`hash` must be 32 lowercase hex characters"),
            ("assets.\"a\" = { id = 1 }", "`id` and `hash` come together"),
            ("assets.\"a\" = { id = 0, hash = \"H\" }", "`id` must be a positive integer"),
            ("assets.\"a\" = { next = \"H\", op = \"operations/a/b\" }", "`op` must be operations/<id>"),
            ("assets.\"a\" = { op = \"operations/a\" }", "a record needs `id`, `next` or `rejected`"),
            ("assets.\"a\" = { id = 1, hash = \"H\", url = \"x\" }", "unknown field `url`"),
            ("assets = { a = { next = \"H\" }, b = { next = \"H\" } }", "one record per line"),
            ("universes.1.\"pass/vip\" = { price = \"1\" }", "`price` must be a positive integer"),
            ("universes.1.\"pass/vip\" = { stale = [1, -2] }", "`stale` must be a positive integer"),
            ("universes.1.\"gamepass/vip\" = { id = 1 }", "is not pass/"),
            ("universes.x.\"pass/vip\" = { id = 1 }", "`x` is not a universe id"),
            ("[assets]", "the lock has no sections"),
            ("\"a\" = { id = 1, hash = \"H\" }", "is not version, creator"),
            ("assets.\"a\" = { id = 1,", ""),
            ("creator = { user = 1, group = 2 }", "`creator` must be"),
            ("assets.\"a\" = { next = \"H\", parts = {} }", "`parts` needs `id`"),
            ("assets.\"a\" = { id = 1, hash = \"H\", parts = { hull = { mesh = 1 } } }", "needs `mesh` and `size`"),
            ("packs.\"SurfaceAppearance|rbxassetid://1\" = 2", "is not <class>|<color>"),
        ];
        for (body, message) in cases {
            let text = format!("version = 1\n{}\n", body.replace('H', &hash));
            let (lock, diags) = parse(&text, Path::new(FILE));
            assert_eq!(diags.len(), 1, "{body}: {:?}", diags.iter().map(|d| &d.message).collect::<Vec<_>>());
            assert_eq!((diags[0].code.as_str(), diags[0].line), ("lock.syntax", text.lines().count() as u32), "{body}");
            assert!(diags[0].message.contains(message), "{body}: {}", diags[0].message);
            assert!(lock.assets.is_empty() && lock.universes.is_empty());
        }
        let (lock, diags) = parse("version = 2\nassets.\"a\" = { next = \"0000\" }\n", Path::new(FILE));
        assert_eq!((diags.len(), diags[0].code.as_str()), (1, "lock.version"));
        assert!(diags[0].message.contains("version 2; this opossum reads 1") && lock.is_empty());
    }

    #[test]
    fn a_save_never_overwrites_a_lock_something_else_changed() {
        let root = temp("save");
        let cache = Cache { dir: root.join("cache") };
        let guard = RemoteGuard::acquire(&cache).unwrap();
        let (mut lock, _) = read(&root).unwrap();
        assert_eq!(save(&mut lock, &root, &guard).unwrap(), None, "an empty lock is not written");
        fs::write(root.join(".gitattributes"), "*.png binary").unwrap();
        lock.assets.insert("a.png".into(), Asset { next: Some("1".repeat(32)), ..Asset::default() });
        let note = save(&mut lock, &root, &guard).unwrap();
        assert_eq!(note.as_deref(), Some("wrote .gitattributes (opossum.lock merge=union)"));
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            "*.png binary\nopossum.lock merge=union\n"
        );
        lock.assets.get_mut("a.png").unwrap().op = Some("operations/1".into());
        assert_eq!(save(&mut lock, &root, &guard).unwrap(), None);
        assert_eq!(fs::read_to_string(root.join(FILE)).unwrap(), render(&lock));

        // git checks out another lock mid-run
        fs::write(root.join(FILE), "version = 1\n").unwrap();
        lock.assets.get_mut("a.png").unwrap().id = Some(5);
        lock.assets.get_mut("a.png").unwrap().hash = Some("1".repeat(32));
        let err = save(&mut lock, &root, &guard).unwrap_err();
        assert_eq!(err.code, "lock.changedOnDisk");
        assert_eq!(fs::read_to_string(root.join(FILE)).unwrap(), "version = 1\n");
        let recovered = fs::read_to_string(root.join(".opossum/opossum.lock.recovered")).unwrap();
        assert_eq!(recovered, render(&lock));
        assert!(super::recovered(&root).is_some_and(|d| d.code == "lock.recovered"));

        // the fold the refusal advises reads back clean, with the run's ids
        let mut text = fs::read_to_string(root.join(FILE)).unwrap();
        text.push_str(&recovered);
        fs::write(root.join(FILE), text).unwrap();
        fs::remove_file(root.join(".opossum/opossum.lock.recovered")).unwrap();
        assert!(super::recovered(&root).is_none());
        let (mut lock, diags) = read(&root).unwrap();
        assert!(diags.is_empty() && lock.assets["a.png"].id == Some(5));

        // a fresh read writes again, and the attribute is not added twice
        lock.creator = Some(Creator::Group(3));
        assert_eq!(save(&mut lock, &root, &guard).unwrap(), None);
        assert_eq!(read(&root).unwrap().0.creator, Some(Creator::Group(3)));
        let _ = fs::remove_dir_all(&root);
    }

    // dev's pack capture saves beside the user's own commands: when git rewrote the lock between its read and its
    // save, it leaves that lock alone and no recovered state that would refuse every command that writes the lock
    #[test]
    fn a_capture_save_racing_a_rewritten_lock_leaves_no_recovered_file() {
        let root = temp("drop");
        let cache = Cache { dir: root.join("cache") };
        let guard = RemoteGuard::acquire(&cache).unwrap();
        fs::write(root.join(FILE), "version = 1\n").unwrap();
        let (mut lock, _) = read(&root).unwrap();
        let checkout = "version = 1\npacks.\"MaterialVariant|rbxassetid://5|||\" = 7\n";
        fs::write(root.join(FILE), checkout).unwrap();
        lock.packs.insert("SurfaceAppearance|rbxassetid://1|||".into(), 2);
        assert!(!save_or_drop(&mut lock, &root, &guard).unwrap());
        assert_eq!(fs::read_to_string(root.join(FILE)).unwrap(), checkout);
        assert!(recovered(&root).is_none() && !root.join(".opossum").exists());

        // the next tick reads the new lock and saves into it
        let (mut lock, _) = read(&root).unwrap();
        lock.packs.insert("SurfaceAppearance|rbxassetid://1|||".into(), 2);
        assert!(save_or_drop(&mut lock, &root, &guard).unwrap());
        assert_eq!(read(&root).unwrap().0.packs.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }
}
