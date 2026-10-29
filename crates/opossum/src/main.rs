//! opossum: one binary from a TypeScript + Luau Roblox project to a Studio session or a place. See DESIGN.md.

mod assets;
mod build;
mod cloud_exec;
mod config;
mod deploy;
mod dev;
mod diag;
mod editor;
mod event;
mod exec;
mod guard;
mod ids;
mod inputs;
mod legacy;
mod lock;
mod migrate;
mod packages;
mod plan;
mod resources;
mod signals;
mod upload;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use build::Session;
use cloud::{Client, Clock};
use config::Config;
use diag::Diag;
use guard::{Cache, RemoteGuard};
use serde_json::{Value, json};

#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

const USAGE: &str = "usage: opossum [-C <dir>] <command>                          flags go anywhere; -h prints this
  build [--profile <name>] [--environment <e>] [-o <file.rbxl>] [--emit <dir>]
                                                             build the place (.opossum/place.rbxl)
  check [--profile <name>] [--environment <e>] [--json]      diagnostics, no output
  dev [--environment <e>] [--json]                           watch, rebuild and sync to Studio
  exec [<target>] [--timeout <s>] [--json] (<file.luau> | -e <code> | -)
                                                             run Luau where the target says
  test [<target>] [--timeout <s>] [--allow-focused] [--json] [<filter>]
                                                             run the specs (bench: the benchmarks)
    <target>: [--realm <r>]                                  in Studio through opossum dev (the default)
              --local [--interpreted]                        in fresh Luau VMs here, src/**/*.local.spec.luau
              --cloud [--place <p>] [--environment <e>] [--profile <name>] [--head]
                                                             on Open Cloud, on the place version holding this
                                                             build (saved to Roblox unless one already is);
                                                             exec --head: on the current head, no build
  plan [--environment <e>] [--json]                          every asset's state and every resource change; offline
  upload [--json]                                            upload new and changed assets, ids into opossum.lock
  provision [--environment <e>] [--allow-spend] [--json]     create and update passes, products, badges and the
                                                             experience; --allow-spend lets badge creates pay
  deploy [--environment <e>] [--saved] [--allow-spend] [--json] [<place>...]
                                                             upload, provision, then build and publish the places
                                                             (--saved: save them unpublished)
  adopt [--json] asset <file> (<id> | --none)                settle an upload that may or may not have happened
  adopt [--environment <e>] [--json] (pass | product | badge | thumbnail) <key> (<id> | --none)
                                                             settle a create, or bind one made elsewhere
  play [--run] [--timeout <s>] [--json]                      start a Studio playtest through opossum dev
  stop [--timeout <s>] [--json]                              end it and sum up what it logged
  pull [--timeout <s>] [--json] <Instance.Path> [<file.rbxm>]
                                                             write an instance from Studio into the files
  status [--json]                                            the opossum devs running and which holds Studio
  attach [--force] [--timeout <s>] [--json]                  move Studio to this project's opossum dev
  where <file | Instance.Path[:line]>                        a file's instance, or an instance's file
  sourcemap                                                  .opossum/sourcemap.json for luau-lsp
  plugin install [<path>]                                    the Studio plugin .rbxm
  migrate                                                    convert Rojo meta files, refs and [place], and adopt
                                                             tessera's config, lock and ids and hexagon's imports";

enum Command {
    Build { profile: Option<String>, environment: Option<String>, out: Option<PathBuf>, emit: Option<PathBuf> },
    Check { profile: Option<String>, environment: Option<String>, json: bool },
    Dev { environment: Option<String>, json: bool },
    Plan { environment: Option<String>, json: bool },
    Upload { json: bool },
    Provision { environment: Option<String>, allow_spend: bool, json: bool },
    Deploy { environment: Option<String>, places: Vec<String>, saved: bool, allow_spend: bool, json: bool },
    Adopt { json: bool, what: Adopted, id: Option<u64> },
    Exec(exec::Options, exec::Target, exec::Source),
    Test(exec::Options, exec::Target, exec::Tests),
    Play(exec::Options, bool),
    Stop(exec::Options),
    Pull(exec::Options, String, Option<PathBuf>),
    Status(exec::Options),
    Attach(exec::Options, bool),
    Where(String),
    Sourcemap,
    Plugin(Option<PathBuf>),
    Migrate,
    Help,
}

