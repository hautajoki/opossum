//! `opossum.lock`: the ids opossum created on Roblox, committed beside `opossum.toml`. TOML with one record per line,
//! sorted, each naming its own table (`assets."<file>" = …`, `universes.<id>."<kind>/<key>" = …`) with no section
//! headers, so every record stands on its own wherever a merge puts it: the first write adds `opossum.lock
//! merge=union` to `.gitattributes`, and [`parse`] reads what any merge leaves (both sides of a conflict, a key twice,
//! lines in any order) without a hand edit. Builds only read
//! it; the commands that change it write it through [`save`], which takes the [`RemoteGuard`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::SystemTime;

use cloud::Creator;

use crate::diag::{Diag, Severity};
use crate::guard::RemoteGuard;

pub const FILE: &str = "opossum.lock";
/// Where [`save`] puts the state it could not write, under `.opossum/`.
const RECOVERED: &str = "opossum.lock.recovered";
const HEADER: &str =
    "# opossum.lock: ids opossum created on Roblox. Commit it. opossum keeps it; a merge needs no hand edits.";
const VERSION: i64 = 1;

/// One source file's record. `hash` is the input hash of what `id` holds; `next` is the input hash of a create in
/// flight and `op` its operation, so `next` without `op` is a create that may or may not have happened.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Asset {
    pub id: Option<u64>,
    pub hash: Option<String>,
    pub next: Option<String>,
    pub op: Option<String>,
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
    /// Who the `[assets]` ids belong to, set by the first upload.
    pub creator: Option<Creator>,
    /// By source file path relative to the project root, with its extension, so renaming a set orphans nothing.
    pub assets: BTreeMap<String, Asset>,
    /// Every different record a merge left for one file, each written back until a command that hashed the file
    /// [`Lock::settle`]s it; `assets` holds the build's pick meanwhile.
    pub merged: BTreeMap<String, Vec<Asset>>,
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
/// (assets) or marked in `conflicts` (resources).
pub fn parse(text: &str, file: &Path) -> (Lock, Vec<Diag>) {
    let mut lock = Lock::default();
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
            "creator" => creator(&value).map(|c| match lock.creator {
                Some(old) if old != c && !lock.other_creators.contains(&c) => lock.other_creators.push(c),
                Some(_) => {}
                _ => lock.creator = Some(c),
            }),
            "assets" => one(&value, "assets.\"<file>\"").and_then(|(key, value)| {
                let a = asset(value)?;
                match lock.assets.get(key) {
                    None => {
                        lock.assets.insert(key.clone(), a);
                    }
                    Some(old) if *old == a => {}
                    Some(old) => {
                        let all = lock.merged.entry(key.clone()).or_insert_with(|| vec![old.clone()]);
                        if !all.contains(&a) {
                            all.push(a);
                        }
                    }
                }
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
            _ => Err(format!("`{key}` is not version, creator, assets.\"<file>\" or universes.<id>.\"<kind>/<key>\"")),
        };
        if let Err(e) = read {
            fail("lock.syntax", e);
        }
    }
    if content && !version {
        let message = "opossum.lock has no `version = 1` line".to_owned();
        diags.push(Diag::new("lock.version", Severity::Error, Some(file), message));
    }
    for (key, all) in &lock.merged {
        lock.assets.insert(key.clone(), pick(all, None).clone());
    }
    (lock, diags)
}

/// Which of a merge's records for one file holds (§2.4): with the file's current hash, one whose id holds that
/// content; then the highest id with no create in flight, since Roblox ids grow; then the first create in flight.
fn pick<'a>(all: &'a [Asset], hash: Option<&str>) -> &'a Asset {
    let holds = |a: &&Asset| a.id.is_some() && hash.is_some() && a.hash.as_deref() == hash;
    all.iter()
        .find(holds)
        .or_else(|| all.iter().filter(|a| a.id.is_some() && a.next.is_none()).max_by_key(|a| a.id))
        .or_else(|| all.iter().find(|a| a.next.is_some()))
        .unwrap_or(&all[0])
}

