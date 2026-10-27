//! opossum: one binary from a TypeScript + Luau Roblox project to a Studio session or a place. See DESIGN.md.

mod build;
mod config;
mod dev;
mod diag;
mod editor;
mod inputs;
mod packages;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use build::Session;
use config::Config;
use diag::Diag;

#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

const USAGE: &str = "usage: opossum [-C <dir>] <command>
  build [--profile <name>] [-o <file.rbxl>] [--emit <dir>]   build the place (.opossum/place.rbxl)
  check [--profile <name>] [--json]                          diagnostics, no output
  dev                                                        watch, rebuild and sync to Studio
  where <file | Instance.Path[:line]>                        a file's instance, or an instance's file
  sourcemap                                                  .opossum/sourcemap.json for luau-lsp
  plugin install [<path>]                                    the Studio plugin .rbxm";

enum Command {
    Build { profile: Option<String>, out: Option<PathBuf>, emit: Option<PathBuf> },
    Check { profile: Option<String>, json: bool },
    Dev,
    Where(String),
    Sourcemap,
    Plugin(Option<PathBuf>),
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
    let mut positional: Vec<&str> = Vec::new();
    let mut it = rest.iter().copied().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().map(str::to_owned).ok_or(format!("{arg} needs a value"));
        match arg {
            "--profile" => profile = Some(value()?),
            "-o" => out = Some(PathBuf::from(value()?)),
            "--emit" => emit = Some(PathBuf::from(value()?)),
            "--json" => json = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => positional.push(arg),
        }
    }
    let command = match (rest.first().copied(), positional.as_slice()) {
        (Some("build"), []) if !json => Command::Build { profile, out, emit },
        (Some("check"), []) if out.is_none() && emit.is_none() => Command::Check { profile, json },
        (Some("dev"), []) => Command::Dev,
        (Some("where"), [target]) => Command::Where((*target).to_owned()),
        (Some("sourcemap"), []) => Command::Sourcemap,
        (Some("plugin"), ["install"]) => Command::Plugin(None),
        (Some("plugin"), ["install", path]) => Command::Plugin(Some(PathBuf::from(path))),
        _ => return Err(USAGE.to_owned()),
    };
    Ok((dir, command))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse(&args).and_then(|(dir, command)| {
        // loading, checking and lowering recurse deeply
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
    if let Command::Plugin(path) = command {
        let path = match path {
            Some(path) => path,
            None => PathBuf::from(std::env::var("HOME").map_err(|_| "HOME is not set")?)
                .join("Documents/Roblox/Plugins/opossum.rbxm"),
        };
        sync::install_plugin(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("wrote {}", path.display());
        return Ok(true);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let start = match dir {
        Some(dir) => std::path::absolute(&dir).map_err(|e| format!("{}: {e}", dir.display()))?,
        None => cwd.clone(),
    };
    let cfg = config::load(&config::find(&start)?)?;
    editor::write(&cfg.root, &cfg.ts)?;
    match command {
        Command::Build { profile, out, emit } => build(cfg, profile.as_deref(), out, emit),
        Command::Check { profile, json } => check(cfg, profile.as_deref(), json),
        Command::Dev => dev::run(cfg),
        Command::Where(target) => where_(cfg, &cwd, &target),
        Command::Sourcemap => sourcemap(cfg),
        Command::Plugin(_) => unreachable!("handled above"),
    }
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

fn build(cfg: Config, profile: Option<&str>, out: Option<PathBuf>, emit: Option<PathBuf>) -> Result<bool, String> {
    let start = Instant::now();
    let mut session = Session::new(cfg, profile)?;
    let stats = session.build()?;
    let t = Instant::now();
    let mut place = session.mount(false);
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
    editor::write_if_changed(&dir.join("sourcemap.json"), &place::sourcemap_json(&place, Some(&root)))?;
    println!("wrote {}", path.display());
    let modules = session.scan.modules.len();
    println!(
        "{modules} modules, {} packages in {}",
        session.packages.len(),
        times(&stats, &[("mount", mount), ("write", ms(t))], start)
    );
    Ok(ok)
}

fn check(cfg: Config, profile: Option<&str>, json: bool) -> Result<bool, String> {
    let start = Instant::now();
    let mut session = Session::new(cfg, profile)?;
    let stats = session.build()?;
    session.mount(false);
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

fn sourcemap(cfg: Config) -> Result<bool, String> {
    let mut session = Session::new(cfg, None)?;
    session.shapes()?;
    let place = session.mount(true);
    let root = &session.cfg.root;
    let path = editor::dir(root).join("sourcemap.json");
    editor::write_if_changed(&path, &place::sourcemap_json(&place, Some(root)))?;
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
fn canonical_prefix(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => Ok(canonical_prefix(parent)?.join(name)),
            _ => Err(e),
        },
    }
}

fn where_(cfg: Config, cwd: &Path, target: &str) -> Result<bool, String> {
    let file = [cwd.join(target), cfg.root.join(target)].into_iter().find(|f| f.exists()).unwrap_or_default();
    let mut session = Session::new(cfg, None)?;
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
