//! Project setup before transforming, ported from roblox-ts's `src/Project` (single-project builds): the options of
//! the tsconfig's `rbxts` section, `createProjectData`, `validateCompilerOptions`, the path translator as
//! `ProjectGraph` makes it, and the Rojo checks at the top of `compileFiles`.
//!
//! Messages are roblox-ts's, without the terminal colours.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use tsrs_core::{CompilerOptions, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, Tristate, tspath};
use tsrs_tsoptions::{CompilerOptionsValue, parse_config_file_text_to_json};

use crate::nodepath;
use crate::path_translator::PathTranslator;
use crate::read_log::{ReadLog, Seen, recorded, recording};
use crate::rojo::{NetworkType, RbxPath, RojoError, RojoResolver, js_object_keys};

const NODE_MODULES: &str = "node_modules";
const RBXTS_SCOPE: &str = "@rbxts";

/// How the output is deployed: a project (absolute imports from services), a model, or a package (relative imports, the
/// runtime passed in by the consumer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectType {
    /// A place: imports go through `game:GetService(…)` from the DataModel.
    Game,
    /// A model: imports are relative, between the project's own instances.
    Model,
    /// A package: imports are relative, and the runtime library comes from the consumer (`_G[script]`).
    Package,
}

impl ProjectType {
    /// The name roblox-ts uses (`game`, `model`, `package`).
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectType::Game => "game",
            ProjectType::Model => "model",
            ProjectType::Package => "package",
        }
    }

    /// The project type of a `type` option value, None for any other value.
    pub fn parse(s: &str) -> Option<ProjectType> {
        match s {
            "game" => Some(ProjectType::Game),
            "model" => Some(ProjectType::Model),
            "package" => Some(ProjectType::Package),
            _ => None,
        }
    }
}

/// roblox-ts's `ProjectOptions`, the build-relevant ones (watch, polling and verbosity belong to a CLI).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectOptions {
    /// Where `RuntimeLib.lua` and the other runtime files go. Empty means `<project>/include`.
    pub include_path: String,
    /// An explicit Rojo project file; None auto-detects one in the project directory.
    pub rojo: Option<String>,
    /// None infers it from package.json and the Rojo tree.
    pub project_type: Option<ProjectType>,
    /// `--noInclude`: the runtime files are not copied to `include_path`.
    pub no_include: bool,
    /// `noCompilerHeader`: the outputs start without the `-- Compiled with roblox-ts` line.
    pub no_compiler_header: bool,
    /// `--logTruthyChanges`: log where Luau's truthiness differs from JavaScript's.
    pub log_truthy_changes: bool,
    /// `--writeOnlyChanged`: an output whose text did not change is not written again.
    pub write_only_changed: bool,
    /// `--writeTransformedFiles`: each source is also written as TypeScript after the TS transformers ran.
    pub write_transformed_files: bool,
    /// `--optimizedLoops`: simple counting `for` loops become numeric Luau loops.
    pub optimized_loops: bool,
    /// `--allowCommentDirectives`: `@ts-ignore` / `@ts-expect-error` / `@ts-nocheck` are accepted.
    pub allow_comment_directives: bool,
    /// `.luau` output files (the default) rather than `.lua`.
    pub luau: bool,
}

impl Default for ProjectOptions {
    /// `DEFAULT_PROJECT_OPTIONS`.
    fn default() -> Self {
        ProjectOptions {
            include_path: String::new(),
            rojo: None,
            project_type: None,
            no_include: false,
            no_compiler_header: false,
            log_truthy_changes: false,
            write_only_changed: false,
            write_transformed_files: false,
            optimized_loops: true,
            allow_comment_directives: false,
            luau: true,
        }
    }
}

/// A layer of options: the tsconfig's `rbxts` section, or command-line overrides. Set fields win over lower layers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectOptionLayer {
    /// See [`ProjectOptions::include_path`].
    pub include_path: Option<String>,
    /// See [`ProjectOptions::rojo`].
    pub rojo: Option<String>,
    /// See [`ProjectOptions::project_type`].
    pub project_type: Option<ProjectType>,
    /// See [`ProjectOptions::no_include`].
    pub no_include: Option<bool>,
    /// See [`ProjectOptions::no_compiler_header`].
    pub no_compiler_header: Option<bool>,
    /// See [`ProjectOptions::log_truthy_changes`].
    pub log_truthy_changes: Option<bool>,
    /// See [`ProjectOptions::write_only_changed`].
    pub write_only_changed: Option<bool>,
    /// See [`ProjectOptions::write_transformed_files`].
    pub write_transformed_files: Option<bool>,
    /// See [`ProjectOptions::optimized_loops`].
    pub optimized_loops: Option<bool>,
    /// See [`ProjectOptions::allow_comment_directives`].
    pub allow_comment_directives: Option<bool>,
    /// See [`ProjectOptions::luau`].
    pub luau: Option<bool>,
}

impl ProjectOptionLayer {
    /// `Object.assign(this, other)` for set fields.
    pub fn assign(&mut self, other: &ProjectOptionLayer) {
        macro_rules! take {
            ($($f:ident),*) => { $(if other.$f.is_some() { self.$f = other.$f.clone(); })* };
        }
        take!(
            include_path,
            rojo,
            project_type,
            no_include,
            no_compiler_header,
            log_truthy_changes,
            write_only_changed,
            write_transformed_files,
            optimized_loops,
            allow_comment_directives,
            luau
        );
    }
}

impl ProjectOptions {
    /// `{ ...DEFAULT_PROJECT_OPTIONS, ...rbxts, ...overrides }`, as `ProjectGraph` builds the root project's options.
    pub fn from_layers(layers: &[&ProjectOptionLayer]) -> ProjectOptions {
        let mut merged = ProjectOptionLayer::default();
        for layer in layers {
            merged.assign(layer);
        }
        let d = ProjectOptions::default();
        ProjectOptions {
            include_path: merged.include_path.unwrap_or(d.include_path),
            rojo: merged.rojo.or(d.rojo),
            project_type: merged.project_type.or(d.project_type),
            no_include: merged.no_include.unwrap_or(d.no_include),
            no_compiler_header: merged.no_compiler_header.unwrap_or(d.no_compiler_header),
            log_truthy_changes: merged.log_truthy_changes.unwrap_or(d.log_truthy_changes),
            write_only_changed: merged.write_only_changed.unwrap_or(d.write_only_changed),
            write_transformed_files: merged.write_transformed_files.unwrap_or(d.write_transformed_files),
            optimized_loops: merged.optimized_loops.unwrap_or(d.optimized_loops),
            allow_comment_directives: merged.allow_comment_directives.unwrap_or(d.allow_comment_directives),
            luau: merged.luau.unwrap_or(d.luau),
        }
    }
}

