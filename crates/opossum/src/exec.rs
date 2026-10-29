//! `opossum exec`, `test` and `bench`: Luau run where a [`Target`] says. In Studio it runs in a DataModel through the
//! running `opossum dev`, one `POST /exec` on the port it wrote to `.opossum/port` (DESIGN.md, Exec); `--local` runs
//! it in fresh in-process VMs, with no dev or Studio (DESIGN.md, Tests); `--cloud` runs it on Open Cloud, on the saved
//! place version holding this build (DESIGN.md, Cloud runs). Every target runs the same spec runtime (`spec`) and
//! prints its report the same way. The exit status is 0 when it ran clean, 1 for a script error, a failed or empty test run or an
//! answer that never came, 2 when it never ran (dev or the DataModel cannot be reached, or holds older code than a
//! test allows). `opossum play` and `stop` start and end a playtest the same way (`POST /play`, `/stop`; DESIGN.md,
//! Play), and `opossum pull` writes an instance from Studio into the files (`POST /pull`; DESIGN.md, Pull).
//! `opossum status` and `attach` ask every dev on the sync ports what it holds and move Studio between them
//! (`POST /status`, `/release`; DESIGN.md, Attach).

use std::io::{IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use cloud::Client;
use serde_json::{Value, json};
use spec::{Host, Mode};

use crate::cloud_exec::{self, Placed, Ran};
use crate::config::Config;
use crate::guard::Cache;
use crate::{Outcome, dev, editor};

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
    /// Fresh Luau VMs in this process, no DataModel; `interpreted` turns native code off.
    Local { interpreted: bool },
    /// Open Cloud, on the saved version holding this build or, for an exec, the current head.
    Cloud(Cloud),
}

/// `--cloud`'s flags: the place and environment (the root place and the default environment when None), the profile
/// to build with, and `--head` (exec only: the current head, no build and no save).
pub struct Cloud {
    pub place: Option<String>,
    pub environment: Option<String>,
    pub profile: Option<String>,
    pub head: bool,
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
    let (_, name, code) = read(source)?;
    let body = with_realm(json!({ "name": name, "source": code }), realm);
    let answer = post_run(cfg, options, "/exec", body)?;
    Ok(print_answer(options, &answer))
}

/// Runs Luau in a fresh VM rooted at the project at `root` and prints what it printed and returned: its string
/// requires resolve from the file as the `luau` CLI's do, with no DataModel.
pub fn exec_local(root: &Path, options: &Options, interpreted: bool, source: Source) -> Result<bool, String> {
    let (file, name, code) = read(source)?;
    // the chunk stands where the file is, so its string requires resolve from there
    let chunk = file.map_or(name, |file| shown(root, &file));
    let answer = run_local(root, options, interpreted, &chunk, &code)?;
    Ok(print_answer(options, &answer))
}

/// Runs Luau on Open Cloud: on the saved version holding this build, built with `--profile` or the place's profile, or
/// on the current head with `--head`. Text prints what it returned (nothing for no value, a string as it is, another
/// value as JSON, several as one JSON array) and puts what it logged and its error on stderr.
pub fn exec_cloud(
    cfg: Config,
    client: &Client,
    cache: &Cache,
    options: &Options,
    run: &Cloud,
    source: Source,
) -> Outcome {
    let code = match read(source) {
        Ok((_, _, code)) => code,
        Err(error) => return Outcome::refused("exec.source", error),
    };
    let at = match cloud_exec::resolve(&cfg, run.environment.as_deref(), run.place.as_deref()) {
        Ok(at) => at,
        Err(error) => return Outcome::refused("config", error),
    };
    let built = if run.head {
        None
    } else {
        let profile = run.profile.as_deref().or(at.profile.as_deref());
        match cloud_exec::build(cfg, profile, &at.environment) {
            Ok(built) => Some(built),
            Err(failure) => return Outcome::refused(failure.code, failure.error),
        }
    };
    let index = built.as_ref().map(|(session, _)| dev::log_index(session)).unwrap_or_default();
    let note = built.as_ref().map_or("", |(session, _)| reproducible(session));
    let timeout = options.timeout.map_or(cloud::MAX_TASK_TIMEOUT, Duration::from_secs_f64);
    let rbxl = built.as_ref().map(|(_, rbxl)| rbxl.as_slice());
    let mut answer = json!({
        "ok": false, "realm": "cloud", "environment": at.environment, "place": at.place, "placeId": at.id,
    });
    let ran = match cloud_exec::run(client, cache, &at, rbxl, note, &code, timeout) {
        Ok(ran) => ran,
        Err(failure) if !failure.ran => return Outcome::refused(failure.code, failure.error),
        Err(failure) => {
            eprintln!("opossum: {}", failure.error);
            return Outcome::failed(answer, String::new(), failure.code, failure.error);
        }
    };
    let output: Vec<Value> = mapped_logs(&ran, &index);
    let failure = task_failure(&ran, &index);
    if !options.json {
        output.iter().for_each(|line| eprintln!("{}", line["text"].as_str().unwrap_or("")));
        if let Some((_, error)) = &failure {
            eprintln!("{error}");
        }
    }
    answer["ok"] = failure.is_none().into();
    answer["version"] = ran.version.into();
    answer["saved"] = ran.saved.into();
    answer["ms"] = ran.ms.into();
    answer["output"] = output.into();
    answer["values"] = ran.task.results.clone().into();
    if let Some((code, error)) = failure {
        answer["code"] = code.into();
        answer["error"] = error.into();
    }
    let exit = if answer["ok"] == true { 0 } else { 1 };
    Outcome { exit, json: answer, text: results_text(&ran.task.results) }
}

