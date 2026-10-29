//! `opossum migrate`: the one-time conversions into opossum, every one planned before anything is written, so a
//! refusal changes nothing and a second run finds nothing to do. A Rojo-era `place/` and an old `[place]` table move
//! into properties files (`place::plan_migration`). Tessera's `tessera.config.ts` becomes opossum.toml's tables and
//! `tessera.lock` becomes opossum.lock records with no upload and no create: an id whose file is as tessera uploaded
//! it is current, one whose file changed since is marked changed, and a record with no file goes. Imports of tessera's
//! generated modules and of hexagon become `opossum/<set>`, `opossum/resources` and `opossum/test`. What opossum
//! cannot see into (scripts, package.json, other tools' configs) is named in notes for the user to change.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use cloud::{AssetKind, Creator};
use globset::GlobSet;
use media::{ImageOptions, Preset};
use tsrs_ast::{Kind, Node, SourceFileParseOptions};
use tsrs_core::P;
use tsrs_core::ScriptKind;

use crate::config::{self, Config};
use crate::diag::{self, Diag, Severity};
use crate::guard::{Cache, RemoteGuard};
use crate::legacy::{self, Lit, Value};
use crate::lock::{self, Lock};
use crate::plan::count;

const CONFIG: &str = "tessera.config.ts";
const LOCK: &str = "tessera.lock";
const HEXAGON: &str = "@hautajoki/hexagon";
/// What a refusal of something tessera did and opossum does not take over says to do instead.
const BY_HAND: &str = "finish or remove it with tessera first, or move it by hand";

/// Everything one migration writes, in the order it is written.
struct Plan {
    root: PathBuf,
    place: Vec<place::Write>,
    /// opossum.toml's new text, when it changes.
    toml: Option<String>,
    /// The lock with tessera's records in it, the line saying what came over, and whether its text changes.
    lock: Option<(Lock, String, bool)>,
    /// Source files with their imports rewritten, and how many imports that is.
    sources: Vec<(PathBuf, String)>,
    imports: usize,
    /// Removed after everything else is written: tessera's files and its generated modules.
    removals: Vec<PathBuf>,
    /// tessera's generated directory, removed when the removals leave it empty.
    generated: Option<PathBuf>,
    notes: Vec<String>,
    diags: Vec<Diag>,
}

/// `opossum migrate` in the project of `file` (its opossum.toml), printing each file it writes or removes and what
/// it leaves to the user. Ok(false) when something refuses it: then nothing was written.
pub fn run(file: &Path) -> Result<bool, String> {
    let plan = plan(file)?;
    if !plan.diags.is_empty() {
        crate::report(&plan.diags, &plan.root);
        println!("nothing changed");
        return Ok(false);
    }
    let root = &plan.root;
    let shown = |path: &Path| path.strip_prefix(root).unwrap_or(path).display().to_string();
    let io = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    let mut did = false;
    for write in &plan.place {
        did = true;
        match &write.text {
            Some(text) => {
                if let Some(dir) = write.path.parent() {
                    fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
                }
                fs::write(&write.path, text).map_err(|e| io(&write.path, e))?;
                println!("wrote {}", shown(&write.path));
            }
            None => {
                fs::remove_file(&write.path).map_err(|e| io(&write.path, e))?;
                println!("removed {}", shown(&write.path));
            }
        }
    }
    // the lock first: it is what can refuse (the guard, a lock changed on disk), and a rerun imports into it again
    if let Some((mut lock, line, changed)) = plan.lock {
        did = true;
        let guard = RemoteGuard::acquire(&Cache::default_dir()?)?;
        let note = lock::save(&mut lock, root, &guard).map_err(|d| d.human(root))?;
        println!("{} {} {line}", if changed { "wrote" } else { "unchanged:" }, lock::FILE);
        if let Some(note) = note {
            println!("{note}");
        }
    }
    if let Some(text) = &plan.toml {
        did = true;
        let path = root.join(config::FILE);
        fs::write(&path, text).map_err(|e| io(&path, e))?;
        println!("wrote {}", config::FILE);
    }
    for (path, text) in &plan.sources {
        fs::write(path, text).map_err(|e| io(path, e))?;
    }
    if !plan.sources.is_empty() {
        did = true;
        let (n, files) = (plan.imports, plan.sources.len());
        println!(
            "rewrote {n} imports in {files} files under src/ to opossum/<set>, opossum/resources and opossum/test"
        );
    }
    for path in &plan.removals {
        did = true;
        fs::remove_file(path).map_err(|e| io(path, e))?;
        println!("removed {}", shown(path));
    }
    if let Some(dir) = &plan.generated
        && fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none())
    {
        fs::remove_dir(dir).map_err(|e| io(dir, e))?;
        println!("removed {}/", shown(dir));
    }
    for note in &plan.notes {
        println!("note: {note}");
    }
    if !did {
        println!("nothing to migrate");
    }
    Ok(true)
}