/// A setup failure: roblox-ts's `ProjectError` / `DiagnosticError` (one or more text diagnostics). The build stops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectError {
    /// The messages, in report order.
    pub messages: Vec<String>,
}

impl ProjectError {
    /// An error with one message.
    pub fn new(message: impl Into<String>) -> Self {
        ProjectError { messages: vec![message.into()] }
    }
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.messages.join("\n"))
    }
}

impl std::error::Error for ProjectError {}

/// A [`ProjectDiagnostic`]'s severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Fails the build.
    Error,
    /// Reported; the build goes on.
    Warning,
}

/// A diagnostic from setup that names no source position (roblox-ts's text diagnostics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDiagnostic {
    /// Whether it fails the build.
    pub severity: Severity,
    /// The text, as roblox-ts prints it.
    pub message: String,
}

impl ProjectDiagnostic {
    fn error(message: String) -> Self {
        ProjectDiagnostic { severity: Severity::Error, message }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// tsconfig.json → `rbxts` options

/// `findTsConfigPath` of the CLI: `project` itself when it is a file, else the nearest `tsconfig.json` from it
/// upwards. The result is resolved against `cwd`.
pub fn find_tsconfig_path(project: &str, cwd: &str) -> Result<String, ProjectError> {
    let resolved = nodepath::resolve_from(cwd, &[project]);
    if fs::metadata(&resolved).is_ok_and(|m| m.is_file()) {
        return Ok(resolved);
    }
    let mut dir = tspath::normalize_slashes(&resolved);
    loop {
        let candidate = tspath::combine_paths(&dir, &["tsconfig.json"]);
        if fs::metadata(&candidate).is_ok_and(|m| m.is_file()) {
            return Ok(nodepath::resolve_from(cwd, &[&candidate]));
        }
        let parent = tspath::get_directory_path(&dir);
        if parent == dir {
            return Err(ProjectError::new(format!("Unable to find tsconfig.json in {resolved}")));
        }
        dir = parent;
    }
}

/// The tsconfig's raw JSON (comments and trailing commas allowed), as TypeScript's `readConfigFile` reads it.
pub fn read_config_json(config_path: &str) -> Result<Value, ProjectError> {
    let text = recorded::read_to_string(config_path)
        .map_err(|_| ProjectError::new(format!("Cannot read file '{}'.", tspath::normalize_slashes(config_path))))?;
    let normalized = tspath::normalize_path(config_path);
    let path = tspath::to_path(&normalized, "", false);
    let (value, errors) = parse_config_file_text_to_json(&normalized, path, &text);
    if !errors.is_empty() {
        return Err(ProjectError::new(format!("Failed to parse \"{config_path}\" as JSON")));
    }
    Ok(options_value_to_json(&value))
}

fn options_value_to_json(v: &CompilerOptionsValue) -> Value {
    match v {
        CompilerOptionsValue::Bool(b) => Value::Bool(*b),
        CompilerOptionsValue::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        CompilerOptionsValue::Int(i) => Value::Number((*i).into()),
        CompilerOptionsValue::String(s) => Value::String(s.clone()),
        CompilerOptionsValue::Array(items) => Value::Array(items.iter().map(options_value_to_json).collect()),
        CompilerOptionsValue::NilArray => Value::Array(Vec::new()),
        CompilerOptionsValue::StringArray(items) => Value::Array(items.iter().cloned().map(Value::String).collect()),
        CompilerOptionsValue::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), options_value_to_json(v))).collect())
        }
        CompilerOptionsValue::EmptyStruct => Value::Object(Map::new()),
        _ => Value::Null,
    }
}

struct OsResolutionHost {
    cwd: String,
}

impl OsResolutionHost {
    /// The host for `cwd`: one per directory for the process (tsrs's resolution host must be `'static`, and this
    /// one keeps no state, so a watcher's builds share it instead of leaking one per bare-name `extends`).
    fn interned(cwd: &str) -> &'static OsResolutionHost {
        static HOSTS: std::sync::Mutex<Option<std::collections::HashMap<String, &'static OsResolutionHost>>> =
            std::sync::Mutex::new(None);
        let mut hosts = HOSTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        hosts
            .get_or_insert_with(std::collections::HashMap::new)
            .entry(cwd.to_owned())
            .or_insert_with(|| Box::leak(Box::new(OsResolutionHost { cwd: cwd.to_owned() })))
    }
}