/// What a cloud exec prints on stdout: nothing for no value, one string as it is, one other value as
/// JSON, several as one compact JSON array; each but nothing ends its line.
fn results_text(values: &[Value]) -> String {
    match values {
        [] => String::new(),
        [Value::String(text)] => format!("{text}\n"),
        [value] => format!("{value}\n"),
        values => format!("{}\n", Value::Array(values.to_vec())),
    }
}

/// What a task logged, `{kind, text}` each, its places mapped to the files.
fn mapped_logs(ran: &Ran, index: &dev::LogIndex) -> Vec<Value> {
    let line = |text: &String| json!({ "kind": "output", "text": dev::map_log(text, index).0 });
    ran.logs.iter().map(line).collect()
}

/// A task that did not complete: its code (`cloud.timeout` when it ran out of time) and error, mapped.
fn task_failure(ran: &Ran, index: &dev::LogIndex) -> Option<(&'static str, String)> {
    let task = &ran.task;
    if task.state == "COMPLETE" {
        return None;
    }
    let mapped = |text: &str| text.lines().map(|line| dev::map_log(line, index).0).collect::<Vec<_>>().join("\n");
    Some(match &task.error {
        Some((code, message)) if code == "DEADLINE_EXCEEDED" => {
            ("cloud.timeout", format!("{code}: {}", mapped(message)))
        }
        Some((code, message)) => ("exec.failed", format!("{code}: {}", mapped(message))),
        None if !task.terminal() => ("cloud.timeout", format!("the task was still {} at its deadline", task.state)),
        None => ("exec.failed", format!("the task ended {}", task.state)),
    })
}

/// What a save of a build adds when its bytes can never match an earlier one's.
fn reproducible(session: &crate::build::Session) -> &'static str {
    if session.timestamped() { " (this build is not reproducible: $buildTime)" } else { "" }
}