fn plan(file: &Path) -> Result<Plan, String> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let root = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut plan = Plan {
        root: root.clone(),
        place: Vec::new(),
        toml: None,
        lock: None,
        sources: Vec::new(),
        imports: 0,
        removals: Vec::new(),
        generated: None,
        notes: Vec::new(),
        diags: Vec::new(),
    };

    // place/ and [place]
    let cut = config::cut_place(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let moved = match &cut {
        Some((_, table)) => {
            place::entries_from_toml(table, file).map_err(|e| format!("{}: [place] {e}", file.display()))?
        }
        None => Vec::new(),
    };
    match place::plan_migration(&root.join("place"), moved) {
        Ok(writes) => plan.place = writes,
        Err(diags) => plan.diags.extend(diags.iter().map(diag::from_place)),
    }
    let mut toml = match cut {
        Some((rest, _)) => {
            plan.toml = Some(rest.clone());
            rest
        }
        None => text,
    };

    // tessera.config.ts
    let ts = root.join(CONFIG);
    let tessera = match fs::read_to_string(&ts) {
        Ok(source) => {
            let mut reader = Reader { file: &ts, text: &source, diags: Vec::new() };
            let tessera = reader.config(&toml);
            plan.diags.append(&mut reader.diags);
            match tessera {
                Some(t) => {
                    if let Some(text) = &t.toml {
                        toml = text.clone();
                        plan.toml = Some(toml.clone());
                    }
                    plan.notes.extend(t.notes.iter().cloned());
                    plan.removals.push(ts.clone());
                    Some(t)
                }
                None => return Ok(plan),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", ts.display())),
    };
    let cfg = config::parse(&toml, root.clone());
    if let (Some(_), Err(e)) = (&tessera, &cfg) {
        let message = format!("the opossum.toml it migrates to does not load: {e}");
        plan.diags.push(Diag::new("migrate.tesseraConfig", Severity::Error, Some(&ts), message));
        return Ok(plan);
    }

    // tessera.lock
    let tessera_lock = root.join(LOCK);
    match fs::read(&tessera_lock) {
        Ok(bytes) => match &cfg {
            Ok(cfg) => {
                plan.lock = import_lock(cfg, tessera.as_ref(), &bytes, &mut plan.diags);
                plan.removals.push(tessera_lock.clone());
            }
            Err(e) => {
                let message = format!("{} must load to import {LOCK} into: {e}", config::FILE);
                plan.diags.push(Diag::new("config", Severity::Error, Some(file), message));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", tessera_lock.display())),
    }

    // imports of the generated modules (with tessera in the project) and of hexagon (always)
    let generated = tessera.as_ref().map_or("src/generated".to_owned(), |t| t.generated.clone());
    let mut modules: Vec<(String, String)> = Vec::new();
    if tessera.is_some() || plan.lock.is_some() {
        let names: Vec<String> = match (&tessera, &cfg) {
            (Some(t), _) => t.sets.iter().map(|s| s.name.clone()).collect(),
            (None, Ok(cfg)) => cfg.assets.iter().map(|s| s.name.clone()).collect(),
            (None, Err(_)) => Vec::new(),
        };
        for name in names.iter().map(String::as_str).chain(["resources"]) {
            modules.push((format!("{generated}/{name}"), format!("opossum/{name}")));
            for ext in [".luau", ".d.ts"] {
                let path = root.join(format!("{generated}/{name}{ext}"));
                if path.is_file() {
                    plan.removals.push(path);
                }
            }
        }
        plan.generated = Some(root.join(&generated));
    }
    rewrite_sources(&mut plan, &generated, &modules);
    if tessera.is_some() || plan.lock.is_some() || plan.imports > 0 {
        let skip: Vec<String> =
            cfg.as_ref().map(|c| c.assets.iter().map(|s| s.root.clone()).collect()).unwrap_or_default();
        leftovers(&mut plan, &generated, &skip);
    }
    Ok(plan)
}

/// What a `tessera.config.ts` came to.
struct Tessera {
    /// opossum.toml with tessera's tables in it; None when it already holds them.
    toml: Option<String>,
    /// tessera's `generated` directory, relative to the project: where its modules were written.
    generated: String,
    sets: Vec<Set>,
    notes: Vec<String>,
}

/// A tessera asset set as tessera discovered it, to check that opossum keys every recorded file the same way.
struct Set {
    name: String,
    root: String,
    include: GlobSet,
    exclude: GlobSet,
}

/// Reads tessera.config.ts's literals, refusing at their offsets.
struct Reader<'a> {
    file: &'a Path,
    text: &'a str,
    diags: Vec<Diag>,
}

impl Reader<'_> {
    fn refuse(&mut self, at: u32, message: String) {
        let before = &self.text[..(at as usize).min(self.text.len())];
        let line = before.matches('\n').count() as u32 + 1;
        let col = (before.len() - before.rfind('\n').map_or(0, |n| n + 1)) as u32 + 1;
        let mut d = Diag::new("migrate.tesseraConfig", Severity::Error, Some(self.file), message);
        (d.line, d.col) = (line, col);
        self.diags.push(d);
    }

    fn str(&mut self, lit: &Lit, path: &str) -> Option<String> {
        match &lit.value {
            Value::Str(s) => Some(s.clone()),
            _ => self.wrong(lit, path, "a string"),
        }
    }

    fn num(&mut self, lit: &Lit, path: &str) -> Option<u64> {
        match &lit.value {
            Value::Num(n) if n.bytes().all(|b| b.is_ascii_digit() || b == b'_') => n.replace('_', "").parse().ok(),
            _ => self.wrong(lit, path, "a whole number"),
        }
    }

    fn bool(&mut self, lit: &Lit, path: &str) -> Option<bool> {
        match &lit.value {
            Value::Bool(b) => Some(*b),
            _ => self.wrong(lit, path, "true or false"),
        }
    }

    fn strings(&mut self, lit: &Lit, path: &str) -> Option<Vec<String>> {
        match &lit.value {
            Value::List(items) => items.iter().map(|i| self.str(i, path)).collect(),
            _ => self.wrong(lit, path, "a list of strings"),
        }
    }

    fn object<'l>(&mut self, lit: &'l Lit, path: &str) -> Option<&'l [(String, Lit)]> {
        match &lit.value {
            Value::Object(fields) => Some(fields),
            _ => self.wrong(lit, path, "an object"),
        }
    }

    fn wrong<T>(&mut self, lit: &Lit, path: &str, what: &str) -> Option<T> {
        self.refuse(lit.at, format!("`{path}` must be {what}"));
        None
    }

    fn unknown(&mut self, lit: &Lit, path: &str) {
        self.refuse(lit.at, format!("opossum has no `{path}`; {BY_HAND}"));
    }

    /// opossum.toml's text `toml` with tessera's declarations in it; None after refusing.
    fn config(&mut self, toml: &str) -> Option<Tessera> {
        let lit = match legacy::config(self.text) {
            Ok(lit) => lit,
            Err((at, message)) => {
                self.refuse(at, message);
                return None;
            }
        };
        let fields = self.object(&lit, "export default")?;
        let mut t = Tessera { toml: None, generated: "src/generated".into(), sets: Vec::new(), notes: Vec::new() };
        let mut top = String::new();
        let mut tables = String::new();
        // the tables, written in opossum.toml's own order once the top-level keys are read
        let mut groups: Vec<(&str, &Lit)> = Vec::new();
        let mut default = None;
        for (key, value) in fields {
            match key.as_str() {
                "creator" => {
                    let mut creator = (None, None);
                    for (k, v) in self.object(value, key).unwrap_or_default() {
                        match k.as_str() {
                            "kind" => creator.0 = self.str(v, "creator.kind"),
                            "id" => creator.1 = self.num(v, "creator.id"),
                            _ => self.unknown(v, &format!("creator.{k}")),
                        }
                    }
                    match creator {
                        (Some(kind), Some(id)) if kind == "user" || kind == "group" => {
                            let _ = writeln!(top, "creator = {{ {kind} = {id} }}");
                        }
                        _ => self.refuse(value.at, "`creator` must be { kind: 'user' | 'group', id }".into()),
                    }
                }
                "defaultEnvironment" => default = self.str(value, key),
                "build" => {
                    if let Some(argv) = self.strings(value, key) {
                        t.notes
                            .push(format!("tessera built with `{}`; opossum builds the place itself", argv.join(" ")));
                    }
                }
                "generated" => {
                    if let Some(dir) = self.str(value, key) {
                        match config::normalize(&dir) {
                            Some(dir) if !dir.is_empty() => t.generated = dir,
                            _ => self.refuse(value.at, "`generated` must be a directory inside the project".into()),
                        }
                    }
                }
                "assets" | "places" | "environments" | "passes" | "products" | "badges" | "experience" => {
                    groups.push((key, value));
                }
                _ => self.unknown(value, key),
            }
        }
        let mut environments = 0;
        for group in ["assets", "places", "environments", "passes", "products", "badges", "experience"] {
            let Some(&(_, value)) = groups.iter().find(|(g, _)| *g == group) else { continue };
            match group {
                "assets" => self.sets(value, &t.generated.clone(), &mut t.sets, &mut tables),
                "places" => self.places(value, &mut tables),
                "environments" => {
                    for (name, env) in self.object(value, group).unwrap_or_default() {
                        self.environment(name, env, &mut tables);
                        environments += 1;
                    }
                }
                "badges" => self.badges(value, &mut tables),
                "experience" => self.experience(value, &mut tables),
                _ => self.sales(group, value, &mut tables),
            }
        }
        match (default, environments) {
            (Some(name), 2..) => {
                let _ = writeln!(top, "defaultEnvironment = {}", lock::string(&name));
            }
            (None, 2..) => self.refuse(lit.at, "choose `defaultEnvironment`: there are several environments".into()),
            _ => {}
        }
        if !self.diags.is_empty() {
            return None;
        }
        t.toml = self.splice(toml, &top, &tables, &mut t.notes)?;
        Some(t)
    }

    fn places(&mut self, value: &Lit, tables: &mut String) {
        let places = self.object(value, "places").unwrap_or_default();
        match places {
            [] => {}
            [(name, place)] => {
                for (k, v) in self.object(place, &format!("places.{name}")).unwrap_or_default() {
                    if !matches!(k.as_str(), "artifact" | "root") {
                        self.unknown(v, &format!("places.{name}.{k}"));
                    }
                }
                if name != "main" {
                    let _ = write!(tables, "\n[places.{}]\n", lock::key(name));
                }
            }
            _ => self.refuse(value.at, "several places: map each place to a build profile in [places] by hand".into()),
        }
    }

    fn environment(&mut self, name: &str, value: &Lit, tables: &mut String) {
        let mut universe = None;
        let mut places = Vec::new();
        for (k, v) in self.object(value, &format!("environments.{name}")).unwrap_or_default() {
            let path = format!("environments.{name}.{k}");
            match k.as_str() {
                "universeId" => universe = self.num(v, &path),
                "places" => {
                    for (place, id) in self.object(v, &path).unwrap_or_default() {
                        if let Some(id) = self.num(id, &format!("{path}.{place}")) {
                            places.push(format!("{} = {id}", lock::key(place)));
                        }
                    }
                }
                _ => self.unknown(v, &path),
            }
        }
        let _ = write!(tables, "\n[environments.{}]\n", lock::key(name));
        if let Some(u) = universe {
            let _ = writeln!(tables, "universeId = {u}");
        }
        let _ = writeln!(tables, "places = {{ {} }}", places.join(", "));
    }

    /// `[assets]`: a set opossum's discovery keys the same way, with the image options its pipeline has. Excluding
    /// another set's root is what opossum's nested roots do already, so that exclude goes.
    fn sets(&mut self, value: &Lit, generated: &str, sets: &mut Vec<Set>, tables: &mut String) {
        let declared = self.object(value, "assets").unwrap_or_default();
        // every set's root first, for the excludes that name another's
        let roots: Vec<Option<String>> = declared
            .iter()
            .map(|(_, set)| match &set.value {
                Value::Object(fields) => match fields.iter().find(|(k, _)| k == "root") {
                    Some((_, Lit { value: Value::Str(root), .. })) => config::normalize(root),
                    Some(_) => None,
                    None => Some(String::new()),
                },
                _ => None,
            })
            .collect();
        let mut lines = Vec::new();
        for (i, (name, set)) in declared.iter().enumerate() {
            let at = format!("assets.{name}");
            let valid = name.bytes().enumerate().all(|(i, b)| {
                b.is_ascii_alphabetic() || i > 0 && (b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            });
            if !valid || name == "test" || name == "resources" {
                let message = format!(
                    "a set is the module opossum/<name>, so `{name}` must match [A-Za-z][A-Za-z0-9_-]* and not be test \
                     or resources; rename it with tessera first"
                );
                self.refuse(set.at, message);
                continue;
            }
            let mut include: Option<Vec<String>> = None;
            let mut exclude: Vec<String> = Vec::new();
            let mut image = ImageOptions::DEFAULT;
            let mut strip = false;
            for (k, v) in self.object(set, &at).unwrap_or_default() {
                let path = format!("{at}.{k}");
                match k.as_str() {
                    "root" => {}
                    "include" => include = self.globs(v, &path),
                    "exclude" => exclude = self.globs(v, &path).unwrap_or_default(),
                    "kind" => match self.str(v, &path).as_deref() {
                        None | Some("auto" | "animation") => {}
                        Some(kind) => self.refuse(
                            v.at,
                            format!(
                                "opossum reads each file's kind from the file, so a set cannot be `{kind}`; {BY_HAND}"
                            ),
                        ),
                    },
                    // imports are rewritten where tessera writes a set's module by default, and nowhere else
                    "output" => {
                        let out = self.str(v, &path).as_deref().and_then(config::normalize);
                        let module = format!("{generated}/{name}");
                        let default = [format!("{module}.luau"), format!("{module}.d.ts"), module.clone()];
                        if !out.is_some_and(|out| out == generated || default.contains(&out)) {
                            let message = format!(
                                "opossum rewrites imports of the set's module at {module}, and `{path}` puts it \
                                 elsewhere; drop the output with tessera first"
                            );
                            self.refuse(v.at, message);
                        }
                    }
                    "typescript" => {}
                    "stripExtensions" => strip = self.bool(v, &path).unwrap_or(true),
                    "style" if matches!(&v.value, Value::Str(s) if s == "nested") => {}
                    "content" if matches!(v.value, Value::Bool(false)) => {}
                    "image" => self.image(v, &path, &mut image),
                    _ => self.unknown(v, &path),
                }
            }
            if !strip {
                let message = format!(
                    "opossum keys files without their extension, and `{at}` keys them with it; set stripExtensions and \
                     change the imports that use the keys first"
                );
                self.refuse(set.at, message);
            }
            let Some(include) = include else {
                self.refuse(set.at, format!("`{at}.include` is missing"));
                continue;
            };
            let Some(root) = roots[i].clone() else {
                self.refuse(set.at, format!("`{at}.root` must be a directory inside the project"));
                continue;
            };
            // a set's exclude of another set's root inside it is what the nested root does already
            let nested = |p: &String| {
                p.strip_suffix("/**").is_some_and(|sub| roots.iter().flatten().any(|r| *r == join(&root, sub)))
            };
            let kept: Vec<String> = exclude.iter().filter(|p| !nested(p)).cloned().collect();
            let all = matches!(include.as_slice(), [p] if p == "**" || p == "**/*");
            let globs = |patterns: &[String]| config::glob_set(patterns, &at).unwrap_or_else(|_| GlobSet::empty());
            sets.push(Set {
                name: name.clone(),
                root: root.clone(),
                include: globs(&include),
                exclude: globs(&exclude),
            });
            if all && kept.is_empty() && image == ImageOptions::DEFAULT {
                lines.push(format!("{} = {}", lock::key(name), lock::string(&root)));
                continue;
            }
            let mut fields = vec![format!("root = {}", lock::string(&root))];
            if image != ImageOptions::DEFAULT {
                let (preset, bleed) = (lock::string(image.preset.name()), image.bleed);
                fields.push(format!("image = {{ preset = {preset}, bleed = {bleed} }}"));
            }
            let list = |l: &[String]| l.iter().map(|p| lock::string(p)).collect::<Vec<_>>().join(", ");
            if !all {
                fields.push(format!("include = [{}]", list(&include)));
            }
            if !kept.is_empty() {
                fields.push(format!("exclude = [{}]", list(&kept)));
            }
            lines.push(format!("{} = {{ {} }}", lock::key(name), fields.join(", ")));
        }
        if !lines.is_empty() {
            let _ = write!(tables, "\n[assets]\n{}\n", lines.join("\n"));
        }
    }

    /// Glob patterns as opossum reads them: tessera's own syntax except `{a,b}`, which tessera took literally.
    fn globs(&mut self, lit: &Lit, path: &str) -> Option<Vec<String>> {
        let globs = self.strings(lit, path)?;
        if globs.iter().any(|g| g.contains(['{', '}'])) {
            let message =
                format!("`{path}` has a brace, which tessera matched literally and opossum as alternatives; {BY_HAND}");
            self.refuse(lit.at, message);
            return None;
        }
        if let Err(e) = config::glob_set(&globs, path) {
            self.refuse(lit.at, e);
            return None;
        }
        Some(globs)
    }

    /// The image options opossum's pipeline has: a preset (its box, contained) and bleed. Every other knob must be
    /// at the value tessera's preset would have given it.
    fn image(&mut self, value: &Lit, path: &str, image: &mut ImageOptions) {
        let (mut width, mut height) = (0, 0);
        for (k, v) in self.object(value, path).unwrap_or_default() {
            let at = format!("{path}.{k}");
            let other = |reader: &mut Self| {
                let message = format!("opossum's image pipeline has no `{at}` but tessera's default; {BY_HAND}");
                reader.refuse(v.at, message);
            };
            match k.as_str() {
                "preset" => match self.str(v, &at).as_deref().map(Preset::parse) {
                    Some(Some(preset)) => image.preset = preset,
                    Some(None) => self.refuse(v.at, format!("`{at}` is not one of tessera's presets")),
                    None => {}
                },
                "bleed" => image.bleed = self.bool(v, &at).unwrap_or(true),
                "width" => width = self.num(v, &at).unwrap_or(0),
                "height" => height = self.num(v, &at).unwrap_or(0),
                "fit" if matches!(&v.value, Value::Str(s) if s == "contain") => {}
                "edge" if matches!(&v.value, Value::Str(s) if s == "transparent") => {}
                "alphaThreshold" if matches!(&v.value, Value::Num(n) if n == "0") => {}
                "svgScale" if matches!(&v.value, Value::Num(n) if n == "1") => {}
                "fit" | "edge" | "alphaThreshold" | "svgScale" => other(self),
                _ => self.unknown(v, &at),
            }
        }
        if !legacy::same_box(image.preset, width, height) {
            let message =
                format!("opossum's preset {} has its own size, not {width}×{height}; {BY_HAND}", image.preset.name());
            self.refuse(value.at, message);
        }
    }

    fn sales(&mut self, group: &str, value: &Lit, tables: &mut String) {
        for (key, sale) in self.object(value, group).unwrap_or_default() {
            let mut lines = String::new();
            for (k, v) in self.object(sale, &format!("{group}.{key}")).unwrap_or_default() {
                let path = format!("{group}.{key}.{k}");
                match k.as_str() {
                    "name" | "description" | "icon" => self.field_str(&mut lines, k, v, &path),
                    "price" => self.field_num(&mut lines, k, v, &path),
                    "forSale" | "managedPricing" => self.field_bool(&mut lines, k, v, &path),
                    _ => self.unknown(v, &path),
                }
            }
            let _ = write!(tables, "\n[{group}.{}]\n{lines}", lock::key(key));
        }
    }

    fn badges(&mut self, value: &Lit, tables: &mut String) {
        for (key, badge) in self.object(value, "badges").unwrap_or_default() {
            let mut lines = String::new();
            for (k, v) in self.object(badge, &format!("badges.{key}")).unwrap_or_default() {
                let path = format!("badges.{key}.{k}");
                match k.as_str() {
                    "name" | "description" | "icon" => self.field_str(&mut lines, k, v, &path),
                    "expectedCost" => self.field_num(&mut lines, k, v, &path),
                    "enabled" => self.field_bool(&mut lines, k, v, &path),
                    _ => self.unknown(v, &path),
                }
            }
            let _ = write!(tables, "\n[badges.{}]\n{lines}", lock::key(key));
        }
    }

    /// `[experience]`, its thumbnails an array in `thumbnailOrder`'s order (else the order they are declared in).
    fn experience(&mut self, value: &Lit, tables: &mut String) {
        let mut lines = String::new();
        let mut thumbnails: Vec<(&str, &Lit)> = Vec::new();
        let mut order: Option<Vec<String>> = None;
        for (k, v) in self.object(value, "experience").unwrap_or_default() {
            let path = format!("experience.{k}");
            match k.as_str() {
                "name" | "description" | "icon" => self.field_str(&mut lines, k, v, &path),
                "thumbnails" => {
                    thumbnails.extend(self.object(v, &path).unwrap_or_default().iter().map(|(k, v)| (k.as_str(), v)))
                }
                "thumbnailOrder" => order = self.strings(v, &path),
                _ => self.unknown(v, &path),
            }
        }
        let _ = write!(tables, "\n[experience]\n{lines}");
        if let Some(order) = order {
            let mut sorted = Vec::new();
            for key in &order {
                match thumbnails.iter().find(|(k, _)| k == key) {
                    Some(t) => sorted.push(*t),
                    None => {
                        self.refuse(value.at, format!("`experience.thumbnailOrder` names {key}, which is no thumbnail"))
                    }
                }
            }
            if sorted.len() != thumbnails.len() {
                self.refuse(value.at, "`experience.thumbnailOrder` must list every thumbnail once".into());
            }
            thumbnails = sorted;
        }
        for (key, thumbnail) in thumbnails {
            let mut lines = format!("key = {}\n", lock::string(key));
            for (k, v) in self.object(thumbnail, &format!("experience.thumbnails.{key}")).unwrap_or_default() {
                let path = format!("experience.thumbnails.{key}.{k}");
                match k.as_str() {
                    "image" | "altText" => self.field_str(&mut lines, k, v, &path),
                    _ => self.unknown(v, &path),
                }
            }
            let _ = write!(tables, "\n[[experience.thumbnails]]\n{lines}");
        }
    }

    fn field_str(&mut self, out: &mut String, key: &str, value: &Lit, path: &str) {
        if let Some(s) = self.str(value, path) {
            let _ = writeln!(out, "{key} = {}", lock::string(&s));
        }
    }

    fn field_num(&mut self, out: &mut String, key: &str, value: &Lit, path: &str) {
        if let Some(n) = self.num(value, path) {
            let _ = writeln!(out, "{key} = {n}");
        }
    }

    fn field_bool(&mut self, out: &mut String, key: &str, value: &Lit, path: &str) {
        if let Some(b) = self.bool(value, path) {
            let _ = writeln!(out, "{key} = {b}");
        }
    }

    /// `toml` with the `top` keys after its own top-level keys, `tables` at its end and `placeId` gone (the default
    /// environment's root place is the Studio place now). None after refusing, Some(None) when it already holds
    /// exactly these declarations (a migration that stopped before removing tessera.config.ts).
    fn splice(&mut self, toml: &str, top: &str, tables: &str, notes: &mut Vec<String>) -> Option<Option<String>> {
        let ours: toml::Table = match format!("{top}{tables}").parse() {
            Ok(t) => t,
            Err(e) => {
                self.refuse(0, format!("the tables it migrates to do not parse: {e}"));
                return None;
            }
        };
        let mut doc: toml::Table = toml.parse().ok()?;
        let held = ours.iter().filter(|(k, v)| doc.get(*k) == Some(v)).count();
        if held == ours.len() && doc.get("placeId").is_none() {
            return Some(None);
        }
        if let Some(k) = ours.keys().find(|k| doc.contains_key(*k)) {
            let message = format!("{} already declares `{k}`; keep it there or here, not in both", config::FILE);
            self.refuse(0, message);
            return None;
        }
        let place_id =
            |line: &str| line.trim_start().strip_prefix("placeId").is_some_and(|r| r.trim_start().starts_with('='));
        let lines: Vec<&str> = toml.split_inclusive('\n').collect();
        let header = |l: &str| l.trim_start().starts_with('[') && l.parse::<toml::Table>().is_ok();
        let first = lines.iter().position(|l| header(l)).unwrap_or(lines.len());
        let last_key = lines[..first].iter().rposition(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && !place_id(l)
        });
        let mut out = String::with_capacity(toml.len() + top.len() + tables.len());
        if last_key.is_none() {
            out.push_str(top);
        }
        for (i, line) in lines.iter().enumerate() {
            if i < first && place_id(line) {
                continue;
            }
            out.push_str(line);
            if Some(i) == last_key {
                if !line.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(top);
            }
        }
        let mut out = format!("{}\n{tables}", out.trim_end());
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if let Some(id) = doc.remove("placeId") {
            notes.push(format!("dropped placeId = {id}: Studio auto-connects to the default environment's root place"));
        }
        doc.extend(ours);
        if out.parse::<toml::Table>().ok() != Some(doc) {
            self.refuse(0, format!("could not add tessera's tables to {}; move them by hand", config::FILE));
            return None;
        }
        Some(Some(out))
    }
}

