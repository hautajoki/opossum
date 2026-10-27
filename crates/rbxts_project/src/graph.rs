//! TypeScript project references: roblox-ts's `ProjectGraph` (`Project/classes/ProjectGraph.ts`) and
//! `validateProjectOutputs` (`Project/functions/getProjectOutputs.ts`).
//!
//! A tsconfig with `references` is built as a graph of projects, each in TypeScript's build order (`tsc -b`: the
//! references depth first, a project after everything it references; a cycle is TypeScript's error 6202). Per project:
//! - **options:** `{ ...defaults, ...root rbxts, ...own rbxts, ...command line }`. When the graph is one deployment
//!   (the root is not a solution config, or it has a Rojo file or `rbxts.rojo`), a referenced project copies no
//!   runtime files, uses the root's include folder, and, unless it is a package or names its own `rbxts.rojo`,
//!   the root's Rojo file. A solution root's `type`, `rojo` and `includePath` are not inherited otherwise;
//! - **path translator:** none for a solution config (no files, only references: nothing to build);
//! - **reference paths** (`mapReferencePaths`): for every file of a project it references, directly or not, the
//!   output an import of that file names, also from the `.d.ts` TypeScript emits for it (what a program sees of a
//!   referenced project);
//! - **checks:** a referenced project must be `composite` and may not disable emit; a project mounting a referenced
//!   project's files elsewhere than that project's own Rojo file does is an error (`validateReferenceRojoPaths`);
//!   output directories may not hold sources, overlap, or hold another project's build info
//!   (`validateProjectOutputs`).
//!
//! Errors are roblox-ts's messages. The graph is made anew for every build, as roblox-ts's `ProjectBuild.refresh`
//! does.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use tsrs_ast::Diagnostic;
use tsrs_core::{CompilerOptions, P, tspath};
use tsrs_tsoptions::{ParseConfigHost, ParsedCommandLine, get_parsed_command_line_of_config_file};
use tsrs_vfs::{FS, bundled, osvfs};

use crate::nodepath;
use crate::path_translator::PathTranslator;
use crate::rojo::{RbxPath, RojoResolver};
use crate::setup::{
    self, ProjectData, ProjectError, ProjectOptionLayer, ProjectOptions, ProjectType, create_path_translator,
    get_root_dirs, is_path_descendant_of, ts5_build_info_path,
};

/// `projectPathKey`: `getCanonicalFileName(path.normalize(filePath))`, the key of every path the graph compares.
pub fn reference_key(path: &str, use_case_sensitive_file_names: bool) -> String {
    let normalized = nodepath::normalize(path);
    if use_case_sensitive_file_names { normalized } else { normalized.to_lowercase() }
}

/// Whether the tsconfig at `tsconfig_path` names project references itself (`references`, which `extends` does not
/// carry): only then is a build a graph. A config that cannot be read is not (the build reports it).
pub fn has_references(tsconfig_path: &str) -> bool {
    setup::read_config_json(tsconfig_path)
        .ok()
        .and_then(|json| json.get("references").cloned())
        .is_some_and(|references| !matches!(&references, Value::Array(list) if list.is_empty()))
}

/// One project of the graph.
#[derive(Debug)]
pub struct ProjectNode {
    /// [`reference_key`] of its tsconfig.
    pub key: String,
    /// Its parsed tsconfig (with `rootDir` set to the tsconfig's directory for a composite project without one, as
    /// roblox-ts's `getParsedCommandLine` sets it).
    pub config: P<ParsedCommandLine>,
    /// Its compiler options as built with (the `rootDir` above).
    pub options: CompilerOptions,
    /// Its project data (roblox-ts's `createProjectData`).
    pub data: ProjectData,
    /// The keys of the projects it references, in order.
    pub dependencies: Vec<String>,
    /// None for a solution config.
    pub path_translator: Option<PathTranslator>,
    /// tsc's own build info (`getTsBuildInfoEmitOutputFilePath` before roblox-ts renames it).
    pub tsc_build_info_path: Option<String>,
    /// Its tsconfig sets `rbxts.rojo`.
    pub own_rojo: bool,
    /// See [`crate::ProjectServices::project_reference_paths`].
    pub reference_paths: HashMap<String, String>,
    /// The values of `reference_paths` in roblox-ts's `Map` order (a key's first insertion, its last value): the
    /// order `validateReferenceRojoPaths` checks them in, which decides the error reported.
    pub reference_path_order: Vec<String>,
}

