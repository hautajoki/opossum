use std::path::{Path, PathBuf};

use crate::*;

fn standard(fallback: bool) -> Routes {
    let mut pairs = vec![
        ("server", "ServerScriptService/server"),
        ("client", "StarterPlayer/StarterPlayerScripts/client"),
        ("shared", "ReplicatedStorage/shared"),
    ];
    if fallback {
        pairs.push(("*", "ReplicatedStorage/shared"));
    }
    Routes::new(pairs).unwrap()
}

fn at(routes: &Routes, rel: &str) -> String {
    match route(routes, Path::new(rel)) {
        Ok(Some(m)) => m.instance.join("/"),
        Ok(None) => "-".into(),
        Err(d) => d.code.as_str().into(),
    }
}

fn scan_of(routes: &Routes, files: &[&str]) -> Scan {
    scan_files(Path::new("/src"), files.iter().map(PathBuf::from).collect(), routes)
}

fn codes(s: &Scan) -> Vec<(&'static str, String)> {
    s.diags
        .iter()
        .map(|d| {
            (
                d.code.as_str(),
                d.path.strip_prefix("/src").unwrap().display().to_string(),
            )
        })
        .collect()
}

#[test]
fn outermost_route_wins() {
    let r = standard(true);
    assert_eq!(at(&r, "a/server/b.ts"), "ServerScriptService/server/a/b");
    // A different key below the governing one is an error, never a silent reroute.
    assert_eq!(at(&r, "a/shared/x/server/b.ts"), "route.nested");
    assert_eq!(at(&r, "a/server/b@client.ts"), "route.nested");
    assert_eq!(at(&r, "a@server/client/b.ts"), "route.nested");
    // The governing key restated: the folder stays a folder, the suffix comes off.
    assert_eq!(
        at(&r, "a/server/x/server/b.ts"),
        "ServerScriptService/server/a/x/server/b"
    );
    assert_eq!(at(&r, "a/server/b@server.ts"), "ServerScriptService/server/a/b");
    // `*` only catches what nothing routes; it never conflicts with a route below it.
    assert_eq!(at(&r, "a/client/b.ts"), "StarterPlayer/StarterPlayerScripts/client/a/b");
    assert_eq!(at(&standard(false), "a/b.ts"), "route.unrouted");
}

#[test]
fn only_folders_and_at_suffixes_route() {
    let r = standard(false);
    assert_eq!(at(&r, "Server/a.ts"), "ServerScriptService/server/a");
    assert_eq!(at(&r, "SERVER/a.ts"), "route.unrouted");
    for name in [
        "a-server.ts",
        "a_server.ts",
        "aServer.ts",
        "a.server.ts",
        "server.ts",
        "x/HttpClient.ts",
    ] {
        assert_eq!(at(&r, name), "route.unrouted", "{name}");
    }
}

#[test]
fn names_drop_suffixes_and_extensions() {
    let r = standard(false);
    assert_eq!(at(&r, "inv/save@server.luau"), "ServerScriptService/server/inv/save");
    assert_eq!(at(&r, "inv@Server/save.tsx"), "ServerScriptService/server/inv/save");
    assert_eq!(at(&r, "@server/inv/save.ts"), "ServerScriptService/server/inv/save");
    assert_eq!(at(&r, "inv/@server.ts"), "scan.portableName");
    // An @ that names no route is part of the name; JSON keeps its dots.
    assert_eq!(at(&r, "shared/pkg@1/x.ts"), "ReplicatedStorage/shared/pkg@1/x");
    assert_eq!(at(&r, "shared/deck.spec.ts"), "ReplicatedStorage/shared/deck.spec");
    assert_eq!(at(&r, "shared/x.server.json"), "ReplicatedStorage/shared/x.server");
    let m = route(&r, Path::new("server/boot@server.server.luau")).unwrap().unwrap();
    assert_eq!(
        (m.instance.join("/"), m.class),
        ("ServerScriptService/server/boot".into(), Class::Script)
    );
    for never in [
        "shared/x.d.ts",
        "shared/README.md",
        "shared/.luaurc",
        "shared/notes.txt",
    ] {
        assert_eq!(at(&r, never), "-", "{never}");
    }
}

