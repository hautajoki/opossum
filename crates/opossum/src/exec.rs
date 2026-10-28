//! `opossum exec`, `test` and `bench`: Luau run in a Studio DataModel through the running `opossum dev`, one
//! `POST /exec` on the port it wrote to `.opossum/port` (DESIGN.md, Exec). The exit status is 0 when it ran clean, 1
//! for a script error, a failed test or an answer that never came, 2 when it never ran (dev or the DataModel cannot be
//! reached, or holds older code than a test allows). `opossum play` and `stop` start and end a playtest the same way
//! (`POST /play`, `/stop`; DESIGN.md, Play), and `opossum pull` writes an instance from Studio into the files
//! (`POST /pull`; DESIGN.md, Pull). `opossum status` and `attach` ask every dev on the sync ports what it holds and
//! move Studio between them (`POST /status`, `/release`; DESIGN.md, Attach). `opossum test --cloud` runs the specs on
//! Open Cloud through tessera instead, with no dev or Studio involved (DESIGN.md, Cloud tests).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::build::Session;
use crate::config::Config;
use crate::diag::Diag;
use crate::{dev, editor};

/// The longest a run on Open Cloud may take, in seconds (tessera's and Open Cloud's limit).
const CLOUD_TIMEOUT: f64 = 300.0;

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
    let answer = post(cfg, options, "/exec", json!({ "name": name, "source": code }))?;
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
    let ok = print_error(&answer);
    print_stale(&answer);
    Ok(ok)
}

/// Runs hexagon's in-engine runner (`mode` "test" or "bench") over every matching suite in the DataModel.
pub fn test(cfg: &Config, options: &Options, mode: &str, filter: Option<&str>) -> Result<bool, String> {
    let modules: Map<String, Value> =
        runtime(&cfg.root)?.into_iter().map(|(name, source)| (name, source.into())).collect();
    // the report as a table: dev maps its strings (messages, traces) like log lines
    let source = format!(
        "local HttpService = game:GetService(\"HttpService\")
return HttpService:JSONDecode(require(script.cloud_runner).run({}))",
        run_request(mode, filter)
    );
    // specs must never pass against older code than the files: a playtest DataModel holding it refuses
    let body = json!({ "name": "hexagon", "source": source, "modules": modules, "map": true, "fresh": true });
    let mut answer = post(cfg, options, "/exec", body)?;
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
        print_stale(&answer);
        return Ok(false);
    };
    let ran = format!("{}, generation {}", answer["realm"].as_str().unwrap_or("?"), answer["gen"]);
    // the frames that are not the runner's own
    let shown = |line: &str| !["hexagon", "[C]", "user_opossum"].iter().any(|p| line.starts_with(p));
    print_report(&report, mode, &ran, shown);
    print_stale(&answer);
    Ok(report["ok"].as_bool() == Some(true))
}

/// Where `opossum test --cloud` runs: tessera's logical place and environment (its defaults when None).
pub struct Cloud {
    pub place: Option<String>,
    pub environment: Option<String>,
}