impl ProjectNode {
    /// `isPackageProject`.
    pub fn is_package(&self) -> bool {
        is_package_project(&self.data)
    }

    /// `getOutputRoots`: the outDir and, for a declaration build with one, the `declarationDir`.
    pub fn output_roots(&self) -> Vec<String> {
        let Some(translator) = &self.path_translator else { return Vec::new() };
        let mut roots = vec![translator.out_dir.clone()];
        if self.options.get_emit_declarations() && !self.options.declaration_dir.is_empty() {
            let declaration_dir = nodepath::normalize(&self.options.declaration_dir);
            if !roots.contains(&declaration_dir) {
                roots.push(declaration_dir);
            }
        }
        roots
    }

    /// Its two build infos: roblox-ts's (`.rbxtsc.tsbuildinfo`) and tsc's.
    pub fn build_infos(&self) -> Vec<String> {
        let own = self.path_translator.as_ref().and_then(|t| t.build_info_output_path.clone());
        own.into_iter().chain(self.tsc_build_info_path.clone()).collect()
    }
}

fn is_package_project(data: &ProjectData) -> bool {
    data.options.project_type == Some(ProjectType::Package) || (data.options.project_type.is_none() && data.is_package)
}

/// The projects of a build with project references.
#[derive(Debug)]
pub struct ProjectGraph {
    /// In build order: every project after the projects it references; the root last.
    pub nodes: Vec<ProjectNode>,
    /// The graph is one deployment (see the module documentation).
    pub shared_deployment: bool,
    /// Every tsconfig of the graph and the configs they extend, and every Rojo project file the graph's Rojo files
    /// name: never assets, and watched.
    pub config_paths: Vec<String>,
}

struct ConfigHost {
    fs: Arc<dyn FS>,
    cwd: String,
}

impl ConfigHost {
    /// The host for `cwd`: one per directory for the process (tsrs's parse host must be `'static`, and this one
    /// keeps no state of a build, so a watcher's graphs share it instead of leaking one each).
    fn interned(cwd: &str) -> &'static ConfigHost {
        static HOSTS: std::sync::Mutex<Option<HashMap<String, &'static ConfigHost>>> = std::sync::Mutex::new(None);
        let mut hosts = HOSTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        hosts.get_or_insert_with(HashMap::new).entry(cwd.to_owned()).or_insert_with(|| {
            let fs: Arc<dyn FS> = Arc::new(bundled::wrap_fs(osvfs::fs()));
            Box::leak(Box::new(ConfigHost { fs, cwd: cwd.to_owned() }))
        })
    }
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// A graph error: roblox-ts's text diagnostics, or TypeScript's (a tsconfig that cannot be parsed, a cycle).
#[derive(Debug)]
pub enum GraphError {
    /// A roblox-ts text diagnostic (an unreadable tsconfig, an invalid `rbxts` option, a project reference roblox-ts refuses).
    Project(ProjectError),
    /// TypeScript's own diagnostics for a tsconfig (parse errors, a reference cycle).
    Diagnostics(Vec<P<Diagnostic>>),
}

impl From<ProjectError> for GraphError {
    fn from(error: ProjectError) -> GraphError {
        GraphError::Project(error)
    }
}

