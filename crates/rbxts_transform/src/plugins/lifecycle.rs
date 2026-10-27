//! The build lifecycle around the transform (DESIGN §4, §14): which plugins are on, `setup` and `finish` with their
//! backends, the late-string key check, the commit, and `explain_checker_diagnostic` on files that failed checking.
//!
//! The build's kind, changes, state origin and whether every file is transformed come from the build session
//! ([`crate::build`]), which also keeps the `Shared`s and committed states across builds, the cached file parts
//! and late values, and the artifact set of the last build (stale deletions).

use std::cell::{OnceCell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustc_hash::FxHashSet;
use serde_json::{Map, Value};

use rbxts_luau::LateStrings;
use rbxts_plugin::erased::{DynPlugin, ErasedPart, ErasedShared, ErasedState};
use rbxts_plugin::{
    BuildKind, ChangeSet, DirEntry, FileIndex, FinishBackend, FinishCx, GlobMatcher, PluginInfo, SetupBackend, SetupCx,
    StateOrigin, Utf16Pos,
};

use crate::build::plugin_inputs::{
    DirRules, EnvSnapshot, FinishRead, RecordedGlob, plain, read_answer, read_dir_now, read_file_now, record_read,
    valid_env_name,
};
use rbxts_project::{ModuleResolver, ProjectServices, ResolvedModuleInfo, nodepath};
use tsrs_ast::SourceFile;
use tsrs_compiler::Program;
use tsrs_core::{CompilerOptions, P};

use super::HookKind;
use super::host::{HookTime, resolve_module_from};

/// The plugins of a build and the switches that go with them.
#[derive(Default)]
pub struct PluginOptions<'p> {
    /// Registered plugins, in registration order (the claim order).
    pub plugins: &'p [Box<dyn DynPlugin>],
    /// A tsconfig `plugins` entry whose `transform` no registered plugin claims is an error ("not compiled into
    /// this toolchain") when stock would run it, else a warning as in stock roblox-ts (`activate`); with this set it
    /// is ignored (measurement runs on a project's own tsconfig).
    pub ignore_unknown_tsconfig_plugins: bool,
    /// `RBXTS_PLUGIN_CLAIMS=all`: ask every matching plugin, and report a double claim as an error. The
    /// environment variable turns it on too.
    pub claims_all: bool,
}

/// A registered plugin that is on for this project.
pub(crate) struct ActivePlugin<'p> {
    pub plugin: &'p dyn DynPlugin,
    pub info: PluginInfo,
    /// Its tsconfig `plugins` entry.
    pub entry: Option<Map<String, Value>>,
}

/// What a plugin read in `setup`: recorded so incremental and watch builds can rebuild when one changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildInput {
    /// A file read through `read_input`.
    File {
        /// The plugin's name.
        plugin: &'static str,
        /// The file, absolute.
        path: String,
        /// Whether it existed.
        exists: bool,
        /// The version of its text, `None` when it could not be read.
        version: Option<crate::build::graph::Version>,
    },
    /// An environment variable read through `env`.
    Env {
        /// The plugin's name.
        plugin: &'static str,
        /// The variable.
        name: String,
        /// Its value, `None` when unset.
        value: Option<String>,
    },
    /// A `resolve_module` from `setup`.
    Resolution {
        /// The plugin's name.
        plugin: &'static str,
        /// The specifier resolved.
        specifier: String,
        /// The file it was resolved from.
        from: String,
        /// The file it resolved to, `None` when it did not resolve.
        resolved: Option<String>,
    },
    /// A glob matched through `record_glob`.
    Glob {
        /// The plugin's name.
        plugin: &'static str,
        /// The glob, with its matcher and answer.
        glob: RecordedGlob,
    },
}

/// One plugin's hook times.
#[derive(Clone, Debug)]
pub struct PluginTimings {
    /// The plugin's name.
    pub name: &'static str,
    /// Per hook kind: calls and time, without the compiler work hooks asked for.
    pub hooks: Vec<(HookKind, HookTime)>,
}

/// The late-string key of a plugin's key: keys are per plugin.
pub(crate) fn late_key(plugin: &str, key: &str) -> String {
    format!("{plugin}\u{0}{key}")
}

/// The plugin's own key of a namespaced late-string key.
pub(crate) fn plain_late_key(key: &str) -> &str {
    key.split_once('\u{0}').map_or(key, |(_, plain)| plain)
}