/// Runs the specs on Roblox's Open Cloud Luau execution instead of Studio: `tessera run --fresh` builds the place
/// with its configured build command, saves it as a place version (or reuses an identical saved one) and runs one
/// script holding hexagon's runtime there. The project is built here too, first: a build with errors never reaches
/// Roblox, and its log index maps the report like `test`'s.
pub fn cloud(cfg: Config, options: &Options, cloud: &Cloud, mode: &str, filter: Option<&str>) -> Result<bool, String> {
    let json = options.json;
    let refused = |error| refused(json, error);
    if options.timeout.is_some_and(|t| t > CLOUD_TIMEOUT) {
        return Err(refused(format!("--timeout with --cloud is at most {CLOUD_TIMEOUT} s, Open Cloud's limit")));
    }
    let runtime = runtime(&cfg.root).map_err(refused)?;
    editor::write(&cfg.root, &cfg.ts).map_err(refused)?;
    let mut session = Session::new(cfg, None).map_err(refused)?;
    session.build().map_err(refused)?;
    session.tree(false);
    let root = session.cfg.root.clone();
    let errors: Vec<Diag> = session.diagnostics().into_iter().filter(Diag::is_error).collect();
    if !errors.is_empty() {
        for d in &errors {
            eprintln!("{}", d.human(&root));
        }
        return Err(refused("the project does not compile, so nothing was sent to Open Cloud".to_owned()));
    }
    let index = dev::log_index(&session);
    let script = editor::dir(&root).join("hexagon.luau");
    std::fs::write(&script, bundle(&runtime, &run_request(mode, filter)))
        .map_err(|e| refused(format!("{}: {e}", script.display())))?;
    let mut tessera = Command::new("tessera");
    tessera.current_dir(&root).args(["run", "--json", "--logs", "--script"]).arg(&script);
    if let Some(timeout) = options.timeout {
        tessera.arg("--timeout").arg(format!("{timeout}s"));
    }
    if let Some(environment) = &cloud.environment {
        tessera.args(["--environment", environment]);
    }
    tessera.args(["--fresh", cloud.place.as_deref().unwrap_or("main")]);
    // its progress and errors go straight to the terminal
    let run = tessera.stderr(Stdio::inherit()).output().map_err(|e| refused(format!("could not run tessera: {e}")))?;
    let failed = |error| failed(json, error);
    if !run.status.success() {
        return failed(format!("tessera failed ({})", run.status));
    }
    let Ok(result) = serde_json::from_slice::<Value>(&run.stdout) else {
        return failed("tessera did not answer like `tessera run --json`".to_owned());
    };
    let report = result["results"][0].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
    let Some(mut report) = report.filter(Value::is_object) else {
        return failed("the run returned no hexagon report".to_owned());
    };
    dev::map_strings(&mut report, &index);
    let logs = result["logs"].as_array().into_iter().flatten().filter_map(Value::as_str);
    let output: Vec<String> = logs.map(|line| dev::map_log(line, &index).0).collect();
    let ok = report["ok"].as_bool() == Some(true);
    if json {
        let output: Vec<Value> = output.iter().map(|text| json!({ "kind": "output", "text": text })).collect();
        let answer = json!({
            "ok": true, "realm": "cloud", "environment": result["environment"], "place": result["place"],
            "placeId": result["placeId"], "version": result["version"], "output": output, "report": report,
        });
        println!("{answer}");
        return Ok(ok);
    }
    for line in &output {
        println!("{line}");
    }
    let ran = format!(
        "{}/{} v{}",
        result["environment"].as_str().unwrap_or("?"),
        result["place"].as_str().unwrap_or("?"),
        result["version"]
    );
    // the script's own chunk has a name of Open Cloud's choosing: the frames shown are the project's files
    let files: Vec<&str> = index.values().map(|(file, _)| file.as_str()).collect();
    let shown = |line: &str| files.iter().any(|file| line.strip_prefix(file).is_some_and(|rest| rest.starts_with(':')));
    print_report(&report, mode, &ran, shown);
    Ok(ok)
}

/// hexagon's in-engine runtime from the project's `node_modules`, by module name: sent along as the exec's modules
/// in Studio, bundled into one script on Open Cloud.
fn runtime(root: &Path) -> Result<Vec<(String, String)>, String> {
    let dir = root.join("node_modules/@hautajoki/hexagon/runtime");
    let entries = std::fs::read_dir(&dir)
        .map_err(|_| format!("this project has no hexagon runtime ({} is missing)", dir.display()))?;
    let mut modules = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".luau")) else {
            continue;
        };
        if name != "local_runner" {
            let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            modules.push((name.to_owned(), source));
        }
    }
    modules.sort();
    Ok(modules)
}

/// The argument of hexagon's `cloud_runner.run`.
fn run_request(mode: &str, filter: Option<&str>) -> String {
    format!("{{ mode = \"{mode}\", filter = {} }}", filter.map_or_else(|| "nil".to_owned(), luau_string))
}