fn join(dir: &str, rel: &str) -> String {
    match (dir, rel) {
        ("", rel) => rel.to_owned(),
        (dir, "") => dir.to_owned(),
        (dir, rel) => format!("{dir}/{rel}"),
    }
}

/// One tessera record whose file opossum has: what the lock gets for it.
struct Import<'r> {
    record: &'r legacy::Record,
    file: String,
    set: usize,
    ext: String,
}

/// tessera.lock's records as opossum.lock records, into the lock already there; None after refusing. Every record
/// must find its file where opossum's discovery keys it, and where tessera's did when its config is being migrated.
fn import_lock(
    cfg: &Config,
    tessera: Option<&Tessera>,
    bytes: &[u8],
    diags: &mut Vec<Diag>,
) -> Option<(Lock, String, bool)> {
    let root = &cfg.root;
    let at = root.join(LOCK);
    let refuse = |code: &str, message: String| Diag::new(code, Severity::Error, Some(&at), message);
    let records = match legacy::lock(bytes) {
        Ok(records) => records,
        Err(reasons) => {
            diags.extend(reasons.into_iter().map(|r| refuse("migrate.lock", r)));
            return None;
        }
    };
    let (found, discovered) = crate::assets::discover(root, &cfg.assets);
    if discovered.iter().any(Diag::is_error) {
        diags.extend(discovered);
        return None;
    }
    let ours: HashMap<String, usize> =
        found.iter().enumerate().map(|(i, a)| (format!("{}/{}", cfg.assets[a.set].name, a.key), i)).collect();
    let theirs = tessera.map(|t| tessera_keys(root, &t.sets));
    let mut imports = Vec::new();
    let mut orphans = 0;
    for r in &records {
        let mine = ours.get(&r.key).map(|&i| &found[i]);
        if let Some(theirs) = &theirs
            && let Some(file) = theirs.get(&r.key)
            && mine.is_none_or(|m| m.file != *file)
        {
            let now = mine.map_or("no file".to_owned(), |m| m.file.clone());
            let message = format!(
                "{} is {file} under tessera but {now} under the migrated {}; tessera's include and exclude select \
                 differently from opossum's, so {BY_HAND}",
                r.key,
                config::FILE
            );
            diags.push(Diag::new("migrate.tesseraConfig", Severity::Error, Some(&root.join(CONFIG)), message));
            continue;
        }
        // an orphan is a file gone from a set opossum knows; a set it does not know would drop every id in it
        let Some(a) = mine else {
            let set = r.key.split_once('/').map_or(r.key.as_str(), |(set, _)| set);
            if cfg.assets.iter().any(|s| s.name == set) {
                orphans += 1;
            } else {
                let message = format!(
                    "tessera set `{set}` is not an [assets] set in {}; name the set holding its files so and migrate \
                     again, or its ids are lost",
                    config::FILE
                );
                diags.push(refuse("migrate.lock", message));
            }
            continue;
        };
        if let (Some(theirs), Some(ours)) = (r.creator, cfg.creator)
            && theirs != ours
        {
            let message =
                format!("{} was uploaded as {} but {} says {}", a.file, name(theirs), config::FILE, name(ours));
            diags.push(refuse("migrate.lock", message));
            continue;
        }
        imports.push(Import { record: r, file: a.file.clone(), set: a.set, ext: a.ext.clone() });
    }

    // every file read once, on all cores: opossum's hash, tessera's fingerprint, and what it goes up as
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(imports.len()).max(1);
    let mut read: Vec<Result<(String, String, AssetKind), String>> =
        imports.iter().map(|_| Err(String::new())).collect();
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(im) = imports.get(i) else { break done };
                        let image = cfg.assets[im.set].image;
                        let result =
                            fs::read(root.join(&im.file)).map_err(|e| format!("{}: {e}", im.file)).and_then(|b| {
                                let kind = crate::assets::kind(&im.ext, &b).map_err(|e| format!("{}: {e}", im.file))?;
                                Ok((
                                    crate::assets::hash(&im.ext, image, &b),
                                    legacy::fingerprint(&b, &im.ext, image),
                                    kind,
                                ))
                            });
                        done.push((i, result));
                    }
                })
            })
            .collect();
        for w in workers {
            for (i, r) in w.join().unwrap() {
                read[i] = r;
            }
        }
    });

    let (mut lock, problems) = match lock::read(root) {
        Ok(read) => read,
        Err(e) => {
            diags.push(Diag::new("lock.io", Severity::Error, Some(&root.join(lock::FILE)), e));
            return None;
        }
    };
    diags.extend(problems.into_iter().filter(Diag::is_error));
    diags.extend(lock::recovered(root));
    if let Some(c) = cfg.creator {
        diags.extend(lock.check_creator(root, c));
        lock.creator = Some(c);
    }
    let before = lock::render(&lock);
    let (mut current, mut changed, mut kept) = (0, 0, 0);
    for (im, read) in imports.iter().zip(read) {
        let (hash, fingerprint, kind) = match read {
            Ok(read) => read,
            Err(e) => {
                diags.push(Diag::new("asset.root", Severity::Error, Some(&root.join(&im.file)), e));
                continue;
            }
        };
        let file = &im.file;
        let animation = kind == AssetKind::Animation;
        if im.record.animation != animation {
            let message = if im.record.animation {
                format!("{file} was uploaded as an Animation but is not a KeyframeSequence or CurveAnimation")
            } else {
                format!(
                    "{file} went up as a Model, but it is a lone KeyframeSequence, which opossum uploads as an \
                     Animation; upload it in an animation set with tessera first, or move it out of the asset roots"
                )
            };
            diags.push(refuse("migrate.lock", message));
            continue;
        }
        let fresh = fingerprint == im.record.fingerprint;
        let new = lock::Asset {
            id: Some(im.record.id),
            hash: Some(if fresh { hash.clone() } else { "0".repeat(32) }),
            ..lock::Asset::default()
        };
        // a merge's several records for the file settle on the one holding it as it is
        let _ = lock.settle(root, file, &hash);
        // the record already there wins when it holds the file as it is, tessera's when it does instead
        let take = match lock.assets.get(file) {
            None => Ok(true),
            Some(old) if *old == new || old.id.is_some() && old.hash.as_deref() == Some(&hash) => Ok(false),
            Some(old) if fresh && old.next.is_none() || old.id == new.id => Ok(fresh),
            Some(old) => Err(old.id),
        };
        match take {
            Ok(true) => {
                lock.assets.insert(file.clone(), new);
            }
            Ok(false) => kept += 1,
            Err(old) => {
                let theirs = old.map_or("an upload in flight".to_owned(), |id| format!("id {id}"));
                let message = format!(
                    "{file} is {theirs} in {} and id {} in {LOCK}, and neither holds the file as it is now; delete \
                     the record you do not want from {} and migrate again",
                    lock::FILE,
                    im.record.id,
                    lock::FILE
                );
                diags.push(Diag::new("migrate.clash", Severity::Error, Some(&root.join(lock::FILE)), message));
                continue;
            }
        }
        if fresh { current += 1 } else { changed += 1 }
    }
    let mut line = format!(
        "({} assets: {} current, {} changed since upload, {} orphans dropped",
        count(current + changed),
        count(current),
        count(changed),
        count(orphans)
    );
    if kept > 0 {
        let _ = write!(line, ", {} already there", count(kept));
    }
    line.push(')');
    let changed_text = lock::render(&lock) != before || !root.join(lock::FILE).exists();
    Some((lock, line, changed_text))
}

