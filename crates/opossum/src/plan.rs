//! `opossum plan`: what going outward would do, worked out offline with no credentials. Every asset file is hashed and
//! classified against `opossum.lock` exactly as `opossum upload` classifies it ([`classify`]), so the plan is what an
//! upload does and also the dump of every id; with an environment the resources it declares are compared with the
//! records of its universe. It also reads the place, resolved as a build resolves it, for the texture packs Studio has
//! still to bake.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use serde_json::json;

use crate::assets::Compiled;
use crate::config::Config;
use crate::diag::{Diag, Severity};
use crate::lock::{self, Asset, Lock};
use crate::{Outcome, assets, ids, resources};

/// What `opossum upload` does with one found file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Class {
    /// Its id holds what the file holds.
    Current,
    /// Sent: `changed` when its id holds an older version, else new.
    Send { changed: bool },
    /// Binds the id the lock holds for the same content under another file, with no request.
    Reuses(u64),
    /// Goes up with `found[i]`'s upload (or resumed operation) of the same content.
    Joins(usize),
    /// An upload is in flight and its operation can be asked how it went, or a glb's upload is not read back yet.
    Pending,
    /// An upload may or may not have happened, and nothing says which: what settles it.
    Ambiguous(String),
    /// Roblox refused this content when it was sent: it is not sent again until the file changes, or `opossum adopt
    /// asset --none` forgets the refusal.
    Rejected,
    /// The file it would send fails its checks ([`check`]): the code and why.
    Invalid(String, String),
}

/// Each [`Class`]'s name in `plan`, in [`Class::index`] order.
const NAMES: [&str; 9] =
    ["current", "changed", "new", "reused", "joined", "pending", "ambiguous", "rejected", "invalid"];

impl Class {
    pub fn name(&self) -> &'static str {
        NAMES[self.index()]
    }

    fn index(&self) -> usize {
        match self {
            Class::Current => 0,
            Class::Send { changed: true } => 1,
            Class::Send { changed: false } => 2,
            Class::Reuses(_) => 3,
            Class::Joins(_) => 4,
            Class::Pending => 5,
            Class::Ambiguous(_) => 6,
            Class::Rejected => 7,
            Class::Invalid(..) => 8,
        }
    }
}

