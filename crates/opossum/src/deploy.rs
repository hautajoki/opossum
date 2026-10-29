//! `opossum deploy`: the whole project outward in one run. Every refusal it can know of comes before its first
//! request: each asset file to upload is compiled and checked, every resource request prepared, and every target place
//! built. Then the assets go up, the resources are applied, and the places are built again with the ids that gave them
//! and saved, up to four at once, each with its own outcome.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use cloud::Client;
use serde_json::{Value, json};

use crate::build::Session;
use crate::cloud_exec::{self, Failure, Placed};
use crate::config::{self, Config};
use crate::guard::{Cache, RemoteGuard};
use crate::plan::count;
use crate::{Outcome, editor, resources, upload};

/// Place saves in flight at once.
const SAVES: usize = 4;

/// A place's new version, or the code and error that say why it has none.
type Saved = Result<u64, (&'static str, String)>;

/// The project built with one place profile, for every target place with that profile.
struct Build {
    profile: Option<String>,
    session: Session,
}

/// `opossum deploy [--environment e] [--saved] [--allow-spend] [place...]`: uploads new and changed assets, applies
/// the environment's resources, then saves the named places (else every place the environment maps) as new versions,
/// published unless `saved`. Exit 2 when nothing was sent: the config, the lock, an asset file, a resource or a build
/// is wrong, or a badge create lacks `allow_spend`. Exit 1 when it stopped part way or any place was not saved, with
/// what it did recorded.
#[allow(clippy::too_many_arguments)]
pub fn deploy(
    cfg: &Config,
    client: &Client,
    guard: &RemoteGuard,
    cache: &Cache,
    environment: Option<&str>,
    names: &[String],
    saved: bool,
    allow_spend: bool,
    stop: &AtomicBool,
) -> Outcome {
    let env = match cfg.environment(environment) {
        Ok(env) => env,
        Err(error) => return Outcome::refused("config", error),
    };
    // they narrow only what is saved: assets and resources are always the whole project
    let mut names: Vec<&str> = match names {
        [] => env.places.iter().map(|(p, _)| p.as_str()).collect(),
        names => names.iter().map(String::as_str).collect(),
    };
    names.sort_unstable();
    names.dedup();
    let mut places: Vec<Placed> = Vec::new();
    for name in names {
        match cloud_exec::resolve(cfg, Some(&env.name), Some(name)) {
            Ok(at) => places.push(at),
            Err(error) => return Outcome::refused("config", error),
        }
    }

    // preflight, offline: whatever would stop the deploy part way refuses it now
    let assets = match upload::prepare(cfg) {
        Ok(assets) => assets,
        Err(refusal) => return refusal,
    };
    if let Some(refusal) = assets.as_ref().and_then(upload::Prepared::refusal) {
        return refusal;
    }
    let lock = match resources::read(cfg) {
        Ok(lock) => lock,
        Err(refusal) => return refusal,
    };
    let (wants, ops) = match resources::survey(cfg, env, &lock) {
        Ok(planned) => planned,
        Err(refusal) => return refusal,
    };
    let records = lock.universes.get(&env.universe).cloned().unwrap_or_default();
    let images = match resources::prepare(&wants, &ops, &records, allow_spend) {
        Ok(images) => images,
        Err((code, error)) => return Outcome::refused(code, error),
    };
    // ids change string and number values, never whether the project compiles or roughly its size
    let mut builds: Vec<Build> = Vec::new();
    for at in &places {
        if builds.iter().any(|b| b.profile == at.profile) {
            continue;
        }
        let profile = at.profile.as_deref().map_or(String::new(), |p| format!(" with profile {p}"));
        eprintln!("building {}{profile}…", at.label());
        // each session owns its config
        let built = config::load(&cfg.root.join(config::FILE))
            .map_err(|error| Failure { code: "config", error, ran: false })
            .and_then(|cfg| cloud_exec::build(cfg, at.profile.as_deref(), &env.name));
        match built {
            Ok((session, _)) => builds.push(Build { profile: at.profile.clone(), session }),
            Err(f) => return Outcome::refused(f.code, f.error),
        }
    }

    // outward
    let kind = if saved { "Saved" } else { "Published" };
    let upload = match assets {
        Some(assets) => upload::send(cfg, client, guard, assets, stop),
        None => upload::idle(),
    };
    let uploaded = upload.json["uploaded"].as_u64().unwrap_or(0) as usize;
    let mut json = json!({ "ok": true, "environment": env.name, "upload": upload.json, "versionType": kind,
                           "places": [] });
    if upload.exit != 0 {
        let (code, error) = (json["upload"]["code"].clone(), json["upload"]["error"].clone());
        return failed(json, upload.text + "nothing was provisioned and no place saved\n", &code, &error);
    }
    let mut lock = match resources::read(cfg) {
        Ok(lock) => lock,
        Err(refusal) => {
            let text = format!("{}\nnothing was provisioned and no place saved\n", refusal.text);
            return failed(json, text, &refusal.json["code"], &refusal.json["error"]);
        }
    };
    let target = resources::Target::new(cfg, env, client, guard);
    let (tally, fail) = resources::apply(&target, &mut lock, &wants, &ops, &images, stop);
    let provision = resources::outcome(env, &ops, &tally, fail);
    json["provision"] = provision.json.clone();
    let mut text =
        format!("{}: {} assets uploaded, {} resources applied\n", env.name, count(uploaded), count(tally.applied));
    if provision.exit != 0 {
        let _ = writeln!(text, "failed: {}\nno place was saved", provision.json["error"].as_str().unwrap_or(""));
        return failed(json, text, &provision.json["code"], &provision.json["error"]);
    }

    // the places with the ids the lock holds now, each build's bytes for every place of its profile
    let mut bytes = Vec::new();
    for b in &mut builds {
        let built = rebuild(&mut b.session).map(|(rbxl, packs)| {
            let labels: Vec<String> = places.iter().filter(|p| p.profile == b.profile).map(Placed::label).collect();
            if packs.packed > 0 || !packs.missing.is_empty() {
                eprintln!("{}: {}", labels.join(", "), packed(&packs));
            }
            let hash = blake3::hash(&rbxl).to_hex().to_string();
            // a build holding `$buildTime()` is never built again, so the cache could never reuse it
            (rbxl, (!b.session.timestamped()).then_some(hash))
        });
        bytes.push(built.map_err(|f| (f.code, f.error)));
    }
    let next = AtomicUsize::new(0);
    let save = || {
        let mut done = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            let Some(at) = places.get(i) else { return done };
            let b = builds.iter().position(|b| b.profile == at.profile).expect("every profile was built");
            let result = match &bytes[b] {
                Err((code, error)) => Err((*code, format!("not saved: {error}"))),
                Ok(_) if stop.load(Ordering::SeqCst) => {
                    Err(("deploy.stopped", "not saved: opossum was stopped".into()))
                }
                Ok((rbxl, hash)) => {
                    eprintln!("{} {}…", if saved { "saving" } else { "publishing" }, at.label());
                    match client.save_place(at.universe, at.id, rbxl, !saved) {
                        Ok(version) => {
                            if let Some(hash) = hash {
                                cloud_exec::remember(&cloud_exec::versions(cache, client, at), hash, version);
                            }
                            Ok(version)
                        }
                        Err(e) if e.definite() => Err((e.diag(), e.to_string())),
                        Err(e) => Err((e.diag(), format!("{e}; Roblox may have saved it all the same"))),
                    }
                }
            };
            done.push((i, result));
        }
    };
    let mut results: Vec<(usize, Saved)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..SAVES.min(places.len())).map(|_| s.spawn(save)).collect();
        workers.into_iter().flat_map(|w| w.join().expect("a place save panicked")).collect()
    });
    results.sort_by_key(|(i, _)| *i);

    let mut list = Vec::new();
    let mut first = None;
    for (i, result) in results {
        let at = &places[i];
        match result {
            Ok(version) => {
                let _ = writeln!(text, "{} v{version} {}", at.label(), kind.to_lowercase());
                list.push(json!({ "name": at.place, "placeId": at.id, "version": version }));
            }
            Err((code, error)) => {
                let _ = writeln!(text, "{} failed: {error}", at.label());
                list.push(json!({ "name": at.place, "placeId": at.id, "error": error, "code": code }));
                first.get_or_insert(code);
            }
        }
    }
    let failures = list.iter().filter(|p| p.get("error").is_some()).count();
    json["places"] = list.into();
    let Some(code) = first else { return Outcome { exit: 0, json, text } };
    let error = format!("{} of {} places were not saved", failures, places.len());
    failed(json, text, &code.into(), &error.into())
}