/// One script holding hexagon's runtime and a run of it (`tessera run --script`), returning the report as JSON
/// text. Each module is a function of the `require` its string requires (`./name`) go through; anything else, a
/// spec's instance, is required as usual.
fn bundle(runtime: &[(String, String)], request: &str) -> String {
    let mut out = String::from(
        "-- hexagon's runtime and one run of it, written by `opossum test --cloud` for `tessera run --script`\n\
         local modules: { [string]: (any) -> any } = {}\n",
    );
    for (name, source) in runtime {
        out.push_str(&format!("modules[{}] = function(require)\n{source}\nend\n", luau_string(name)));
    }
    out.push_str(&format!(
        "local loaded: {{ [string]: any }} = {{}}
local outer = require
local function load(name: string): any
\tif loaded[name] == nil then
\t\tlocal body = modules[name] or error(`hexagon's runtime has no module {{name}}`)
\t\tloaded[name] = body(function(target: any): any
\t\t\tif type(target) == \"string\" then return load((string.gsub(target, \"^%./\", \"\"))) end
\t\t\treturn outer(target)
\t\tend)
\tend
\treturn loaded[name]
end
return load(\"cloud_runner\").run({request})
"
    ));
    out
}

/// Prints a hexagon report: each benchmark, each failure with its place and the first frames of its trace that are
/// `shown`, then the counts and where it `ran`.
fn print_report(report: &Value, mode: &str, ran: &str, shown: impl Fn(&str) -> bool) {
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
        for line in failure["trace"].as_str().unwrap_or("").lines().filter(|l| shown(l)).take(3) {
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
        "{pass} passed, {fail} failed, {skip} skipped in {:.2} s ({ran})",
        report["duration_seconds"].as_f64().unwrap_or(0.0)
    );
}

/// Starts a playtest (Play Solo, or with `run` Run) and returns once its DataModels connected.
pub fn play(cfg: &Config, options: &Options, run: bool) -> Result<bool, String> {
    let answer = post(cfg, options, "/play", json!({ "run": run }))?;
    if options.json {
        println!("{answer}");
        return Ok(answer["ok"] == true);
    }
    let realms: Vec<&str> = answer["realms"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if answer["ok"] != true {
        let connected = if realms.is_empty() { "none".to_owned() } else { realms.join(", ") };
        eprintln!("opossum: {} (connected: {connected})", answer["error"].as_str().unwrap_or("failed"));
        return Ok(false);
    }
    let seconds = answer["ms"].as_f64().unwrap_or(0.0) / 1000.0;
    println!("playtest running: {}, generation {} ({seconds:.1} s)", realms.join(", "), answer["gen"]);
    if answer["fresh"] == false {
        eprintln!(
            "opossum: the play server holds an older build than the files (restart the playtest, or turn on hotReload)"
        );
    }
    print_stale(&answer);
    Ok(true)
}

/// Ends the running playtest and prints what it logged: counts, then the first error lines.
pub fn stop(cfg: &Config, options: &Options) -> Result<bool, String> {
    let answer = post(cfg, options, "/stop", json!({}))?;
    if options.json {
        println!("{answer}");
        return Ok(answer["ok"] == true);
    }
    if answer["ok"] != true {
        eprintln!("opossum: {}", answer["error"].as_str().unwrap_or("failed"));
        return Ok(false);
    }
    let count = |key: &str, word: &str| {
        let n = answer[key].as_u64().unwrap_or(0);
        format!("{n} {word}{}", if n == 1 { "" } else { "s" })
    };
    println!("playtest stopped: {}, {}", count("errors", "error"), count("warnings", "warning"));
    for line in answer["first"].as_array().into_iter().flatten() {
        let text = line["text"].as_str().unwrap_or("").replace('\n', "\n    ");
        println!("  [{}] {text}", line["realm"].as_str().unwrap_or("?"));
    }
    Ok(true)
}

/// Writes the instance at `path` (`Workspace.Boat`, or `Workspace/Boat` when a name holds a dot) from Studio into the
/// files as an `.rbxm`: `file`, or where `place/` reads it from.
pub fn pull(cfg: &Config, options: &Options, path: &str, file: Option<&Path>) -> Result<bool, String> {
    let slashed = if path.contains('/') { path.to_owned() } else { path.replace('.', "/") };
    let mut body = json!({ "path": slashed });
    if let Some(file) = file {
        let file = std::path::absolute(file).map_err(|e| format!("{}: {e}", file.display()))?;
        body["file"] = file.to_string_lossy().into();
    }
    let answer = post(cfg, options, "/pull", body)?;
    if options.json {
        println!("{answer}");
        return Ok(true);
    }
    let kb = answer["bytes"].as_f64().unwrap_or(0.0) / 1024.0;
    let class = answer["class"].as_str().unwrap_or("?");
    let file = answer["file"].as_str().unwrap_or("?");
    println!("pulled {path} ({class}, {kb:.1} KB) into {file}, generation {}", answer["gen"]);
    if let Some(warning) = answer["warning"].as_str() {
        eprintln!("opossum: warning: {warning}");
    }
    Ok(true)
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

/// One line naming the files whose source did not compile, so the build the DataModel holds has their last good
/// output, or none: whatever ran there met older code of them than the files.
fn print_stale(answer: &Value) {
    const SHOWN: usize = 5;
    let files: Vec<&str> = answer["stale"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if files.is_empty() {
        return;
    }
    let mut list = files[..files.len().min(SHOWN)].join(", ");
    if files.len() > SHOWN {
        list.push_str(&format!(" and {} more", files.len() - SHOWN));
    }
    let (n, s) = (files.len(), if files.len() == 1 { "" } else { "s" });
    eprintln!(
        "opossum: warning: {n} file{s} did not compile, so Studio holds their last good output (or none): {list}"
    );
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
fn post(cfg: &Config, options: &Options, path: &str, body: Value) -> Result<Value, String> {
    send(cfg, options, path, body).map_err(|error| refused(options.json, error))
}

fn send(cfg: &Config, options: &Options, path: &str, mut body: Value) -> Result<Value, String> {
    body["root"] = cfg.root.to_string_lossy().into();
    if let Some(realm) = &options.realm {
        body["realm"] = realm.as_str().into();
    }
    if let Some(timeout) = options.timeout {
        body["timeout"] = timeout.into();
    }
    let port = port(cfg)?;
    // dev bounds the wait itself: its fence, then the time limit and some slack
    let limit = Duration::from_secs_f64(options.timeout.unwrap_or(60.0) + 180.0);
    let (status, answer) = request(port, path, &body, limit)?;
    if status != 200 {
        return Err(answer["error"].as_str().map_or_else(|| answer.to_string(), str::to_owned));
    }
    Ok(answer)
}

/// The port of this project's dev, as it wrote it to `.opossum/port`: the dev every command posts to.
fn port(cfg: &Config) -> Result<u16, String> {
    let file = editor::dir(&cfg.root).join("port");
    std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| format!("opossum dev is not running here ({} is missing)", shown(&cfg.root, &file)))
}

/// One `POST` of `body` to whatever listens on `port`: its status and JSON answer.
fn request(port: u16, path: &str, body: &Value, limit: Duration) -> Result<(u16, Value), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200))
        .map_err(|_| format!("opossum dev is not running (nothing answers on port {port})"))?;
    let _ = stream.set_read_timeout(Some(limit));
    let body = body.to_string();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let io = |e: std::io::Error| format!("opossum dev on port {port}: {e}");
    stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes())).map_err(io)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(io)?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("port {port} did not answer like opossum"))?;
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let answer: Value =
        serde_json::from_str(body).map_err(|_| format!("port {port} did not answer like opossum dev"))?;
    Ok((status, answer))
}