/// Every found file's [`Class`] against the lock, settling what a merge left for it (each pick printed): a file whose
/// content the lock holds an id for binds it, files sending the same content go up once, and a file holding the
/// content of an upload that may or may not have happened is ambiguous with it.
pub fn classify(root: &Path, lock: &mut Lock, found: &[assets::Asset], hashes: &[String]) -> Vec<Class> {
    // an upload in flight of a file that is gone (renamed, moved) follows its content to a file holding it now, so it
    // is resumed or settled there rather than dropped with the old name and sent again
    let present: HashSet<&str> = found.iter().map(|a| a.file.as_str()).collect();
    let gone: Vec<String> = lock
        .assets
        .iter()
        .filter(|(file, a)| a.next.is_some() && !present.contains(file.as_str()) && !lock.merged.contains_key(*file))
        .map(|(file, _)| file.clone())
        .collect();
    for file in gone {
        let next = lock.assets[&file].next.clone();
        let free = |a: &assets::Asset| lock.assets.get(&a.file).is_none_or(|r| r.next.is_none());
        let Some(i) = (0..found.len()).find(|&i| Some(&hashes[i]) == next.as_ref() && free(&found[i])) else {
            continue;
        };
        let moved = lock.assets.remove(&file).expect("a gone record");
        let target = lock.assets.entry(found[i].file.clone()).or_default();
        (target.next, target.op) = (moved.next, moved.op);
    }
    // content the lock holds an id for, a merge's every candidate included
    let held: HashMap<String, u64> = lock
        .assets
        .values()
        .chain(lock.merged.values().flatten())
        .filter_map(|a| match a {
            Asset { id: Some(id), hash: Some(hash), next: None, .. } => Some((hash.clone(), *id)),
            _ => None,
        })
        .collect();
    let unsettled = unsettled(lock);
    // content Roblox refused, under whatever file sent it: a rename or a copy never sends it again
    let refused: HashSet<String> =
        lock.assets.values().chain(lock.merged.values().flatten()).filter_map(|a| a.rejected.clone()).collect();
    let mut classes = Vec::with_capacity(found.len());
    for (a, hash) in found.iter().zip(hashes) {
        if let Some(warning) = lock.settle(root, &a.file, hash) {
            eprintln!("{}", warning.human(root));
        }
        // a create in flight decides over the id it would replace
        let class = match lock.assets.get(&a.file) {
            Some(Asset { next: Some(_), op: None, .. }) => Class::Ambiguous(settle(&a.file, &a.key)),
            Some(Asset { next: Some(_), .. }) => Class::Pending,
            Some(Asset { rejected: Some(r), .. }) if r == hash => Class::Rejected,
            Some(Asset { id: Some(_), hash: Some(h), parts: None, .. }) if h == hash && place::is_glb(&a.file) => {
                Class::Pending
            }
            Some(Asset { id: Some(_), hash: Some(h), .. }) if h == hash => Class::Current,
            record => {
                let changed = record.is_some_and(|r| r.id.is_some());
                match (unsettled.get(hash), held.get(hash)) {
                    _ if refused.contains(hash) => Class::Rejected,
                    (Some(other), _) => Class::Ambiguous(copy_of(other)),
                    (None, Some(&id)) => Class::Reuses(id),
                    (None, None) => Class::Send { changed },
                }
            }
        };
        classes.push(class);
    }
    // an upload in flight leads its content (it is resumed), else the first file sending it; an unread glb is read by
    // its id, so it leads nothing
    let mut leaders: HashMap<&str, usize> = HashMap::new();
    for pending in [true, false] {
        for (i, a) in found.iter().enumerate() {
            let content = match &classes[i] {
                Class::Pending if pending => match lock.assets[&a.file].next.as_deref() {
                    Some(next) => next,
                    None => continue,
                },
                Class::Send { .. } if !pending => hashes[i].as_str(),
                _ => continue,
            };
            match leaders.get(content) {
                Some(&leader) => classes[i] = Class::Joins(leader),
                None => drop(leaders.insert(content, i)),
            }
        }
    }
    classes
}