/// A session built before the upload, built again for the ids it gave: new ids only set the id modules' sources, and
/// a resource's first id (which changes its type) loads the program again.
fn rebuild(session: &mut Session) -> Result<(Vec<u8>, place::PackStats), Failure> {
    let fail = |error| Failure { code: "build", error, ran: true };
    if session.render_modules().map_err(fail)? == Some(true) {
        editor::write(&session.cfg.root, &session.cfg.ts, &session.modules).map_err(fail)?;
        session.update(&[], true, &[]).map_err(fail)?;
    }
    cloud_exec::rbxl(session)
}

/// `packs: 168 applied, 3 without a pack yet (SurfaceAppearance 2, MaterialVariant 1)`: the only sign that a map
/// uploaded again lost its texture packs.
fn packed(packs: &place::PackStats) -> String {
    let mut line = format!("packs: {} applied", count(packs.packed));
    if !packs.missing.is_empty() {
        let missing: Vec<String> = packs.missing.iter().map(|(class, n)| format!("{class} {n}")).collect();
        let n = packs.missing.iter().map(|(_, n)| n).sum::<usize>();
        let _ = write!(line, ", {} without a pack yet ({})", count(n), missing.join(", "));
    }
    line
}

/// It ran and stopped short: `ok` false with the code and error that say why.
fn failed(mut json: Value, text: String, code: &Value, error: &Value) -> Outcome {
    json["ok"] = false.into();
    json["ran"] = true.into();
    json["code"] = code.clone();
    json["error"] = error.clone();
    Outcome { exit: 1, json, text }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};

    use cloud::mock::{Mock, Reply, auto};
    use cloud::{Clock, Fake};

    use super::*;
    use crate::lock;

    /// The types a TypeScript program needs from a lib, which a test project has no node_modules for.
    const GLOBALS: &str = "interface Array<T> {}\ninterface Boolean {}\ninterface CallableFunction {}\n\
                           interface Function {}\ninterface IArguments {}\ninterface NewableFunction {}\n\
                           interface Number {}\ninterface Object {}\ninterface RegExp {}\ninterface String {}\n";

    /// A project with an asset set `art` holding `a.glb`, a pass, places `main` and `places` in universe 7 (ids from
    /// 10), and `files` written over it.
    fn project(name: &str, places: &[&str], extra: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("opossum-deploy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["art", "src/shared"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let ids: Vec<String> = places.iter().enumerate().map(|(i, p)| format!("{p} = {}", 10 + i)).collect();
        let declared: String = places.iter().skip(1).map(|p| format!("[places.{p}]\n")).collect();
        let toml = format!(
            "name = \"d\"\ncreator = {{ user = 1 }}\n[routes]\nshared = \"ReplicatedStorage/shared\"\n[assets]\n\
             art = \"art\"\n[places.main]\nroot = true\n{declared}[environments.dev]\nuniverseId = 7\n\
             places = {{ {} }}\n[passes.vip]\nname = \"VIP\"\nprice = 100\n{extra}",
            ids.join(", ")
        );
        let write = |file: &str, text: &str| std::fs::write(root.join(file), text).unwrap();
        write("opossum.toml", &toml);
        write("src/globals.d.ts", GLOBALS);
        write("src/shared/a.luau", "return 1\n");
        write("art/a.glb", "x");
        for (file, text) in files {
            write(file, text);
        }
        std::fs::canonicalize(root).unwrap()
    }

    /// `opossum deploy` of `root` against `mock`, on a thread with the compiler's stack.
    fn deploy_at(root: &Path, mock: &Mock, allow_spend: bool) -> Outcome {
        let (root, base) = (root.to_path_buf(), mock.base.clone());
        let run = move || {
            let cfg = config::load(&root.join(config::FILE)).unwrap();
            let client = Client::new("test-key", &base, Clock::Fake(Fake::new())).unwrap();
            let cache = Cache { dir: root.join(".cache") };
            let guard = RemoteGuard::acquire(&cache).unwrap();
            deploy(&cfg, &client, &guard, &cache, None, &[], false, allow_spend, &AtomicBool::new(false))
        };
        std::thread::Builder::new().stack_size(lower::STACK_SIZE).spawn(run).unwrap().join().unwrap()
    }

    fn sent(mock: &Mock) -> Vec<String> {
        mock.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect()
    }

    /// Whatever would stop a deploy part way (an asset file, a blocked or unpaid resource, a build error, a place the
    /// environment lacks) refuses it before the first request, with the lock as it was.
    #[test]
    fn every_refusal_comes_before_the_first_request() {
        let next = "1".repeat(32);
        let blocked = format!("version = 1\n[universes.7]\n\"pass/vip\" = {{ next = \"{next}\" }}\n");
        let ambiguous =
            format!("version = 1\ncreator = {{ user = 1 }}\n[assets]\n\"art/a.glb\" = {{ next = \"{next}\" }}\n");
        let badge = "[badges.welcome]\nname = \"W\"\ndescription = \"Hi\"\nicon = \"art/a.glb\"\nexpectedCost = 5\n";
        // each case writes one file over the project; the spend case's is harmless
        let cases = [
            ("image", "", ("art/bad.png", "not a png"), "asset.image"),
            ("ambiguous", "", ("opossum.lock", ambiguous.as_str()), "asset.ambiguous"),
            ("blocked", "", ("opossum.lock", blocked.as_str()), "resource.blocked"),
            ("spend", badge, ("src/shared/b.luau", "return 2\n"), "resource.spend"),
            ("build", "", ("src/shared/t.ts", "export const x: number = \"s\";\n"), "build"),
        ];
        for (name, extra, file, code) in cases {
            let root = project(name, &["main"], extra, &[file]);
            let lock = std::fs::read(root.join(lock::FILE)).ok();
            let mock = Mock::bind(0, Some(auto())).unwrap();
            let out = deploy_at(&root, &mock, false);
            assert_eq!((out.exit, out.json["code"].as_str()), (2, Some(code)), "{name}: {}", out.text);
            assert!(mock.requests().is_empty(), "{name}: {:?}", sent(&mock));
            assert_eq!(std::fs::read(root.join(lock::FILE)).ok(), lock, "{name}");
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    /// Five places, four saves at a time: one place open in Studio fails alone, the others are published with the ids
    /// this deploy's upload and provision gave, and each save is remembered, so a cloud run of the same build reuses
    /// it.
    #[test]
    fn every_place_has_its_own_outcome_built_with_the_new_ids_and_remembered() {
        let names = ["main", "b", "c", "d", "e"];
        let root = project("places", &names, "", &[]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        // b, c, d and main (sorted: b c d e main) answer only once four are in flight; e is open in Studio
        let four = Arc::new(Barrier::new(4));
        for (id, version) in [(11, 41), (12, 42), (13, 43), (10, 40)] {
            let four = four.clone();
            let reply = move |_: &cloud::mock::Request| {
                four.wait();
                Reply::json(200, &serde_json::json!({ "versionNumber": version }))
            };
            mock.on("POST", &format!("universes/v1/7/places/{id}/versions"), Reply::Run(Box::new(reply)));
        }
        let busy = Reply::Status { status: 409, headers: vec![], body: b"Server is busy".to_vec() };
        mock.on("POST", "universes/v1/7/places/14/versions", busy);
        let before = build_now(&root);

        let out = deploy_at(&root, &mock, false);
        assert_eq!((out.exit, out.json["code"].as_str()), (1, Some("cloud.studioOpen")), "{}", out.text);
        let places: Vec<String> = out.json["places"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| format!("{} {}", p["name"], p.get("version").unwrap_or(&p["code"])))
            .collect();
        assert_eq!(places, ["\"b\" 41", "\"c\" 42", "\"d\" 43", "\"e\" \"cloud.studioOpen\"", "\"main\" 40"]);
        assert!(out.text.contains("dev/e failed: open cloud save place") && out.text.contains("open in Studio"));
        assert!(out.text.starts_with("dev: 1 assets uploaded, 1 resources applied\ndev/b v41 published\n"));
        assert_eq!(mock.peak(), 4);

        // the upload and the pass went first, and every place got the one build that holds their ids
        let requests = mock.requests();
        let saves: Vec<&cloud::mock::Request> = requests.iter().filter(|r| r.path.ends_with("/versions")).collect();
        assert_eq!(saves.len(), 5);
        assert!(saves.iter().all(|r| r.query == "versionType=Published" && r.body == saves[0].body));
        let first = requests.iter().position(|r| r.path.ends_with("/versions")).unwrap();
        let before_saves: Vec<&str> = requests[..first].iter().map(|r| r.path.as_str()).collect();
        assert!(
            before_saves.contains(&"assets/v1/assets")
                && before_saves.contains(&"game-passes/v1/universes/7/game-passes")
        );
        let after = build_now(&root);
        assert_eq!(saves[0].body, after);
        assert_ne!(before, after);

        let hash = blake3::hash(&after).to_hex().to_string();
        let host = mock.base.trim_start_matches("http://").replace(':', "-");
        let cache = |id: u64| std::fs::read_to_string(root.join(format!(".cache/versions/{host}/7-{id}"))).ok();
        assert_eq!(cache(10), Some(format!("{hash} 40\n")));
        assert_eq!(cache(14), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A deploy that stops in its upload or its provision saves no place.
    #[test]
    fn a_failed_upload_or_provision_saves_no_place() {
        let root = project("partial", &["main"], "", &[]);
        let mock = Mock::bind(0, Some(auto())).unwrap();
        mock.on("POST", "assets/v1/assets", Reply::json(503, &serde_json::json!({})));
        let out = deploy_at(&root, &mock, false);
        assert_eq!((out.exit, out.json["code"].as_str()), (1, Some("asset.ambiguous")), "{}", out.text);
        assert!(out.json.get("provision").is_none() && out.json["places"] == serde_json::json!([]));
        assert_eq!(sent(&mock), ["POST assets/v1/assets"]);

        mock.on("POST", "game-passes/", Reply::json(400, &serde_json::json!({})));
        std::fs::remove_file(root.join(lock::FILE)).unwrap();
        let out = deploy_at(&root, &mock, false);
        assert_eq!((out.exit, out.json["code"].as_str()), (1, Some("cloud.http")), "{}", out.text);
        assert_eq!(out.json["upload"]["uploaded"], 1);
        assert!(!sent(&mock).iter().any(|r| r.ends_with("/versions")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The root place's bytes as a build of the project now makes them.
    fn build_now(root: &Path) -> Vec<u8> {
        let root = root.to_path_buf();
        let run = move || {
            let cfg = config::load(&root.join(config::FILE)).unwrap();
            cloud_exec::build(cfg, None, "dev").map_err(|f| f.error).unwrap().1
        };
        std::thread::Builder::new().stack_size(lower::STACK_SIZE).spawn(run).unwrap().join().unwrap()
    }
}