/// What `adopt` settles: an asset file, or a resource by kind and key in an environment.
enum Adopted {
    Asset(String),
    Resource { environment: Option<String>, kind: String, key: String },
}

fn parse(args: &[String]) -> Result<(Option<PathBuf>, Command), String> {
    let mut dir = None;
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(arg) = it.next() {
        match arg {
            "-C" => dir = Some(PathBuf::from(it.next().ok_or("-C needs a directory")?)),
            _ => rest.push(arg),
        }
    }
    let (mut profile, mut out, mut emit, mut json) = (None, None, None, false);
    let (mut realm, mut timeout, mut code, mut run) = (None, None, None, false);
    let (mut local, mut interpreted, mut cloud, mut place, mut environment) = (false, false, false, None, None);
    let (mut force, mut allow_focused, mut none, mut head, mut allow_spend) = (false, false, false, false, false);
    let mut saved = false;
    let mut help = matches!(rest.first(), Some(&("-h" | "--help")));
    let mut positional: Vec<&str> = Vec::new();
    let mut it = rest.iter().copied().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().map(str::to_owned).ok_or(format!("{arg} needs a value"));
        match arg {
            "--profile" => profile = Some(value()?),
            "-o" => out = Some(PathBuf::from(value()?)),
            "--emit" => emit = Some(PathBuf::from(value()?)),
            "--json" => json = true,
            "--realm" => realm = Some(value()?),
            "--timeout" => {
                let seconds =
                    value()?.parse::<f64>().ok().filter(|t| t.is_finite() && *t > 0.0 && *t <= dev::MAX_TIMEOUT);
                timeout = Some(seconds.ok_or("--timeout takes seconds, more than 0 and at most a day")?);
            }
            "-e" => code = Some(value()?),
            "--run" => run = true,
            "--local" => local = true,
            "--interpreted" => interpreted = true,
            "--cloud" => cloud = true,
            "--place" => place = Some(value()?),
            "--environment" => environment = Some(value()?),
            "--force" => force = true,
            "--allow-focused" => allow_focused = true,
            "--none" => none = true,
            "--head" => head = true,
            "--allow-spend" => allow_spend = true,
            "--saved" => saved = true,
            "-h" | "--help" => help = true,
            "-" => positional.push(arg),
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => positional.push(arg),
        }
    }
    if help {
        return Ok((dir, Command::Help));
    }
    let verb = rest.first().copied();
    let tests = matches!(verb, Some("test" | "bench"));
    let runs = tests || verb == Some("exec");
    // where exec, test and bench run: each target takes only its own flags, and a cloud run builds with its profile
    let profiled = if runs && cloud { profile.take() } else { None };
    let elsewhere = place.is_some() || environment.is_some() || profiled.is_some() || head;
    let target = match (local, cloud) {
        (false, false) if !interpreted && !elsewhere => Some(exec::Target::Studio { realm: realm.clone() }),
        (true, false) if realm.is_none() && !elsewhere => Some(exec::Target::Local { interpreted }),
        (false, true) if !interpreted && realm.is_none() && (!head || (!tests && profiled.is_none())) => {
            if timeout.is_some_and(|t| t > exec::CLOUD_TIMEOUT) {
                return Err(format!("--timeout with --cloud is at most {} s, Open Cloud's limit", exec::CLOUD_TIMEOUT));
            }
            let (place, environment) = (place.clone(), environment.clone());
            Some(exec::Target::Cloud(exec::Cloud { place, environment, profile: profiled, head }))
        }
        _ => None,
    };
    let timed = runs || matches!(verb, Some("play" | "stop" | "pull" | "attach"));
    let targeted = realm.is_some() || local || interpreted || cloud || place.is_some() || head;
    // the ids `opossum/resources` holds, or where a cloud run runs
    let environed = runs || matches!(verb, Some("build" | "check" | "dev" | "plan" | "provision" | "deploy" | "adopt"));
    let stray = targeted && !runs
        || environment.is_some() && !environed
        || timeout.is_some() && !timed
        || allow_focused && !tests;
    if stray
        || code.is_some() && verb != Some("exec")
        || run && verb != Some("play")
        || force && verb != Some("attach")
        || none && verb != Some("adopt")
        || allow_spend && !matches!(verb, Some("provision" | "deploy"))
        || saved && verb != Some("deploy")
    {
        return Err(USAGE.to_owned());
    }
    let options = || exec::Options { json, timeout };
    let built = profile.is_some() || out.is_some() || emit.is_some();
    let command = match (verb, positional.as_slice(), target) {
        (Some("build"), [], _) if !json => Command::Build { profile, environment, out, emit },
        (Some("check"), [], _) if out.is_none() && emit.is_none() => Command::Check { profile, environment, json },
        (Some("dev"), [], _) if !built => Command::Dev { environment, json },
        (Some("plan"), [], _) if !built => Command::Plan { environment, json },
        (Some("upload"), [], _) if !built => Command::Upload { json },
        (Some("provision"), [], _) if !built => Command::Provision { environment, allow_spend, json },
        (Some("deploy"), places, _) if !built => {
            let places = places.iter().map(|p| (*p).to_owned()).collect();
            Command::Deploy { environment, places, saved, allow_spend, json }
        }
        (Some("adopt"), [kind @ ("asset" | "pass" | "product" | "badge" | "thumbnail"), name, id @ ..], _)
            if !built && id.len() == usize::from(!none) && (*kind != "asset" || environment.is_none()) =>
        {
            let id = match id.first() {
                Some(id) => Some(id.parse().ok().filter(|&id| id > 0).ok_or(format!("{id} is not an id"))?),
                None => None,
            };
            let what = match *kind {
                "asset" => Adopted::Asset((*name).to_owned()),
                _ => Adopted::Resource { environment, kind: (*kind).to_owned(), key: (*name).to_owned() },
            };
            Command::Adopt { json, what, id }
        }
        (Some("exec"), [], Some(target)) if code.is_some() && !built => {
            Command::Exec(options(), target, exec::Source::Code(code.clone().unwrap_or_default()))
        }
        (Some("exec"), ["-"], Some(target)) if code.is_none() && !built => {
            Command::Exec(options(), target, exec::Source::Stdin)
        }
        (Some("exec"), [file], Some(target)) if code.is_none() && !built => {
            Command::Exec(options(), target, exec::Source::File((*file).to_owned()))
        }
        (Some(verb @ ("test" | "bench")), filter, Some(target)) if filter.len() <= 1 && !built => {
            let mode = if verb == "test" { spec::Mode::Test } else { spec::Mode::Bench };
            let filter = filter.first().map(|f| (*f).to_owned());
            Command::Test(options(), target, exec::Tests { mode, filter, allow_focused })
        }
        (Some("play"), [], _) if !built => Command::Play(options(), run),
        (Some("stop"), [], _) if !built => Command::Stop(options()),
        (Some("pull"), [path, file @ ..], _) if file.len() <= 1 && !built => {
            Command::Pull(options(), (*path).to_owned(), file.first().map(PathBuf::from))
        }
        (Some("status"), [], _) if !built => Command::Status(options()),
        (Some("attach"), [], _) if !built => Command::Attach(options(), force),
        (Some("where"), [target], _) if !built && !json => Command::Where((*target).to_owned()),
        (Some("sourcemap"), [], _) if !built && !json => Command::Sourcemap,
        (Some("plugin"), ["install", path @ ..], _) if path.len() <= 1 && !built && !json => {
            Command::Plugin(path.first().map(PathBuf::from))
        }
        (Some("migrate"), [], _) if !built && !json => Command::Migrate,
        _ => return Err(USAGE.to_owned()),
    };
    Ok((dir, command))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // loading, checking and lowering recurse deeply
    let result = parse(&args).and_then(|(dir, command)| {
        std::thread::Builder::new()
            .stack_size(lower::STACK_SIZE)
            .spawn(move || run(dir, command))
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "the compiler panicked".to_owned())?
    });
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("opossum: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(dir: Option<PathBuf>, command: Command) -> Result<bool, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let start = match dir {
        Some(dir) => std::path::absolute(&dir).map_err(|e| format!("{}: {e}", dir.display()))?,
        None => cwd.clone(),
    };
    let file = || config::find(&start);
    let load = || config::load(&file()?);
    // local runs read nothing of the config: the project is the directory holding it
    let root = || {
        let file = file()?;
        std::fs::canonicalize(file.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())
    };
    match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(true)
        }
        Command::Plugin(path) => plugin(path),
        // finds the config without loading it: the config it fixes does not load
        Command::Migrate => migrate::run(&file()?),
        // loads it itself, so a broken one is an event too
        Command::Dev { environment, json } => dev::run(&start, json, environment.as_deref()),
        Command::Build { profile, environment, out, emit } => {
            build(load()?, profile.as_deref(), environment.as_deref(), out, emit)
        }
        Command::Check { profile, environment, json } => {
            check(load()?, profile.as_deref(), environment.as_deref(), json)
        }
        Command::Where(target) => where_(session(load()?, None, None)?, &cwd, &target),
        Command::Sourcemap => sourcemap(session(load()?, None, None)?),
        Command::Exec(options, exec::Target::Local { interpreted }, source) => {
            exec::exec_local(&root()?, &options, interpreted, source)
        }
        Command::Test(options, exec::Target::Local { interpreted }, tests) => {
            exec::test_local(&root()?, &options, interpreted, &tests)
        }
        // dev wrote what the editor reads; these only talk to it
        Command::Exec(options, exec::Target::Studio { realm }, source) => {
            exec::exec(&load()?, &options, &realm, source)
        }
        Command::Test(options, exec::Target::Studio { realm }, tests) => {
            exec::test_studio(&load()?, &options, &realm, &tests)
        }
        Command::Play(options, run) => exec::play(&load()?, &options, run),
        Command::Stop(options) => exec::stop(&load()?, &options),
        Command::Pull(options, path, file) => exec::pull(&load()?, &options, &path, file.as_deref()),
        Command::Status(options) => exec::status(&load()?, &options),
        Command::Attach(options, force) => exec::attach(&load()?, &options, force),
        // the commands with one outcome: refusals are outcomes too
        Command::Exec(options, exec::Target::Cloud(run), source) => {
            let outcome = match remote(&file()?) {
                Ok((cfg, client, cache)) => exec::exec_cloud(cfg, &client, &cache, &options, &run, source),
                Err(refusal) => refusal,
            };
            finish(outcome, options.json)
        }
        Command::Test(options, exec::Target::Cloud(run), tests) => match remote(&file()?) {
            Ok((cfg, client, cache)) => exec::test_cloud(cfg, &client, &cache, &options, &run, &tests),
            Err(refusal) => finish(refusal, options.json),
        },
        Command::Plan { environment, json } => {
            let outcome = match config::load(&file()?) {
                Ok(cfg) => plan::plan(&cfg, environment.as_deref()),
                Err(error) => Outcome::refused("config", error),
            };
            finish(outcome, json)
        }
        Command::Upload { json } => {
            let outcome = outward(&file()?, |cfg, client, guard, _| upload::upload(cfg, client, guard, &signals::STOP));
            finish(outcome, json)
        }
        Command::Provision { environment, allow_spend, json } => {
            let outcome = outward(&file()?, |cfg, client, guard, _| {
                resources::provision(cfg, client, guard, environment.as_deref(), allow_spend, &signals::STOP)
            });
            finish(outcome, json)
        }
        Command::Deploy { environment, places, saved, allow_spend, json } => {
            let outcome = outward(&file()?, |cfg, client, guard, cache| {
                let (environment, stop) = (environment.as_deref(), &signals::STOP);
                deploy::deploy(cfg, client, guard, cache, environment, &places, saved, allow_spend, stop)
            });
            finish(outcome, json)
        }
        Command::Adopt { json, what, id } => finish(adopt(&file()?, &cwd, &what, id), json),
    }
}