/// `opossum plan [--environment e]`. Exit 1 when an upload is ambiguous or a resource is blocked (the user must act),
/// 2 when the config, the lock, an asset root or a resource image has errors.
pub fn plan(cfg: &Config, environment: Option<&str>) -> Outcome {
    let root = &cfg.root;
    let environment = match cfg.build_environment(environment) {
        Ok(environment) => environment,
        Err(error) => return Outcome::refused("config", error),
    };
    let (mut lock, found, hashes) = match survey(cfg, "planned") {
        Ok(survey) => survey,
        Err(refusal) => return refusal,
    };
    let mut classes = classify(root, &mut lock, &found, &hashes);
    check(cfg, &found, &hashes, &mut classes);
    let mut text = String::new();
    let mut sets = Vec::new();
    for (i, set) in cfg.assets.iter().enumerate() {
        let (mut files, mut lines, mut counts) = (Vec::new(), String::new(), [0; NAMES.len()]);
        for (a, class) in found.iter().zip(&classes).filter(|(a, _)| a.set == i) {
            counts[class.index()] += 1;
            let name = class.name();
            let mut file = json!({ "file": a.file, "key": a.key, "state": name });
            let id = match class {
                Class::Reuses(id) => Some(*id),
                _ => lock.assets.get(&a.file).and_then(|r| r.id),
            };
            if let Some(id) = id {
                file["id"] = id.into();
            }
            let _ = match class {
                Class::Current => Ok(()),
                Class::Reuses(id) => writeln!(lines, "{name:<9} {} (id {id}, which the lock holds for it)", a.file),
                Class::Joins(j) => {
                    file["with"] = found[*j].file.as_str().into();
                    writeln!(lines, "{name:<9} {} (one upload with {})", a.file, found[*j].file)
                }
                Class::Ambiguous(how) => writeln!(lines, "{name:<9} {} ({how})", a.file),
                Class::Rejected => writeln!(lines, "{name:<9} {} ({})", a.file, refused(&a.file)),
                Class::Pending if lock.assets[&a.file].next.is_none() => {
                    writeln!(lines, "{name:<9} {} ({})", a.file, crate::ids::UNREAD)
                }
                Class::Invalid(code, message) => {
                    (file["code"], file["error"]) = (code.as_str().into(), message.as_str().into());
                    writeln!(lines, "{name:<9} {} ({message})", a.file)
                }
                _ => writeln!(lines, "{name:<9} {}", a.file),
            };
            files.push(file);
        }
        // current, then every state a file is in
        let mut line = format!("{}: {}", set.name, counted(files.len(), "file"));
        for (n, name) in NAMES.iter().enumerate() {
            if n == 0 || counts[n] > 0 {
                let _ = write!(line, ", {} {name}", count(counts[n]));
            }
        }
        let _ = writeln!(text, "{line}");
        text.push_str(&lines);
        sets.push(json!({ "set": set.name, "module": format!("opossum/{}", set.name), "files": files }));
    }
    // discovery was clean (survey refuses otherwise), so a record with no file is a file that is gone
    let present: HashSet<&str> = found.iter().map(|a| a.file.as_str()).collect();
    let dropped: Vec<&str> = lock.assets.keys().map(String::as_str).filter(|f| !present.contains(f)).collect();
    for file in &dropped {
        let _ = writeln!(text, "dropped   {file} (no longer found)");
    }
    let (line, packs) = self::packs(cfg, &lock, &found);
    text.push_str(&line);
    let ambiguous = classes.iter().filter(|c| matches!(c, Class::Ambiguous(_))).count();
    let rejected = classes.iter().filter(|c| **c == Class::Rejected).count();
    let invalid = classes.iter().find_map(|c| match c {
        Class::Invalid(code, _) => Some(code.as_str()),
        _ => None,
    });
    if cfg.assets.is_empty() && environment.is_none() {
        text.push_str("nothing to plan: opossum.toml declares no [assets] and no [environments]\n");
    }
    let mut json = json!({ "ok": true, "assets": sets, "dropped": dropped, "packs": packs });
    let mut blocked = 0;
    if let Some(env) = environment {
        let (_, ops) = match resources::survey(cfg, env, &lock) {
            Ok(planned) => planned,
            Err(refusal) => return refusal,
        };
        if !ops.is_empty() {
            let _ = writeln!(text, "resources of {} (universe {}):", env.name, env.universe);
            text.push_str(&resources::table(&ops));
        }
        blocked = resources::blocked(&ops);
        json["environment"] = env.name.as_str().into();
        json["resources"] = resources::ops_json(&ops);
    }
    let error = if ambiguous > 0 {
        ("asset.ambiguous", self::ambiguous(ambiguous))
    } else if rejected > 0 {
        let error = format!(
            "Roblox refused the content of {}; change the content or forget the refusal",
            counted(rejected, "file")
        );
        ("asset.rejected", error)
    } else if let Some(code) = invalid {
        let n = classes.iter().filter(|c| matches!(c, Class::Invalid(..))).count();
        (
            code,
            format!(
                "{} failed the checks before upload: upload skips it and deploy sends nothing",
                counted(n, "asset file")
            ),
        )
    } else if blocked > 0 {
        let error = format!(
            "creates of {} may or may not have happened; `opossum adopt` settles each",
            counted(blocked, "resource")
        );
        ("resource.blocked", error)
    } else {
        return Outcome { exit: 0, json, text };
    };
    Outcome::failed(json, text, error.0, error.1)
}

