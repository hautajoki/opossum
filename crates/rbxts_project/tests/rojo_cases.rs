//! The Rojo resolver on the hand-made fixture trees (`tests/fixtures/rojo`) against the JavaScript resolver's answers
//! (`tools/gen-rojo-cases.cjs` wrote `rojo_cases.json`). The answers are win32 ones, so this runs on Windows.
//!
//! What the fixtures cover: `$path` to a directory, to a `.lua` file (looked up as `.luau`), to a `.model.json` file and
//! to a project file; a directory holding `default.project.json` (mounted as that project) and one found while
//! searching a partition (named after the project, the folder name dropped); a nested `*.project.json` found in a
//! partition (its name added under the folder); `init.*` scripts; `.server` / `.client` / `.plugin` / `.meta.json` /
//! `.model.json` / `.toml` / other files; optional, missing and invalid `$path`s and project files; `DataModel` and
//! service containers (network types, isolated containers); array-index child names (JavaScript key order); forward
//! slashes, lowercased roots, uppercased directories and `.` / `..` segments in queries; a child named `..dots`.

#![cfg(windows)]

use rbxts_project::nodepath;
use rbxts_project::rojo::{FileRelation, NetworkType, RbxType, RelativeRbxPathPart, RojoResolver};
use serde_json::{Value, json};