impl tsrs_module::ResolutionHost for OsResolutionHost {
    fn fs(&self) -> &dyn tsrs_vfs::FS {
        // what the resolution reads goes to this thread's recording, if any
        static FS: crate::read_log::ThreadRecordingFs = crate::read_log::ThreadRecordingFs;
        &FS
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// `getExtendedConfigPaths`: relative or rooted `extends` entries resolve against the config's directory (adding
/// `.json` when needed); bare names resolve as packages, as TypeScript resolves `extends`.
fn get_extended_config_paths(config_path: &str, extends: Option<&Value>) -> Result<Vec<String>, ProjectError> {
    let values: Vec<&str> = match extends {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let file_exists = |p: &str| recorded::metadata(p).is_ok_and(|m| m.is_file());
    let mut paths = Vec::new();
    for value in values {
        let normalized = tspath::normalize_slashes(value);
        let resolved = if tspath::is_rooted_disk_path(&normalized)
            || normalized.starts_with("./")
            || normalized.starts_with("../")
        {
            let mut resolved = tspath::get_normalized_absolute_path(
                &normalized,
                nodepath::dirname(&tspath::normalize_slashes(config_path)),
            );
            if !file_exists(&resolved) && !resolved.ends_with(".json") {
                resolved.push_str(".json");
            }
            Some(resolved)
        } else {
            let host = OsResolutionHost::interned(&tspath::normalize_slashes(nodepath::dirname(config_path)));
            let result = tsrs_module::resolve_config(&normalized, &tspath::normalize_slashes(config_path), host);
            result.is_resolved().then(|| result.resolved_file_name.to_string())
        };
        match resolved {
            Some(resolved) if file_exists(&resolved) => paths.push(nodepath::normalize(&resolved)),
            _ => return Err(ProjectError::new(format!("File '{value}' not found."))),
        }
    }
    Ok(paths)
}

/// The tsconfig's `compilerOptions.plugins` entries (the objects, in order), from the config itself or else from the
/// last config it extends that sets them: TypeScript replaces an option array from an extended config as a whole.
/// Compiled-in plugins are activated by these entries' `transform` (DESIGN §11.5).
pub fn read_tsconfig_plugins(config_path: &str) -> Result<Vec<Map<String, Value>>, ProjectError> {
    Ok(read_tsconfig_plugins_rec(config_path, 0)?.unwrap_or_default())
}

fn read_tsconfig_plugins_rec(config_path: &str, depth: usize) -> Result<Option<Vec<Map<String, Value>>>, ProjectError> {
    if depth > 64 {
        return Err(ProjectError::new(format!("Circularity detected while resolving configuration: {config_path}")));
    }
    let config = read_config_json(config_path)?;
    if let Some(plugins) = config.get("compilerOptions").and_then(|options| options.get("plugins")) {
        let entries = plugins.as_array().map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect());
        return Ok(Some(entries.unwrap_or_default()));
    }
    let mut found = None;
    for extended in get_extended_config_paths(config_path, config.get("extends"))? {
        if let Some(entries) = read_tsconfig_plugins_rec(&extended, depth + 1)? {
            found = Some(entries);
        }
    }
    Ok(found)
}

/// The result of reading the `rbxts` sections of a tsconfig and the configs it extends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RbxtsConfig {
    /// The options the sections set, the tsconfig's own over those of the configs it extends.
    pub options: ProjectOptionLayer,
    /// Unsupported keys, ignored.
    pub warnings: Vec<String>,
}

/// `readProjectOptions`: the `rbxts` section of `config_path`, over those of the configs it extends (in order).
/// Path options resolve against the directory of the config that sets them.
pub fn read_project_options(config_path: &str) -> Result<RbxtsConfig, ProjectError> {
    let mut out = RbxtsConfig::default();
    let mut cache = HashMap::new();
    out.options = read_project_options_rec(config_path, &mut cache, &mut out.warnings)?;
    Ok(out)
}

fn read_project_options_rec(
    config_path: &str,
    cache: &mut HashMap<String, ProjectOptionLayer>,
    warnings: &mut Vec<String>,
) -> Result<ProjectOptionLayer, ProjectError> {
    let key = nodepath::normalize(config_path).to_lowercase();
    if let Some(cached) = cache.get(&key) {
        return Ok(cached.clone());
    }
    let config = read_config_json(config_path)?;
    let mut options = ProjectOptionLayer::default();
    for extended in get_extended_config_paths(config_path, config.get("extends"))? {
        options.assign(&read_project_options_rec(&extended, cache, warnings)?);
    }
    options.assign(&read_own_options(config_path, config.get("rbxts"), warnings)?);
    cache.insert(key, options.clone());
    Ok(options)
}

fn read_own_options(
    config_path: &str,
    value: Option<&Value>,
    warnings: &mut Vec<String>,
) -> Result<ProjectOptionLayer, ProjectError> {
    let mut options = ProjectOptionLayer::default();
    let Some(value) = value else { return Ok(options) };
    let invalid =
        |message: &str| ProjectError::new(format!("Invalid \"rbxts\" configuration in \"{config_path}\": {message}"));
    let Value::Object(map) = value else { return Err(invalid("expected an object")) };
    for key in js_object_keys(map) {
        let setting = &map[key.as_str()];
        let flag = |options: &mut ProjectOptionLayer| -> Result<(), ProjectError> {
            let b = setting.as_bool().ok_or_else(|| invalid(&format!("\"{key}\" must be a boolean")))?;
            let slot = match key.as_str() {
                "noInclude" => &mut options.no_include,
                "luau" => &mut options.luau,
                "optimizedLoops" => &mut options.optimized_loops,
                "allowCommentDirectives" => &mut options.allow_comment_directives,
                "logTruthyChanges" => &mut options.log_truthy_changes,
                "writeOnlyChanged" => &mut options.write_only_changed,
                _ => &mut options.write_transformed_files,
            };
            *slot = Some(b);
            Ok(())
        };
        match key.as_str() {
            "noInclude"
            | "luau"
            | "optimizedLoops"
            | "allowCommentDirectives"
            | "logTruthyChanges"
            | "writeOnlyChanged"
            | "writeTransformedFiles" => flag(&mut options)?,
            "rojo" | "includePath" => {
                let s = setting.as_str().ok_or_else(|| invalid(&format!("\"{key}\" must be a string")))?;
                let resolved =
                    if s.is_empty() { String::new() } else { nodepath::resolve(&[nodepath::dirname(config_path), s]) };
                if key == "rojo" {
                    options.rojo = Some(resolved);
                } else {
                    options.include_path = Some(resolved);
                }
            }
            "type" => {
                let t = setting
                    .as_str()
                    .and_then(ProjectType::parse)
                    .ok_or_else(|| invalid("\"type\" must be \"game\", \"model\", or \"package\""))?;
                options.project_type = Some(t);
            }
            _ => warnings.push(format!("Ignoring unsupported \"rbxts\" option \"{key}\" in \"{config_path}\".")),
        }
    }
    Ok(options)
}

// ---------------------------------------------------------------------------------------------------------------
// createProjectData

/// roblox-ts's `ProjectData` for one project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectData {
    /// The tsconfig's path, absolute.
    pub tsconfig_path: String,
    /// The tsconfig's directory.
    pub project_path: String,
    /// package.json's name is scoped (`@scope/name`).
    pub is_package: bool,
    /// `node_modules` next to the nearest package.json.
    pub node_modules_path: String,
    /// With `include_path` resolved.
    pub options: ProjectOptions,
    /// The Rojo project file the build uses (the option, or the one found in the project directory); None without one.
    pub rojo_config_path: Option<String>,
    /// From looking for the Rojo config.
    pub warnings: Vec<String>,
}

