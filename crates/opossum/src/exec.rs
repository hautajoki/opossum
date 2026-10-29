//! `opossum exec`, `test` and `bench`: Luau run where a [`Target`] says. In Studio it runs in a DataModel through the
//! running `opossum dev`, one `POST /exec` on the port it wrote to `.opossum/port` (DESIGN.md, Exec); `test --local`
//! runs the local suites in fresh in-process VMs, with no dev or Studio (DESIGN.md, Tests); `test --cloud` runs the specs on Open Cloud
//! through tessera (DESIGN.md, Cloud tests). Every target runs the same spec runtime (`spec`) and prints its report
//! the same way. The exit status is 0 when it ran clean, 1 for a script error, a failed or empty test run or an
//! answer that never came, 2 when it never ran (dev or the DataModel cannot be reached, or holds older code than a
//! test allows). `opossum play` and `stop` start and end a playtest the same way (`POST /play`, `/stop`; DESIGN.md,
//! Play), and `opossum pull` writes an instance from Studio into the files (`POST /pull`; DESIGN.md, Pull).
//! `opossum status` and `attach` ask every dev on the sync ports what it holds and move Studio between them
//! (`POST /status`, `/release`; DESIGN.md, Attach).

use std::io::{BufRead, IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use spec::{Host, Mode};

use crate::build::Session;
use crate::config::Config;
use crate::diag::Diag;
use crate::{dev, editor};

/// The longest a run on Open Cloud may take, in seconds (tessera's and Open Cloud's limit).
const CLOUD_TIMEOUT: f64 = 300.0;
/// How long one file of a local test run may take by default, in seconds; a local bench run has no limit by default.
const LOCAL_TEST_TIMEOUT: f64 = 300.0;

pub enum Source {
    File(String),
    Code(String),
    Stdin,
}

pub struct Options {
    pub json: bool,
    pub timeout: Option<f64>,
}

/// Where `exec`, `test` and `bench` run. Each target holds only its own flags, so a stray combination never parses.
pub enum Target {
    /// A DataModel `opossum dev` is connected to: `realm`, or dev's default.
    Studio { realm: Option<String> },
    /// Fresh Luau VMs in this process, no DataModel (test and bench only); `interpreted` turns native code off.
    Local { interpreted: bool },
    /// Open Cloud through tessera (test and bench only): its logical place and environment, its defaults when None.
    Cloud { place: Option<String>, environment: Option<String> },
}

/// What a `test` or `bench` run runs, wherever it runs.
pub struct Tests {
    pub mode: Mode,
    /// Selects cases by a case-insensitive substring of their labels joined by ` / `.
    pub filter: Option<String>,
    /// A run focused by `.only` passes instead of exiting 1.
    pub allow_focused: bool,
}

/// Runs Luau in the DataModel of `realm` (dev's default when None) and prints what it printed and returned.
pub fn exec(cfg: &Config, options: &Options, realm: &Option<String>, source: Source) -> Result<bool, String> {
    let (name, code) = read(source)?;
    let answer = post(cfg, options, "/exec", with_realm(json!({ "name": name, "source": code }), realm))?;
    Ok(print_answer(options, &answer))
}

/// The code to run: the name its errors give it, and its text.
fn read(source: Source) -> Result<(String, String), String> {
    match source {
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
            Ok((name, code))
        }
        Source::Code(code) => Ok(("exec".to_owned(), code)),
        Source::Stdin => {
            let mut code = String::new();
            std::io::stdin().read_to_string(&mut code).map_err(|e| format!("stdin: {e}"))?;
            Ok(("stdin".to_owned(), code))
        }
    }
}

/// Prints an exec's answer: the object with `--json`, else what it printed, then what it returned (strings as they
/// are, other values as JSON), then its error. Returns whether it ran clean.
fn print_answer(options: &Options, answer: &Value) -> bool {
    if options.json {
        println!("{answer}");
        return answer["ok"].as_bool() == Some(true);
    }
    print_output(answer);
    for value in answer["values"].as_array().into_iter().flatten() {
        match value {
            Value::String(text) => println!("{text}"),
            _ => println!("{value}"),
        }
    }
    let ok = print_error(answer);
    print_stale(answer);
    ok
}

