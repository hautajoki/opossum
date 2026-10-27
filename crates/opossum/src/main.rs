//! A temporary driver for the compiler core:
//!
//! - `opossum emit <project-dir> <out-dir>`: type-checks `<project-dir>/src`, lowers every file and writes one `.luau`
//!   per TypeScript file under `<out-dir>` (mirroring `src/`), then prints the diagnostics and the timings.
//! - `opossum gate <dir>`: compiles every `.luau` under `<dir>` with Luau at `-O2`, printing each error.
//!
//! Routing is a placeholder until the router lands: the outermost `server/`, `client/` or `shared/` folder of a path
//! picks the route (and is consumed), anything else goes to `ReplicatedStorage/shared`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use lower::{LoadOptions, Location, Locator, Lowered, Realm};
use luau::ModulePath;
use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_core::P;
use tsrs_diagnostics::Category;

#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

const USAGE: &str = "usage: opossum emit <project-dir> <out-dir> | opossum gate <dir>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["emit", project, out] => run_on_big_stack(project, out),
        ["gate", dir] => gate(Path::new(dir)),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("opossum: {message}");
            ExitCode::from(2)
        }
    }
}

fn run_on_big_stack(project: &str, out: &str) -> Result<bool, String> {
    let (project, out) = (absolute(project)?, absolute(out)?);
    std::thread::Builder::new()
        .stack_size(lower::STACK_SIZE)
        .spawn(move || emit(&project, &out))
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "the compiler panicked".to_owned())?
}

fn absolute(path: &str) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{path}: {e}"))
}

/// `path` with forward slashes, as tsrs spells file names.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn emit(project: &Path, out: &Path) -> Result<bool, String> {
    let start = Instant::now();
    let src = project.join("src");
    let mut files = Vec::new();
    walk(&src, &mut |p| {
        let name = p.to_string_lossy();
        if name.ends_with(".ts") || name.ends_with(".tsx") {
            files.push(slashed(p));
        }
    })?;
    files.sort();

    let checkers = std::thread::available_parallelism().map_or(4, usize::from);
    let options = LoadOptions { root: slashed(project), files, checkers, flags: strict_flags() };
    let project = lower::load(&options, None)?;
    let load_time = start.elapsed();

    let ctx = tsrs_compiler::Context::background();
    let sources = project.project_files();
    let t = Instant::now();
    let checked = project.check(&ctx, &sources);
    let check_time = t.elapsed();

    let locator = placeholder_locator(&project);
    let t = Instant::now();
    let lowered = lower::lower_files(&project, &locator, &sources);
    let lower_time = t.elapsed();

    let t = Instant::now();
    let src_prefix = format!("{}/", slashed(&src));
    let written = write_outputs(&lowered, &locator, &src_prefix, out)?;
    let write_time = t.elapsed();

    let mut diagnostics: Vec<P<Diagnostic>> = checked;
    diagnostics.extend(lowered.iter().flat_map(|l| l.diagnostics.iter().copied()));
    let errors = report(&diagnostics);
    let unwritten = lowered.len() - written;
    println!(
        "{} files: {written} written, {unwritten} not written; {} diagnostics ({errors} errors)",
        lowered.len(),
        diagnostics.len()
    );
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "load {:.0} ms, check {:.0} ms, lower {:.0} ms, render+write {:.0} ms, total {:.0} ms",
        ms(load_time),
        ms(check_time),
        ms(lower_time),
        ms(write_time),
        ms(start.elapsed())
    );
    Ok(errors == 0)
}

/// the project's strictness flags, until `opossum.toml` carries them.
fn strict_flags() -> Vec<(String, bool)> {
    [
        ("noUncheckedIndexedAccess", true),
        ("noImplicitReturns", true),
        ("noImplicitOverride", true),
        ("noFallthroughCasesInSwitch", true),
        ("noPropertyAccessFromIndexSignature", true),
        ("useUnknownInCatchVariables", true),
        ("allowUnreachableCode", false),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect()
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(&path, f)?;
        } else {
            f(&path);
        }
    }
    Ok(())
}

/// The file's module name: `a/b.ts` → `a/b`, `a/index.ts` (or `init`) → `a`, as a folder module.
fn module_names(rel: &str) -> (Vec<String>, bool) {
    let stem = [".d.ts", ".tsx", ".ts", ".luau", ".lua"].iter().find_map(|ext| rel.strip_suffix(ext)).unwrap_or(rel);
    let mut names: Vec<String> = stem.split('/').filter(|s| !s.is_empty()).map(str::to_owned).collect();
    let init = names.last().is_some_and(|n| n == "index" || n == "init");
    if init {
        names.pop();
    }
    (names, init)
}

/// A source file's placeholder route: the outermost route folder, consumed.
fn route_source(rel: &str) -> Location {
    let (mut names, init) = module_names(rel);
    let routed = names.iter().position(|n| matches!(n.as_str(), "server" | "client" | "shared"));
    let (root, realm): (&[&str], Realm) = match routed.map(|i| names.remove(i)).as_deref() {
        Some("server") => (&["ServerScriptService", "server"], Realm::Server),
        Some("client") => (&["StarterPlayer", "StarterPlayerScripts", "client"], Realm::Client),
        _ => (&["ReplicatedStorage", "shared"], Realm::Shared),
    };
    let mut path: Vec<String> = root.iter().map(|s| (*s).to_owned()).collect();
    path.extend(names);
    Location { module: ModulePath { names: path, root: root.len(), init }, realm }
}

