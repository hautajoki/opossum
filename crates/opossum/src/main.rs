//! opossum: one binary from a TypeScript + Luau Roblox project to a Studio session or a place. See DESIGN.md.

mod build;
mod config;
mod dev;
mod diag;
mod editor;
mod event;
mod exec;
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
  dev [--json]                                               watch, rebuild and sync to Studio
  exec [--realm <r>] [--timeout <s>] [--json] (<file.luau> | -e <code> | -)
                                                             run Luau in Studio through opossum dev
  test [--realm <r>] [--timeout <s>] [--json] [<filter>]     run hexagon specs in Studio (bench: benchmarks)
  where <file | Instance.Path[:line]>                        a file's instance, or an instance's file
  sourcemap                                                  .opossum/sourcemap.json for luau-lsp
  plugin install [<path>]                                    the Studio plugin .rbxm
  migrate                                                    convert Rojo meta files and refs, and [place]";

enum Command {
    Build { profile: Option<String>, out: Option<PathBuf>, emit: Option<PathBuf> },
    Check { profile: Option<String>, json: bool },
    Dev { json: bool },
    Exec(exec::Options, exec::Source),
    Test(exec::Options, &'static str, Option<String>),
    Where(String),
    Sourcemap,
    Plugin(Option<PathBuf>),
    Migrate,
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
    let (mut realm, mut timeout, mut code) = (None, None, None);
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
            "--timeout" => timeout = Some(value()?.parse::<f64>().map_err(|_| "--timeout takes seconds")?),
            "-e" => code = Some(value()?),
            "-" => positional.push(arg),
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => positional.push(arg),
        }
    }
    let studio = matches!(rest.first().copied(), Some("exec" | "test" | "bench"));
    if !studio && (realm.is_some() || timeout.is_some()) || code.is_some() && rest.first() != Some(&"exec") {
        return Err(USAGE.to_owned());
    }
    let options = || exec::Options { realm: realm.clone(), json, timeout };
    let command = match (rest.first().copied(), positional.as_slice()) {
        (Some("build"), []) if !json => Command::Build { profile, out, emit },
        (Some("check"), []) if out.is_none() && emit.is_none() => Command::Check { profile, json },
        (Some("dev"), []) if profile.is_none() && out.is_none() && emit.is_none() => Command::Dev { json },
        (Some("exec"), []) if code.is_some() => {
            Command::Exec(options(), exec::Source::Code(code.clone().unwrap_or_default()))
        }
        (Some("exec"), ["-"]) if code.is_none() => Command::Exec(options(), exec::Source::Stdin),
        (Some("exec"), [file]) if code.is_none() => Command::Exec(options(), exec::Source::File((*file).to_owned())),
        (Some(mode @ ("test" | "bench")), filter)
            if filter.len() <= 1 && profile.is_none() && out.is_none() && emit.is_none() =>
        {
            let mode = if mode == "test" { "test" } else { "bench" };
            Command::Test(options(), mode, filter.first().map(|f| (*f).to_owned()))
        }
        (Some("where"), [target]) => Command::Where((*target).to_owned()),
        (Some("sourcemap"), []) => Command::Sourcemap,
        (Some("plugin"), ["install"]) => Command::Plugin(None),
        (Some("plugin"), ["install", path]) => Command::Plugin(Some(PathBuf::from(path))),
        (Some("migrate"), []) if profile.is_none() && out.is_none() && emit.is_none() && !json => Command::Migrate,
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
    if let Command::Migrate = command {
        // before loading: the config it fixes does not load
        return migrate(&config::find(&start)?);
    }
    let cfg = config::load(&config::find(&start)?)?;
    // dev wrote what the editor reads; these only talk to it
    match command {
        Command::Exec(options, source) => return exec::exec(&cfg, &options, source),
        Command::Test(options, mode, filter) => return exec::test(&cfg, &options, mode, filter.as_deref()),
        _ => {}
    }
    editor::write(&cfg.root, &cfg.ts)?;
    match command {
        Command::Build { profile, out, emit } => build(cfg, profile.as_deref(), out, emit),
        Command::Check { profile, json } => check(cfg, profile.as_deref(), json),
        Command::Dev { json } => dev::run(cfg, json),
        Command::Where(target) => where_(cfg, &cwd, &target),
        Command::Sourcemap => sourcemap(cfg),
        Command::Plugin(_) | Command::Migrate | Command::Exec(..) | Command::Test(..) => unreachable!("handled above"),
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

fn check(cfg: Config, profile: Option<&str>, json: bool) -> Result<bool, String> {
    let start = Instant::now();
    let mut session = Session::new(cfg, profile)?;
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

fn sourcemap(cfg: Config) -> Result<bool, String> {
    let mut session = Session::new(cfg, None)?;
    session.shapes()?;
    let tree = session.tree(true);
    let root = &session.cfg.root;
    let path = editor::dir(root).join("sourcemap.json");
    editor::write_if_changed(&path, &tree.sourcemap_json(Some(root)))?;
    println!("wrote {}", path.display());
    Ok(true)
}

/// `opossum migrate`: converts a Rojo-era `place/` in place (`init.meta.json` files into properties files, Rojo
/// ref attributes into paths) and moves opossum.toml's `[place]` into properties files, printing each file it
/// changed. The whole conversion is planned first, so a conflict changes nothing and a second run finds nothing.
fn migrate(file: &Path) -> Result<bool, String> {
    let root = file.parent().unwrap_or(Path::new("."));
    let shown = |path: &Path| path.strip_prefix(root).unwrap_or(path).display().to_string();
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let cut = config::cut_place(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let moved = match &cut {
        Some((_, table)) => {
            place::entries_from_toml(table, file).map_err(|e| format!("{}: [place] {e}", file.display()))?
        }
        None => Vec::new(),
    };
    let writes = match place::plan_migration(&root.join("place"), moved) {
        Ok(writes) => writes,
        Err(diags) => {
            report(&diags.iter().map(diag::from_place).collect::<Vec<_>>(), root);
            println!("nothing changed");
            return Ok(false);
        }
    };
    let io = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    for write in &writes {
        match &write.text {
            Some(text) => {
                if let Some(dir) = write.path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
                }
                std::fs::write(&write.path, text).map_err(|e| io(&write.path, e))?;
                println!("wrote {}", shown(&write.path));
            }
            None => {
                std::fs::remove_file(&write.path).map_err(|e| io(&write.path, e))?;
                println!("removed {}", shown(&write.path));
            }
        }
    }
    if let Some((rest, _)) = cut {
        std::fs::write(file, rest).map_err(|e| io(file, e))?;
        println!("wrote {}", shown(file));
    } else if writes.is_empty() {
        println!("nothing to migrate");
    }
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