/// The registered plugins that are on for the project: `tsconfig_transform: None`, or a tsconfig `plugins` entry
/// with that `transform`, with the warnings to report. Errors: an entry no registered plugin claims that Node would
/// load (unless ignored), two plugins with one name.
///
/// An entry no registered plugin claims goes as stock roblox-ts 3.0.0's `createTransformerList` goes: when
/// `resolve.sync` can't find it from the project directory ([`resolve_sync`]), or Node can't `require` it or finds
/// no factory function in it ([`node_load`]), a warning with `transformerNotFound`'s text and the error, and the build
/// goes on without it. One whose factory Node finds is an error: stock would run that transformer, which rbxts-rs
/// can't (also when Node can't be run to tell).
pub(crate) fn activate<'p>(
    options: &PluginOptions<'p>,
    tsconfig_path: &str,
) -> Result<(Vec<ActivePlugin<'p>>, Vec<String>), Vec<String>> {
    let entries = rbxts_project::setup::read_tsconfig_plugins(tsconfig_path).map_err(|e| e.messages)?;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let project_dir = nodepath::resolve(&[nodepath::dirname(&nodepath::resolve(&[tsconfig_path]))]);
    let mut names = FxHashSet::default();
    for plugin in options.plugins {
        let name = plugin.info().name;
        if !names.insert(name) {
            errors.push(format!("two plugins are named `{name}`: plugin names must be unique"));
        }
    }
    for entry in &entries {
        let Some(transform) = entry.get("transform").and_then(Value::as_str) else { continue };
        let claimed = options.plugins.iter().any(|p| p.info().tsconfig_transform == Some(transform));
        if claimed || options.ignore_unknown_tsconfig_plugins {
            continue;
        }
        let import = entry.get("import").and_then(Value::as_str);
        let failed = match resolve_sync(transform, &project_dir) {
            Err(error) => Some(error),
            Ok(module) => match node_load(&module, import, &project_dir) {
                Some(Err(error)) => Some(error),
                Some(Ok(())) | None => None,
            },
        };
        if let Some(error) = failed {
            warnings.push(format!(
                "Transformer `{transform}` was not found!\nMore info: {error}\nSuggestion: Did you forget to install \
                 the package?"
            ));
        } else {
            errors.push(format!(
                "tsconfig plugin \"{transform}\" is not compiled into this toolchain (no registered plugin has \
                 that tsconfig_transform)"
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut active = Vec::new();
    for plugin in options.plugins {
        let info = plugin.info();
        let entry = match info.tsconfig_transform {
            None => None,
            Some(transform) => {
                match entries.iter().find(|e| e.get("transform").and_then(Value::as_str) == Some(transform)) {
                    Some(entry) => Some(entry.clone()),
                    None => continue,
                }
            }
        };
        active.push(ActivePlugin { plugin: &**plugin, info, entry });
    }
    Ok((active, warnings))
}

/// Node's core modules (`is-core-module` 2.16, which roblox-ts 3.0.0's `resolve` 1.22 uses, for Node 22.14 and later,
/// 23.4 and later included): `resolve.sync` returns their name before looking in any `node_modules`. Each is also
/// a core module with `node:` in front.
const NODE_CORE_MODULES: &[&str] = &[
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_stream_writable",
    "_tls_common",
    "_tls_wrap",
];

/// The core modules `is-core-module` knows only with `node:` in front (`node:test`, not `test`).
const NODE_PREFIXED_ONLY_CORE_MODULES: &[&str] = &["sea", "sqlite", "test", "test/reporters"];

/// `isCore` of `resolve` (`is-core-module`): a core module's name, with or without `node:`, or one of the names
/// it knows only with `node:`. Any other `node:` name is a package name (`Cannot find module 'node:x'`).
fn is_node_core_module(specifier: &str) -> bool {
    match specifier.strip_prefix("node:") {
        Some(name) => NODE_CORE_MODULES.contains(&name) || NODE_PREFIXED_ONLY_CORE_MODULES.contains(&name),
        None => NODE_CORE_MODULES.contains(&specifier),
    }
}

/// roblox-ts's transformer lookup, `resolve.sync(specifier, { basedir })` of `resolve` 1.22 with its defaults
/// (`extensions: [".js"]`, symlinks kept): the module path, a core module's name, or the error roblox-ts reports
/// (`String(err)`).
///
/// - a path (`./x`, `../x`, `.`, `..`, `/x`, `\x`, `C:\x`): resolved against `basedir`, as a file (`x`, then `x.js`)
///   or as a directory;
/// - a core module: its name;
/// - otherwise `<dir>/node_modules/<specifier>` for `basedir` and each ancestor (when that `node_modules` is a
///   directory), then `~/.node_modules` and `~/.node_libraries`.
///
/// A directory: its `package.json` `main` (`.` or `./` is `index`) as a file, then as a directory, then `index.js`;
/// a `main` that isn't a string is a TypeError.
fn resolve_sync(specifier: &str, basedir: &str) -> Result<String, String> {
    fn is_file(path: &str) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_file())
    }
    fn load_as_file(path: &str) -> Option<String> {
        [path.to_owned(), format!("{path}.js")].into_iter().find(|candidate| is_file(candidate))
    }
    fn js_string(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
    fn load_as_directory(path: &str) -> Result<Option<String>, String> {
        let package = std::fs::read_to_string(nodepath::join(&[path, "/package.json"]))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        // a falsy `main` (missing, "", 0, false, null) is none
        let main = package.as_ref().and_then(|p| p.get("main")).filter(|main| match main {
            Value::Null | Value::Bool(false) => false,
            Value::String(text) => !text.is_empty(),
            Value::Number(n) => n.as_f64() != Some(0.0),
            _ => true,
        });
        if let Some(main) = main {
            let Value::String(main) = main else {
                let name = package.as_ref().and_then(|p| p.get("name")).map_or("undefined".to_owned(), js_string);
                return Err(format!("TypeError: package \u{201c}{name}\u{201d} `main` must be a string"));
            };
            let main = if main == "." || main == "./" { "index" } else { main.as_str() };
            let main = nodepath::resolve(&[path, main]);
            // resolve swallows errors here
            if let Some(found) = load_as_file(&main).or_else(|| load_as_directory(&main).ok().flatten()) {
                return Ok(Some(found));
            }
        }
        Ok(load_as_file(&nodepath::join(&[path, "/index"])))
    }
    let absolute_start = nodepath::resolve(&[basedir]);
    let bytes = specifier.as_bytes();
    let relative = specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || matches!(bytes.first(), Some(b'/' | b'\\'))
        || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\'));
    if relative {
        let mut path = nodepath::resolve(&[&absolute_start, specifier]);
        if specifier == "." || specifier == ".." || specifier.ends_with('/') {
            path.push('/');
        }
        if let Some(found) = load_as_file(&path) {
            return Ok(found);
        }
        if let Some(found) = load_as_directory(&path)? {
            return Ok(found);
        }
    } else if is_node_core_module(specifier) {
        return Ok(specifier.to_owned());
    } else {
        let mut dirs = Vec::new();
        let mut dir = absolute_start.clone();
        loop {
            dirs.push(nodepath::resolve(&[&dir, "node_modules"]));
            let parent = nodepath::dirname(&dir).to_owned();
            if parent == dir {
                break;
            }
            dir = parent;
        }
        if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            let home = home.to_string_lossy().into_owned();
            dirs.push(nodepath::join(&[&home, ".node_modules"]));
            dirs.push(nodepath::join(&[&home, ".node_libraries"]));
        }
        for modules in dirs {
            let candidate = nodepath::join(&[&modules, specifier]);
            if !std::fs::metadata(nodepath::dirname(&candidate)).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            if let Some(found) = load_as_file(&candidate) {
                return Ok(found);
            }
            if let Some(found) = load_as_directory(&candidate)? {
                return Ok(found);
            }
        }
    }
    Err(format!("Error: Cannot find module '{specifier}' from '{basedir}'"))
}

/// What roblox-ts's `createTransformerList` does after resolving a transformer, run by Node: `require` it and take its
/// factory (`config.import`, else `default`; a module that is a function is the default). The error roblox-ts would
/// report (`String(err)`, "Error: factory not a function" included), `Ok` when the factory is a function (stock then
/// calls it with the program, which rbxts-rs can't), or `None` when Node can't be run.
fn node_load(module_path: &str, import: Option<&str>, cwd: &str) -> Option<Result<(), String>> {
    const SCRIPT: &str = "let r;try{const m=require(process.argv[1]);const f=typeof m==='function'?{default:m}:m;\
        const factory=f[process.argv[2]];if(!factory||typeof factory!=='function')throw new Error('factory not a \
        function');r='ok'}catch(e){r='error:'+e}process.stdout.write(JSON.stringify(r))";
    let output = std::process::Command::new("node")
        .args(["-e", SCRIPT, module_path, import.unwrap_or("default")])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let answer: String = serde_json::from_slice(&output.stdout).ok()?;
    Some(match answer.strip_prefix("error:") {
        Some(error) => Err(error.to_owned()),
        None => Ok(()),
    })
}

/// A path a plugin named: absolute, or relative to the project directory.
fn plugin_path(services: &ProjectServices, path: &Path) -> String {
    let text = path.to_string_lossy();
    if nodepath::is_absolute(&text) {
        nodepath::normalize(&text)
    } else {
        nodepath::resolve(&[&services.data.project_path, &text])
    }
}

/// A path as a comparison key (case-insensitive unless the file system is case-sensitive).
fn path_key(services: &ProjectServices, path: &str) -> String {
    let normalized = nodepath::normalize(path).replace('\\', "/");
    if services.use_case_sensitive_file_names { normalized } else { normalized.to_lowercase() }
}

/// A component Windows would rewrite when opening it (a trailing `.` or space), so that two spellings name one
/// file: `a.luau.` is `a.luau`.
fn has_rewritten_component(path: &str) -> bool {
    path.split(['/', '\\']).any(|c| c != "." && c != ".." && (c.ends_with('.') || c.ends_with(' ')))
}

/// The path as the file system resolves it: its deepest existing ancestor canonicalised (junctions, symlinks,
/// case), the rest appended. A comparison key like [`path_key`].
fn real_key(services: &ProjectServices, path: &str) -> String {
    let mut existing = PathBuf::from(nodepath::normalize(path));
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                rest.push(name.to_owned());
                existing.pop();
            }
            None => break,
        }
    }
    let mut real = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.iter().rev() {
        real.push(name);
    }
    let text = real.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    path_key(services, text)
}