fn name(c: Creator) -> String {
    match c {
        Creator::User(id) => format!("user:{id}"),
        Creator::Group(id) => format!("group:{id}"),
    }
}

/// `<set>/<logical path>` → project file, as tessera discovered them: every regular file under the root (dot-files
/// too, symbolic links not), selected by include and exclude on the whole path under the root, keyed by that path
/// without its last extension.
fn tessera_keys(root: &Path, sets: &[Set]) -> HashMap<String, String> {
    let mut keys = HashMap::new();
    for set in sets {
        let mut files = Vec::new();
        walk(&root.join(&set.root), "", &mut files);
        for rel in files {
            if !set.include.is_match(&rel) || set.exclude.is_match(&rel) {
                continue;
            }
            let name_at = rel.rfind('/').map_or(0, |i| i + 1);
            let logical = match rel[name_at..].rfind('.') {
                Some(dot) => &rel[..name_at + dot],
                None => &rel[..],
            };
            keys.entry(format!("{}/{logical}", set.name)).or_insert_with(|| join(&set.root, &rel));
        }
    }
    keys
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let (Ok(name), Ok(t)) = (entry.file_name().into_string(), entry.file_type()) else { continue };
        let rel = join(rel, &name);
        if t.is_dir() {
            walk(&entry.path(), &rel, out);
        } else if t.is_file() {
            out.push(rel);
        }
    }
}

