//! `nodepath` against Node's own answers (`tools/gen-nodepath-cases.cjs` wrote the fixture).

use rbxts_project::nodepath::{Relation, posix, win32};
use serde_json::Value;

struct Flavor {
    normalize: fn(&str) -> String,
    join: fn(&[&str]) -> String,
    resolve_from: fn(&str, &[&str]) -> String,
    relative_from: fn(&str, &str, &str) -> String,
    dirname: fn(&str) -> &str,
    basename: fn(&str) -> &str,
    basename_without: for<'a> fn(&'a str, &str) -> &'a str,
    extname: fn(&str) -> &str,
    is_absolute: fn(&str) -> bool,
    relation: fn(&str, &str) -> Relation,
    sep: char,
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn check(name: &str, cases: &Value, f: &Flavor) -> usize {
    let cwd = s(&cases["cwd"]);
    let mut failures = Vec::new();
    let mut count = 0;
    let mut expect = |what: &str, input: String, got: String, want: &str| {
        count += 1;
        if got != want {
            failures.push(format!("{name} {what}({input}): got {got:?}, want {want:?}"));
        }
    };
    for c in cases["normalize"].as_array().unwrap() {
        expect("normalize", format!("{:?}", s(&c[0])), (f.normalize)(s(&c[0])), s(&c[1]));
    }
    for c in cases["dirname"].as_array().unwrap() {
        expect("dirname", format!("{:?}", s(&c[0])), (f.dirname)(s(&c[0])).into(), s(&c[1]));
    }
    for c in cases["basename"].as_array().unwrap() {
        expect("basename", format!("{:?}", s(&c[0])), (f.basename)(s(&c[0])).into(), s(&c[1]));
    }
    for c in cases["basenameExt"].as_array().unwrap() {
        let got = (f.basename_without)(s(&c[0]), s(&c[1]));
        expect("basename", format!("{:?}, {:?}", s(&c[0]), s(&c[1])), got.into(), s(&c[2]));
    }
    for c in cases["extname"].as_array().unwrap() {
        expect("extname", format!("{:?}", s(&c[0])), (f.extname)(s(&c[0])).into(), s(&c[1]));
    }
    for c in cases["isAbsolute"].as_array().unwrap() {
        let got = (f.is_absolute)(s(&c[0]));
        expect("isAbsolute", format!("{:?}", s(&c[0])), got.to_string(), &c[1].as_bool().unwrap().to_string());
    }
    for c in cases["join"].as_array().unwrap() {
        let parts: Vec<&str> = c[0].as_array().unwrap().iter().map(s).collect();
        expect("join", format!("{parts:?}"), (f.join)(&parts), s(&c[1]));
    }
    for c in cases["resolve"].as_array().unwrap() {
        let parts: Vec<&str> = c[0].as_array().unwrap().iter().map(s).collect();
        expect("resolve", format!("{parts:?}"), (f.resolve_from)(cwd, &parts), s(&c[1]));
    }
    for c in cases["relative"].as_array().unwrap() {
        let from = (f.resolve_from)(cwd, &[s(&c[0])]);
        let to = (f.resolve_from)(cwd, &[s(&c[1])]);
        expect("relative", format!("{from:?}, {to:?}"), (f.relative_from)(cwd, &from, &to), s(&c[2]));
        // `relation` (fast path for resolved paths) must agree with Node's relative
        let r = s(&c[2]);
        let want = if r.is_empty() {
            Relation::Same
        } else if (f.is_absolute)(r) {
            Relation::OtherRoot
        } else if r == ".." || r.starts_with(&format!("..{}", f.sep)) {
            Relation::Outside
        } else {
            Relation::Inside { dotdot_name: r.starts_with("..") }
        };
        expect(
            "relation",
            format!("{from:?}, {to:?}"),
            format!("{:?}", (f.relation)(&from, &to)),
            &format!("{want:?}"),
        );
    }
    assert!(failures.is_empty(), "{} of {count} cases differ:\n{}", failures.len(), failures.join("\n"));
    count
}

#[test]
fn nodepath_matches_node() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/nodepath_cases.json"))
        .expect("fixture");
    let cases: Value = serde_json::from_str(&text).unwrap();
    let w = Flavor {
        normalize: win32::normalize,
        join: win32::join,
        resolve_from: win32::resolve_from,
        relative_from: win32::relative_from,
        dirname: win32::dirname,
        basename: win32::basename,
        basename_without: win32::basename_without,
        extname: win32::extname,
        is_absolute: win32::is_absolute,
        relation: win32::relation,
        sep: win32::SEP,
    };
    let p = Flavor {
        normalize: posix::normalize,
        join: posix::join,
        resolve_from: posix::resolve_from,
        relative_from: posix::relative_from,
        dirname: posix::dirname,
        basename: posix::basename,
        basename_without: posix::basename_without,
        extname: posix::extname,
        is_absolute: posix::is_absolute,
        relation: posix::relation,
        sep: '/',
    };
    let n = check("win32", &cases["win32"], &w) + check("posix", &cases["posix"], &p);
    assert!(n > 1000);
}