/// Every opossum dev on the sync ports, as its `/status` answers, port order. Other servers there are skipped.
fn devs() -> Vec<Value> {
    let ports = sync::DEFAULT_PORT..sync::DEFAULT_PORT + sync::PORTS;
    let answers = ports.filter_map(|port| request(port, "/status", &json!({}), Duration::from_secs(2)).ok());
    answers.filter(|(status, answer)| *status == 200 && answer["port"].is_u64()).map(|(_, answer)| answer).collect()
}

/// The realms connected to a dev, by its status.
fn realms(dev: &Value) -> Vec<&str> {
    dev["connected"].as_array().into_iter().flatten().filter_map(|c| c["realm"].as_str()).collect()
}

fn port_of(dev: &Value) -> u16 {
    dev["port"].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(0)
}

/// This project's dev among `devs`: the one on the port `.opossum/port` names, which exec, test, play and the rest
/// post to. Another dev serving the same root (started later or earlier in the same place) is not it.
fn here<'a>(cfg: &Config, devs: &'a [Value]) -> Option<&'a Value> {
    let port = port(cfg).ok()?;
    devs.iter().find(|d| port_of(d) == port && serves(d, &cfg.root))
}

/// Whether `dev` serves the project at `root`.
fn serves(dev: &Value, root: &Path) -> bool {
    let same = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    dev["root"].as_str().is_some_and(|r| same(Path::new(r)) == same(root))
}