/// A package declaration file's module, under `ReplicatedStorage/rbxts/node_modules/<scope>/<name>`: the package's
/// `main` for its types entry, else the file itself.
fn route_package(file: &str) -> Option<Location> {
    let at = file.rfind("/node_modules/")?;
    let rest = &file[at + "/node_modules/".len()..];
    let mut parts = rest.splitn(3, '/');
    let (scope, name, inner) = (parts.next()?, parts.next()?, parts.next()?);
    if !scope.starts_with('@') {
        return None;
    }
    let package_dir = &file[..at + "/node_modules/".len() + scope.len() + 1 + name.len()];
    let manifest = std::fs::read_to_string(format!("{package_dir}/package.json")).unwrap_or_default();
    let types = json_string(&manifest, "types").or_else(|| json_string(&manifest, "typings"));
    let inner = match (types, json_string(&manifest, "main")) {
        (Some(types), Some(main)) if types.trim_start_matches("./") == inner => {
            main.trim_start_matches("./").to_owned()
        }
        _ => inner.to_owned(),
    };
    let (names, init) = module_names(&inner);
    let mut path: Vec<String> =
        ["ReplicatedStorage", "rbxts", "node_modules", scope, name].iter().map(|s| (*s).to_owned()).collect();
    let root = path.len();
    path.extend(names);
    let init = init || path.len() == root;
    Some(Location { module: ModulePath { names: path, root, init }, realm: Realm::Shared })
}

/// The string value of a top-level `"key": "value"` in a package manifest (no JSON parser for a placeholder).
fn json_string(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

fn placeholder_locator(project: &lower::Project) -> Locator {
    let src = format!("{}/src/", project.root);
    let mut files = rustc_hash::FxHashMap::default();
    for &file in project.program.source_files() {
        let name = file.file_name();
        let location = match name.strip_prefix(&src) {
            Some(rel) => Some(route_source(rel)),
            None => route_package(name),
        };
        if let Some(location) = location {
            files.insert(name.to_owned(), location);
        }
    }
    let runtime = ["ReplicatedStorage", "rbxts", "RuntimeLib"].iter().map(|s| (*s).to_owned()).collect();
    let runtime_lib =
        Some(Location { module: ModulePath { names: runtime, root: 2, init: false }, realm: Realm::Shared });
    Locator { files, runtime_lib }
}

/// Renders and writes every file lowered without errors; returns how many were written.
fn write_outputs(lowered: &[Lowered], locator: &Locator, src_prefix: &str, out: &Path) -> Result<usize, String> {
    let rendered = lower::parallel::map(lowered, |l| {
        let name = l.file.file_name();
        let (Some(ast), Some(location), Some(rel)) = (&l.ast, locator.get(name), name.strip_prefix(src_prefix)) else {
            return None;
        };
        if l.has_errors() {
            return None;
        }
        let stem = rel.strip_suffix(".tsx").or_else(|| rel.strip_suffix(".ts")).unwrap_or(rel);
        Some((out.join(format!("{stem}.luau")), luau::render(ast, &location.module)))
    });
    let mut written = 0;
    for (path, text) in rendered.into_iter().flatten() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

/// 1-based line and column of `pos` in `file`.
fn line_col(file: P<SourceFile>, pos: u32) -> (usize, usize) {
    let text = file.text();
    let before = &text[..(pos as usize).min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rfind('\n').map_or(before.len(), |nl| before.len() - nl - 1) + 1;
    (line, col)
}

/// Prints every diagnostic and a count per kind (the dotted code, or `TS<code>`); returns the error count.
fn report(diagnostics: &[P<Diagnostic>]) -> usize {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut errors = 0;
    for &d in diagnostics {
        let text = d.localize();
        let category = match d.category() {
            Category::Error => {
                errors += 1;
                "error"
            }
            Category::Warning => "warning",
            _ => "message",
        };
        let at = match (d.file(), d.pos()) {
            (Some(file), Some(pos)) => {
                let (line, col) = line_col(file, pos);
                format!("{}:{line}:{col}", file.file_name())
            }
            _ => "<global>".to_owned(),
        };
        let kind = match text.split_once(": ") {
            Some((code, _)) if code.contains('.') && !code.contains(' ') => code.to_owned(),
            _ if d.code() > 0 => format!("TS{}", d.code()),
            _ => "roblox-ts".to_owned(),
        };
        if d.code() > 0 {
            println!("{at} - {category} {kind}: {text}");
        } else {
            println!("{at} - {category}: {text}");
        }
        *kinds.entry(kind).or_default() += 1;
    }
    for (kind, count) in kinds {
        println!("  {count:>4} {kind}");
    }
    errors
}

fn gate(dir: &Path) -> Result<bool, String> {
    let mut files = Vec::new();
    walk(dir, &mut |p| {
        if p.extension().is_some_and(|e| e == "luau") {
            files.push(p.to_path_buf());
        }
    })?;
    let start = Instant::now();
    let results = lower::parallel::map(&files, |path| {
        let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        luau::gate::compile(&source, 2, 1).map_err(|e| format!("{}: {}", e.line, e.message))
    });
    let mut failed = 0;
    for (path, result) in files.iter().zip(results) {
        if let Err(message) = result {
            failed += 1;
            println!("{}:{message}", path.display());
        }
    }
    println!(
        "gate: {} files at -O2 (Luau {}), {failed} errors, {:.0} ms",
        files.len(),
        luau::gate::LUAU_VERSION,
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(failed == 0)
}