#[test]
fn init_files_make_the_folder_the_module() {
    let r = standard(true);
    assert_eq!(at(&r, "mods/noise/init.luau"), "ReplicatedStorage/shared/mods/noise");
    assert_eq!(at(&r, "mods/flux/index.ts"), "ReplicatedStorage/shared/mods/flux");
    assert_eq!(
        at(&r, "mods/flux/index.luau"),
        "ReplicatedStorage/shared/mods/flux/index"
    );
    // A routing folder holding the init file is consumed, so the module takes its parent's name.
    let m = route(&r, Path::new("app/server/index.server.ts")).unwrap().unwrap();
    assert_eq!(
        (m.instance.join("/"), m.class, m.init),
        ("ServerScriptService/server/app".into(), Class::Script, true)
    );
    assert_eq!(
        at(
            &Routes::new([("server", "ServerScriptService")]).unwrap(),
            "server/init.luau"
        ),
        "tree.instanceClash"
    );

    let s = scan_of(
        &r,
        &[
            "m/init.luau",
            "m/a.luau",
            "m/core/server/b.luau",
            "m/index.d.ts",
            "t.d.ts",
        ],
    );
    assert_eq!(codes(&s), [("route.splitFolder", "m/core/server/b.luau".into())]);
    assert_eq!(s.modules[0].types.as_deref(), Some(Path::new("/src/m/index.d.ts")));
    assert_eq!(s.decls, [PathBuf::from("/src/t.d.ts")]);
    assert_eq!(
        s.find(Path::new("m/index.d.ts")).map(|m| &m.file),
        Some(&PathBuf::from("/src/m/init.luau"))
    );
}

#[test]
fn tree_clashes() {
    let r = standard(true);
    let s = scan_of(
        &r,
        &[
            "a/x.ts",
            "a/shared/x.ts", // same instance as a/x.ts
            "b/y.ts",        // a module where b/y/ is a folder
            "b/y/z.ts",
            "c/Thing.ts", // case-only sibling of c/thing
            "c/shared/thing.ts",
            "k/core/init.luau", // core/shared is consumed, so its init lands on core
            "k/core/shared/init.luau",
            "w/worker.server.luau", // a loose Script in ReplicatedStorage never runs
            "p/init.luau",
            "p/worker.server.luau", // a template its module clones
        ],
    );
    assert_eq!(
        codes(&s),
        [
            ("tree.instanceClash", "a/shared/x.ts".into()),
            ("tree.instanceClash", "b/y.ts".into()),
            ("tree.instanceClash", "k/core/shared/init.luau".into()),
            ("tree.deadScript", "w/worker.server.luau".into()),
            ("scan.portableName", "c/shared/thing.ts".into()),
        ]
    );
}

#[test]
fn require_spelling() {
    let r = standard(true);
    let s = scan_of(
        &r,
        &[
            "f/shared/a.ts",
            "f/shared/b/c.ts",
            "f/shared/m/index.ts",
            "f/shared/m/d.ts",
            "f/client/e.ts",
            "f/client/g.ts",
            "f/server/h.ts",
            "lib/x.ts", // `*`: the same target as shared, so relative
        ],
    );
    let req = |from: &str, to: &str| require(&r, s.find(Path::new(from)).unwrap(), s.find(Path::new(to)).unwrap());
    assert_eq!(req("f/shared/a.ts", "f/shared/b/c.ts").as_deref(), Ok("./b/c"));
    assert_eq!(req("f/shared/b/c.ts", "f/shared/a.ts").as_deref(), Ok("../a"));
    assert_eq!(req("f/shared/m/index.ts", "f/shared/m/d.ts").as_deref(), Ok("@self/d"));
    assert_eq!(req("f/shared/m/d.ts", "f/shared/m/index.ts").as_deref(), Ok("../m"));
    assert_eq!(req("f/shared/a.ts", "lib/x.ts").as_deref(), Ok("../lib/x"));
    assert_eq!(req("f/client/e.ts", "f/client/g.ts").as_deref(), Ok("./g"));
    assert_eq!(
        req("f/client/e.ts", "f/shared/a.ts").as_deref(),
        Ok("@game/ReplicatedStorage/shared/f/a")
    );
    assert_eq!(req("f/server/h.ts", "f/client/e.ts"), Err(Code::Boundary));
    assert_eq!(req("f/shared/a.ts", "f/server/h.ts"), Err(Code::Boundary));
    assert_eq!(boundary(Realm::Client, Realm::Server), Some(Code::Boundary));
    assert_eq!(boundary(Realm::Server, Realm::Shared), None);

    let (m, line) = s.where_instance("Players.Bob.PlayerScripts.client.f.e:12").unwrap();
    assert_eq!((m.file.as_path(), line), (Path::new("/src/f/client/e.ts"), Some(12)));
}