/// Lists every opossum dev on the sync ports with the DataModels it holds, marking this project's. Exit status 0
/// when Studio's edit DataModel syncs from this project's dev, 1 when it does not, 2 when this project has no dev.
pub fn status(cfg: &Config, options: &Options) -> Result<bool, String> {
    let devs = devs();
    let here = here(cfg, &devs);
    let attached = here.is_some_and(|d| realms(d).contains(&"edit"));
    if options.json {
        let port = here.map_or(Value::Null, |d| d["port"].clone());
        println!("{}", json!({ "ok": attached, "here": port, "devs": devs }));
    } else {
        let port = here.map(port_of);
        for dev in &devs {
            let mark = match (Some(port_of(dev)) == port, serves(dev, &cfg.root)) {
                (true, _) => " (this project)".to_owned(),
                // the commands go to the dev .opossum/port names; attach moves Studio off this one
                (false, true) => format!(" (this project too, but its commands go to port {})", port.unwrap_or(0)),
                _ => String::new(),
            };
            println!(
                "port {}, pid {}: {} ({}), generation {}{mark}",
                dev["port"],
                dev["pid"],
                dev["project"].as_str().unwrap_or("?"),
                dev["root"].as_str().unwrap_or("?"),
                dev["gen"]
            );
            for c in dev["connected"].as_array().into_iter().flatten() {
                println!("  {}: place {}, generation {}", c["realm"].as_str().unwrap_or("?"), c["place"], c["gen"]);
            }
            if dev["connected"].as_array().is_none_or(Vec::is_empty) {
                println!("  no Studio connected");
            }
            let execs = dev["execs"].as_u64().unwrap_or(0);
            if execs > 0 {
                println!("  {execs} exec{} running", if execs == 1 { "" } else { "s" });
            }
        }
        if devs.is_empty() {
            println!("no opossum dev is running");
        }
    }
    if here.is_none() {
        return Err("opossum dev is not running here".to_owned());
    }
    Ok(attached)
}