/// `PACKAGE_REGEX`: npm allows any non-empty scope, including `.`, `_` and `~`.
fn is_scoped_package_name(name: &str) -> bool {
    name.strip_prefix('@').and_then(|rest| rest.find('/')).is_some_and(|i| i > 0)
}

/// `ts.findPackageJson`: the nearest package.json from `directory` upwards.
pub fn find_package_json(directory: &str) -> Option<String> {
    find_package_json_recorded(directory, None)
}

/// [`find_package_json`], each place looked at recorded in `reads` (a watcher watches them).
pub fn find_package_json_recorded(directory: &str, reads: Option<&ReadLog>) -> Option<String> {
    let mut dir = directory.to_string();
    loop {
        let candidate = tspath::combine_paths(&dir, &["package.json"]);
        let found = fs::metadata(&candidate).is_ok_and(|m| m.is_file());
        if let Some(reads) = reads {
            reads.see(&candidate, if found { Seen::FILE } else { Seen::NO_FILE });
        }
        if found {
            return Some(candidate);
        }
        let parent = tspath::get_directory_path(&tspath::normalize_slashes(&dir));
        if parent == tspath::normalize_slashes(&dir) {
            return None;
        }
        dir = parent;
    }
}

/// `createProjectData`.
pub fn create_project_data(tsconfig_path: &str, options: ProjectOptions) -> Result<ProjectData, ProjectError> {
    create_project_data_recorded(tsconfig_path, options, None)
}