// --- setup ---

/// The compiler's side of `SetupCx` for one plugin.
pub(crate) struct Setup<'s> {
    pub program: &'static Program,
    pub services: &'s ProjectServices,
    pub plugin: &'static str,
    pub entry: Option<&'s Map<String, Value>>,
    pub resolver: &'s OnceCell<ModuleResolver>,
    /// The build's environment snapshot (`Cx::env` reads the same).
    pub env: &'s EnvSnapshot,
    /// What setup read (a `RefCell`: `resolve_module` takes `&self`).
    pub inputs: RefCell<Vec<BuildInput>>,
    pub messages: &'s mut Vec<String>,
    pub warnings: &'s mut Vec<String>,
    pub kind: BuildKind,
    /// Every source file is transformed this build.
    pub all: bool,
    /// What changed since the last committed build (`None` for a clean build).
    pub changes: Option<&'s ChangeSet>,
    pub origin: StateOrigin,
}

impl SetupBackend for Setup<'_> {
    fn program(&self) -> &'static Program {
        self.program
    }

    fn options(&self) -> P<CompilerOptions> {
        self.program.options()
    }

    fn project(&self) -> &ProjectServices {
        self.services
    }

    fn tsconfig_entry(&self) -> Option<&Map<String, Value>> {
        self.entry
    }

    fn resolve_module(&self, specifier: &str, from_file: &str) -> Option<ResolvedModuleInfo> {
        let resolved = resolve_module_from(self.program, self.resolver, specifier, from_file);
        // a build input: a package installed mid-watch must trigger a rebuild
        self.inputs.borrow_mut().push(BuildInput::Resolution {
            plugin: self.plugin,
            specifier: specifier.to_owned(),
            from: from_file.to_owned(),
            resolved: resolved.as_ref().map(|r| r.file_name.clone()),
        });
        resolved
    }

    fn read_input(&mut self, path: &Path) -> io::Result<Option<String>> {
        let path = plugin_path(self.services, path);
        let result = match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        };
        let exists = matches!(result, Ok(Some(_)));
        let version = match &result {
            Ok(Some(text)) => Some(crate::build::graph::Version::of(text)),
            _ => None,
        };
        self.inputs.borrow_mut().push(BuildInput::File { plugin: self.plugin, path, exists, version });
        result
    }

    fn env(&mut self, name: &str) -> Option<String> {
        // a name the environment cannot hold is unset (on Windows `std::env::var` would answer for the hidden `=C:`
        // variables), as `Cx::env` has it; nothing to record
        if !valid_env_name(name) {
            return None;
        }
        let value = self.env.get(name);
        self.inputs.borrow_mut().push(BuildInput::Env {
            plugin: self.plugin,
            name: name.to_owned(),
            value: value.clone(),
        });
        value
    }

    fn record_glob(&mut self, root: &Path, pattern: &str, matcher: Arc<dyn GlobMatcher>) -> Vec<String> {
        let root = plain(&plugin_path(self.services, root));
        let glob = RecordedGlob::record(0, root, pattern, matcher);
        let answer = glob.answer.clone();
        let mut inputs = self.inputs.borrow_mut();
        // one plugin's records with an equal (root, pattern) are one record
        inputs.retain(|input| !matches!(input, BuildInput::Glob { glob: g, .. } if g.same_record(&glob)));
        inputs.push(BuildInput::Glob { plugin: self.plugin, glob });
        answer
    }

    fn build_kind(&self) -> BuildKind {
        self.kind
    }

    fn transforms_all_files(&self) -> bool {
        self.all
    }

    fn changes(&self) -> Option<&ChangeSet> {
        self.changes
    }

    fn state_origin(&self) -> StateOrigin {
        self.origin
    }

    fn read_persisted(&mut self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(plugin_path(self.services, path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn info(&mut self, message: &str) {
        self.messages.push(format!("{}: {message}", self.plugin));
    }

    fn warn(&mut self, message: &str, hint: Option<&str>) {
        self.warnings.push(match hint {
            Some(hint) => format!("{}: {message}\n{hint}", self.plugin),
            None => format!("{}: {message}", self.plugin),
        });
    }
}

/// Runs one plugin's `setup` on `state`: `default_state()` for a clean or incremental build (the plugin reloads
/// what it persisted itself with `read_persisted`), a copy of the committed state for a watch rebuild.
pub(crate) fn run_setup(
    active: &ActivePlugin<'_>,
    backend: &mut Setup<'_>,
    mut state: ErasedState,
) -> (Result<ErasedShared, rbxts_plugin::Fatal>, ErasedState, Duration) {
    let start = std::time::Instant::now();
    let shared = active.plugin.setup(&mut SetupCx::new(backend), &mut state);
    (shared, state, start.elapsed())
}

// --- finish ---

/// What `finish` queued for the commit.
#[derive(Default)]
pub(crate) struct CommitQueue {
    /// (plugin, absolute path, bytes): written in step 2, only if changed.
    pub artifacts: Vec<(&'static str, String, Vec<u8>)>,
    /// Written in step 4.
    pub state_files: Vec<(&'static str, String, Vec<u8>)>,
    /// Deleted in step 3.
    pub removals: Vec<(&'static str, String)>,
    /// Glob roots to watch.
    pub watch_dirs: Vec<(&'static str, String)>,
    /// `remove_empty_dir` calls (checked at the call): removed in step 3, last, when empty.
    pub empty_dirs: Vec<(&'static str, String)>,
    /// What `finish` read through the compiler (`record_glob`, `read_dir`, `exists`, `read_file`), for the
    /// watcher.
    pub reads: Vec<FinishRead>,
}

/// The compiler's side of `FinishCx` for one plugin.
pub(crate) struct Finish<'s> {
    /// The build's files in transform order: [`FileIndex`] indexes it.
    pub order: &'s [P<SourceFile>],
    pub services: &'s ProjectServices,
    pub plugin: &'static str,
    /// The plugin's index among the active plugins.
    pub index: usize,
    /// The directory rules of `remove_empty_dir`.
    pub rules: &'s DirRules,
    /// Where outputs go (the override when there is one).
    pub out_dir: &'s str,
    /// The path keys of this build's outputs (Luau, assets, declaration and transformed outputs).
    pub luau_outputs: &'s FxHashSet<String>,
    pub late: &'s mut LateStrings,
    pub resolved: &'s mut FxHashSet<String>,
    pub queue: &'s mut CommitQueue,
    pub errors: &'s mut Vec<String>,
    pub warnings: &'s mut Vec<String>,
    pub messages: &'s mut Vec<String>,
    pub kind: BuildKind,
    pub all: bool,
}

impl Finish<'_> {
    /// Checks an artifact or state-file path: under the project directory, the outDir or the include path, and not
    /// a Luau output. Records an error naming the plugin otherwise.
    fn checked_path(&mut self, path: &Path, what: &str) -> Option<String> {
        let absolute = plugin_path(self.services, path);
        if has_rewritten_component(&absolute) {
            self.errors.push(format!(
                "plugin `{}`: {what} {absolute} has a path component ending in `.` or a space",
                self.plugin
            ));
            return None;
        }
        // through the file system: a junction or symlink inside the project can lead outside it
        let key = real_key(self.services, &absolute);
        let data = &self.services.data;
        let roots = [
            data.project_path.as_str(),
            self.services.path_translator.out_dir.as_str(),
            self.out_dir,
            data.options.include_path.as_str(),
        ];
        let under_root = roots.iter().filter(|r| !r.is_empty()).any(|root| {
            let root = real_key(self.services, root);
            key.len() > root.len() && key.starts_with(&root) && key.as_bytes()[root.len()] == b'/'
        });
        if !under_root {
            self.errors.push(format!(
                "plugin `{}`: {what} {absolute} is outside the project directory, the outDir and the include path",
                self.plugin
            ));
            return None;
        }
        if self.luau_outputs.contains(&key) {
            self.errors.push(format!("plugin `{}`: {what} {absolute} is an output of this build", self.plugin));
            return None;
        }
        Some(absolute)
    }
}

impl FinishBackend for Finish<'_> {
    fn project(&self) -> &ProjectServices {
        self.services
    }

    fn build_kind(&self) -> BuildKind {
        self.kind
    }

    fn transforms_all_files(&self) -> bool {
        self.all
    }

    fn resolve_late(&mut self, key: &str, escaped: String) {
        let key = late_key(self.plugin, key);
        self.resolved.insert(key.clone());
        self.late.insert(key, escaped);
    }

    fn write_artifact(&mut self, path: &Path, bytes: Vec<u8>) {
        if let Some(path) = self.checked_path(path, "artifact") {
            self.queue.artifacts.retain(|(_, p, _)| *p != path);
            self.queue.artifacts.push((self.plugin, path, bytes));
        }
    }

    fn write_state_file(&mut self, path: &Path, bytes: Vec<u8>) {
        if let Some(path) = self.checked_path(path, "state file") {
            self.queue.state_files.retain(|(_, p, _)| *p != path);
            self.queue.state_files.push((self.plugin, path, bytes));
        }
    }

    fn remove_artifact(&mut self, path: &Path) {
        if let Some(path) = self.checked_path(path, "artifact") {
            self.queue.removals.push((self.plugin, path));
        }
    }

    fn watch_dir(&mut self, path: &Path) {
        if let Some(path) = self.checked_path(path, "watched directory") {
            self.queue.watch_dirs.push((self.plugin, path));
        }
    }

    fn record_glob(&mut self, root: &Path, pattern: &str, matcher: Arc<dyn GlobMatcher>) -> Vec<String> {
        // only read: the root is not path-checked (Flamework's is the package root)
        let root = plain(&plugin_path(self.services, root));
        let glob = RecordedGlob::record(self.index, root, pattern, matcher);
        let answer = glob.answer.clone();
        record_read(&mut self.queue.reads, FinishRead::Glob(glob));
        answer
    }

    fn remove_empty_dir(&mut self, path: &Path) -> Result<(), String> {
        let absolute = plugin_path(self.services, path);
        match self.rules.refusal(self.services, &absolute, self.luau_outputs) {
            Some(why) => self.errors.push(format!("plugin `{}`: remove_empty_dir: {absolute} {why}", self.plugin)),
            None => {
                if !self.queue.empty_dirs.iter().any(|(_, queued)| *queued == absolute) {
                    self.queue.empty_dirs.push((self.plugin, absolute));
                }
            }
        }
        Ok(())
    }

    fn read_dir(&mut self, path: &Path) -> Option<Vec<DirEntry>> {
        let path = plain(&plugin_path(self.services, path));
        let answer = read_dir_now(Path::new(&path));
        record_read(&mut self.queue.reads, FinishRead::ReadDir { plugin: self.index, path, answer: answer.clone() });
        answer
    }

    fn exists(&mut self, path: &Path) -> bool {
        let path = plain(&plugin_path(self.services, path));
        let answer = Path::new(&path).exists();
        record_read(&mut self.queue.reads, FinishRead::Exists { plugin: self.index, path, answer });
        answer
    }

    fn read_file(&mut self, path: &Path) -> io::Result<Option<String>> {
        let path = plain(&plugin_path(self.services, path));
        let result = read_file_now(Path::new(&path));
        let answer = read_answer(&result);
        record_read(&mut self.queue.reads, FinishRead::ReadFile { plugin: self.index, path, answer });
        result
    }

    fn warn_at(&mut self, file: FileIndex, pos: Utf16Pos, message: &str, hint: Option<&str>) {
        let name = self.order.get(file.0 as usize).map_or("<unknown file>", |f| f.get().file_name());
        let relative = nodepath::relative(&self.services.data.project_path, name).replace('\\', "/");
        let mut text = format!("{relative}:{}:{}: {}: {message}", pos.line + 1, pos.character + 1, self.plugin);
        if let Some(hint) = hint {
            text.push('\n');
            text.push_str(hint);
        }
        self.warnings.push(text);
    }

    fn info(&mut self, message: &str) {
        self.messages.push(format!("{}: {message}", self.plugin));
    }
}

/// Runs one plugin's `finish` with the parts of every file, sorted by `FileIndex`.
pub(crate) fn run_finish(
    active: &ActivePlugin<'_>,
    shared: &ErasedShared,
    state: &mut ErasedState,
    parts: &[(FileIndex, &ErasedPart)],
    backend: &mut Finish<'_>,
) -> (Result<(), rbxts_plugin::Fatal>, Duration) {
    let start = std::time::Instant::now();
    let result = active.plugin.finish(shared, state, parts, &mut FinishCx::new(backend));
    (result, start.elapsed())
}

// --- commit ---

/// Whether an I/O error is worth retrying: a file another process holds open (Windows sharing and lock
/// violations, access denied while a scanner or editor has it).
fn is_transient(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// Writes `bytes` to a temporary file next to `path` and renames it into place (`MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING`), retrying with backoff while another process holds the target. The error is
/// returned after the last attempt; nothing is skipped silently.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = stage(path, bytes)?;
    commit_staged(&temp, path)
}

/// [`write_atomic`]'s first half: writes `bytes` to a temporary file next to `path` (creating its directory) and
/// returns that file. A directory at `path` is an error here already, not only at the rename.
pub fn stage(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        return Err(io::Error::from(io::ErrorKind::IsADirectory));
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp: PathBuf = dir.join(format!(".{name}.rbxts-tmp-{}-{n}", std::process::id()));
    if let Err(e) = std::fs::write(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(temp)
}

/// A file's modification time in nanoseconds since the Unix epoch, 0 when unknown.
pub fn modified_nanos(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// How close to now a modification time must not be for a stamp to keep it: an edit within the same tick of the file
/// system's clock would get the same time (git's "racily clean" entries). Such a stamp keeps no time, so the next
/// check compares the bytes.
const RACY: Duration = Duration::from_secs(2);

/// `modified` (nanoseconds since the Unix epoch) when it is far enough in the past to tell a later edit apart, else 0.
pub fn settled_time(modified: u128) -> u128 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    if modified != 0 && now >= modified + RACY.as_nanos() { modified } else { 0 }
}

/// The modification time of the file at `path` when it holds exactly `bytes` (0 when unknown or too recent:
/// [`settled_time`]), `None` otherwise (missing, or other bytes). The time is taken from the open file before its
/// bytes are read, so an edit after that shows as a later time, never as these bytes with the edit's time.
fn holds(path: &Path, bytes: &[u8]) -> Option<u128> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
        return None;
    }
    let modified = modified_nanos(&metadata);
    let mut existing = Vec::with_capacity(bytes.len());
    file.read_to_end(&mut existing).ok()?;
    (existing == bytes).then(|| settled_time(modified))
}

/// [`stage`] when the file is missing or its bytes differ (`None`: it holds them already).
pub fn stage_if_changed(path: &Path, bytes: &[u8]) -> io::Result<Option<PathBuf>> {
    stage_if_changed_timed(path, bytes).map(|(temp, _)| temp)
}

/// [`stage_if_changed`], with the modification time to stamp the bytes `path` holds once a commit renames the staged
/// file into place: when nothing is staged, the existing file's as it was read ([`holds`]); a staged file's is too
/// recent to tell a later edit apart, so 0 (the next check compares its bytes). With `bytes`, it makes the stamp of
/// what the commit leaves, never read back from the disk after the commit (where a hand edit landing after the rename
/// would be taken for the commit's bytes).
pub fn stage_if_changed_timed(path: &Path, bytes: &[u8]) -> io::Result<(Option<PathBuf>, u128)> {
    if let Some(modified) = holds(path, bytes) {
        return Ok((None, modified));
    }
    stage(path, bytes).map(|temp| (Some(temp), 0))
}

/// [`write_atomic`]'s second half: renames the [`stage`]d `temp` into place (`MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING`), retrying with backoff while another process holds the target; removes `temp` when
/// it fails at last.
pub fn commit_staged(temp: &Path, path: &Path) -> io::Result<()> {
    let mut delay = Duration::from_millis(10);
    let mut attempt = 0;
    loop {
        match std::fs::rename(temp, path) {
            Ok(()) => return Ok(()),
            Err(e) if is_transient(&e) && attempt < 8 => {
                attempt += 1;
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(500));
            }
            Err(e) => {
                let _ = std::fs::remove_file(temp);
                return Err(e);
            }
        }
    }
}

/// [`write_atomic`] when the file is missing or its bytes differ. Returns whether it wrote.
pub fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(false);
    }
    write_atomic(path, bytes)?;
    Ok(true)
}