/// Moves Studio to this project's dev: the dev holding it tells its plugins to connect to this one's port, and this
/// returns once edit connected here (DESIGN.md, Attach). With `force`, a running playtest moves too and a running
/// exec loses its answer.
pub fn attach(cfg: &Config, options: &Options, force: bool) -> Result<bool, String> {
    let start = Instant::now();
    let json = options.json;
    let refused = |error| refused(json, error);
    let devs = devs();
    let here = here(cfg, &devs).ok_or_else(|| refused("opossum dev is not running here".to_owned()))?;
    let port = port_of(here);
    let project = here["project"].as_str().unwrap_or("?");
    if realms(here).contains(&"edit") {
        if json {
            println!("{}", json!({ "ok": true, "port": port, "from": [], "realms": realms(here), "ms": 0 }));
        } else {
            println!("Studio is already attached to {project} on port {port}");
        }
        return Ok(true);
    }
    // another dev of this project too: Studio moves off it to the one the other commands reach
    let holders: Vec<&Value> = devs.iter().filter(|d| port_of(d) != port && !realms(d).is_empty()).collect();
    if holders.is_empty() {
        return Err(refused(
            "no opossum dev holds Studio (is the place open, with the opossum plugin installed?)".to_owned(),
        ));
    }
    // the plugin checks the place in this dev's hello: one it would refuse goes back to the first dev on the ports
    let place = |c: &Value| (c["realm"] == "edit").then(|| c["place"].as_u64()).flatten();
    let edit = holders.iter().flat_map(|d| d["connected"].as_array().into_iter().flatten()).find_map(place);
    if let Some(open) = edit
        && here["place"].as_u64() != Some(open)
        && !(open == 0 && here["unpublished"] == true)
    {
        let serves =
            if here["unpublished"] == true { "unpublished places".to_owned() } else { here["place"].to_string() };
        return Err(refused(format!("Studio has place {open} open, and {project} syncs {serves}")));
    }
    let mut moved: Vec<String> = Vec::new();
    let mut from = Vec::new();
    for dev in &holders {
        let (at, root) = (port_of(dev), dev["root"].as_str().unwrap_or("?"));
        let body = json!({ "port": port, "force": force });
        let (status, answer) = request(at, "/release", &body, Duration::from_secs(30)).map_err(refused)?;
        let error = answer["error"].as_str().unwrap_or("it did not answer");
        if status != 200 {
            return Err(refused(format!("the opossum dev of {root} on port {at} keeps Studio: {error}")));
        }
        if answer["ok"] != true {
            return failed(json, format!("the opossum dev of {root} on port {at}: {error}"));
        }
        moved.extend(answer["realms"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
        from.push(json!({ "port": at, "root": root, "pid": dev["pid"] }));
    }
    // the plugins connect here: edit and, when a playtest moved, its DataModels
    let limit = Duration::from_secs_f64(options.timeout.unwrap_or(60.0));
    loop {
        let status = request(port, "/status", &json!({}), Duration::from_secs(2)).map(|(_, s)| s);
        let connected: Vec<String> = status.iter().flat_map(|s| realms(s)).map(str::to_owned).collect();
        if moved.iter().all(|r| connected.contains(r)) {
            break;
        }
        if start.elapsed() > limit {
            let missing: Vec<&str> = moved.iter().filter(|r| !connected.contains(r)).map(String::as_str).collect();
            let error = format!(
                "Studio left, but {} did not connect here within {} s",
                missing.join(", "),
                limit.as_secs_f64()
            );
            return failed(json, error);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    if json {
        println!("{}", json!({ "ok": true, "port": port, "from": from, "realms": moved, "ms": ms }));
        return Ok(true);
    }
    let left: Vec<String> =
        from.iter().map(|f| format!("{} on port {}", f["root"].as_str().unwrap_or("?"), f["port"])).collect();
    println!(
        "attached Studio ({}) to {project} on port {port}, from {} ({:.2} s)",
        moved.join(", "),
        left.join(" and "),
        ms / 1000.0
    );
    Ok(true)
}

/// An error that never ran (exit status 2), which `--json` prints as `{"ok": false, "ran": false, "error"}` too.
fn refused(json: bool, error: String) -> String {
    if json {
        println!("{}", json!({ "ok": false, "ran": false, "error": error }));
    }
    error
}

/// An `ok: false` answer (exit status 1): it was asked, and did not come to pass.
fn failed(json: bool, error: String) -> Result<bool, String> {
    if json {
        println!("{}", json!({ "ok": false, "error": error }));
    } else {
        eprintln!("opossum: {error}");
    }
    Ok(false)
}

fn shown(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).display().to_string()
}

/// `text` as a Luau string literal.
pub fn luau_string(text: &str) -> String {
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