/// `lock.merged` for a file with several records after a merge: what builds use meanwhile, or (`settled`) the one a
/// command that hashed the file keeps.
fn merged_warning(root: &Path, file: &str, a: &Asset, settled: bool) -> Diag {
    let used = match a.id {
        Some(id) => format!("id {id}"),
        None => "the upload in flight".to_owned(),
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
            let message = "two different `creator` lines after a merge; keep the one the [assets] ids belong to";
            return Some(Diag::new("lock.conflict", Severity::Error, Some(&file), message.into()));
        }
        let ours = self.creator.filter(|&c| c != creator)?;
        let message = format!(
            "opossum.lock's assets were uploaded as {} but opossum.toml says {}. Their ids keep working. To upload \
             everything again as the new creator, delete `creator` and [assets] from opossum.lock",
            creator_name(ours),
            creator_name(creator)
        );
        Some(Diag::new("lock.creator", Severity::Error, Some(&file), message))
    }

    fn is_empty(&self) -> bool {
        self.creator.is_none() && self.assets.is_empty() && self.universes.values().all(BTreeMap::is_empty)
    }
}

fn creator_name(c: Creator) -> String {
    match c {
        Creator::User(id) => format!("user:{id}"),
        Creator::Group(id) => format!("group:{id}"),
    }
}

fn creator(v: &toml::Value) -> Result<Creator, String> {
    let bad = || "`creator` must be { user = <id> } or { group = <id> }".to_owned();
    let table = v.as_table().filter(|t| t.len() == 1).ok_or_else(bad)?;
    let (kind, id) = table.iter().next().ok_or_else(bad)?;
    let id = positive(id, kind).map_err(|_| bad())?;
    match kind.as_str() {
        "user" => Ok(Creator::User(id)),
        "group" => Ok(Creator::Group(id)),
        _ => Err(bad()),
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
    if a.id.is_none() && a.next.is_none() {
        return Err("a record needs `id` or `next`".into());
    }
    if a.op.is_some() && a.next.is_none() {
        return Err("`op` needs `next`".into());
    }
    Ok(a)
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

/// The lock's only form: the header, `version` and `creator`, then the asset records and each universe's (by number),
/// a blank line before each group, records sorted bytewise by key, each record's fields in a fixed order.
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
    for (name, value) in [("hash", &a.hash), ("next", &a.next), ("op", &a.op)] {
        if let Some(s) = value {
            fields.push((name, string(s)));
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
fn inline(fields: &[(&str, String)]) -> String {
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
    let text = render(lock);
    let unchanged = match &lock.disk {
        Some(disk) => disk.text == text,
        None => lock.is_empty(),
    };
    if unchanged {
        return Ok(None);
    }
    let fail = |code: &str, message: String| Diag::new(code, Severity::Error, Some(&path), message);
    let io = |e: std::io::Error| fail("lock.io", e.to_string());
    let now = fs::metadata(&path).ok().map(|m| (m.len(), m.modified().ok()));
    if now != lock.disk.as_ref().map(|d| (d.len, d.mtime)) {
        let dir = crate::editor::dir(root);
        fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join(RECOVERED), &text)).map_err(io)?;
        let message = format!(
            "opossum.lock changed while opossum was writing it (git?), so nothing was overwritten: this run's ids are \
             in .opossum/{RECOVERED}, and no command writes the lock until they are folded back (lock.recovered)"
        );
        return Err(fail("lock.changedOnDisk", message));
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
        return Ok(None);
    }
    let attributes = root.join(".gitattributes");
    let old = match fs::read_to_string(&attributes) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail("lock.io", format!("{}: {e}", attributes.display()))),
    };
    if old.lines().any(|l| l.contains(FILE)) {
        return Ok(None);
    }
    let line = format!("{}{FILE} merge=union\n", if old.is_empty() || old.ends_with('\n') { "" } else { "\n" });
    let append = fs::OpenOptions::new().append(true).create(true).open(&attributes);
    append
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| fail("lock.io", format!("{}: {e}", attributes.display())))?;
    Ok(Some(format!("wrote .gitattributes ({FILE} merge=union)")))
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
assets."place/assets/sfx/new.ogg" = { next = "9b1f0c2d4e5a6b7c8d9e0f1a2b3c4d5e", op = "operations/9f2c41" }
assets."place/assets/sfx/odd.ogg" = { id = 140112233445566, hash = "00000000000000000000000000000001", next = "00000000000000000000000000000002" }

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
    fn every_line_is_checked_on_its_own() {
        let hash = "0".repeat(32);
        let cases = [
            ("assets.\"a\" = { id = 1, hash = \"x\" }", "`hash` must be 32 lowercase hex characters"),
            ("assets.\"a\" = { id = 1 }", "`id` and `hash` come together"),
            ("assets.\"a\" = { id = 0, hash = \"H\" }", "`id` must be a positive integer"),
            ("assets.\"a\" = { next = \"H\", op = \"operations/a/b\" }", "`op` must be operations/<id>"),
            ("assets.\"a\" = { op = \"operations/a\" }", "a record needs `id` or `next`"),
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
}