/// [`write_if_changed`], with the modification time of the bytes it leaves at `path` ([`stage_if_changed_timed`]).
pub fn write_if_changed_timed(path: &Path, bytes: &[u8]) -> io::Result<(bool, u128)> {
    match stage_if_changed_timed(path, bytes)? {
        (None, modified) => Ok((false, modified)),
        (Some(temp), modified) => {
            commit_staged(&temp, path)?;
            Ok((true, modified))
        }
    }
}

/// Deletes a file, retrying while another process holds it; a missing file is fine.
pub fn remove_file(path: &Path) -> io::Result<bool> {
    let mut delay = Duration::from_millis(10);
    let mut attempt = 0;
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) if is_transient(&e) && attempt < 8 => {
                attempt += 1;
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Commit step 2: the plugin artifacts, each only if its bytes changed. Returns the paths written, or the first
/// failure (which stops the commit there).
pub(crate) fn commit_artifacts(queue: &CommitQueue) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    for (plugin, path, bytes) in &queue.artifacts {
        match write_if_changed(Path::new(path), bytes) {
            Ok(true) => written.push(path.clone()),
            Ok(false) => {}
            Err(e) => return Err(format!("plugin `{plugin}`: could not write {path}: {e}")),
        }
    }
    Ok(written)
}