/// Runs the project's local suites (`*.local.spec.luau`, bench: `*.local.bench.luau`) in fresh VMs, one per file,
/// and prints the report: what each file printed after its name.
pub fn test_local(root: &Path, options: &Options, interpreted: bool, tests: &Tests) -> Result<bool, String> {
    let default = (tests.mode == Mode::Test).then_some(LOCAL_TEST_TIMEOUT);
    let local = spec::LocalOptions {
        mode: tests.mode,
        filter: tests.filter.clone(),
        timeout: options.timeout.or(default).map(Duration::from_secs_f64),
        codegen: !interpreted,
    };
    let (report, outputs) = spec::run_local(root, &local).map_err(|e| refused(options.json, e))?;
    let output = outputs.into_iter().map(|(file, text)| json!({ "kind": "output", "file": file, "text": text }));
    let run = Run {
        target: "local",
        ran: if interpreted { "local, interpreted" } else { "local" }.to_owned(),
        about: json!({ "interpreted": interpreted }),
        output: output.collect(),
        report,
    };
    Ok(finish(options, tests, run, &spec::local::project_frame))
}

/// The spec runtime in the DataModel dev holds, over the suites the files route there: the report comes back mapped
/// to the files by dev. Specs never pass against older code than the files: a playtest DataModel holding it refuses.
pub fn test_studio(cfg: &Config, options: &Options, realm: &Option<String>, tests: &Tests) -> Result<bool, String> {
    let routes = route::Routes::new(cfg.routes.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .map_err(|e| refused(options.json, e))?;
    let suites = suites(&route::scan(&cfg.src(), &routes), &cfg.root, tests.mode);
    let request = spec::Request { suites: &suites, mode: tests.mode, filter: tests.filter.as_deref(), focus: None };
    let source = spec::bundle(Host::Roblox, &request);
    let body = json!({ "name": "spec", "source": source, "report": true, "fresh": true });
    let mut answer = post(cfg, options, "/exec", with_realm(body, realm))?;
    let Some(report) = Some(answer["values"][0].take()).filter(Value::is_object) else {
        if options.json {
            if let Some(object) = answer.as_object_mut() {
                object.remove("values");
                object.insert("report".into(), Value::Null);
            }
            println!("{answer}");
        } else {
            print_output(&answer);
            print_error(&answer);
            print_stale(&answer);
        }
        return Ok(false);
    };
    let realm = answer["realm"].as_str().unwrap_or("?");
    let run = Run {
        target: "studio",
        ran: format!("{realm}, generation {}", answer["gen"]),
        about: json!({ "realm": realm, "gen": answer["gen"], "stale": answer["stale"], "ms": answer["ms"] }),
        output: answer["output"].as_array().cloned().unwrap_or_default(),
        report,
    };
    let ok = finish(options, tests, run, &|line| project_line(&cfg.root, line));
    if !options.json {
        print_stale(&answer);
    }
    Ok(ok)
}

/// Runs the specs on Roblox's Open Cloud Luau execution instead of Studio: `tessera run --fresh` builds the place
/// with its configured build command, saves it as a place version (or reuses an identical saved one) and runs one
/// script holding the spec runtime and this build's suites there. The project is built here too, first: a build
/// with errors never reaches Roblox, and its log index maps the report like Studio's. A suite the place tessera built
/// does not hold fails to load.
pub fn test_cloud(
    cfg: Config,
    options: &Options,
    place: Option<&str>,
    environment: Option<&str>,
    tests: &Tests,
) -> Result<bool, String> {
    let json = options.json;
    let refused = |error| refused(json, error);
    if options.timeout.is_some_and(|t| t > CLOUD_TIMEOUT) {
        return Err(refused(format!("--timeout with --cloud is at most {CLOUD_TIMEOUT} s, Open Cloud's limit")));
    }
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
    let suites = suites(&session.scan, &root, tests.mode);
    let request = spec::Request { suites: &suites, mode: tests.mode, filter: tests.filter.as_deref(), focus: None };
    let script = editor::dir(&root).join("spec.luau");
    std::fs::write(&script, spec::bundle(Host::Roblox, &request))
        .map_err(|e| refused(format!("{}: {e}", script.display())))?;
    let mut tessera = Command::new("tessera");
    tessera.current_dir(&root).args(["run", "--json", "--logs", "--script"]).arg(&script);
    if let Some(timeout) = options.timeout {
        tessera.arg("--timeout").arg(format!("{timeout}s"));
    }
    if let Some(environment) = environment {
        tessera.args(["--environment", environment]);
    }
    tessera.args(["--fresh", place.unwrap_or("main")]);
    // its progress and errors are shown as they come, mapped: a failed run's logs and error are only ever there
    let mut child = tessera
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| refused(format!("could not run tessera: {e}")))?;
    let errors = std::io::BufReader::new(child.stderr.take().expect("piped"));
    let run = std::thread::scope(|s| {
        s.spawn(|| {
            errors.lines().map_while(Result::ok).for_each(|line| eprintln!("{}", dev::map_log(&line, &index).0))
        });
        child.wait_with_output()
    });
    let failed = |error| failed(json, error);
    let run = match run {
        Ok(run) => run,
        Err(e) => return failed(format!("tessera: {e}")),
    };
    if !run.status.success() {
        return failed(format!("tessera failed ({})", run.status));
    }
    let Ok(result) = serde_json::from_slice::<Value>(&run.stdout) else {
        return failed("tessera did not answer like `tessera run --json`".to_owned());
    };
    let report = result["results"][0].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
    let Some(mut report) = report.filter(Value::is_object) else {
        return failed("the run returned no report".to_owned());
    };
    dev::map_strings(&mut report, &index);
    let logs = result["logs"].as_array().into_iter().flatten().filter_map(Value::as_str);
    let output = logs.map(|line| json!({ "kind": "output", "text": dev::map_log(line, &index).0 }));
    let (environment, place, version) = (&result["environment"], &result["place"], &result["version"]);
    let run = Run {
        target: "cloud",
        ran: format!("{}/{} v{version}", environment.as_str().unwrap_or("?"), place.as_str().unwrap_or("?")),
        about: json!({ "environment": environment, "place": place, "placeId": result["placeId"], "version": version }),
        output: output.collect(),
        report,
    };
    // the script's own chunk has a name of Open Cloud's choosing: the frames shown are the project's files
    Ok(finish(options, tests, run, &|line| project_line(&root, line)))
}