/// Rewrites the module specifier strings of every TS file under `src/` that name a module in `modules` (by the
/// project path it resolves to: from the file for `./`, from `src/` otherwise) or hexagon. Only the string's bytes
/// change.
fn rewrite_sources(plan: &mut Plan, generated: &str, modules: &[(String, String)]) {
    let mut files = Vec::new();
    let skip = plan.root.join(generated);
    sources(&plan.root.join("src"), &skip, &mut files);
    let needle = generated.rsplit('/').next().unwrap_or(generated);
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else { continue };
        if !text.contains(HEXAGON) && (modules.is_empty() || !text.contains(needle)) {
            continue;
        }
        let rel = path.strip_prefix(&plan.root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for (start, end, spec) in specifiers(&text, &path) {
            let target = if spec == HEXAGON {
                Some("opossum/test".to_owned())
            } else {
                let resolved = if spec.starts_with("./") || spec.starts_with("../") {
                    config::normalize(&join(dir, &spec))
                } else {
                    Some(join("src", &spec))
                };
                resolved.and_then(|r| modules.iter().find(|(m, _)| *m == r).map(|(_, to)| to.clone()))
            };
            if let Some(target) = target {
                edits.push((start, end, target));
            }
        }
        if edits.is_empty() {
            continue;
        }
        let mut out = text.clone();
        for (start, end, target) in edits.iter().rev() {
            out.replace_range(start..end, target);
        }
        plan.imports += edits.len();
        plan.sources.push((path, out));
    }
}