/// Commit step 3 for plugins: `remove_artifact`s, and the artifacts the last build wrote but this one did not
/// (`stale`, which the caller diffs). Returns the paths deleted.
pub(crate) fn commit_removals(queue: &CommitQueue, stale: &[String]) -> Result<Vec<String>, String> {
    let mut deleted = Vec::new();
    let removals = queue.removals.iter().map(|(plugin, path)| (*plugin, path)).chain(stale.iter().map(|p| ("", p)));
    for (plugin, path) in removals {
        match remove_file(Path::new(path)) {
            Ok(true) => deleted.push(path.clone()),
            Ok(false) => {}
            Err(e) if plugin.is_empty() => return Err(format!("could not delete the stale artifact {path}: {e}")),
            Err(e) => return Err(format!("plugin `{plugin}`: could not delete {path}: {e}")),
        }
    }
    Ok(deleted)
}

/// Commit step 4 for plugins: the state files, each only if its bytes changed. Returns the paths written.
pub(crate) fn commit_state_files(queue: &CommitQueue) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    for (plugin, path, bytes) in &queue.state_files {
        match write_if_changed(Path::new(path), bytes) {
            Ok(true) => written.push(path.clone()),
            Ok(false) => {}
            Err(e) => return Err(format!("plugin `{plugin}`: could not write {path}: {e}")),
        }
    }
    Ok(written)
}