/// The code to run: the file's absolute path for a file, the name its errors give it, and its text.
fn read(source: Source) -> Result<(Option<std::path::PathBuf>, String, String), String> {
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
            Ok((Some(std::path::absolute(file).map_err(|e| format!("{path}: {e}"))?), name, code))
        }
        Source::Code(code) => Ok((None, "exec".to_owned(), code)),
        Source::Stdin => {
            let mut code = String::new();
            std::io::stdin().read_to_string(&mut code).map_err(|e| format!("stdin: {e}"))?;
            Ok((None, "stdin".to_owned(), code))
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

/// `exec --local`: `code` as the chunk `chunk` (a path from `root`, or absolute outside it) in a fresh VM rooted at
/// the project, interrupted after `--timeout` when one is given. The answer has exec's shape, realm `local`.
fn run_local(root: &Path, options: &Options, interpreted: bool, chunk: &str, code: &str) -> Result<Value, String> {
    let start = Instant::now();
    let mut vm = luau::vm::Vm::new(root, !interpreted).map_err(|e| refused(options.json, e))?;
    let (done, finished) = mpsc::channel::<()>();
    let watchdog = options.timeout.map(|seconds| {
        let interrupter = vm.interrupter();
        std::thread::spawn(move || {
            if finished.recv_timeout(Duration::from_secs_f64(seconds)) == Err(RecvTimeoutError::Timeout) {
                interrupter.interrupt();
            }
        })
    });
    let result = vm.eval(chunk, &returning_json(code));
    drop(done);
    watchdog.map(std::thread::JoinHandle::join);
    let output: Vec<Value> = vm.take_output().lines().map(|text| json!({ "kind": "output", "text": text })).collect();
    let mut answer = json!({ "ok": result.is_ok(), "realm": "local", "ms": start.elapsed().as_secs_f64() * 1000.0, "output": output, "values": [] });
    match result {
        Ok(text) => answer["values"] = serde_json::from_str(&text).unwrap_or_default(),
        Err(error) => {
            let (error, trace) = error.split_once("\nstack traceback:\n").unwrap_or((&error, ""));
            // the splice's own frames, past the code's last line, are not the code's
            let last = code.lines().count().max(1);
            let line = |frame: &str| frame.strip_prefix(chunk)?.strip_prefix(':')?.split(' ').next()?.parse().ok();
            let trace: Vec<&str> = trace.lines().filter(|frame| line(frame).is_none_or(|n: usize| n <= last)).collect();
            answer["error"] = error.into();
            answer["trace"] = trace.join("\n").into();
        }
    }
    Ok(answer)
}

/// `code` made to return its results as one JSON array: its first code, past the comments and directives that must
/// stay first, opens a function on the same line, so line numbers stay the file's. Each result is JSON, or its
/// `tostring` when JSON has no word for it (a function, a cycle).
fn returning_json(code: &str) -> String {
    let open = "local __results = table.pack((function(...) ";
    let mut out = match first_code(code) {
        Some(at) => format!("{}{open}{}\nend)())\n", &code[..at], &code[at..]),
        None => format!("{code}\n{open}\nend)())\n"),
    };
    out.push_str(&format!("local __json = (function()\n{}\nend)()\n", spec::JSON));
    out.push_str(
        "local __values = table.create(__results.n)
for index = 1, __results.n do
\tlocal value = __results[index]
\tlocal ok, text = pcall(__json, value)
\t__values[index] = if ok then text else __json(tostring(value))
end
return `[{table.concat(__values, \",\")}]`
",
    );
    out
}

/// Where the first code of `source` starts, past blank space and comments (directives such as `--!native` are
/// comments); None when it holds none.
fn first_code(source: &str) -> Option<usize> {
    let mut at = 0;
    loop {
        let start = at + source[at..].find(|c: char| !c.is_ascii_whitespace())?;
        let rest = &source[start..];
        if !rest.starts_with("--") {
            return Some(start);
        }
        // a long comment, `--[==[ ... ]==]`, or a line comment
        let level = rest[2..].strip_prefix('[').and_then(|r| r.find(|c| c != '=').filter(|&n| r[n..].starts_with('[')));
        at = match level {
            Some(n) => {
                let close = format!("]{}]", "=".repeat(n));
                start + 4 + n + rest[4 + n..].find(&close)? + close.len()
            }
            None => start + rest.find('\n')? + 1,
        };
    }
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
    let mut answer = post_run(cfg, options, "/exec", with_realm(body, realm))?;
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

/// Runs the specs on Open Cloud instead of Studio: the project is built here, once, with `--profile` if given (a
/// place's own profile is for what it publishes, and may leave the specs out), refused with errors, saved as a place
/// version unless the cache names one holding the same bytes, and one task runs the spec runtime and this build's
/// suites on exactly that version. Its log index maps the report like Studio's.
pub fn test_cloud(
    cfg: Config,
    client: &Client,
    cache: &Cache,
    options: &Options,
    run: &Cloud,
    tests: &Tests,
) -> Result<bool, String> {
    let json = options.json;
    let refused = |code, error| crate::finish(Outcome::refused(code, error), json);
    let at = match cloud_exec::resolve(&cfg, run.environment.as_deref(), run.place.as_deref()) {
        Ok(at) => at,
        Err(e) => return refused("config", e),
    };
    let (session, rbxl) = match cloud_exec::build(cfg, run.profile.as_deref(), &at.environment) {
        Ok(built) => built,
        Err(f) => return refused(f.code, f.error),
    };
    let root = session.cfg.root.clone();
    let index = dev::log_index(&session);
    let suites = suites(&session.scan, &root, tests.mode);
    let request = spec::Request { suites: &suites, mode: tests.mode, filter: tests.filter.as_deref(), focus: None };
    let script = spec::bundle(Host::Roblox, &request);
    let timeout = options.timeout.map_or(cloud::MAX_TASK_TIMEOUT, Duration::from_secs_f64);
    let ran = match cloud_exec::run(client, cache, &at, Some(&rbxl), reproducible(&session), &script, timeout) {
        Ok(ran) => ran,
        Err(failure) if !failure.ran => return refused(failure.code, failure.error),
        Err(failure) => return cloud_failed(json, about(&at, None), Vec::new(), failure.code, failure.error),
    };
    let output = mapped_logs(&ran, &index);
    let about = about(&at, Some(&ran));
    if let Some((code, error)) = task_failure(&ran, &index) {
        return cloud_failed(json, about, output, code, error);
    }
    let report = ran.task.results.first().and_then(Value::as_str).and_then(|text| serde_json::from_str(text).ok());
    let Some(mut report) = report.filter(Value::is_object) else {
        return cloud_failed(json, about, output, "exec.failed", "the run returned no report".to_owned());
    };
    dev::map_strings(&mut report, &index);
    let run = Run { target: "cloud", ran: format!("{} v{}", at.label(), ran.version), about, output, report };
    // the script's own chunk has a name of Open Cloud's choosing: the frames shown are the project's files
    Ok(finish(options, tests, run, &|line| project_line(&root, line)))
}

/// A cloud run's `ran`: where it ran and, once a task started, the version and whether this run saved it.
fn about(at: &Placed, ran: Option<&Ran>) -> Value {
    let mut about = json!({ "environment": at.environment, "place": at.place, "placeId": at.id });
    if let Some(ran) = ran {
        about["version"] = ran.version.into();
        about["saved"] = ran.saved.into();
    }
    about
}

/// A cloud test run that brought back no report (exit status 1): what it logged, then why.
fn cloud_failed(json: bool, about: Value, output: Vec<Value>, code: &str, error: String) -> Result<bool, String> {
    if json {
        let answer = json!({
            "ok": false, "target": "cloud", "ran": about, "output": output, "report": null, "code": code, "error": error,
        });
        println!("{answer}");
    } else {
        output.iter().for_each(|line| println!("{}", line["text"].as_str().unwrap_or("")));
        eprintln!("opossum: {error}");
    }
    Ok(false)
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

/// [`post`] for code that runs in Studio by default: with no dev here, it also says where else it runs.
fn post_run(cfg: &Config, options: &Options, path: &str, body: Value) -> Result<Value, String> {
    if let Err(error) = port(cfg) {
        return Err(refused(options.json, format!("{error}; start `opossum dev`, or run with --local or --cloud")));
    }
    post(cfg, options, path, body)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloud_exec_prints_one_string_raw_and_anything_else_as_json() {
        // scripts parse this: a project's tooling driver reads one string result raw
        let text = |values: Value| results_text(values.as_array().unwrap());
        assert_eq!(text(json!([])), "");
        assert_eq!(text(json!(["a \"b\"\n"])), "a \"b\"\n\n");
        assert_eq!(text(json!([1.5])), "1.5\n");
        assert_eq!(text(json!([1, "a", { "k": [true] }])), "[1,\"a\",{\"k\":[true]}]\n");
    }

    #[test]
    fn local_exec_keeps_lines_directives_and_every_result() {
        let root = std::env::temp_dir().join(format!("opossum-exec-local-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/m.luau"), "return { n = 2 }").unwrap();
        let options = Options { json: true, timeout: None };
        let run = |code: &str| run_local(&root, &options, false, "src/x.luau", code).unwrap();
        // comments and directives stay before the spliced function; a `./` require resolves from the file
        let code =
            "--!native\n--[==[ a\n]] still ]==] -- the code\nprint(...)\nreturn require('./m').n, nil, 'a', print";
        let answer = run(code);
        let values = json!([2, null, "a", answer["values"][3].as_str().filter(|f| f.starts_with("function"))]);
        assert_eq!(
            (&answer["ok"], &answer["values"], &answer["output"]),
            (&true.into(), &values, &json!([{ "kind": "output", "text": "" }]))
        );
        // an error names the file's own line
        let answer = run("-- one\n\nerror('boom')");
        assert_eq!(
            (&answer["error"], &answer["trace"]),
            (&"src/x.luau:3: boom".into(), &"[C] function error\nsrc/x.luau:3".into())
        );
        assert_eq!(run("-- nothing to run")["values"], json!([]));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