/// `opossum plugin install [path]`.
fn plugin(path: Option<PathBuf>) -> Result<bool, String> {
    let path = match path {
        Some(path) => path,
        None => PathBuf::from(std::env::var("HOME").map_err(|_| "HOME is not set")?)
            .join("Documents/Roblox/Plugins/opossum.rbxm"),
    };
    sync::install_plugin(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(true)
}

/// What a command that may change Roblox state or `opossum.lock` did, for [`finish`] to print: its exit status (0
/// done, 1 it ran and something failed or needs the user, 2 it never ran and sent nothing), its `--json` object and
/// its text. A refusal's text is its error.
pub struct Outcome {
    pub exit: u8,
    pub json: Value,
    pub text: String,
}

impl Outcome {
    /// It never ran: `{"ok": false, "ran": false, "error", "code"}`, `code` a diagnostic code agents branch on.
    pub fn refused(code: &str, error: String) -> Outcome {
        Outcome { exit: 2, json: json!({ "ok": false, "ran": false, "error": error, "code": code }), text: error }
    }
}

/// Prints `outcome` on stdout: its JSON object with `--json`, else its text. A refusal's error comes back for `main`
/// to print on stderr and exit 2 with.
fn finish(outcome: Outcome, json: bool) -> Result<bool, String> {
    if json {
        println!("{}", outcome.json);
    } else if outcome.exit != 2 {
        print!("{}", outcome.text);
    }
    match outcome.exit {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(outcome.text),
    }
}

/// The config, the client with the project's key, and the cache, for every command that talks to Open Cloud.
fn remote(file: &Path) -> Result<(Config, Client, Cache), Outcome> {
    let cfg = config::load(file).map_err(|error| Outcome::refused("config", error))?;
    let client = open_cloud(&cfg.root)?;
    let cache = Cache::default_dir().map_err(|error| Outcome::refused("cache", error))?;
    Ok((cfg, client, cache))
}

/// [`remote`] with the guard held and Ctrl-C asking the run to stop: upload, provision and deploy.
fn outward(file: &Path, run: impl FnOnce(&Config, &Client, &RemoteGuard, &Cache) -> Outcome) -> Outcome {
    let (cfg, client, cache) = match remote(file) {
        Ok(remote) => remote,
        Err(refusal) => return refusal,
    };
    let guard = match RemoteGuard::acquire(&cache) {
        Ok(guard) => guard,
        Err(error) => return Outcome::refused("guard", error),
    };
    signals::install();
    run(&cfg, &client, &guard, &cache)
}

/// `opossum adopt`: offline, but it writes the lock, so it holds the guard.
fn adopt(file: &Path, cwd: &Path, what: &Adopted, id: Option<u64>) -> Outcome {
    let cfg = match config::load(file) {
        Ok(cfg) => cfg,
        Err(error) => return Outcome::refused("config", error),
    };
    let guard = match Cache::default_dir().and_then(|cache| RemoteGuard::acquire(&cache)) {
        Ok(guard) => guard,
        Err(error) => return Outcome::refused("guard", error),
    };
    match what {
        Adopted::Asset(asset) => upload::adopt_asset(&cfg, &guard, cwd, asset, id),
        Adopted::Resource { environment, kind, key } => {
            resources::adopt(&cfg, &guard, environment.as_deref(), kind, key, id)
        }
    }
}

/// The Open Cloud client with the project's API key, refused without one. `OPOSSUM_OPEN_CLOUD_URL` points it at a mock
/// on this machine instead (`http://127.0.0.1:<port>` or `http://localhost:<port>`, nothing else), which is sent the
/// key `mock`: the project's key is not even read then, so an end-to-end check never hands it to whatever listens on
/// that port.
fn open_cloud(root: &Path) -> Result<Client, Outcome> {
    let (base, key) = match std::env::var("OPOSSUM_OPEN_CLOUD_URL") {
        Err(std::env::VarError::NotPresent) => match inputs::api_key(root) {
            Some(key) => (cloud::ROBLOX.to_owned(), key),
            None => {
                let error =
                    "no Open Cloud API key: set ROBLOX_OPEN_CLOUD_API_KEY in the environment, .env.local or .env";
                return Err(Outcome::refused("cloud.credentials", error.into()));
            }
        },
        Ok(url) if cloud::loopback(&url) => (url, "mock".to_owned()),
        _ => {
            let error = "OPOSSUM_OPEN_CLOUD_URL must be http://127.0.0.1:<port> or http://localhost:<port>";
            return Err(Outcome::refused("cloud.baseUrl", error.into()));
        }
    };
    Client::new(&key, &base, Clock::Real).map_err(|error| Outcome::refused("cloud.credentials", error))
}

/// A session for the one-shot commands, with the editor's files written for its modules.
fn session(cfg: Config, profile: Option<&str>, environment: Option<&str>) -> Result<Session, String> {
    let session = Session::new(cfg, profile, environment)?;
    editor::write(&session.cfg.root, &session.cfg.ts, &session.modules)?;
    Ok(session)
}

/// Prints the diagnostics and a count per code; returns whether there were no errors.
fn report(diags: &[Diag], root: &Path) -> bool {
    let mut codes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut errors = 0;
    for d in diags {
        println!("{}", d.human(root));
        *codes.entry(d.code.as_str()).or_default() += 1;
        errors += usize::from(d.is_error());
    }
    if !codes.is_empty() {
        let counts: Vec<String> = codes.iter().map(|(code, n)| format!("{n} {code}")).collect();
        println!("{} diagnostics ({errors} errors): {}", diags.len(), counts.join(", "));
    }
    errors == 0
}

fn times(stats: &build::Stats, extra: &[(&str, f64)], start: Instant) -> String {
    let mut parts: Vec<String> = stats.times.iter().map(|(p, ms)| format!("{p} {ms:.0}")).collect();
    parts.extend(extra.iter().map(|(p, ms)| format!("{p} {ms:.0}")));
    format!("{} ms ({} ms)", start.elapsed().as_millis(), parts.join(", "))
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn build(
    cfg: Config,
    profile: Option<&str>,
    environment: Option<&str>,
    out: Option<PathBuf>,
    emit: Option<PathBuf>,
) -> Result<bool, String> {
    let start = Instant::now();
    let mut session = session(cfg, profile, environment)?;
    let stats = session.build()?;
    let t = Instant::now();
    let tree = session.tree(false);
    let mut place = tree.to_place();
    let mount = ms(t);
    let root = session.cfg.root.clone();
    let ok = report(&session.diagnostics(), &root);
    if let Some(dir) = emit {
        build::emit(&dir, &session.outputs())?;
    }
    let dir = editor::dir(&root);
    let t = Instant::now();
    // written with errors too (without the modules that failed), as `dev` syncs; the exit status tells
    let path = out.unwrap_or_else(|| dir.join("place.rbxl"));
    let packs = place::read_texture_packs(&root.join("place/texture-packs.json"))?;
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    place::write_rbxl(&mut place, &packs, std::io::BufWriter::new(file))?;
    editor::write_if_changed(&dir.join("sourcemap.json"), &tree.sourcemap_json(Some(&root)))?;
    println!("wrote {}", path.display());
    let modules = session.scan.modules.len();
    println!(
        "{modules} modules, {} packages in {}",
        session.packages.len(),
        times(&stats, &[("mount", mount), ("write", ms(t))], start)
    );
    Ok(ok)
}

fn check(cfg: Config, profile: Option<&str>, environment: Option<&str>, json: bool) -> Result<bool, String> {
    let start = Instant::now();
    let mut session = session(cfg, profile, environment)?;
    let stats = session.build()?;
    session.tree(false);
    let diags = session.diagnostics();
    let root = &session.cfg.root;
    if json {
        let list: Vec<serde_json::Value> = diags.iter().map(|d| d.json(root)).collect();
        println!("{}", serde_json::Value::Array(list));
        return Ok(!diags.iter().any(Diag::is_error));
    }
    let ok = report(&diags, root);
    println!("checked {} files in {}", stats.checked, times(&stats, &[], start));
    Ok(ok)
}

fn sourcemap(mut session: Session) -> Result<bool, String> {
    session.shapes()?;
    let tree = session.tree(true);
    let root = &session.cfg.root;
    let path = editor::dir(root).join("sourcemap.json");
    editor::write_if_changed(&path, &tree.sourcemap_json(Some(root)))?;
    println!("wrote {}", path.display());
    Ok(true)
}

/// A path as the project spells it: under the config's root, also when it was reached through a link.
pub fn project_path(cfg: &Config, path: &Path) -> PathBuf {
    if path.starts_with(&cfg.root) {
        return path.to_path_buf();
    }
    for dir in ["src", "place"] {
        let linked = cfg.root.join(dir);
        if let (Ok(real), Ok(target)) = (std::fs::canonicalize(&linked), canonical_prefix(path))
            && let Ok(rel) = target.strip_prefix(&real)
        {
            return linked.join(rel);
        }
    }
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` with its longest existing ancestor canonicalized (a deleted file keeps its name).
pub fn canonical_prefix(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => Ok(canonical_prefix(parent)?.join(name)),
            _ => Err(e),
        },
    }
}

fn where_(mut session: Session, cwd: &Path, target: &str) -> Result<bool, String> {
    let root = &session.cfg.root;
    let file = [cwd.join(target), root.join(target)].into_iter().find(|f| f.exists()).unwrap_or_default();
    session.rescan();
    let root = session.cfg.root.clone();
    if file.is_file() {
        let file = project_path(&session.cfg, &file);
        let Some(m) = session.scan.find(&file) else {
            println!("{} routes nowhere", file.strip_prefix(&root).unwrap_or(&file).display());
            return Ok(false);
        };
        let template = if m.template { ", template" } else { "" };
        println!("{} ({:?}, {:?}{template})", m.full_name(), m.realm, m.class);
        return Ok(true);
    }
    let Some((m, line)) = session.scan.where_instance(target) else {
        println!("{target} is no routed module");
        return Ok(false);
    };
    let (file, kind) = (m.file.clone(), m.kind);
    let shown = file.strip_prefix(&root).unwrap_or(&file).display().to_string();
    match (line, kind) {
        (Some(line), route::Kind::Ts | route::Kind::Tsx) => {
            let name = build::slashed(&file);
            match session.compile_one(&name)?.and_then(|c| c.lines.source_line(line)) {
                Some(ts) => println!("{shown}:{ts}"),
                None => println!("{shown} (line {line} of the output maps to no statement)"),
            }
        }
        (Some(line), _) => println!("{shown}:{line}"),
        (None, _) => println!("{shown}"),
    }
    Ok(true)
}