/// The texture packs the place's SurfaceAppearances and MaterialVariants need, by key: baked (the lock or the model
/// file holds a pack), unbaked (each with the files using it: `opossum dev` bakes them in Studio, and `deploy` refuses
/// until it did), and the surfaces waiting (a map's `$asset` has no upload or read yet, so the key is not known). The
/// text (empty for a place with none) and `{baked, unbaked: [{class, maps, usedBy}], waiting}`.
fn packs(cfg: &Config, lock: &Lock, found: &[assets::Asset]) -> (String, serde_json::Value) {
    let (mut place, _) = place::read_place(&cfg.root.join("place"), &cfg.asset_roots());
    place.resolve_assets(&ids::resolver(lock, found));
    let (mut baked, mut waiting) = (BTreeSet::new(), 0);
    let mut unbaked: BTreeMap<String, (&str, BTreeSet<String>)> = BTreeMap::new();
    let surfaces = place.surfaces();
    for s in &surfaces {
        let Some(key) = &s.key else {
            waiting += 1;
            continue;
        };
        if s.packed || lock.packs.contains_key(key) {
            baked.insert(key);
        } else {
            let file = s.file.strip_prefix(&cfg.root).unwrap_or(&s.file).display().to_string();
            unbaked.entry(key.clone()).or_insert((s.class, BTreeSet::new())).1.insert(file);
        }
    }
    let list: Vec<serde_json::Value> = unbaked
        .iter()
        .map(|(key, (class, files))| {
            let maps: Vec<&str> = key.split('|').skip(1).collect();
            json!({ "class": class, "maps": maps, "usedBy": files })
        })
        .collect();
    let json = json!({ "baked": baked.len(), "unbaked": list, "waiting": waiting });
    if surfaces.is_empty() {
        return (String::new(), json);
    }
    let mut text = format!("packs: {} baked", count(baked.len()));
    if !unbaked.is_empty() {
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        for (class, _) in unbaked.values() {
            *classes.entry(class).or_default() += 1;
        }
        let classes: Vec<String> = classes.iter().map(|(class, n)| format!("{class} {n}")).collect();
        let _ = write!(text, ", {} unbaked ({})", count(unbaked.len()), classes.join(", "));
    }
    if waiting > 0 {
        let _ = write!(text, ", {} waiting on uploads", counted(waiting, "surface"));
    }
    text.push('\n');
    for (key, (_, files)) in &unbaked {
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let _ = writeln!(text, "unbaked   {key} (used by {})", files.join(", "));
    }
    (text, json)
}

/// Compiles, in parallel, the one file each upload would send (one sent, not joining another's upload), before
/// anything is sent: what fails its checks (size, image, audio, a model that decodes) is [`Class::Invalid`], with the
/// files joining it. The compiled bytes by file index, for `upload` to send.
pub fn check(
    cfg: &Config,
    found: &[assets::Asset],
    hashes: &[String],
    classes: &mut [Class],
) -> HashMap<usize, Compiled> {
    let sends: Vec<usize> = (0..found.len()).filter(|&i| matches!(classes[i], Class::Send { .. })).collect();
    let results = lower::parallel::map(&sends, |&i| compile(cfg, &found[i], &hashes[i]));
    let mut compiled = HashMap::new();
    for (i, result) in sends.into_iter().zip(results) {
        match result {
            Ok(c) => drop(compiled.insert(i, c)),
            Err(d) => classes[i] = Class::Invalid(d.code, d.message),
        }
    }
    for i in 0..classes.len() {
        if let Class::Joins(leader) = classes[i]
            && let Class::Invalid(code, message) = &classes[leader]
        {
            classes[i] = Class::Invalid(code.clone(), message.clone());
        }
    }
    compiled
}

/// The bytes a file's upload sends, read again and checked against the hash it was classified by.
fn compile(cfg: &Config, a: &assets::Asset, hash: &str) -> Result<Compiled, Diag> {
    let path = cfg.root.join(&a.file);
    let image = cfg.assets[a.set].image;
    let fail = |message: String| Diag::new("asset.root", Severity::Error, Some(&path), message);
    let bytes = std::fs::read(&path).map_err(|e| fail(e.to_string()))?;
    if assets::hash(&a.ext, image, &bytes) != hash {
        return Err(fail("the file changed while opossum read it; run again".into()));
    }
    assets::compile(&path, &a.ext, image, bytes)
}

/// Content an upload that may or may not have happened sent, and the file it was for: a new or changed file holding the
/// same content is ambiguous too, since sending it could upload that content twice.
pub fn unsettled(lock: &Lock) -> HashMap<String, String> {
    let merged = lock.merged.iter().flat_map(|(file, all)| all.iter().map(move |a| (file, a)));
    lock.assets
        .iter()
        .chain(merged)
        .filter_map(|(file, a)| match a {
            Asset { next: Some(next), op: None, .. } => Some((next.clone(), file.clone())),
            _ => None,
        })
        .collect()
}

