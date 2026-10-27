//! `rbxts_cli::yargs` against roblox-ts's own yargs: `tests/fixtures/yargs_cases.jsonl` holds 2539 argument vectors
//! (edge cases and seeded random ones) with what roblox-ts master's CLI setup (yargs 18.2.0, yargs-parser 22.0.0,
//! `src/CLI/cli.ts` and `commands/build.ts`) made of each: the failure message, the parsed options, or help/version.
//! The vectors and expectations come from `scratchpad/batch/rbxts-rs/c2/yargs/{gen,probe}.cjs`.

use rbxts_cli::yargs::{Parsed, Value, parse};
use serde_json::Value as Json;

/// roblox-ts's options; rbxts-rs's own (`timings`, `events`) are not in the stock vectors.
const KEYS: &[&str] = &[
    "project",
    "watch",
    "usePolling",
    "verbose",
    "noInclude",
    "logTruthyChanges",
    "writeOnlyChanged",
    "writeTransformedFiles",
    "optimizedLoops",
    "type",
    "includePath",
    "rojo",
    "allowCommentDirectives",
    "luau",
];

fn to_json(value: &Value) -> Json {
    match value {
        Value::Bool(b) => Json::Bool(*b),
        Value::Str(s) => Json::String(s.clone()),
        Value::Num(n) => serde_json::Number::from_f64(*n).map_or(Json::Null, Json::Number),
        Value::Arr(items) => Json::Array(items.iter().map(to_json).collect()),
        Value::Obj(entries) => Json::Object(entries.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
    }
}

/// Numbers compared as numbers (`1000` and `1000.0`).
fn same(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        (Json::Array(x), Json::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// Every vector of `cases` parses as roblox-ts's yargs parsed it, compared over `keys`; the number of vectors.
fn check(cases: &str, keys: &[&str]) -> usize {
    let mut failures = Vec::new();
    let mut count = 0;
    for line in cases.lines().filter(|l| !l.trim().is_empty()) {
        count += 1;
        let case: Json = serde_json::from_str(line).unwrap();
        let args: Vec<String> =
            case["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect();
        let ours = parse(&args);
        let matches = match (&ours, &case) {
            (Parsed::Fail(message), case) if case.get("fail").is_some() => case["fail"].as_str() == Some(message),
            (Parsed::Help, case) => case.get("other").and_then(Json::as_str) == Some("help"),
            (Parsed::Version, case) => case.get("other").and_then(Json::as_str) == Some("version"),
            (Parsed::Options(options), case) if case.get("ok").is_some() => {
                let ours: serde_json::Map<String, Json> = options
                    .iter()
                    .filter(|(k, _)| keys.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), to_json(v)))
                    .collect();
                same(&Json::Object(ours), &case["ok"])
            }
            _ => false,
        };
        if !matches {
            failures.push(format!("{args:?}: stock {line}, rbxts-rs {ours:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {count} differ:\n{}", failures.len(), failures.join("\n"));
    count
}

#[test]
fn every_vector_parses_as_roblox_ts_parses_it() {
    assert_eq!(check(include_str!("fixtures/yargs_cases.jsonl"), KEYS), 2539);
}

/// rbxts-rs's own options (`--timings`, `--events`, `--checkers`) against roblox-ts's yargs with them declared as
/// rbxts-rs declares them (`tools/yargs-own-cases.cjs` wrote the vectors).
#[test]
fn own_options_parse_as_yargs_would_parse_them() {
    let keys: Vec<&str> = KEYS.iter().copied().chain(["timings", "events", "checkers"]).collect();
    assert_eq!(check(include_str!("fixtures/yargs_own_cases.jsonl"), &keys), 29);
}