fn sources(dir: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let (path, name) = (entry.path(), entry.file_name());
        let name = name.to_string_lossy();
        let Ok(t) = entry.file_type() else { continue };
        if name.starts_with('.') || name == "node_modules" || path == skip {
            continue;
        }
        if t.is_dir() {
            sources(&path, skip, out);
        } else if t.is_file() && (name.ends_with(".ts") || name.ends_with(".tsx")) {
            out.push(path);
        }
    }
}

/// The byte range inside the quotes and the text of every module specifier in a TS file: imports, exports from,
/// `import()`, `import x = require()` and `import("…")` types. A specifier written with escapes is left alone.
fn specifiers(text: &str, path: &Path) -> Vec<(usize, usize, String)> {
    let name = path.to_string_lossy().replace('\\', "/");
    let kind = if name.ends_with(".tsx") { ScriptKind::TSX } else { ScriptKind::TS };
    let file =
        tsrs_parser::parse_source_file(SourceFileParseOptions { file_name: name, ..Default::default() }, text, kind);
    let mut found = Vec::new();
    visit(file.as_node(), &mut found);
    let mut out = Vec::new();
    for lit in found {
        if lit.kind() != Kind::StringLiteral {
            continue;
        }
        let (start, end) = legacy::span(text, lit);
        let (start, end) = (start + 1, end.saturating_sub(1));
        if start <= end && text.get(start..end) == Some(lit.text()) {
            out.push((start, end, lit.text().to_owned()));
        }
    }
    out.sort();
    out
}

fn visit(node: P<Node>, out: &mut Vec<P<Node>>) {
    match node.kind() {
        Kind::ImportDeclaration | Kind::ExportDeclaration => out.extend(node.module_specifier()),
        Kind::ExternalModuleReference => out.extend(node.expression()),
        Kind::CallExpression if node.expression().is_some_and(|e| e.kind() == Kind::ImportKeyword) => {
            out.extend(node.arguments().first().copied());
        }
        Kind::ImportType => {
            let arg = node.as_import_type_node().argument;
            if arg.kind() == Kind::LiteralType {
                out.push(arg.as_literal_type_node().literal);
            }
        }
        _ => {}
    }
    node.for_each_child(&mut |child| {
        visit(child, out);
        false
    });
}

