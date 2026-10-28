//! `opossum exec`, `test` and `bench`: Luau run in a Studio DataModel through the running `opossum dev`, one
//! `POST /exec` on the port it wrote to `.opossum/port` (DESIGN.md, Exec). The exit status is 0 when it ran clean, 1
//! for a script error, a failed test or an answer that never came, 2 when it never ran (dev or the DataModel cannot be
//! reached, or holds older code than a test allows).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::editor;

pub enum Source {
    File(String),
    Code(String),
    Stdin,
}

pub struct Options {
    pub realm: Option<String>,
    pub json: bool,
    pub timeout: Option<f64>,
}

pub fn exec(cfg: &Config, options: &Options, source: Source) -> Result<bool, String> {
    let (name, code) = match source {
        Source::File(path) => {
            let code = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
            // Luau cuts long chunk names in errors: an absolute path is shown from the working directory, or by name
            let file = Path::new(&path);
            let cwd = std::env::current_dir().unwrap_or_default();
            let name = match file.strip_prefix(&cwd) {
                Ok(rel) if file.is_absolute() => rel.display().to_string(),
                _ if file.is_absolute() => file.file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned()),
                _ => path.clone(),
            };
            (name, code)
        }
        Source::Code(code) => ("exec".to_owned(), code),
        Source::Stdin => {
            let mut code = String::new();
            std::io::stdin().read_to_string(&mut code).map_err(|e| format!("stdin: {e}"))?;
            ("stdin".to_owned(), code)
        }
    };
    let answer = post(cfg, options, json!({ "name": name, "source": code }))?;
    if options.json {
        println!("{answer}");
        return Ok(answer["ok"].as_bool() == Some(true));
    }
    print_output(&answer);
    for value in answer["values"].as_array().into_iter().flatten() {
        match value {
            Value::String(text) => println!("{text}"),
            _ => println!("{value}"),
        }
    }
    Ok(print_error(&answer))
}