/// What settles a file whose upload may or may not have happened, `key` the set key it went up named after.
pub fn settle(file: &str, key: &str) -> String {
    format!(
        "an upload may have happened: if Roblox has an asset named \"{}\" from then, `opossum adopt asset {file} <its \
         id>`, else `--none`",
        cloud::display_name(key)
    )
}

/// Why a command stops at `n` such files.
pub fn ambiguous(n: usize) -> String {
    format!("uploads of {} may or may not have happened; `opossum adopt asset` settles each", counted(n, "file"))
}

/// What settles a file holding the content of `other`'s upload that may or may not have happened.
pub fn copy_of(other: &str) -> String {
    format!(
        "{other} holds the same content and its upload may have happened; `opossum adopt asset {other}` settles both"
    )
}

/// What to do about a file whose content Roblox refused.
pub fn refused(file: &str) -> String {
    format!(
        "Roblox refused this content when it was sent; change the file, or `opossum adopt asset {file} --none` to send \
         it again"
    )
}

/// The lock, every asset file and each file's input hash, for a command about to compare them. Errors in the lock or
/// the asset roots are printed on stderr and refuse the command: nothing was `done` (planned, uploaded).
pub fn survey(cfg: &Config, done: &str) -> Result<(Lock, Vec<assets::Asset>, Vec<String>), Outcome> {
    let root = &cfg.root;
    let (lock, mut diags) = lock::read(root).map_err(|error| Outcome::refused("lock.io", error))?;
    let (found, discovered) = assets::discover(root, &cfg.assets);
    diags.extend(discovered);
    let mut hashes = Vec::with_capacity(found.len());
    for (a, hash) in found.iter().zip(assets::hash_all(root, &cfg.assets, &found)) {
        match hash {
            Ok(hash) => hashes.push(hash),
            Err(e) => diags.push(Diag::new("asset.root", Severity::Error, Some(&root.join(&a.file)), e)),
        }
    }
    let errors: Vec<&Diag> = diags.iter().filter(|d| d.is_error()).collect();
    let Some(first) = errors.first() else { return Ok((lock, found, hashes)) };
    for d in &errors {
        eprintln!("{}", d.human(root));
    }
    let error = format!("{} in opossum.lock or the asset roots, so nothing was {done}", counted(errors.len(), "error"));
    Err(Outcome::refused(&first.code, error))
}

/// `1 file`, `1,300 files`.
pub fn counted(n: usize, word: &str) -> String {
    format!("{} {word}{}", count(n), if n == 1 { "" } else { "s" })
}