fn fixtures() -> String {
    nodepath::resolve(&[concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rojo")])
}

fn expand(s: &str, root: &str) -> String {
    s.replace("{root_fwd}", &root.replace('\\', "/"))
        .replace("{root_lower}", &root.to_lowercase())
        .replace("{root}", root)
}

fn placeholder(s: &str, root: &str) -> String {
    s.replace(root, "{root}")
}

fn network(n: NetworkType) -> &'static str {
    match n {
        NetworkType::Unknown => "Unknown",
        NetworkType::Client => "Client",
        NetworkType::Server => "Server",
    }
}

fn resolver_json(r: &RojoResolver, root: &str) -> Value {
    let mut mounts: Vec<(String, &Vec<String>)> =
        r.file_mounts().iter().map(|(k, v)| (placeholder(k, root), v)).collect();
    mounts.sort();
    json!({
        "isGame": r.is_game,
        "warnings": r.warnings().iter().map(|w| placeholder(w, root)).collect::<Vec<_>>(),
        "partitions": r.partitions().iter()
            .map(|p| json!({"fsPath": placeholder(&p.fs_path, root), "rbxPath": p.rbx_path}))
            .collect::<Vec<_>>(),
        "fileMounts": mounts.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
    })
}

fn query_json(r: &RojoResolver, query: &str, root: &str) -> Value {
    let path = expand(query, root);
    let rbx_path = r.get_rbx_path_from_file_path(&path);
    let rbx_type = match r.get_rbx_type_from_file_path(&path) {
        RbxType::ModuleScript => "ModuleScript",
        RbxType::Script => "Script",
        RbxType::LocalScript => "LocalScript",
    };
    json!({
        "query": query,
        "rbxPath": rbx_path,
        "rbxType": rbx_type,
        "network": rbx_path.as_ref().map(|p| network(r.get_network_type(p))),
        "isolated": rbx_path.as_ref().map(|p| r.is_isolated(p)),
    })
}

fn cases() -> Value {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rojo_cases.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn resolvers_match_javascript() {
    let root = fixtures();
    let cases = cases();
    let mut failures = Vec::new();
    let mut compared = 0;
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let resolver = match case["kind"].as_str().unwrap() {
            "fromPath" => RojoResolver::from_path(&expand(case["config"].as_str().unwrap(), &root)),
            _ => RojoResolver::synthetic(&expand(case["base"].as_str().unwrap(), &root)),
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        compared += 1;
        let got = resolver_json(&resolver, &root);
        if got != case["resolver"] {
            failures.push(format!("{name} resolver:\n  got  {got}\n  want {}", case["resolver"]));
        }
        for q in case["queries"].as_array().unwrap() {
            compared += 1;
            let got = query_json(&resolver, q["query"].as_str().unwrap(), &root);
            if &got != q {
                failures.push(format!("{name} query:\n  got  {got}\n  want {q}"));
            }
        }
    }
    assert!(failures.is_empty(), "{} of {compared} differ:\n{}", failures.len(), failures.join("\n"));
    assert!(compared > 250);
}

#[test]
fn relations_match_javascript() {
    let root = fixtures();
    let game = RojoResolver::from_path(&format!("{root}\\game\\default.project.json")).unwrap();
    for case in cases()["relations"].as_array().unwrap() {
        let path = |v: &Value| v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect::<Vec<_>>();
        let (from, to) = (path(&case["from"]), path(&case["to"]));
        let relation = match game.get_file_relation(&from, &to) {
            FileRelation::OutToOut => "OutToOut",
            FileRelation::OutToIn => "OutToIn",
            FileRelation::InToOut => "InToOut",
            FileRelation::InToIn => "InToIn",
        };
        let relative: Vec<String> = RojoResolver::relative(&from, &to)
            .into_iter()
            .map(|p| match p {
                RelativeRbxPathPart::Parent => "..".to_owned(),
                RelativeRbxPathPart::Child(name) => name,
            })
            .collect();
        let got = json!({
            "from": from, "to": to, "relation": relation, "relative": relative,
            "isolatedFrom": game.is_isolated(&from), "networkFrom": network(game.get_network_type(&from)),
        });
        assert_eq!(&got, case);
    }
}

#[test]
fn config_search_matches_javascript() {
    let root = fixtures();
    for case in cases()["find"].as_array().unwrap() {
        let found = RojoResolver::find_rojo_config_file_path(&expand(case["dir"].as_str().unwrap(), &root)).unwrap();
        let got = json!({
            "dir": case["dir"],
            "path": found.path.map(|p| placeholder(&p, &root)),
            "warnings": found.warnings.iter().map(|w| placeholder(w, &root)).collect::<Vec<_>>(),
        });
        assert_eq!(&got, case);
    }
}

/// Spelled-out expectations for the cases that matter most to imports (the same answers as the JavaScript's).
#[test]
fn readable_expectations() {
    let root = fixtures();
    let game = RojoResolver::from_path(&format!("{root}\\game\\default.project.json")).unwrap();
    let rbx = |rel: &str| game.get_rbx_path_from_file_path(&format!("{root}\\game\\{rel}"));
    let p = |s: &[&str]| Some(s.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    assert!(game.is_game);
    // init.* names its folder; .server / .client are dropped
    assert_eq!(rbx("out\\shared\\init.luau"), p(&["ReplicatedStorage", "Shared"]));
    assert_eq!(rbx("out\\shared\\sub\\init.server.luau"), p(&["ReplicatedStorage", "Shared", "sub"]));
    assert_eq!(rbx("out\\shared\\local.client.luau"), p(&["ReplicatedStorage", "Shared", "local"]));
    // only .json is stripped from .model.json / .meta.json, and init.meta.json is not an init script
    assert_eq!(rbx("out\\shared\\thing.model.json"), p(&["ReplicatedStorage", "Shared", "thing.model"]));
    assert_eq!(rbx("out\\shared\\sub\\init.meta.json"), p(&["ReplicatedStorage", "Shared", "sub", "init.meta"]));
    // a $path to a .lua file is a mount looked up as .luau, exactly (case-sensitive)
    assert_eq!(rbx("out\\single.lua"), p(&["ReplicatedStorage", "Single"]));
    assert_eq!(rbx("OUT\\single.luau"), None);
    // partitions match case-insensitively and keep the query's spelling
    assert_eq!(rbx("OUT\\SHARED\\Module.luau"), p(&["ReplicatedStorage", "Shared", "Module"]));
    // nested projects: $path to a project file, a directory with default.project.json, both found while searching
    assert_eq!(rbx("nested\\src\\x.luau"), p(&["ReplicatedStorage", "Nested", "x"]));
    assert_eq!(rbx("withdefault\\lib\\y.luau"), p(&["ReplicatedStorage", "WithDefault", "y"]));
    assert_eq!(rbx("out\\shared\\withdef\\src\\w.luau"), p(&["ReplicatedStorage", "Shared", "SubDefault", "w"]));
    assert_eq!(
        rbx("out\\shared\\nestedproj\\lib\\z.luau"),
        p(&["ReplicatedStorage", "Shared", "nestedproj", "FooProject", "z"])
    );
    // the package's descendant test: a child named `..dots` is outside its partition
    assert_eq!(rbx("out\\shared\\..dots\\d.luau"), None);
    // services and isolation
    let client = rbx("out\\client\\main.client.luau").unwrap();
    assert_eq!(game.get_network_type(&client), NetworkType::Client);
    assert!(game.is_isolated(&client));
    let server = rbx("out\\store\\s.luau").unwrap();
    assert_eq!(game.get_network_type(&server), NetworkType::Server);
    assert_eq!(game.get_file_relation(&client, &server), FileRelation::InToOut);
    // a model (no DataModel) has no containers
    let model = RojoResolver::from_path(&format!("{root}\\model\\default.project.json")).unwrap();
    let b = model.get_rbx_path_from_file_path(&format!("{root}\\model\\out\\a\\b.client.luau")).unwrap();
    assert_eq!(b, ["a", "b"]);
    assert!(!model.is_game && !model.is_isolated(&b));
    assert_eq!(model.get_network_type(&["StarterGui".to_owned()]), NetworkType::Unknown);
}

/// Links are followed into (as `realpathSync` does), and a link back to a directory being searched is an error rather
/// than the package's endless recursion. Junctions need no privileges; the test skips if `mklink` is unavailable.
#[test]
fn junctions_are_followed_and_cycles_reported() {
    let base = std::env::temp_dir().join(format!("rbxts-rojo-junction-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("out/real/inner")).unwrap();
    std::fs::create_dir_all(base.join("elsewhere/pkg")).unwrap();
    std::fs::write(base.join("elsewhere/pkg/thing.project.json"), r#"{"name":"Thing","tree":{"$path":"src"}}"#)
        .unwrap();
    std::fs::write(
        base.join("default.project.json"),
        r#"{"name":"j","tree":{"$className":"DataModel","ReplicatedStorage":{"Out":{"$path":"out"}}}}"#,
    )
    .unwrap();
    let mklink = |link: &std::path::Path, target: &std::path::Path| {
        // mklink wants backslashes
        let win = |p: &std::path::Path| p.to_string_lossy().replace('/', "\\");
        let out = std::process::Command::new("cmd").args(["/C", "mklink", "/J", &win(link), &win(target)]).output();
        if let Ok(o) = &out
            && !o.status.success()
        {
            eprintln!("mklink: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        }
        out.is_ok_and(|o| o.status.success())
    };
    if !mklink(&base.join("out/linked"), &base.join("elsewhere/pkg")) {
        eprintln!("mklink /J unavailable; skipped");
        return;
    }
    let config = nodepath::resolve(&[&base.join("default.project.json").to_string_lossy()]);
    let r = RojoResolver::from_path(&config).unwrap();
    // the project file inside the linked directory was found through the link
    let out = nodepath::join(&[nodepath::dirname(&config), "out"]);
    assert_eq!(
        r.partitions()[0].rbx_path,
        ["ReplicatedStorage", "Out", "linked", "Thing"],
        "{:?} (out: {out})",
        r.partitions()
    );
    assert!(mklink(&base.join("out/real/inner/back"), &base.join("out")));
    let err = RojoResolver::from_path(&config).unwrap_err();
    assert!(err.0.contains("cycle"), "{err}");
    let _ = std::fs::remove_dir_all(&base);
}
