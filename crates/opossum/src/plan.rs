//! `opossum plan`: what going outward would do, worked out offline with no credentials. Every asset file is hashed and
//! compared with its record in `opossum.lock`, so the plan is also the dump of every id, and with an environment the
//! resources it declares are compared with the records of its universe.

use std::fmt::Write as _;

use serde_json::json;

use crate::config::Config;
use crate::diag::{Diag, Severity};
use crate::lock::{self, Asset, Lock};
use crate::{Outcome, assets, resources};

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
    /// Roblox refused this content when it was sent: it is not sent again until the file changes, or `opossum adopt
    /// asset --none` forgets the refusal.
    Rejected,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Current => "current",
            State::Changed => "changed",
            State::New => "new",
            State::Pending => "pending",
            State::Ambiguous => "ambiguous",
            State::Rejected => "rejected",
        }
    }
}

/// The state of a file whose input hash is `hash`, with `record` its record in the lock. A create in flight decides
/// over the id it would replace.
pub fn state(record: Option<&Asset>, hash: &str) -> State {
    match record {
        Some(Asset { next: Some(_), op: None, .. }) => State::Ambiguous,
        Some(Asset { next: Some(_), .. }) => State::Pending,
        Some(Asset { rejected: Some(r), .. }) if r == hash => State::Rejected,
        Some(Asset { id: Some(_), hash: Some(h), .. }) if h == hash => State::Current,
        Some(Asset { id: Some(_), .. }) => State::Changed,
        _ => State::New,
    }
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
    let mut text = String::new();
    let mut sets = Vec::new();
    let (mut ambiguous, mut rejected) = (0, 0);
    for (i, set) in cfg.assets.iter().enumerate() {
        let (mut files, mut lines, mut counts) = (Vec::new(), String::new(), [0; 6]);
        for (a, hash) in found.iter().zip(&hashes).filter(|(a, _)| a.set == i) {
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
                State::Rejected => writeln!(lines, "rejected  {} ({})", a.file, refused(&a.file)),
                _ => writeln!(lines, "{:<9} {}", state.name(), a.file),
            };
        }
        ambiguous += counts[State::Ambiguous as usize];
        rejected += counts[State::Rejected as usize];
        let n = |state: State| count(counts[state as usize]);
        let _ = writeln!(
            text,
            "{}: {} files, {} current, {} changed, {} new, {} pending, {} ambiguous, {} rejected",
            set.name,
            count(files.len()),
            n(State::Current),
            n(State::Changed),
            n(State::New),
            n(State::Pending),
            n(State::Ambiguous),
            n(State::Rejected)
        );
        text.push_str(&lines);
        sets.push(json!({ "set": set.name, "module": format!("opossum/{}", set.name), "files": files }));
    }
    if cfg.assets.is_empty() && environment.is_none() {
        text.push_str("nothing to plan: opossum.toml declares no [assets] and no [environments]\n");
    }
    let mut json = json!({ "ok": true, "assets": sets });
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
        let error =
            format!("uploads of {ambiguous} files may or may not have happened; `opossum adopt asset` settles each");
        ("asset.ambiguous", error)
    } else if rejected > 0 {
        let error = format!("Roblox refused the content of {rejected} files; change them or forget the refusal");
        ("asset.rejected", error)
    } else if blocked > 0 {
        let error =
            format!("creates of {blocked} resources may or may not have happened; `opossum adopt` settles each");
        ("resource.blocked", error)
    } else {
        return Outcome { exit: 0, json, text };
    };
    json["ok"] = false.into();
    json["ran"] = true.into();
    json["code"] = error.0.into();
    json["error"] = error.1.into();
    Outcome { exit: 1, json, text }
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
    let error = format!("{} errors in opossum.lock or the asset roots, so nothing was {done}", errors.len());
    Err(Outcome::refused(&first.code, error))
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
            "version = 1\n\
             assets.\"art/current.png\" = {{ id = 1, hash = \"{}\" }}\n\
             assets.\"art/changed.png\" = {{ id = 2, hash = \"{old}\" }}\n\
             assets.\"art/pending.ogg\" = {{ id = 3, hash = \"{}\", next = \"{next}\", op = \"operations/x\" }}\n\
             assets.\"art/ambiguous.ogg\" = {{ next = \"{next}\" }}\n",
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