/// The suites of a scan for `mode`: every ModuleScript named `*.spec` (`*.bench`), labelled by its source file and
/// found by its instance path, in file order.
fn suites(scan: &route::Scan, root: &Path, mode: Mode) -> Vec<spec::Suite> {
    let suffix = match mode {
        Mode::Test => ".spec",
        Mode::Bench => ".bench",
    };
    let suites = scan.modules.iter().filter(|m| m.class == route::Class::ModuleScript);
    let suites = suites.filter(|m| m.instance.last().is_some_and(|name| name.ends_with(suffix)));
    let mut suites: Vec<spec::Suite> =
        suites.map(|m| spec::Suite { label: shown(root, &m.file), path: m.instance.clone() }).collect();
    suites.sort_by(|a, b| a.label.cmp(&b.label));
    suites
}

/// Whether a trace line, mapped, is a frame of one of the project's files (`src/x.ts:12 function f`).
fn project_line(root: &Path, line: &str) -> bool {
    line.split_once(':').is_some_and(|(file, _)| !file.is_empty() && root.join(file).is_file())
}

/// A finished run, whatever its target: `ran` says where for the text (`edit, generation 12`), `about` for JSON.
struct Run {
    target: &'static str,
    ran: String,
    about: Value,
    /// What it printed, `{kind, text}` and for a local file's whole output `{kind, file, text}`.
    output: Vec<Value>,
    report: Value,
}

/// Prints what the run printed and its report, `project` keeping a trace's project frames, and judges it by the policy
/// every target shares: false when it selected nothing, was focused without `--allow-focused`, or a case failed.
fn finish(options: &Options, tests: &Tests, run: Run, project: &dyn Fn(&str) -> bool) -> bool {
    let verdict = spec::policy(&run.report, tests.allow_focused);
    if options.json {
        let mut answer = json!({
            "ok": verdict.is_none(), "target": run.target, "ran": run.about, "output": run.output, "report": run.report,
        });
        if let Some((code, error)) = &verdict {
            answer["code"] = (*code).into();
            answer["error"] = error.as_str().into();
        }
        println!("{answer}");
        return verdict.is_none();
    }
    for line in &run.output {
        let text = line["text"].as_str().unwrap_or("");
        match (line["file"].as_str(), line["kind"].as_str()) {
            (Some(file), _) => text.lines().for_each(|text| println!("{file}: {text}")),
            (None, Some("warning")) => println!("warning: {text}"),
            _ => println!("{text}"),
        }
    }
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    print!("{}", spec::print(&run.report, &run.ran, project, color));
    if let Some(("test.focused", message)) = verdict.as_ref().map(|(code, message)| (*code, message)) {
        eprintln!("opossum: {message}");
    }
    verdict.is_none()
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

/// `body` asking for `realm`, when one is named.
fn with_realm(mut body: Value, realm: &Option<String>) -> Value {
    if let Some(realm) = realm {
        body["realm"] = realm.as_str().into();
    }
    body
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