/// The key for a Luau output path, to keep artifacts off them.
pub(crate) fn output_key(services: &ProjectServices, path: &str) -> String {
    real_key(services, path)
}

#[cfg(test)]
mod tests {
    use super::{node_load, resolve_sync, settled_time, write_if_changed_timed};
    use crate::build::outputs::{Found, Stamp, content_hash};

    /// The stamp of a committed file is made from the bytes written, with no time while the write is recent: a hand
    /// edit landing right after the commit (same size, same clock tick) is found by its bytes, never taken for the
    /// commit's.
    #[test]
    fn stamps_come_from_the_bytes_written() {
        let dir = rbxts_test_support::TempDir::new("stamp-bytes");
        let path = dir.path().join("a.luau");
        let path_text = path.to_string_lossy().into_owned();
        let bytes = b"return 1
";
        let stamp = |modified| Stamp { size: bytes.len() as u64, hash: content_hash(bytes), modified };
        assert_eq!(write_if_changed_timed(&path, bytes).unwrap(), (true, 0), "a new write keeps no time");
        // the edit lands right after the commit: the stamp has the commit's bytes, not the edit's
        std::fs::write(&path, b"return 2
").unwrap();
        assert_eq!(stamp(0).check(&path_text, Found::at(&path_text)), None);
        // the bytes as committed, written an hour ago: an unchanged commit keeps that time
        assert_eq!(write_if_changed_timed(&path, bytes).unwrap(), (true, 0));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        let (written, modified) = write_if_changed_timed(&path, bytes).unwrap();
        assert!(!written);
        let found = Found::at(&path_text).unwrap();
        assert_eq!(modified, found.modified, "the time of the bytes found");
        assert_eq!(stamp(modified).check(&path_text, Some(found)), Some(stamp(modified)));
        assert_eq!(settled_time(found.modified), found.modified);
        // a time a moment ago is not one to trust
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        assert_eq!(settled_time(now), 0);
    }

    /// `resolve.sync` 1.22's answers for the same layout (checked with roblox-ts's own `resolve`).
    #[test]
    fn transformer_lookup_is_resolve_sync() {
        let dir = rbxts_test_support::TempDir::new("resolve-sync");
        let root = dir.path().to_string_lossy().replace('\\', "/");
        let write = |relative: &str, text: &str| {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("project/local.js", "");
        write("project/data.json", "{}");
        write("node_modules/main-file/package.json", r#"{ "main": "lib/main" }"#);
        write("node_modules/main-file/lib/main.js", "");
        write("node_modules/main-dir/package.json", r#"{ "main": "lib" }"#);
        write("node_modules/main-dir/lib/index.js", "");
        write("node_modules/main-number/package.json", r#"{ "name": "n", "main": 5 }"#);
        write("node_modules/json-index/index.json", "{}");
        write("node_modules/dot-main/package.json", r#"{ "main": "./" }"#);
        write("node_modules/dot-main/index.js", "");
        write("node_modules/fs/index.js", "");
        let project = format!("{root}/project");
        let found = |specifier: &str| resolve_sync(specifier, &project).map(|p| p.replace('\\', "/"));
        let missing = |specifier: &str| Err(format!("Error: Cannot find module '{specifier}' from '{project}'"));
        assert_eq!(found("./local"), Ok(format!("{root}/project/local.js")));
        // `.js` only, as resolve's default extensions
        assert_eq!(found("./data"), missing("./data"));
        assert_eq!(found("main-file"), Ok(format!("{root}/node_modules/main-file/lib/main.js")));
        assert_eq!(found("main-dir"), Ok(format!("{root}/node_modules/main-dir/lib/index.js")));
        assert_eq!(found("main-number"), Err("TypeError: package \u{201c}n\u{201d} `main` must be a string".into()));
        assert_eq!(found("json-index"), missing("json-index"));
        assert_eq!(found("dot-main"), Ok(format!("{root}/node_modules/dot-main/index.js")));
        // a core module wins over a package of its name
        assert_eq!(found("fs"), Ok("fs".into()));
        // `node:` names: only those `is-core-module` lists
        assert_eq!(found("node:fs"), Ok("node:fs".into()));
        assert_eq!(found("node:fs/promises"), Ok("node:fs/promises".into()));
        assert_eq!(found("node:test"), Ok("node:test".into()));
        assert_eq!(found("node:test/reporters"), Ok("node:test/reporters".into()));
        assert_eq!(found("test"), missing("test"));
        assert_eq!(found("node:nope"), missing("node:nope"));
        assert_eq!(found("node:test/mock_loader"), missing("node:test/mock_loader"));
        // `.\` is no path for resolve: a package name
        assert_eq!(found(r".\local"), missing(r".\local"));
    }

    /// Node's `require` of a resolved transformer, when Node is there.
    #[test]
    fn transformer_load_errors_are_node_s() {
        let dir = rbxts_test_support::TempDir::new("node-load");
        let cwd = dir.path().to_string_lossy().into_owned();
        let write = |name: &str, text: &str| {
            std::fs::write(dir.path().join(name), text).unwrap();
            dir.path().join(name).to_string_lossy().into_owned()
        };
        let function = write("function.js", "module.exports = () => () => (file) => file;\n");
        let Some(loaded) = node_load(&function, None, &cwd) else { return };
        assert_eq!(loaded, Ok(()));
        let named = write("named.js", "exports.transformer = () => () => (file) => file;\n");
        assert_eq!(node_load(&named, Some("transformer"), &cwd), Some(Ok(())));
        assert_eq!(node_load(&named, None, &cwd), Some(Err("Error: factory not a function".into())));
        let throws = write("throws.js", "throw new Error(\"boom\");\n");
        assert_eq!(node_load(&throws, None, &cwd), Some(Err("Error: boom".into())));
        assert_eq!(node_load("fs", None, &cwd), Some(Err("Error: factory not a function".into())));
    }
}