/// `1,300`.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A create in flight decides a file's state over the id it would replace, and one with no operation to ask fails
    /// the plan: the user has to say whether it happened. Content the lock or another file's upload has is not new.
    #[test]
    fn every_file_gets_its_state_and_an_ambiguous_upload_fails_the_plan() {
        let root = std::env::temp_dir().join(format!("opossum-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("art")).unwrap();
        std::fs::write(root.join("opossum.toml"), "name = \"plan\"\ncreator = { user = 1 }\n[routes]\nshared = \"ReplicatedStorage/shared\"\n[assets]\nart = \"art\"\n")
            .unwrap();
        for file in ["current.mp4", "changed.mp4", "new.mp4", "pending.ogg", "ambiguous.ogg"] {
            std::fs::write(root.join("art").join(file), file).unwrap();
        }
        // what upload binds, sends once or refuses before sending, which a plan says rather than calling them new
        std::fs::write(root.join("art/reused.mp4"), "current.mp4").unwrap();
        std::fs::write(root.join("art/twin.mp4"), "new.mp4").unwrap();
        std::fs::write(root.join("art/broken.png"), "not a png").unwrap();
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let hash = |file: &str, ext: &str| assets::hash(ext, cfg.assets[0].image, file.as_bytes());
        let (old, next) = ("0".repeat(32), "1".repeat(32));
        let lock = format!(
            "version = 1\n\
             assets.\"art/current.mp4\" = {{ id = 1, hash = \"{}\" }}\n\
             assets.\"art/changed.mp4\" = {{ id = 2, hash = \"{old}\" }}\n\
             assets.\"art/pending.ogg\" = {{ id = 3, hash = \"{}\", next = \"{next}\", op = \"operations/x\" }}\n\
             assets.\"art/ambiguous.ogg\" = {{ next = \"{next}\" }}\n",
            hash("current.mp4", ".mp4"),
            hash("pending.ogg", ".ogg"),
        );
        std::fs::write(root.join("opossum.lock"), lock).unwrap();
        let outcome = plan(&cfg, None);
        let files = outcome.json["assets"][0]["files"].as_array().unwrap();
        let states: Vec<(&str, &str)> =
            files.iter().map(|f| (f["key"].as_str().unwrap(), f["state"].as_str().unwrap())).collect();
        assert_eq!(
            states,
            [
                ("ambiguous", "ambiguous"),
                ("broken", "invalid"),
                ("changed", "changed"),
                ("current", "current"),
                ("new", "new"),
                ("pending", "pending"),
                ("reused", "reused"),
                ("twin", "joined")
            ]
        );
        assert_eq!((outcome.exit, &outcome.json["code"]), (1, &"asset.ambiguous".into()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Packs are counted by key, as the lock keeps them: surfaces sharing maps are one pack to bake. A surface whose
    /// map waits on an upload has no key yet, so it is counted as a waiting surface, not as a pack.
    #[test]
    fn plan_counts_texture_packs_by_key() {
        let root = std::env::temp_dir().join(format!("opossum-plan-packs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("art")).unwrap();
        std::fs::create_dir_all(root.join("place/replicated-storage")).unwrap();
        let toml = "name = \"plan\"\ncreator = { user = 1 }\n[routes]\nshared = \"ReplicatedStorage/shared\"\n\
                    [assets]\nart = \"art\"\n";
        std::fs::write(root.join("opossum.toml"), toml).unwrap();
        std::fs::write(root.join("art/color.mp4"), "x").unwrap();
        let surface = |name: &str, class: &str, color: &str| {
            let properties = format!("{{\"ColorMap\": {color}, \"NormalMap\": \"rbxassetid://9\"}}");
            format!("{{\"name\": \"{name}\", \"className\": \"{class}\", \"properties\": {properties}}}")
        };
        let children = [
            surface("Baked", "SurfaceAppearance", "\"rbxassetid://1\""),
            surface("Unbaked", "SurfaceAppearance", "\"rbxassetid://2\""),
            surface("Twin", "SurfaceAppearance", "\"rbxassetid://2\""),
            surface("Variant", "MaterialVariant", "\"rbxassetid://3\""),
            surface("Waiting", "SurfaceAppearance", "{\"$asset\": \"art/color.mp4\"}"),
            surface("Waits", "MaterialVariant", "{\"$asset\": \"art/color.mp4\"}"),
        ];
        let model = format!("{{\"className\": \"Model\", \"children\": [{}]}}", children.join(", "));
        std::fs::write(root.join("place/replicated-storage/m.model.json"), model).unwrap();
        std::fs::write(
            root.join("opossum.lock"),
            "version = 1\npacks.\"SurfaceAppearance|rbxassetid://1|rbxassetid://9||\" = 5\n",
        )
        .unwrap();
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let outcome = plan(&cfg, None);
        let used = ["place/replicated-storage/m.model.json"];
        let expected = json!({
            "baked": 1,
            "unbaked": [
                { "class": "MaterialVariant", "maps": ["rbxassetid://3", "rbxassetid://9", "", ""], "usedBy": used },
                { "class": "SurfaceAppearance", "maps": ["rbxassetid://2", "rbxassetid://9", "", ""], "usedBy": used },
            ],
            "waiting": 2,
        });
        assert_eq!(outcome.json["packs"], expected);
        assert!(
            outcome.text.contains(
                "packs: 1 baked, 2 unbaked (MaterialVariant 1, SurfaceAppearance 1), 2 surfaces waiting on uploads\n"
            ),
            "{}",
            outcome.text
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