/// Runs hexagon's in-engine runner (`mode` "test" or "bench") over every matching suite in the DataModel.
pub fn test(cfg: &Config, options: &Options, mode: &str, filter: Option<&str>) -> Result<bool, String> {
    let runtime = cfg.root.join("node_modules/@hautajoki/hexagon/runtime");
    let entries = std::fs::read_dir(&runtime)
        .map_err(|_| format!("this project has no hexagon runtime ({} is missing)", runtime.display()))?;
    let mut modules = Map::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".luau")) else {
            continue;
        };
        if name != "local_runner" {
            let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            modules.insert(name.to_owned(), source.into());
        }
    }
    let filter = filter.map_or_else(|| "nil".to_owned(), luau_string);
    // the report as a table: dev maps its strings (messages, traces) like log lines
    let source = format!(
        "local HttpService = game:GetService(\"HttpService\")
return HttpService:JSONDecode(require(script.cloud_runner).run({{ mode = \"{mode}\", filter = {filter} }}))"
    );
    // specs must never pass against older code than the files: a playtest DataModel holding it refuses
    let body = json!({ "name": "hexagon", "source": source, "modules": modules, "map": true, "fresh": true });
    let mut answer = post(cfg, options, body)?;
    let report = Some(answer["values"][0].clone()).filter(Value::is_object);
    if options.json {
        if let Some(object) = answer.as_object_mut() {
            object.remove("values");
            object.insert("report".into(), report.clone().unwrap_or(Value::Null));
        }
        println!("{answer}");
        return Ok(report.is_some_and(|r| r["ok"].as_bool() == Some(true)));
    }
    print_output(&answer);
    let Some(report) = report else {
        print_error(&answer);
        return Ok(false);
    };
    let labels = |case: &Value| {
        let names: Vec<&str> = case["labels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        names.join(" / ")
    };
    for bench in report["benchmarks"].as_array().into_iter().flatten() {
        let us = |key: &str| bench[key].as_f64().unwrap_or(0.0) * 1e6;
        println!(
            "{}: mean {:.3} µs, median {:.3} µs, p95 {:.3} µs (±{:.1}%, {} iterations)",
            labels(bench),
            us("mean_seconds"),
            us("median_seconds"),
            us("p95_seconds"),
            bench["margin_percent"].as_f64().unwrap_or(0.0),
            bench["iterations"]
        );
    }
    for failure in report["failures"].as_array().into_iter().flatten() {
        let line = failure["line"].as_u64().map_or("?".to_owned(), |l| l.to_string());
        println!("FAIL {} ({}:{line})", labels(failure), failure["source"].as_str().unwrap_or("?"));
        println!("  {}", failure["message"].as_str().unwrap_or("").replace('\n', "\n  "));
        // the frames below the error that are not the runner's own
        let own = |line: &&str| ["hexagon", "[C]", "user_opossum"].iter().any(|p| line.starts_with(p));
        for line in failure["trace"].as_str().unwrap_or("").lines().filter(|l| !own(l)).take(3) {
            println!("    {line}");
        }
    }
    let count = |key: &str| report[key].as_u64().unwrap_or(0);
    let (pass, fail, skip) = if mode == "bench" {
        (count("bench_pass"), count("bench_fail"), count("bench_skip"))
    } else {
        (count("test_pass"), count("test_fail"), count("test_skip"))
    };
    println!(
        "{pass} passed, {fail} failed, {skip} skipped in {:.2} s ({}, generation {})",
        report["duration_seconds"].as_f64().unwrap_or(0.0),
        answer["realm"].as_str().unwrap_or("?"),
        answer["gen"]
    );
    Ok(report["ok"].as_bool() == Some(true))
}

/// What the script printed (warnings marked), and a note when the DataModel runs older code than the files.
fn print_output(answer: &Value) {
    for line in answer["output"].as_array().into_iter().flatten() {
        let text = line["text"].as_str().unwrap_or("");
        match line["kind"].as_str() {
            Some("warning") => println!("warning: {text}"),
            _ => println!("{text}"),
        }
    }
    if answer["fresh"] == Value::Bool(false) {
        let realm = answer["realm"].as_str().unwrap_or("?");
        let generation = &answer["gen"];
        // a client never takes frames; the play server does with hot reload
        let hint = if realm == "server" { ", or turn on hotReload" } else { "" };
        eprintln!(
            "opossum: the {realm} DataModel takes no frames and runs build {generation}, older than the files (restart \
             the playtest{hint})"
        );
    }
}

/// Prints the script's error and traceback, if it failed; returns whether it ran clean.
fn print_error(answer: &Value) -> bool {
    if answer["ok"].as_bool() == Some(true) {
        return true;
    }
    eprintln!("{}", answer["error"].as_str().unwrap_or("failed"));
    for line in answer["trace"].as_str().unwrap_or("").lines() {
        eprintln!("  {line}");
    }
    false
}

/// Sends `body` to this project's `opossum dev` and returns its answer; an error (exit status 2) when the code never
/// ran, which `--json` prints as `{"ok": false, "ran": false, "error"}` too.
fn post(cfg: &Config, options: &Options, body: Value) -> Result<Value, String> {
    send(cfg, options, body).inspect_err(|error| {
        if options.json {
            println!("{}", json!({ "ok": false, "ran": false, "error": error }));
        }
    })
}

fn send(cfg: &Config, options: &Options, mut body: Value) -> Result<Value, String> {
    body["root"] = cfg.root.to_string_lossy().into();
    if let Some(realm) = &options.realm {
        body["realm"] = realm.as_str().into();
    }
    if let Some(timeout) = options.timeout {
        body["timeout"] = timeout.into();
    }
    let file = editor::dir(&cfg.root).join("port");
    let port: u16 = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| format!("opossum dev is not running here ({} is missing)", shown(&cfg.root, &file)))?;
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|_| format!("opossum dev is not running (nothing answers on port {port})"))?;
    // dev bounds the wait itself: its fence, then the time limit and some slack
    let limit = options.timeout.unwrap_or(60.0) + 180.0;
    let _ = stream.set_read_timeout(Some(Duration::from_secs_f64(limit)));
    let body = body.to_string();
    let head = format!(
        "POST /exec HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let io = |e: std::io::Error| format!("opossum dev on port {port}: {e}");
    stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes())).map_err(io)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(io)?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("port {port} did not answer like opossum"))?;
    let status = head.split(' ').nth(1).unwrap_or("");
    let answer: Value =
        serde_json::from_str(body).map_err(|_| format!("port {port} did not answer like opossum dev"))?;
    if status != "200" {
        return Err(answer["error"].as_str().unwrap_or(body).to_owned());
    }
    Ok(answer)
}

fn shown(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).display().to_string()
}

/// `text` as a Luau string literal.
fn luau_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