impl ProjectGraph {
    /// The graph of the project at `tsconfig_path` (as spelled for the root's project data: absolute, as roblox-ts's
    /// CLI resolves it), with the command line's options `overrides`. `cwd` resolves relative paths in tsconfigs.
    pub fn new(tsconfig_path: &str, overrides: &ProjectOptionLayer, cwd: &str) -> Result<ProjectGraph, GraphError> {
        let case_sensitive = osvfs::fs().use_case_sensitive_file_names();
        let key = |path: &str| reference_key(path, case_sensitive);
        let host = ConfigHost::interned(&tspath::normalize_path(cwd));

        // the solution builder's build order, parsing each config once (`host.getParsedCommandLine`)
        let mut configs: HashMap<String, (String, P<ParsedCommandLine>)> = HashMap::new();
        let mut config_paths: Vec<String> = Vec::new();
        let mut order: Vec<String> = Vec::new();
        let mut circular: Vec<P<Diagnostic>> = Vec::new();
        let root_name = tspath::get_normalized_absolute_path(tsconfig_path, &host.cwd);
        let mut walk = Walk {
            host,
            key: &key,
            configs: &mut configs,
            config_paths: &mut config_paths,
            order: &mut order,
            circular: &mut circular,
            temporary: HashSet::new(),
            permanent: HashSet::new(),
            stack: Vec::new(),
        };
        walk.visit(&root_name, false)?;
        if !circular.is_empty() {
            return Err(GraphError::Diagnostics(circular));
        }

        let root_key = key(tsconfig_path);
        let (_, root_config) = configs.get(&root_key).cloned().expect("the root config was parsed");
        let root_rbxts = setup::read_project_options(&root_name)?;
        let root_options = ProjectOptions::from_layers(&[&root_rbxts.options, overrides]);
        let mut root_data = setup::create_project_data(tsconfig_path, root_options)?;
        root_data.warnings.splice(0..0, root_rbxts.warnings);
        let root_is_solution = is_solution_config(&root_config);
        let shared_deployment = !root_is_solution
            || root_data.options.rojo.as_deref().is_some_and(|r| !r.is_empty())
            || (!root_data.is_package && root_data.rojo_config_path.is_some());

        let mut inherited = root_rbxts.options.clone();
        let mut reference_overrides = overrides.clone();
        if !shared_deployment {
            // a grouping config has no deployment of its own to impose on independent games and packages
            for layer in [&mut inherited, &mut reference_overrides] {
                layer.project_type = None;
                layer.rojo = None;
                layer.include_path = None;
            }
        }

        let mut nodes = Vec::with_capacity(order.len());
        for node_key in &order {
            let (config_path, config) = configs[node_key].clone();
            let own = setup::read_project_options(&config_path)?;
            let own_rojo = own.options.rojo.as_deref().is_some_and(|r| !r.is_empty());
            let mut data = if *node_key == root_key {
                root_data.clone()
            } else {
                let mut options = ProjectOptions::from_layers(&[&inherited, &own.options, &reference_overrides]);
                if shared_deployment {
                    options.no_include = true;
                    options.include_path = root_data.options.include_path.clone();
                    options.rojo = own.options.rojo.clone();
                }
                let mut data = setup::create_project_data(&config_path, options)?;
                data.warnings.splice(0..0, own.warnings);
                // packages resolve the runtime through their consumer, so only games and models join the root's Rojo
                // tree
                if shared_deployment && !own_rojo && !is_package_project(&data) {
                    data.rojo_config_path = root_data.rojo_config_path.clone();
                }
                data
            };

            let mut options = config.compiler_options().map(|o| (*o).clone()).unwrap_or_default();
            let tsc_build_info_path = ts5_build_info_path(&options).map(|p| nodepath::normalize(&p));
            let path_translator = if is_solution_config(&config) {
                if *node_key != root_key || !shared_deployment {
                    // editor solutions may sit beside a package Rojo file unrelated to their test games
                    data.rojo_config_path = None;
                }
                None
            } else {
                // getParsedCommandLine: composite projects default to the config directory, then the checks
                if options.composite.is_true() && options.root_dir.is_empty() {
                    options.root_dir = nodepath::dirname(&data.tsconfig_path).to_owned();
                }
                setup::validate_compiler_options(&options, &data.project_path)?;
                let raw = config.compiler_options().map(|o| (*o).clone()).unwrap_or_default();
                Some(create_path_translator(&raw, config.file_names(), &data, case_sensitive))
            };
            let dependencies = config
                .project_references()
                .iter()
                .map(|r| key(&tsrs_core::resolve_project_reference_path(r)))
                .collect();
            nodes.push(ProjectNode {
                key: node_key.clone(),
                config,
                options,
                data,
                dependencies,
                path_translator,
                tsc_build_info_path,
                own_rojo,
                reference_paths: HashMap::new(),
                reference_path_order: Vec::new(),
            });
        }

        let mut graph = ProjectGraph { nodes, shared_deployment, config_paths };
        graph.validate_references()?;
        graph.map_reference_paths(case_sensitive);
        graph.validate_reference_rojo_paths(case_sensitive)?;
        graph.validate_project_outputs(case_sensitive)?;
        Ok(graph)
    }

