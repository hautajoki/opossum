//! `opossum plan`: what going outward would do, worked out offline with no credentials. Every asset file is hashed and
//! compared with its record in `opossum.lock`, so the plan is also the dump of every id.

use std::fmt::Write as _;

use serde_json::json;

use crate::config::Config;
use crate::diag::Diag;
use crate::lock::{self, Asset};
use crate::{Outcome, assets};

/// Where a file stands against its record in the lock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Its id holds what the file holds.
    Current,
    /// Its id holds an older version of the file.
    Changed,
    /// It has no id and no upload in flight.
    New,
    /// An upload is in flight and its operation can be asked how it went.
    Pending,
    /// An upload may or may not have happened, and nothing says which: `opossum adopt asset` decides.
    Ambiguous,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Current => "current",
            State::Changed => "changed",
            State::New => "new",
            State::Pending => "pending",
            State::Ambiguous => "ambiguous",
        }
    }
}

/// The state of a file whose input hash is `hash`, with `record` its record in the lock. A create in flight decides
/// over the id it would replace.
pub fn state(record: Option<&Asset>, hash: &str) -> State {
    match record {
        Some(Asset { next: Some(_), op: None, .. }) => State::Ambiguous,
        Some(Asset { next: Some(_), .. }) => State::Pending,
        Some(Asset { id: Some(_), hash: Some(h), .. }) if h == hash => State::Current,
        Some(Asset { id: Some(_), .. }) => State::Changed,
        _ => State::New,
    }
}

/// `opossum plan [--environment e]`. Exit 1 when an upload is ambiguous (the user must act), 2 when the config, the
/// lock or an asset root has errors.
pub fn plan(cfg: &Config, environment: Option<&str>) -> Outcome {
    let root = &cfg.root;
    let environment = match cfg.build_environment(environment) {
        Ok(environment) => environment,
        Err(error) => return Outcome::refused("config", error),
    };
    let (mut lock, mut diags) = match lock::read(root) {
        Ok(read) => read,
        Err(error) => return Outcome::refused("lock.io", error),
    };
    let (found, discovered) = assets::discover(root, &cfg.assets);
    diags.extend(discovered);
    let hashes = assets::hash_all(root, &cfg.assets, &found);
    for (a, hash) in found.iter().zip(&hashes) {
        if let Err(e) = hash {
            diags.push(Diag::new("asset.root", crate::diag::Severity::Error, Some(&root.join(&a.file)), e.clone()));
        }
    }
    let errors: Vec<&Diag> = diags.iter().filter(|d| d.is_error()).collect();
    if let Some(first) = errors.first() {
        for d in &errors {
            eprintln!("{}", d.human(root));
        }
        let error = format!("{} errors in opossum.lock or the asset roots, so nothing was planned", errors.len());
        return Outcome::refused(&first.code, error);
    }
    let mut text = String::new();
    let mut sets = Vec::new();
    let mut ambiguous = 0;
    for (i, set) in cfg.assets.iter().enumerate() {
        let (mut files, mut lines, mut counts) = (Vec::new(), String::new(), [0; 5]);
        for (a, hash) in found.iter().zip(&hashes).filter(|(a, _)| a.set == i) {
            let hash = hash.as_deref().unwrap_or_default();
            // a merge's records for the file: the one whose id holds it, if any
            if let Some(warning) = lock.settle(root, &a.file, hash) {
                eprintln!("{}", warning.human(root));
            }
            let record = lock.assets.get(&a.file);
            let state = state(record, hash);
            counts[state as usize] += 1;
            let mut file = json!({ "file": a.file, "key": a.key, "state": state.name() });
            if let Some(id) = record.and_then(|r| r.id) {
                file["id"] = id.into();
            }
            files.push(file);
            let _ = match state {
                State::Current => Ok(()),
                State::Ambiguous => writeln!(
                    lines,
                    "ambiguous {0} (an upload may have happened: opossum adopt asset {0} <id>, or --none)",
                    a.file
                ),
                _ => writeln!(lines, "{:<9} {}", state.name(), a.file),
            };
        }
        ambiguous += counts[State::Ambiguous as usize];
        let n = |state: State| count(counts[state as usize]);
        let _ = writeln!(
            text,
            "{}: {} files, {} current, {} changed, {} new, {} pending, {} ambiguous",
            set.name,
            count(files.len()),
            n(State::Current),
            n(State::Changed),
            n(State::New),
            n(State::Pending),
            n(State::Ambiguous)
        );
        text.push_str(&lines);
        sets.push(json!({ "set": set.name, "module": format!("opossum/{}", set.name), "files": files }));
    }
    if cfg.assets.is_empty() {
        text.push_str("nothing to plan: opossum.toml declares no [assets]\n");
    }
    let mut json = json!({ "ok": ambiguous == 0, "assets": sets });
    if let Some(environment) = environment {
        json["environment"] = environment.name.as_str().into();
    }
    if ambiguous > 0 {
        json["ran"] = true.into();
        json["code"] = "asset.ambiguous".into();
        let error =
            format!("uploads of {ambiguous} files may or may not have happened; `opossum adopt asset` settles each");
        json["error"] = error.into();
    }
    Outcome { exit: u8::from(ambiguous > 0), json, text }
}

/// `1,300`.
fn count(n: usize) -> String {
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
    /// the plan: the user has to say whether it happened.
    #[test]
    fn every_file_gets_its_state_and_an_ambiguous_upload_fails_the_plan() {
        let root = std::env::temp_dir().join(format!("opossum-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("art")).unwrap();
        std::fs::write(root.join("opossum.toml"), "name = \"plan\"\ncreator = { user = 1 }\n[routes]\nshared = \"ReplicatedStorage/shared\"\n[assets]\nart = \"art\"\n")
            .unwrap();
        for file in ["current.png", "changed.png", "new.png", "pending.ogg", "ambiguous.ogg"] {
            std::fs::write(root.join("art").join(file), file).unwrap();
        }
        let cfg = crate::config::load(&root.join("opossum.toml")).unwrap();
        let hash = |file: &str, ext: &str| assets::hash(ext, cfg.assets[0].image, file.as_bytes());
        let (old, next) = ("0".repeat(32), "1".repeat(32));
        let lock = format!(
            "version = 1\n[assets]\n\
             \"art/current.png\" = {{ id = 1, hash = \"{}\" }}\n\
             \"art/changed.png\" = {{ id = 2, hash = \"{old}\" }}\n\
             \"art/pending.ogg\" = {{ id = 3, hash = \"{}\", next = \"{next}\", op = \"operations/x\" }}\n\
             \"art/ambiguous.ogg\" = {{ next = \"{next}\" }}\n",
            hash("current.png", ".png"),
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
                ("changed", "changed"),
                ("current", "current"),
                ("new", "new"),
                ("pending", "pending")
            ]
        );
        assert_eq!((outcome.exit, &outcome.json["code"]), (1, &"asset.ambiguous".into()));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