/// [`create_project_data`], the package.json lookup and read recorded in `reads`.
pub fn create_project_data_recorded(
    tsconfig_path: &str,
    mut options: ProjectOptions,
    reads: Option<&ReadLog>,
) -> Result<ProjectData, ProjectError> {
    let project_path = nodepath::dirname(tsconfig_path).to_owned();
    let pkg_json_path = find_package_json_recorded(&project_path, reads)
        .ok_or_else(|| ProjectError::new("Unable to find package.json"))?;

    // a package.json that cannot be read or parsed means "not a package"
    let text = fs::read_to_string(&pkg_json_path).ok();
    if let Some(reads) = reads {
        reads.see_text(&pkg_json_path, text.as_deref());
    }
    let is_package = text
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|json| json.get("name").and_then(Value::as_str).is_some_and(is_scoped_package_name));

    // `||`: an empty include path means the default
    options.include_path = if options.include_path.is_empty() {
        nodepath::resolve(&[&nodepath::join(&[&project_path, "include"])])
    } else {
        nodepath::resolve(&[&options.include_path])
    };

    let node_modules_path = nodepath::join(&[nodepath::dirname(&pkg_json_path), NODE_MODULES]);

    let mut warnings = Vec::new();
    let rojo_config_path = match options.rojo.as_deref() {
        Some(rojo) if !rojo.is_empty() => Some(nodepath::resolve(&[rojo])),
        _ => {
            let found = RojoResolver::find_rojo_config_file_path(&project_path)
                .map_err(|e| ProjectError::new(e.to_string()))?;
            warnings.extend(found.warnings);
            found.path
        }
    };

    Ok(ProjectData {
        tsconfig_path: tsconfig_path.to_owned(),
        project_path,
        is_package,
        node_modules_path,
        options,
        rojo_config_path,
        warnings,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// validateCompilerOptions

/// `validateCompilerOptions`. `module` / `moduleResolution` must be `commonjs` / `node10` or `preserve` / `bundler`
/// (the pair the tsrs experiment accepts; TypeScript 7 drops `node10`). `importsNotUsedAsValues` is not checked:
/// TypeScript 7 rejects the option itself.
pub fn validate_compiler_options(opts: &CompilerOptions, project_path: &str) -> Result<(), ProjectError> {
    let mut errors: Vec<String> = Vec::new();

    if opts.no_lib != Tristate::True {
        errors.push("\"noLib\" must be true".into());
    }
    if opts.strict != Tristate::True {
        errors.push("\"strict\" must be true".into());
    }
    let module_ok = (opts.module == ModuleKind::CommonJS && opts.module_resolution == ModuleResolutionKind::Node10)
        || (opts.module == ModuleKind::Preserve && opts.module_resolution == ModuleResolutionKind::Bundler);
    if !module_ok {
        errors.push(
            "\"module\" and \"moduleResolution\" must be commonjs and \"Node\", or preserve and \"bundler\"".into(),
        );
    }
    if opts.module_detection != ModuleDetectionKind::Force {
        errors.push("\"moduleDetection\" must be \"force\"".into());
    }
    if opts.allow_synthetic_default_imports != Tristate::True {
        errors.push("\"allowSyntheticDefaultImports\" must be true".into());
    }

    let rbxts_modules = nodepath::join(&[project_path, NODE_MODULES, RBXTS_SCOPE]);
    let valid_type_roots = opts.type_roots.as_ref().is_some_and(|roots| {
        roots.iter().any(|root| {
            let resolved = nodepath::resolve(&[root]);
            nodepath::basename(&resolved) == RBXTS_SCOPE
                && nodepath::basename(nodepath::dirname(&resolved)) == NODE_MODULES
        })
    });
    if !valid_type_roots {
        errors.push(format!("\"typeRoots\" must contain a node_modules/@rbxts directory, such as {rbxts_modules}"));
    }

    let default_roots = vec!["node_modules/@rbxts".to_string()];
    for types_location in opts.types.iter().flatten() {
        let roots = opts.type_roots.as_ref().unwrap_or(&default_roots);
        let found = roots.iter().any(|root| {
            let types_path = nodepath::resolve(&[project_path, root, types_location]);
            recorded::exists(&types_path) || recorded::exists(format!("{types_path}.d.ts"))
        });
        if !found {
            errors.push(format!(
                "\"types\" {types_location} were not found. Make sure the path is relative to `typeRoots`"
            ));
        }
    }

    if opts.root_dir.is_empty() && opts.root_dirs.is_none() {
        errors.push("\"rootDir\" or \"rootDirs\" must be defined".into());
    }
    if opts.out_dir.is_empty() {
        errors.push("\"outDir\" must be defined".into());
    }

    if errors.is_empty() {
        return Ok(());
    }
    let list: String = errors.iter().map(|e| format!("- {e}\n")).collect();
    Err(ProjectError::new(format!(
        "Invalid \"tsconfig.json\" configuration!\nhttps://roblox-ts.com/docs/quick-start#project-folder-setup\n{list}"
    )))
}

// ---------------------------------------------------------------------------------------------------------------
// PathTranslator

/// `findAncestorDir`: the nearest directory (with a trailing separator) that every one of `dirs` is inside, by
/// string prefix. Gives up (where the JavaScript loops forever) when climbing stops changing the path.
pub fn find_ancestor_dir(dirs: &[String]) -> String {
    let sep = nodepath::SEP;
    let dirs: Vec<String> = dirs
        .iter()
        .map(|d| {
            let n = nodepath::normalize(d);
            if n.ends_with(sep) { n } else { format!("{n}{sep}") }
        })
        .collect();
    let Some(mut current) = dirs.first().cloned() else { return String::new() };
    let sep_str = sep.to_string();
    // A relative first entry (an empty common source directory) never becomes a prefix of absolute ones.
    for _ in 0..4096 {
        if dirs.iter().all(|d| d.starts_with(&current)) {
            break;
        }
        let next = nodepath::join(&[&current, "..", &sep_str]);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// `getRootDirs`: `[rootDir]`, else `rootDirs`.
pub fn get_root_dirs(opts: &CompilerOptions) -> Vec<String> {
    if !opts.root_dir.is_empty() { vec![opts.root_dir.clone()] } else { opts.root_dirs.clone().unwrap_or_default() }
}

/// TypeScript 5's `getTsBuildInfoEmitOutputFilePath` (the build info of an incremental or composite build): tsc's
/// own file, which roblox-ts's clean keeps beside its `.rbxtsc.tsbuildinfo`.
pub fn ts5_build_info_path(opts: &CompilerOptions) -> Option<String> {
    if !(opts.incremental.is_true() || opts.composite.is_true()) {
        return None;
    }
    if !opts.ts_build_info_file.is_empty() {
        return Some(opts.ts_build_info_file.clone());
    }
    let extensionless = if !opts.out_file.is_empty() {
        tspath::remove_file_extension(&opts.out_file).to_owned()
    } else {
        if opts.config_file_path.is_empty() {
            return None;
        }
        let config = tspath::remove_file_extension(&opts.config_file_path).to_owned();
        if opts.out_dir.is_empty() {
            config
        } else if !opts.root_dir.is_empty() {
            let compare =
                tspath::ComparePathsOptions { use_case_sensitive_file_names: false, current_directory: String::new() };
            tspath::resolve_path(
                &opts.out_dir,
                &[&tspath::get_relative_path_from_directory(&opts.root_dir, &config, &compare)],
            )
        } else {
            tspath::combine_paths(&opts.out_dir, &[&tspath::get_base_file_name(&config)])
        }
    };
    Some(format!("{extensionless}.tsbuildinfo"))
}

/// The path translator `ProjectGraph` makes for a (non-solution) project.
///
/// - `rootDir`: the common ancestor of TypeScript 5's common source directory of the config (`rootDir`; for a
///   composite project without one, the tsconfig's directory; else computed from the config's non-declaration
///   files, which is not TypeScript 7's rule) and the root dirs.
/// - Build info: TypeScript 5's path, renamed to `*.rbxtsc.tsbuildinfo` as roblox-ts does.
pub fn create_path_translator(
    opts: &CompilerOptions,
    config_file_names: &[String],
    data: &ProjectData,
    use_case_sensitive_file_names: bool,
) -> PathTranslator {
    let config_dir = tspath::get_directory_path(&tspath::normalize_slashes(&data.tsconfig_path));
    // getParsedCommandLine: composite projects default to the config directory
    let root_dir_option = if opts.composite.is_true() && opts.root_dir.is_empty() {
        nodepath::dirname(&data.tsconfig_path).to_owned()
    } else {
        opts.root_dir.clone()
    };
    let common_source_dir = if !root_dir_option.is_empty() {
        tspath::ensure_trailing_directory_separator(&tspath::get_normalized_absolute_path(
            &root_dir_option,
            &config_dir,
        ))
    } else {
        let files: Vec<String> =
            config_file_names.iter().filter(|f| !tspath::is_declaration_file_name(f)).cloned().collect();
        tsrs_tsoptions::outputpaths::get_computed_common_source_directory(
            &files,
            &config_dir,
            use_case_sensitive_file_names,
        )
    };
    let mut dirs = vec![common_source_dir];
    if !root_dir_option.is_empty() {
        dirs.push(root_dir_option);
    } else {
        dirs.extend(opts.root_dirs.clone().unwrap_or_default());
    }
    let root_dir = find_ancestor_dir(&dirs);

    let build_info = ts5_build_info_path(opts).map(|p| {
        let base = p.strip_suffix(".tsbuildinfo").unwrap_or(&p);
        nodepath::normalize(&format!("{base}.rbxtsc.tsbuildinfo"))
    });

    PathTranslator::new(
        root_dir,
        nodepath::normalize(&opts.out_dir),
        build_info,
        opts.get_emit_declarations(),
        data.options.luau,
    )
}

// ---------------------------------------------------------------------------------------------------------------
// compileFiles: Rojo and package setup

/// `createNodeModulesPathMapping`: for every package in the type roots whose package.json has a `main`, its types
/// entry (canonical file name) → its main file. Imports of a package's types resolve to its code through this.
pub fn create_node_modules_path_mapping(
    type_roots: &[String],
    use_case_sensitive_file_names: bool,
) -> Result<HashMap<String, String>, ProjectError> {
    create_node_modules_path_mapping_recorded(type_roots, use_case_sensitive_file_names, None)
}

/// [`create_node_modules_path_mapping`], the type roots and the package.json files read recorded in `reads`.
pub fn create_node_modules_path_mapping_recorded(
    type_roots: &[String],
    use_case_sensitive_file_names: bool,
    reads: Option<&ReadLog>,
) -> Result<HashMap<String, String>, ProjectError> {
    let mut mapping = HashMap::new();
    for scope_path in type_roots {
        let exists = Path::new(scope_path).exists();
        if let Some(reads) = reads {
            reads.see(scope_path, if exists { Seen::DIR } else { Seen::NO_DIR });
        }
        if !exists {
            continue;
        }
        let mut names: Vec<String> = recorded::read_dir_names(scope_path)
            .map_err(|e| ProjectError::new(format!("cannot read directory \"{scope_path}\": {e}")))?;
        if cfg!(not(windows)) {
            names.sort();
        }
        for pkg_name in names {
            let pkg_path = nodepath::join(&[scope_path, &pkg_name]);
            let pkg_json_path = nodepath::join(&[&pkg_path, "package.json"]);
            if !Path::new(&pkg_json_path).exists() {
                if let Some(reads) = reads {
                    reads.see(&pkg_json_path, Seen::NO_FILE);
                }
                continue;
            }
            let text = fs::read_to_string(&pkg_json_path);
            if let Some(reads) = reads {
                reads.see_text(&pkg_json_path, text.as_deref().ok());
            }
            let text = text.map_err(|e| ProjectError::new(format!("{pkg_json_path}: {e}")))?;
            // fs-extra's readJsonSync strips a byte-order mark
            let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
                .map_err(|e| ProjectError::new(format!("{pkg_json_path}: {e}")))?;
            // both "types" and "typings" are valid; `??` keeps an empty string
            let types = match (json.get("types"), json.get("typings")) {
                (Some(v), _) if !v.is_null() => Some(("types", v)),
                (_, Some(v)) if !v.is_null() => Some(("typings", v)),
                _ => None,
            };
            // `if (pkgJson.main)`: JavaScript truthiness
            let main = json.get("main").filter(|main| match main {
                Value::Null => false,
                Value::Bool(b) => *b,
                Value::Number(n) => n.as_f64() != Some(0.0),
                Value::String(s) => !s.is_empty(),
                Value::Array(_) | Value::Object(_) => true,
            });
            if let Some(main) = main {
                // rbxts-rs: fixes roblox-ts crashing (`path.resolve` throws a TypeError) on a `main`, or a `types` /
                // `typings` beside a `main`, that is not a string: a build error naming the field
                let not_a_string = |field: &str, value: &Value| {
                    ProjectError::new(format!("{pkg_json_path}: \"{field}\" must be a string, not {value}"))
                };
                let main = main.as_str().ok_or_else(|| not_a_string("main", main))?;
                let types_path = match types {
                    None => "index.d.ts",
                    Some((field, value)) => value.as_str().ok_or_else(|| not_a_string(field, value))?,
                };
                mapping.insert(
                    tspath::get_canonical_file_name(
                        &nodepath::resolve(&[&pkg_path, types_path]),
                        use_case_sensitive_file_names,
                    ),
                    nodepath::resolve(&[&pkg_path, main]),
                );
            }
        }
    }
    Ok(mapping)
}

/// roblox-ts's `isPathDescendantOf` (Shared): inside `dir_path`, on the same root.
pub fn is_path_descendant_of(file_path: &str, dir_path: &str) -> bool {
    matches!(nodepath::relation(dir_path, file_path), nodepath::Relation::Same | nodepath::Relation::Inside { .. })
}

/// `FILENAME_WARNINGS`: `init.*` source names that should be `index.*`.
fn file_name_warning(base_name: &str) -> Option<String> {
    let rest = base_name.strip_prefix("init")?;
    for sub in [".server", ".client", ""] {
        for ext in [".ts", ".tsx", ".d.ts"] {
            if rest == format!("{sub}{ext}") {
                return Some(format!("index{sub}{ext}"));
            }
        }
    }
    None
}

/// A program source file as setup needs it.
#[derive(Clone, Copy, Debug)]
pub struct InputFile<'a> {
    /// Its path, as the program spells it.
    pub file_name: &'a str,
    /// A `.d.ts` (or another declaration file).
    pub is_declaration: bool,
    /// Loaded from a package (`isSourceFileFromExternalLibrary`).
    pub is_external_library: bool,
}

/// What the compiler and plugins share about the project once setup has run: roblox-ts's `ProjectData` plus the
/// values `compileFiles` builds before transforming.
#[derive(Debug)]
pub struct ProjectServices {
    /// The project data (options, paths).
    pub data: ProjectData,
    /// Source to output paths.
    pub path_translator: PathTranslator,
    /// The project's Rojo resolver (`synthetic(outDir)` without a Rojo config).
    pub rojo_resolver: RojoResolver,
    /// One synthetic resolver per type root, for imports inside packages.
    pub pkg_rojo_resolvers: Vec<RojoResolver>,
    /// Canonical types entry → main file (`createNodeModulesPathMapping`).
    pub node_modules_path_mapping: HashMap<String, String>,
    /// The project type: the option, else inferred (`package.json` without a Rojo tree: a package; a DataModel tree: a project; else a model).
    pub project_type: ProjectType,
    /// The runtime library's instance path (None for packages).
    pub runtime_lib_rbx_path: Option<RbxPath>,
    /// The program's type roots (`typeRoots`, or TypeScript's defaults), absolute.
    pub type_roots: Vec<String>,
    /// Whether file names compare case-sensitively (TypeScript's `useCaseSensitiveFileNames`).
    pub use_case_sensitive_file_names: bool,
    /// Warnings so far (Rojo config search, resolver warnings), in roblox-ts's order.
    pub warnings: Vec<String>,
    /// package.json lookups for plugins (`declaring_package`, version checks).
    pub packages: crate::packages::PackageJsonCache,
    /// `projectReferencePaths` (`ProjectGraph.mapReferencePaths`): for each file of the projects this one references,
    /// directly or not (sources, declaration files inside their root, and the `.d.ts` TypeScript emits for their
    /// sources), keyed by [`crate::graph::reference_key`], the output an import of it names. Empty without project
    /// references.
    pub project_reference_paths: HashMap<String, String>,
}

/// The outcome of `ProjectSetup::new` when the project could be read: the services, plus what `compileFiles`
/// reports before transforming.
#[derive(Debug)]
pub struct ProjectSetup {
    /// What the setup computed.
    pub services: ProjectServices,
    /// From `checkRojoConfig` and `checkFileName`, in that order. Any error stops the build before transforming.
    pub diagnostics: Vec<ProjectDiagnostic>,
    /// The failure `compileFiles` returns on its own (no Rojo file for a non-package project, or a misplaced runtime
    /// library); roblox-ts then reports only this.
    pub fatal: Option<String>,
}

impl ProjectSetup {
    /// The build stops before transforming.
    pub fn has_errors(&self) -> bool {
        self.fatal.is_some() || self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
}

impl ProjectSetup {
    /// Builds the Rojo resolvers and the package mapping and runs `compileFiles`'s checks (`checkRojoConfig`,
    /// `checkFileName`, the Rojo file requirement and the runtime library's placement). `files` are the program's
    /// source files in program order. Errors reading the Rojo project are a `ProjectError`, as in `getRojoProject`.
    pub fn new(
        data: ProjectData,
        path_translator: PathTranslator,
        opts: &CompilerOptions,
        files: &[InputFile<'_>],
        use_case_sensitive_file_names: bool,
    ) -> Result<ProjectSetup, ProjectError> {
        Self::new_recorded(data, path_translator, opts, files, use_case_sensitive_file_names, None)
    }

    /// [`ProjectSetup::new`], recording in `reads` the package.json files the package mapping reads (the Rojo
    /// resolvers keep their own record: [`RojoResolver::config_files`], [`RojoResolver::partitions`]).
    pub fn new_recorded(
        data: ProjectData,
        path_translator: PathTranslator,
        opts: &CompilerOptions,
        files: &[InputFile<'_>],
        use_case_sensitive_file_names: bool,
        reads: Option<&ReadLog>,
    ) -> Result<ProjectSetup, ProjectError> {
        let rojo_error =
            |path: &str, e: RojoError| ProjectError::new(format!("Unable to read Rojo project \"{path}\": {e}"));
        let type_roots = opts.type_roots.clone().unwrap_or_default();
        // The project's resolver, the package resolvers and the package mapping each walk directories on their own
        // (the first two can cover all of node_modules/@rbxts), so they are built at the same time.
        // each thread records into this thread's recording, if any
        let log = crate::read_log::current();
        let log = log.as_ref();
        let (rojo_resolver, pkg_rojo_resolvers, node_modules_path_mapping) = std::thread::scope(|scope| {
            let pkg = scope.spawn(|| {
                type_roots
                    .iter()
                    .map(|root| scope.spawn(move || recording(log, || RojoResolver::synthetic(root))))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|h| h.join().expect("package resolver thread"))
                    .collect::<Result<Vec<_>, _>>()
            });
            let mapping = scope.spawn(|| {
                recording(log, || {
                    create_node_modules_path_mapping_recorded(&type_roots, use_case_sensitive_file_names, reads)
                })
            });
            let main = match &data.rojo_config_path {
                Some(path) => {
                    // getRojoProject reads the file before the resolver does, so a missing file is an error here
                    recorded::read(path)
                        .map_err(|e| rojo_error(path, RojoError(e.to_string())))
                        .and_then(|_| RojoResolver::from_path(path).map_err(|e| rojo_error(path, e)))
                }
                None => RojoResolver::synthetic(&opts.out_dir).map_err(|e| ProjectError::new(e.to_string())),
            };
            let pkg = pkg.join().expect("package resolvers thread").map_err(|e| ProjectError::new(e.to_string()));
            let mapping = mapping.join().expect("package mapping thread");
            (main, pkg, mapping)
        });
        let rojo_resolver = rojo_resolver?;
        let pkg_rojo_resolvers = pkg_rojo_resolvers?;
        let node_modules_path_mapping = node_modules_path_mapping?;

        let mut warnings = data.warnings.clone();
        warnings.extend(rojo_resolver.warnings().iter().cloned());
        let mut diagnostics = Vec::new();

        // checkRojoConfig
        if let Some(rojo_config_path) = &data.rojo_config_path {
            let root_dirs = get_root_dirs(opts);
            // resolved once: `relative` resolves its arguments anyway, and resolved paths take its fast path
            let root_dirs: Vec<String> = root_dirs.iter().map(|d| nodepath::resolve(&[d])).collect();
            let inputs: Vec<String> = files
                .iter()
                .filter(|f| !f.is_declaration && !f.is_external_library)
                .map(|f| nodepath::resolve(&[f.file_name]))
                .filter(|name| root_dirs.iter().any(|root| is_path_descendant_of(name, root)))
                .collect();
            let rojo_config_dir = nodepath::dirname(rojo_config_path);
            for partition in rojo_resolver.partitions() {
                // only flag partitions that would sync compiler input
                if inputs.iter().any(|name| is_path_descendant_of(name, &partition.fs_path)) {
                    let out_path = path_translator.get_output_path(&partition.fs_path);
                    let input_path = nodepath::relative(rojo_config_dir, &partition.fs_path);
                    let suggested_path = nodepath::relative(rojo_config_dir, &out_path);
                    diagnostics.push(ProjectDiagnostic::error(format!(
                        "Invalid Rojo configuration. $path fields should be relative to out directory.\nSuggestion: Change the value of $path from \"{input_path}\" to \"{suggested_path}\"."
                    )));
                }
            }
        }

        // checkFileName
        for f in files {
            if !nodepath::normalize(f.file_name).starts_with(&data.node_modules_path) {
                let base_name = nodepath::basename(f.file_name);
                if let Some(suggested) = file_name_warning(base_name) {
                    diagnostics.push(ProjectDiagnostic::error(format!(
                        "Incorrect file name: `{base_name}`!\nFull path: {}\nSuggestion: Change `{base_name}` to `{suggested}`.",
                        f.file_name
                    )));
                }
            }
        }

        let project_type = data.options.project_type.unwrap_or(if data.is_package {
            ProjectType::Package
        } else if rojo_resolver.is_game {
            ProjectType::Game
        } else {
            ProjectType::Model
        });

        let mut runtime_lib_rbx_path = None;
        let mut fatal = None;
        if project_type != ProjectType::Package && data.rojo_config_path.is_none() {
            fatal = Some("Non-package projects must have a Rojo project file!");
        } else if project_type != ProjectType::Package {
            let runtime_lib = nodepath::join(&[&data.options.include_path, "RuntimeLib.lua"]);
            match rojo_resolver.get_rbx_path_from_file_path(&runtime_lib) {
                None => fatal = Some("Rojo project contained no data for include folder!"),
                Some(path) if rojo_resolver.get_network_type(&path) != NetworkType::Unknown => {
                    fatal = Some("Runtime library cannot be in a server-only or client-only container!");
                }
                Some(path) if rojo_resolver.is_isolated(&path) => {
                    fatal = Some("Runtime library cannot be in an isolated container!");
                }
                Some(path) => runtime_lib_rbx_path = Some(path),
            }
        }

        let services = ProjectServices {
            data,
            path_translator,
            rojo_resolver,
            pkg_rojo_resolvers,
            node_modules_path_mapping,
            project_type,
            runtime_lib_rbx_path,
            type_roots,
            use_case_sensitive_file_names,
            warnings,
            packages: Default::default(),
            project_reference_paths: HashMap::new(),
        };
        Ok(ProjectSetup { services, diagnostics, fatal: fatal.map(str::to_owned) })
    }
}

impl ProjectServices {
    /// The Roblox facts of one project source file, as the transformer reads them.
    pub fn file_info(&self, file_name: &str) -> FileRojoInfo {
        let out_path = self.path_translator.get_output_path(file_name);
        let rbx_path = self.rojo_resolver.get_rbx_path_from_file_path(&out_path);
        let rbx_type = self.rojo_resolver.get_rbx_type_from_file_path(&out_path);
        let (network_type, is_isolated) = match &rbx_path {
            Some(p) => (self.rojo_resolver.get_network_type(p), self.rojo_resolver.is_isolated(p)),
            None => (NetworkType::Unknown, false),
        };
        let in_replicated_first = rbx_path.as_ref().is_some_and(|p| p.first().is_some_and(|s| s == "ReplicatedFirst"));
        FileRojoInfo { out_path, rbx_path, rbx_type, network_type, is_isolated, in_replicated_first }
    }

    /// The main file of a package whose types entry is `types_path` (`nodeModulesPathMapping` lookup).
    pub fn node_modules_main_for(&self, types_path: &str) -> Option<&str> {
        let key = tspath::get_canonical_file_name(&nodepath::normalize(types_path), self.use_case_sensitive_file_names);
        self.node_modules_path_mapping.get(&key).map(String::as_str)
    }

    /// The instance path of a package file relative to its package root (`findRelativeRbxPath`), trying each type
    /// root's resolver in order.
    pub fn package_relative_rbx_path(&self, module_out_path: &str) -> Option<RbxPath> {
        self.pkg_rojo_resolvers.iter().find_map(|r| r.get_rbx_path_from_file_path(module_out_path))
    }
}

/// Where a source file ends up: its output file and that file's place in the Rojo tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRojoInfo {
    /// The output file (Node spelling).
    pub out_path: String,
    /// None when the Rojo project does not sync the output (an import of it is an error).
    pub rbx_path: Option<RbxPath>,
    /// The kind of script the output becomes in Roblox.
    pub rbx_type: crate::rojo::RbxType,
    /// Which side its container runs on.
    pub network_type: NetworkType,
    /// Whether its container is isolated (`StarterPack`, `StarterGui`, the `StarterPlayer` script folders): an import into it from outside is an error.
    pub is_isolated: bool,
    /// Code here should not use the runtime library (roblox-ts warns where it would).
    pub in_replicated_first: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_names() {
        assert!(is_scoped_package_name("@rbxts/x"));
        assert!(is_scoped_package_name("@./x"));
        assert!(!is_scoped_package_name("@/x"));
        assert!(!is_scoped_package_name("rbxts"));
    }

    #[test]
    fn file_name_warnings() {
        assert_eq!(file_name_warning("init.server.ts").as_deref(), Some("index.server.ts"));
        assert_eq!(file_name_warning("init.d.ts").as_deref(), Some("index.d.ts"));
        assert_eq!(file_name_warning("init.luau"), None);
        assert_eq!(file_name_warning("index.ts"), None);
    }

    #[cfg(windows)]
    #[test]
    fn ancestor_dirs() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(find_ancestor_dir(&s(&["E:/p/src/", "E:/p/src"])), "E:\\p\\src\\");
        assert_eq!(find_ancestor_dir(&s(&["E:/p/src/", "E:/p/src-extra"])), "E:\\p\\");
        assert_eq!(find_ancestor_dir(&s(&["E:/p/src/a/", "E:/p/src", "E:/p/test"])), "E:\\p\\");
    }

    /// The package mapping for one package with this package.json: the mapping's single entry (types → main), or the
    /// error.
    fn package_mapping(package_json: &str) -> Result<Vec<(String, String)>, String> {
        let root = std::env::temp_dir().join(format!("rbxts-rs-package-mapping-{}", std::process::id()));
        let package = root.join("@rbxts").join("pkg");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), package_json).unwrap();
        let scope = root.join("@rbxts").to_string_lossy().into_owned();
        let result = create_node_modules_path_mapping(&[scope], false);
        std::fs::remove_dir_all(&root).unwrap();
        let strip = |p: &str| p.rsplit(['/', '\\']).next().unwrap().to_owned();
        result
            .map(|m| m.iter().map(|(types, main)| (strip(types), strip(main))).collect())
            .map_err(|e| e.to_string().rsplit_once("package.json: ").unwrap().1.to_owned())
    }

    #[test]
    fn package_fields_that_are_not_strings_are_errors() {
        // roblox-ts crashes (a TypeError from `path.resolve`) on each of these
        assert_eq!(package_mapping(r#"{"main": 1}"#), Err(r#""main" must be a string, not 1"#.into()));
        assert_eq!(package_mapping(r#"{"main": "a.lua", "types": {}}"#), Err(r#""types" must be a string, not {}"#.into()));
        assert_eq!(
            package_mapping(r#"{"main": "a.lua", "typings": ["x"]}"#),
            Err(r#""typings" must be a string, not ["x"]"#.into())
        );
        // without a truthy `main`, nothing is read (as in roblox-ts)
        assert_eq!(package_mapping(r#"{"main": 0, "types": 5}"#), Ok(vec![]));
        assert_eq!(package_mapping(r#"{"main": "", "types": 5}"#), Ok(vec![]));
        assert_eq!(
            package_mapping(r#"{"main": "a.lua", "types": null, "typings": "t.d.ts"}"#),
            Ok(vec![("t.d.ts".into(), "a.lua".into())])
        );
        assert_eq!(package_mapping(r#"{"main": "a.lua"}"#), Ok(vec![("index.d.ts".into(), "a.lua".into())]));
    }
}