/// A note naming every line outside the TS sources that still mentions tessera, hexagon or the generated directory,
/// with what each becomes: package.json scripts and dependencies, rokit.toml, .gitignore, tooling scripts.
fn leftovers(plan: &mut Plan, generated: &str, asset_roots: &[String]) {
    let mut files = Vec::new();
    let removed: Vec<PathBuf> = plan.removals.clone();
    texts(&plan.root, &plan.root, asset_roots, &mut files);
    let word = |line: &str, w: &str| {
        line.match_indices(w).any(|(i, _)| {
            let before = line[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let after = line[i + w.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            before && after
        })
    };
    let mut lines = Vec::new();
    for path in files.iter().filter(|p| !removed.contains(p)) {
        let Ok(text) = fs::read_to_string(path) else { continue };
        let shown = path.strip_prefix(&plan.root).unwrap_or(path).display().to_string();
        for (n, line) in text.lines().enumerate() {
            if word(line, "tessera") || word(line, "hexagon") || line.contains(generated) {
                let line = line.trim();
                let line = if line.len() > 120 {
                    format!("{}…", &line[..line.floor_char_boundary(120)])
                } else {
                    line.to_owned()
                };
                lines.push(format!("\n  {shown}:{}: {line}", n + 1));
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    plan.notes.push(format!(
        "these lines still name tessera, hexagon or {generated}, which opossum leaves to you: `tessera build`/`deploy` \
         become `opossum build`/`deploy`, `tessera generate` goes, `tessera run` becomes `opossum exec --cloud --head` \
         (`--fresh`: `opossum exec --cloud`), `hexagon test`/`bench` become `opossum test --local`/`bench --local` \
         (`--cloud`: `--cloud`; plain `opossum test` runs in Studio), {generated}/<set>.luau is \
         .opossum/modules/<set>.luau, and the @tessera-rbx/config and @hautajoki/hexagon packages and rokit's tessera \
         go{}",
        lines.concat()
    ));
}

/// Text files a project's tooling lives in, outside `src/`'s TS (rewritten already), `node_modules`, dot-directories,
/// asset roots and `place/`.
fn texts(root: &Path, dir: &Path, asset_roots: &[String], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let Ok(t) = entry.file_type() else { continue };
        if t.is_dir() {
            let skipped =
                name.starts_with('.') || name == "node_modules" || rel == "place" || asset_roots.contains(&rel);
            if !skipped {
                texts(root, &path, asset_roots, out);
            }
            continue;
        }
        let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
        let ts = matches!(ext, "ts" | "tsx") && rel.starts_with("src/");
        let text = matches!(
            ext,
            "luau" | "lua" | "ts" | "tsx" | "js" | "mjs" | "cjs" | "json" | "toml" | "sh" | "yml" | "yaml"
        );
        let small = entry.metadata().is_ok_and(|m| m.len() < 1 << 20);
        if t.is_file() && small && !ts && (text || name == ".gitignore") && name != lock::FILE {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn project(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("opossum-migrate-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, bytes) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        fs::canonicalize(root).unwrap()
    }

    /// A tessera.lock holding these resources in its global scope and `environment` in an environment's.
    fn tessera_lock(global: &[serde_json::Value], environment: &[serde_json::Value]) -> Vec<u8> {
        let json = serde_json::json!({ "version": 1, "files": [
            { "version": 1, "scope": { "kind": "environment", "name": "dev" }, "resources": environment },
            { "version": 1, "scope": { "kind": "global" }, "resources": global },
        ]});
        let mut out = b"TESSERA\0L\x01".to_vec();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(json.to_string().as_bytes()).unwrap();
        out.extend(gz.finish().unwrap());
        out
    }

    fn asset(key: &str, id: u64, fingerprint: &str) -> serde_json::Value {
        let fields = serde_json::json!({ "creatorId": "7", "creatorKind": "user", "inputFingerprint": fingerprint,
            "mime": "image/png", "outputSHA256": "00" });
        serde_json::json!({ "kind": "asset", "key": key, "remoteIds": { "asset": id.to_string() },
            "applied": { "fingerprint": "sha256:00", "fields": fields } })
    }

    const TOML: &str = "name = \"t\"\ncreator = { user = 7 }\n\n[routes]\nshared = \"ReplicatedStorage/shared\"\n\n[assets]\nart = \"art\"\n";
    const MODEL: &[u8] = br#"<roblox version="4"><Item class="Model" referent="RBX0"><Properties><string name="Name">m</string></Properties></Item></roblox>"#;

    fn codes(plan: &Plan) -> Vec<&str> {
        plan.diags.iter().map(|d| d.code.as_str()).collect()
    }

    #[test]
    fn a_tessera_lock_keeps_current_ids_marks_changed_ones_and_drops_orphans() {
        let fp = legacy::fingerprint(b"a", ".png", ImageOptions::DEFAULT);
        let lock =
            tessera_lock(&[asset("art/a", 11, &fp), asset("art/b", 12, "stale"), asset("art/gone", 13, "x")], &[]);
        let root = project(
            "lock",
            &[("opossum.toml", TOML.as_bytes()), ("art/a.png", b"a"), ("art/b.ogg", b"b"), ("tessera.lock", &lock)],
        );
        let plan = plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), Vec::<&str>::new(), "{:?}", plan.diags);
        let (lock, line, changed) = plan.lock.as_ref().unwrap();
        let a = crate::assets::hash(".png", ImageOptions::DEFAULT, b"a");
        let expected = format!(
            "# opossum.lock: ids opossum created on Roblox. Commit it. opossum keeps it; a merge needs no hand edits.\n\
             version = 1\ncreator = {{ user = 7 }}\n\nassets.\"art/a.png\" = {{ id = 11, hash = \"{a}\" }}\n\
             assets.\"art/b.ogg\" = {{ id = 12, hash = \"{}\" }}\n",
            "0".repeat(32)
        );
        assert_eq!(lock::render(lock), expected);
        assert_eq!(line, "(2 assets: 1 current, 1 changed since upload, 1 orphans dropped)");
        assert!(changed);
        assert_eq!(plan.removals, [root.join(LOCK)]);

        // what tessera left in flight or provisioned, and an animation that is no KeyframeSequence, stay with tessera
        let mut pending = asset("art/a", 11, &fp);
        pending["pendingOperations"] = serde_json::json!([{ "kind": "assetIntent", "id": "f" }]);
        let mut animation = asset("art/m", 14, "x");
        animation["applied"]["fields"]["kind"] = "animation".into();
        let pass = serde_json::json!({ "kind": "gamePass", "key": "vip", "outputs": { "id": "5" } });
        let lock = tessera_lock(&[pending], &[pass]);
        fs::write(root.join(LOCK), lock).unwrap();
        assert_eq!(codes(&self::plan(&root.join(config::FILE)).unwrap()), ["migrate.lock", "migrate.lock"]);
        fs::write(root.join("art/m.rbxmx"), MODEL).unwrap();
        fs::write(root.join(LOCK), tessera_lock(&[animation], &[])).unwrap();
        let plan = self::plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), ["migrate.lock"]);
        assert!(plan.diags[0].message.contains("not a KeyframeSequence"), "{}", plan.diags[0].message);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_tessera_lock_merges_into_an_existing_lock_and_a_clash_refuses() {
        let (a, c) = (
            crate::assets::hash(".png", ImageOptions::DEFAULT, b"a"),
            crate::assets::hash(".png", ImageOptions::DEFAULT, b"c"),
        );
        let fp = |b: &[u8]| legacy::fingerprint(b, ".png", ImageOptions::DEFAULT);
        let records = [asset("art/a", 11, &fp(b"a")), asset("art/b", 12, "stale"), asset("art/c", 13, &fp(b"c"))];
        let ours = format!(
            "version = 1\ncreator = {{ user = 7 }}\nassets.\"art/a.png\" = {{ id = 99, hash = \"{a}\" }}\n\
             assets.\"art/b.ogg\" = {{ id = 98, hash = \"{}\" }}\n",
            "1".repeat(32)
        );
        let lock = tessera_lock(&records, &[]);
        let files: [(&str, &[u8]); 6] = [
            ("opossum.toml", TOML.as_bytes()),
            ("opossum.lock", ours.as_bytes()),
            ("art/a.png", b"a"),
            ("art/b.ogg", b"b"),
            ("art/c.png", b"c"),
            ("tessera.lock", &lock),
        ];
        let root = project("merge", &files);
        // b is 98 here and 12 there, and neither holds the file as it is
        let plan = plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), ["migrate.clash"]);
        assert!(plan.diags[0].message.contains("id 98") && plan.diags[0].message.contains("id 12"));

        fs::write(root.join(lock::FILE), ours.lines().filter(|l| !l.contains("b.ogg")).collect::<Vec<_>>().join("\n"))
            .unwrap();
        let plan = self::plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), Vec::<&str>::new(), "{:?}", plan.diags);
        let (lock, line, _) = plan.lock.as_ref().unwrap();
        // a keeps the id already there, which holds the file as it is; b and c come over
        let ids: Vec<(&str, Option<u64>, Option<&str>)> =
            lock.assets.iter().map(|(f, r)| (f.as_str(), r.id, r.hash.as_deref())).collect();
        let zeros = "0".repeat(32);
        assert_eq!(
            ids,
            [
                ("art/a.png", Some(99), Some(&*a)),
                ("art/b.ogg", Some(12), Some(&*zeros)),
                ("art/c.png", Some(13), Some(&*c))
            ]
        );
        assert_eq!(line, "(3 assets: 2 current, 1 changed since upload, 0 orphans dropped, 1 already there)");
        let _ = fs::remove_dir_all(root);
    }

    /// tessera's own config fixture (packages/config/testdata/project.ts), its asset set given stripExtensions.
    const PROJECT_TS: &str = r#"import { defineConfig } from "../src/index.ts";

export default defineConfig({
	defaultEnvironment: "development",
	creator: { kind: "user", id: 42 },
	build: ["bun", "--version"],
	generated: "src/generated",
	places: { main: { artifact: "build/main.rbxl" } },
	environments: {
		development: { universeId: 100, places: { main: 200 } },
	},
	passes: {
		vip: { name: "VIP", icon: "images/vip.png" },
	},
	assets: {
		interface: { root: "images", include: ["**/*.png"], stripExtensions: true },
	},
});
"#;

    #[test]
    fn tessera_s_config_becomes_opossum_toml_and_what_it_cannot_say_is_refused_where_it_is_written() {
        let toml = "name = \"t\"\nplaceId = 5\n\n[routes]\nshared = \"ReplicatedStorage/shared\"\n";
        let files: [(&str, &[u8]); 3] =
            [("opossum.toml", toml.as_bytes()), ("tessera.config.ts", PROJECT_TS.as_bytes()), ("images/vip.png", b"v")];
        let root = project("config", &files);
        let plan = plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), Vec::<&str>::new(), "{:?}", plan.diags);
        let expected = "name = \"t\"\ncreator = { user = 42 }\n\n[routes]\nshared = \"ReplicatedStorage/shared\"\n\n\
                        [assets]\ninterface = { root = \"images\", include = [\"**/*.png\"] }\n\n\
                        [environments.development]\nuniverseId = 100\nplaces = { main = 200 }\n\n\
                        [passes.vip]\nname = \"VIP\"\nicon = \"images/vip.png\"\n";
        assert_eq!(plan.toml.as_deref(), Some(expected));
        assert_eq!(plan.removals, [root.join(CONFIG)]);
        assert!(
            plan.notes.iter().any(|n| n.contains("`bun --version`"))
                && plan.notes.iter().any(|n| n.contains("placeId = 5"))
        );

        // the fixture as tessera has it keys files with their extensions; an evaluated root and a cover fit
        let ts = PROJECT_TS
            .replace(", stripExtensions: true", "")
            .replace("vip: { name", "vip: { name: NAME, title")
            .replace("include: [\"**/*.png\"]", "include: [\"**/*.png\"], image: { fit: \"cover\" }");
        fs::write(root.join(CONFIG), ts).unwrap();
        let plan = self::plan(&root.join(config::FILE)).unwrap();
        let at: Vec<(&str, u32, u32)> = plan.diags.iter().map(|d| (d.code.as_str(), d.line, d.col)).collect();
        assert_eq!(at, [("migrate.tesseraConfig", 13, 16)]);
        fs::write(
            root.join(CONFIG),
            PROJECT_TS
                .replace(", stripExtensions: true", "")
                .replace("include: [\"**/*.png\"]", "include: [\"**/*.png\"], image: { fit: \"cover\" }"),
        )
        .unwrap();
        let plan = self::plan(&root.join(config::FILE)).unwrap();
        let at: Vec<(&str, u32, u32)> = plan.diags.iter().map(|d| (d.code.as_str(), d.line, d.col)).collect();
        assert_eq!(at, [("migrate.tesseraConfig", 16, 69), ("migrate.tesseraConfig", 16, 14)]);

        // a module written elsewhere would keep its imports and keep building with frozen ids
        let elsewhere = ["src/generated/interface.luau", "src/shared/ids.luau"].map(|out| {
            PROJECT_TS.replace("include: [\"**/*.png\"]", &format!("include: [\"**/*.png\"], output: \"{out}\""))
        });
        fs::write(root.join(CONFIG), &elsewhere[0]).unwrap();
        assert_eq!(codes(&self::plan(&root.join(config::FILE)).unwrap()), Vec::<&str>::new());
        fs::write(root.join(CONFIG), &elsewhere[1]).unwrap();
        assert_eq!(codes(&self::plan(&root.join(config::FILE)).unwrap()), ["migrate.tesseraConfig"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn only_the_specifier_bytes_of_generated_and_hexagon_imports_change() {
        let ts = "export default { creator: { kind: 'user', id: 1 }, places: { main: { artifact: 'a.rbxl' } }, \
                  environments: { dev: { universeId: 2, places: { main: 3 } } }, assets: { assets: { root: 'art', \
                  include: ['**/*'], stripExtensions: true } } };";
        let source = "// ä, before the imports\nimport assets from 'generated/assets';\nimport type { Suite } from \"@hautajoki/hexagon\";\n\
                      export { x } from '../src/generated/resources';\nconst lazy = import(\"generated/assets\");\n\
                      type T = typeof import('./generated/assets');\nimport other from 'generated/other';\n\
                      const s = 'generated/assets';\n";
        let files: [(&str, &[u8]); 5] = [
            ("opossum.toml", b"name = \"t\"\n[routes]\nshared = \"ReplicatedStorage/shared\"\n"),
            ("tessera.config.ts", ts.as_bytes()),
            ("art/x.png", b"x"),
            ("src/a.ts", source.as_bytes()),
            ("src/generated/assets.luau", b"return {}"),
        ];
        let root = project("sources", &files);
        let plan = plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), Vec::<&str>::new(), "{:?}", plan.diags);
        let expected = source
            .replacen("'generated/assets'", "'opossum/assets'", 1)
            .replace("@hautajoki/hexagon", "opossum/test")
            .replace("../src/generated/resources", "opossum/resources")
            .replace("\"generated/assets\"", "\"opossum/assets\"")
            .replace("./generated/assets", "opossum/assets");
        assert_eq!(plan.sources, [(root.join("src/a.ts"), expected)]);
        assert_eq!(plan.imports, 5);
        assert_eq!(plan.removals, [root.join(CONFIG), root.join("src/generated/assets.luau")]);
        let _ = fs::remove_dir_all(root);
    }

    /// A tessera.lock a merge brought back after the set was renamed: its records are not orphans, whose ids may go.
    #[test]
    fn a_merged_back_tessera_lock_of_a_set_opossum_does_not_know_refuses() {
        let fp = legacy::fingerprint(b"c", ".png", ImageOptions::DEFAULT);
        let lock = tessera_lock(&[asset("art/c", 13, &fp)], &[]);
        let toml = TOML.replace("art = \"art\"", "pictures = \"art\"");
        let files: [(&str, &[u8]); 4] = [
            ("opossum.toml", toml.as_bytes()),
            ("opossum.lock", b"version = 1\ncreator = { user = 7 }\n"),
            ("art/c.png", b"c"),
            ("tessera.lock", &lock),
        ];
        let root = project("renamed", &files);
        let plan = plan(&root.join(config::FILE)).unwrap();
        assert_eq!(codes(&plan), ["migrate.lock"]);
        assert!(plan.diags[0].message.contains("tessera set `art`"), "{}", plan.diags[0].message);
        let _ = fs::remove_dir_all(root);
    }
}