    /// The root project.
    pub fn root(&self) -> &ProjectNode {
        self.nodes.last().expect("a graph has its root")
    }

    /// A project by key.
    pub fn node(&self, key: &str) -> Option<&ProjectNode> {
        self.nodes.iter().find(|n| n.key == key)
    }

    /// The projects that copy the runtime files: the root of one deployment, else every project that builds.
    pub fn runtime_projects(&self) -> Vec<&ProjectNode> {
        if self.shared_deployment {
            vec![self.root()]
        } else {
            self.nodes.iter().filter(|n| n.path_translator.is_some()).collect()
        }
    }

    /// Every project's output roots ([`ProjectNode::output_roots`]), and the include folders of the runtime projects:
    /// never assets of any project.
    pub fn excluded_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.nodes.iter().flat_map(ProjectNode::output_roots).collect();
        roots.extend(self.runtime_projects().iter().map(|n| n.data.options.include_path.clone()));
        roots
    }

    /// Every project's build infos.
    pub fn build_infos(&self) -> Vec<String> {
        self.nodes.iter().flat_map(ProjectNode::build_infos).collect()
    }

    /// `validateReferences`.
    fn validate_references(&self) -> Result<(), GraphError> {
        for project in &self.nodes {
            for dependency in &project.dependencies {
                let Some(target) = self.node(dependency) else { continue };
                if target.path_translator.is_none() {
                    continue;
                }
                let options = &target.options;
                let message = if !options.composite.is_true() {
                    Some(&tsrs_diagnostics::Referenced_project_0_must_have_setting_composite_Colon_true)
                } else if options.no_emit.is_true() || options.declaration.is_false() {
                    Some(&tsrs_diagnostics::Referenced_project_0_may_not_disable_emit)
                } else {
                    None
                };
                if let Some(message) = message {
                    let diagnostic = tsrs_ast::new_compiler_diagnostic(message, &[&target.data.tsconfig_path]);
                    return Err(GraphError::Diagnostics(vec![diagnostic]));
                }
            }
        }
        Ok(())
    }

    /// The keys of the projects `start` references, transitively, each once: dependencies before their dependents
    /// (`mapReferencePaths`'s walk), or with `pre_order` each before its dependencies (`validateReferenceRojoPaths`'s).
    fn reachable(&self, start: &[String], pre_order: bool) -> Vec<String> {
        fn visit(graph: &ProjectGraph, key: &str, pre: bool, seen: &mut HashSet<String>, out: &mut Vec<String>) {
            if !seen.insert(key.to_owned()) {
                return;
            }
            let Some(node) = graph.node(key) else { return };
            if pre {
                out.push(key.to_owned());
            }
            for dependency in &node.dependencies {
                visit(graph, dependency, pre, seen, out);
            }
            if !pre {
                out.push(key.to_owned());
            }
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for key in start {
            visit(self, key, pre_order, &mut seen, &mut out);
        }
        out
    }

    /// The files of a referenced project whose outputs an import can name: its tsconfig's files, except declaration
    /// files outside its root.
    fn reference_files<'a>(
        reference: &'a ProjectNode,
        translator: &'a PathTranslator,
    ) -> impl Iterator<Item = &'a String> {
        reference
            .config
            .file_names()
            .iter()
            .filter(move |f| !(tspath::is_declaration_file_name(f) && !is_path_descendant_of(f, &translator.root_dir)))
    }

    /// `mapReferencePaths`.
    fn map_reference_paths(&mut self, case_sensitive: bool) {
        let mut all = Vec::with_capacity(self.nodes.len());
        for project in &self.nodes {
            // a `Map`: a key keeps the place of its first `set`
            let mut entries: Vec<(String, String)> = Vec::new();
            let mut at: HashMap<String, usize> = HashMap::new();
            let mut set = |key: String, value: String| match at.get(&key) {
                Some(&index) => entries[index].1 = value,
                None => {
                    at.insert(key.clone(), entries.len());
                    entries.push((key, value));
                }
            };
            for key in self.reachable(&project.dependencies, false) {
                let Some(reference) = self.node(&key) else { continue };
                let Some(translator) = &reference.path_translator else { continue };
                for file_name in Self::reference_files(reference, translator) {
                    let output_path = translator.get_import_path(file_name, false);
                    set(reference_key(file_name, case_sensitive), output_path.clone());
                    if !tspath::is_declaration_file_name(file_name) && !file_name.ends_with(".json") {
                        let declaration = tsrs_tsoptions::outputpaths::get_output_declaration_file_name_worker(
                            file_name,
                            &reference.options,
                            &*reference.config,
                        );
                        set(reference_key(&declaration, case_sensitive), output_path);
                    }
                }
            }
            all.push(entries);
        }
        for (node, entries) in self.nodes.iter_mut().zip(all) {
            node.reference_path_order = entries.iter().map(|(_, value)| value.clone()).collect();
            node.reference_paths = entries.into_iter().collect();
        }
    }

    /// `validateReferenceRojoPaths` (`refreshRojoProjects` loads each Rojo file once; projects with the same file
    /// share its resolver).
    fn validate_reference_rojo_paths(&mut self, case_sensitive: bool) -> Result<(), GraphError> {
        let key = |path: &str| reference_key(path, case_sensitive);
        let mut resolvers: HashMap<String, Option<RojoResolver>> = HashMap::new();
        for node in &self.nodes {
            if let Some(path) = &node.data.rojo_config_path {
                resolvers.entry(key(path)).or_insert_with(|| RojoResolver::from_path(path).ok());
                self.config_paths.push(path.clone());
                if let Some(Some(resolver)) = resolvers.get(&key(path)) {
                    self.config_paths.extend(resolver.config_files().iter().cloned());
                }
            }
        }
        let resolver_of = |node: &ProjectNode| -> Option<(String, &RojoResolver)> {
            let path = node.data.rojo_config_path.as_ref()?;
            let k = key(path);
            resolvers.get(&k)?.as_ref().map(|r| (k, r))
        };
        let root_key = self.root().key.clone();
        for project in &self.nodes {
            if project.path_translator.is_none() && !(self.shared_deployment && project.key == root_key) {
                continue;
            }
            let references: Vec<String> = self
                .reachable(&project.dependencies, true)
                .into_iter()
                .filter(|k| self.node(k).is_some_and(|n| n.path_translator.is_some()))
                .collect();
            for reference_key in references {
                let reference = self.node(&reference_key).expect("a reachable project");
                let translator = reference.path_translator.as_ref().expect("filtered above");
                let reference_is_package = reference.is_package();
                let Some((owner_key, owner)) = resolver_of(reference) else { continue };
                if !reference.own_rojo && reference_is_package {
                    continue;
                }
                let consumer = resolver_of(project);
                let same_runtime = key(&reference.data.options.include_path) == key(&project.data.options.include_path);
                if consumer.as_ref().is_some_and(|(k, _)| *k == owner_key) && (reference_is_package || same_runtime) {
                    continue;
                }
                let consumer = consumer.map(|(_, r)| r);
                let consumer_path = |file: &str| -> Option<RbxPath> { consumer?.get_rbx_path_from_file_path(file) };
                let mut paths: Vec<String> = Vec::new();
                for file_name in Self::reference_files(reference, translator) {
                    let path = translator.get_import_path(file_name, false);
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
                // a solution can group unrelated test games that this consumer does not deploy
                if !project.dependencies.contains(&reference_key) && !paths.iter().any(|p| consumer_path(p).is_some()) {
                    continue;
                }
                for path in &reference.reference_path_order {
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                }
                paths.push(nodepath::join(&[&reference.data.options.include_path, "RuntimeLib.lua"]));
                for file_path in &paths {
                    let owner_path = owner.get_rbx_path_from_file_path(file_path);
                    if owner_path != consumer_path(file_path) {
                        if project.is_package() && !project.own_rojo && !reference_is_package {
                            return Err(ProjectError::new(format!(
                                "Package project \"{}\" references non-package project \"{}\". Set rbxts.rojo on the \
                                 package or compile the reference as a package.",
                                project.data.tsconfig_path, reference.data.tsconfig_path
                            ))
                            .into());
                        }
                        let mounted = owner_path.map_or_else(|| "unmapped".to_owned(), |p| p.join("."));
                        return Err(ProjectError::new(format!(
                            "Project \"{}\" must mount \"{file_path}\" at the same Roblox path as \"{}\" ({mounted}). \
                             Referenced projects with their own rbxts.rojo require consistent module and runtime \
                             mounts.",
                            project.data.tsconfig_path,
                            reference.data.rojo_config_path.as_deref().unwrap_or_default(),
                        ))
                        .into());
                    }
                }
                if !reference_is_package && !same_runtime && !project.is_package() {
                    return Err(ProjectError::new(format!(
                        "Project \"{}\" and referenced project \"{}\" must use the same runtime folder. Compile \
                         reusable libraries as packages to share them between independent deployments.",
                        project.data.tsconfig_path, reference.data.tsconfig_path
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }

    /// `validateProjectOutputs`.
    fn validate_project_outputs(&self, case_sensitive: bool) -> Result<(), GraphError> {
        let key = |path: &str| reference_key(path, case_sensitive);
        let sources: Vec<String> =
            self.nodes.iter().filter(|n| n.path_translator.is_some()).flat_map(|n| get_root_dirs(&n.options)).collect();
        let mut owners: Vec<(String, usize)> = Vec::new();
        let mut build_info_owners: Vec<(String, String, usize)> = Vec::new();
        for (index, project) in self.nodes.iter().enumerate() {
            for root in project.output_roots() {
                for source in &sources {
                    if is_path_descendant_of(source, &root) {
                        return Err(ProjectError::new(format!(
                            "Output directory \"{root}\" contains source directory \"{source}\"."
                        ))
                        .into());
                    }
                }
                for (other_root, owner) in &owners {
                    if *owner != index
                        && (is_path_descendant_of(&root, other_root) || is_path_descendant_of(other_root, &root))
                    {
                        return Err(ProjectError::new(format!(
                            "Projects \"{}\" and \"{}\" have overlapping output directories. Use separate outDir and \
                             declarationDir paths.",
                            self.nodes[*owner].data.tsconfig_path, project.data.tsconfig_path
                        ))
                        .into());
                    }
                }
                // a `Map`: setting a root again keeps its place
                match owners.iter_mut().find(|(r, _)| *r == root) {
                    Some(owner) => owner.1 = index,
                    None => owners.push((root, index)),
                }
            }
            for build_info in project.build_infos() {
                let build_info_key = key(&build_info);
                if build_info_owners.iter().any(|(k, _, _)| *k == build_info_key) {
                    return Err(ProjectError::new(format!(
                        "Multiple projects write \"{build_info}\". Set a unique tsBuildInfoFile for each project."
                    ))
                    .into());
                }
                build_info_owners.push((build_info_key, build_info, index));
            }
        }
        for (build_info_key, _, project) in &build_info_owners {
            for (root, owner) in &owners {
                if owner != project && is_path_descendant_of(build_info_key, &key(root)) {
                    return Err(ProjectError::new(format!(
                        "Build info \"{build_info_key}\" is inside another project's output directory \"{root}\"."
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }
}

/// `ts.isSolutionConfig`: no files, and a `references` property.
fn is_solution_config(config: &ParsedCommandLine) -> bool {
    config.file_names().is_empty() && !config.project_references_is_nil()
}

/// The solution builder's `createBuildOrder`, parsing each config with roblox-ts's `parseProjectConfig`.
struct Walk<'a> {
    host: &'static ConfigHost,
    key: &'a dyn Fn(&str) -> String,
    configs: &'a mut HashMap<String, (String, P<ParsedCommandLine>)>,
    config_paths: &'a mut Vec<String>,
    order: &'a mut Vec<String>,
    circular: &'a mut Vec<P<Diagnostic>>,
    temporary: HashSet<String>,
    permanent: HashSet<String>,
    stack: Vec<String>,
}

impl Walk<'_> {
    fn visit(&mut self, config_name: &str, in_circular_context: bool) -> Result<(), GraphError> {
        let key = (self.key)(config_name);
        if self.permanent.contains(&key) {
            return Ok(());
        }
        if self.temporary.contains(&key) {
            if !in_circular_context {
                let cycle = self.stack.join("\r\n");
                self.circular.push(tsrs_ast::new_compiler_diagnostic(
                    &tsrs_diagnostics::Project_references_may_not_form_a_circular_graph_Cycle_detected_Colon_0,
                    &[&cycle],
                ));
            }
            return Ok(());
        }
        self.temporary.insert(key.clone());
        self.stack.push(config_name.to_owned());
        let config = self.parse(config_name)?;
        for reference in config.project_references() {
            let path = tsrs_core::resolve_project_reference_path(reference);
            self.visit(&path, in_circular_context || reference.circular)?;
        }
        self.stack.pop();
        self.permanent.insert(key.clone());
        self.order.push(key);
        Ok(())
    }

    /// `parseProjectConfig`: the parse's errors stop the build; the `rbxts` options are read for each project later.
    fn parse(&mut self, config_name: &str) -> Result<P<ParsedCommandLine>, GraphError> {
        self.config_paths.push(nodepath::normalize(config_name));
        let (parsed, errors) = tsrs_compiler::worker_pool()
            .install(|| get_parsed_command_line_of_config_file(config_name, None, None, self.host, None));
        let Some(parsed) = parsed.filter(|_| errors.is_empty()) else {
            return Err(GraphError::Diagnostics(errors));
        };
        let config = P::new(parsed);
        self.config_paths.extend(config.extended_source_files().iter().map(|p| nodepath::normalize(p)));
        self.configs.insert((self.key)(config_name), (config_name.to_owned(), config));
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_normalise_and_fold_case() {
        assert_eq!(reference_key("E:/A/b/../C.ts", false), reference_key("e:\\a\\c.ts", false));
        assert_ne!(reference_key("E:/A/C.ts", true), reference_key("e:/a/c.ts", true));
    }
}
