//! `util/createImportExpression.ts` and `util/getSourceFileFromModuleSpecifier.ts`: `TS.import(script, …)` paths
//! from the Rojo tree.

use rbxts_luau::{Expr, create, globals};
use rbxts_project::{
    FileRelation, NetworkType, ProjectType, RbxPath, RbxType, RelativeRbxPathPart, RojoResolver, nodepath,
};
use tsrs_ast::{Kind, Node, SourceFile};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::shared::constants::{NODE_MODULES, PARENT_FIELD};
use crate::shared::diagnostics::errors;
use crate::util::create_get_service::create_get_service;
use crate::util::expression_chain::property_access_expression_chain;
use crate::util::traversal::source_file_of;

/// `getSourceFileFromModuleSpecifier`
pub fn get_source_file_from_module_specifier(
    state: &mut TransformState<'_>,
    module_specifier: P<Node>,
) -> Option<P<SourceFile>> {
    let symbol = state
        .checker
        .get_symbol_at_location_exported(module_specifier)
        .or_else(|| state.checker.resolve_external_module_name_exported(module_specifier, None));
    if let Some(symbol) = symbol {
        let declaration = symbol.value_declaration();
        if let Some(declaration) = declaration
            && declaration.kind() == Kind::ModuleDeclaration
            && let Some(name) = declaration.name()
            && tsrs_ast::is_string_literal_like(name)
        {
            let source_file = source_file_of(module_specifier);
            let mode = state.program.get_mode_for_usage_location(source_file, name);
            if let Some(resolved) = state.program.get_resolved_module(source_file, name.text(), mode)
                && resolved.is_resolved()
            {
                return state.program.get_source_file(resolved.resolved_file_name);
            }
        }
        if let Some(declaration) = declaration
            && declaration.kind() == Kind::SourceFile
        {
            return Some(declaration.as_source_file_p());
        }
    }
    // Fallback for $getModuleTree when module is not referenced by any regular import
    if tsrs_ast::is_string_literal_like(module_specifier) {
        let source_file = source_file_of(module_specifier);
        let program = state.program;
        let resolver = state.module_resolver;
        // `ts.resolveModuleName` is called without a resolution mode; with no mode tsrs picks the same conditions
        // (e.g. "import" under bundler) as TypeScript
        if let Some(resolved) = resolver.resolve(
            module_specifier.text(),
            source_file.get().file_name(),
            tsrs_core::RESOLUTION_MODE_NONE,
        ) {
            return program.get_source_file(&resolved.file_name);
        }
    }
    None
}

/// `ts.isInsideNodeModules`: a `node_modules` path component.
fn is_inside_node_modules(path: &str) -> bool {
    path.split(['/', '\\']).any(|c| c == NODE_MODULES)
}

fn get_absolute_import(module_rbx_path: &RbxPath) -> Vec<Expr> {
    let service_name = module_rbx_path.first().expect("an absolute path starts with a service");
    let mut path_expressions = vec![create_get_service(service_name)];
    for name in &module_rbx_path[1..] {
        path_expressions.push(create::string(name.clone()));
    }
    path_expressions
}

fn get_relative_import(source_rbx_path: &RbxPath, module_rbx_path: &RbxPath) -> Vec<Expr> {
    let relative_path = RojoResolver::relative(source_rbx_path, module_rbx_path);

    // create descending path pieces
    let mut path = Vec::new();
    let mut i = 0;
    while matches!(relative_path.get(i), Some(RelativeRbxPathPart::Parent)) {
        path.push(PARENT_FIELD.to_owned());
        i += 1;
    }

    let mut path_expressions = vec![property_access_expression_chain(globals::script(), &path)];

    // create descending path pieces
    for part in &relative_path[i..] {
        match part {
            RelativeRbxPathPart::Child(name) => path_expressions.push(create::string(name.clone())),
            RelativeRbxPathPart::Parent => panic!("a relative path descends after it ascends"),
        }
    }

    path_expressions
}

fn validate_module(state: &TransformState<'_>, scope: &str) -> bool {
    let scoped_modules = nodepath::join(&[&state.services.data.node_modules_path, scope]);
    let scoped_modules = nodepath::normalize(&scoped_modules);
    state.services.type_roots.iter().any(|type_root| nodepath::normalize(type_root) == scoped_modules)
}

fn relative_to_project(state: &TransformState<'_>, path: &str) -> String {
    nodepath::relative(&state.services.data.project_path, path)
}

fn get_node_modules_import_parts(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    module_specifier: P<Node>,
    module_out_path: &str,
    module_filename: &str,
) -> Vec<Expr> {
    let node_modules_path = state.services.data.node_modules_path.clone();
    let relative_path = nodepath::relative(&node_modules_path, module_out_path);
    let module_scope = relative_path.split(['/', '\\']).next().unwrap_or_default().to_owned();

    if !rbxts_project::setup::is_path_descendant_of(module_out_path, &node_modules_path) {
        let virtual_path = state.guess_virtual_path(module_filename);
        state.add_diagnostic(errors::failed_symlink_resolve(
            module_specifier,
            &node_modules_path,
            module_filename,
            virtual_path.as_deref(),
            &relative_path,
        ));
        return vec![Expr::None];
    } else if !module_scope.starts_with('@') {
        state.add_diagnostic(errors::no_unscoped_module(module_specifier));
        return vec![Expr::None];
    }

    if !validate_module(state, &module_scope) {
        state.add_diagnostic(errors::no_invalid_scope(module_specifier));
        return vec![Expr::None];
    }

    if state.services.project_type == ProjectType::Package {
        let Some(relative_rbx_path) = state.services.package_relative_rbx_path(module_out_path) else {
            let relative = relative_to_project(state, module_out_path);
            state.add_diagnostic(errors::no_rojo_data(module_specifier, &relative, true));
            return vec![Expr::None];
        };

        let module_name = relative_rbx_path.first().expect("a package path names the package").clone();
        let get_module = state.ts(module_specifier.parent().unwrap_or(module_specifier), "getModule");
        let call = create::call(
            get_module,
            vec![globals::script(), create::string(module_scope), create::string(module_name)],
        );
        vec![property_access_expression_chain(call, &relative_rbx_path[1..])]
    } else {
        let mut module_rbx_path = state.services.rojo_resolver.get_rbx_path_from_file_path(module_out_path);
        if module_rbx_path.is_none() {
            // Rojo follows symlinks, so a config may mount a package by its real location (e.g. the pnpm store);
            // the directory is resolved, since the output extension (.luau) may differ from the shipped file
            let dir = nodepath::dirname(module_out_path);
            if let Ok(real_dir) = std::fs::canonicalize(dir) {
                let real_dir = real_dir.to_string_lossy();
                let real_dir = real_dir.strip_prefix(r"\\?\").unwrap_or(&real_dir);
                let candidate = nodepath::join(&[real_dir, nodepath::basename(module_out_path)]);
                module_rbx_path = state.services.rojo_resolver.get_rbx_path_from_file_path(&candidate);
            }
        }
        let Some(module_rbx_path) = module_rbx_path else {
            let relative = relative_to_project(state, module_out_path);
            state.add_diagnostic(errors::no_rojo_data(module_specifier, &relative, true));
            return vec![Expr::None];
        };

        let index_of_scope = module_rbx_path.iter().position(|p| *p == module_scope);
        let scoped =
            index_of_scope.is_some_and(|i| i > 0 && module_rbx_path.get(i - 1).is_some_and(|p| p == NODE_MODULES));
        if !scoped {
            let relative = relative_to_project(state, module_out_path);
            state.add_diagnostic(errors::no_package_import_without_scope(
                module_specifier,
                &relative,
                &module_rbx_path,
            ));
            return vec![Expr::None];
        }

        get_project_import_parts(state, source_file, module_specifier, module_out_path, &module_rbx_path)
    }
}

fn get_project_import_parts(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    module_specifier: P<Node>,
    module_out_path: &str,
    module_rbx_path: &RbxPath,
) -> Vec<Expr> {
    let module_rbx_type = state.services.rojo_resolver.get_rbx_type_from_file_path(module_out_path);
    if matches!(module_rbx_type, RbxType::Script | RbxType::LocalScript) {
        state.add_diagnostic(errors::no_non_module_import(module_specifier));
        return vec![Expr::None];
    }

    let source_out_path = state.services.path_translator.get_output_path(source_file.get().file_name());
    let Some(source_rbx_path) = state.services.rojo_resolver.get_rbx_path_from_file_path(&source_out_path) else {
        let relative = relative_to_project(state, &source_out_path);
        state.add_diagnostic(errors::no_rojo_data(source_file.as_node(), &relative, false));
        return vec![Expr::None];
    };

    if state.services.project_type == ProjectType::Game {
        let resolver = &state.services.rojo_resolver;
        // `import("")` skips the network check: the call may be guarded by RunService checks at run time
        let is_import_call = module_specifier.parent().is_some_and(tsrs_ast::is_import_call);
        if !is_import_call
            && resolver.get_network_type(module_rbx_path) == NetworkType::Server
            && resolver.get_network_type(&source_rbx_path) != NetworkType::Server
        {
            state.add_diagnostic(errors::no_server_import(module_specifier));
            return vec![Expr::None];
        }

        match resolver.get_file_relation(&source_rbx_path, module_rbx_path) {
            FileRelation::OutToOut | FileRelation::InToOut => get_absolute_import(module_rbx_path),
            FileRelation::InToIn => get_relative_import(&source_rbx_path, module_rbx_path),
            FileRelation::OutToIn => {
                state.add_diagnostic(errors::no_isolated_import(module_specifier));
                vec![Expr::None]
            }
        }
    } else {
        get_relative_import(&source_rbx_path, module_rbx_path)
    }
}

/// `getImportParts`: the instance path of the imported module, as `TS.import`'s arguments after `script`.
pub fn get_import_parts(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    module_specifier: P<Node>,
) -> Vec<Expr> {
    let Some(module_file) = get_source_file_from_module_specifier(state, module_specifier) else {
        state.add_diagnostic(errors::no_module_specifier_file(module_specifier));
        return vec![Expr::None];
    };
    get_import_parts_of_file(state, source_file, module_file, module_specifier)
}

/// rbxts-rs: the part of `getImportParts` once the module file is known. `module_specifier` is where diagnostics go:
/// the specifier, or for a plugin's `import_named` the node the plugin was dispatched at.
pub fn get_import_parts_of_file(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    module_file: P<SourceFile>,
    module_specifier: P<Node>,
) -> Vec<Expr> {
    let module_file_name = module_file.get().file_name().to_owned();
    let virtual_path = state.guess_virtual_path(&module_file_name).unwrap_or_else(|| module_file_name.clone());
    let reference_path = if state.services.project_reference_paths.is_empty() {
        None
    } else {
        let key = rbxts_project::graph::reference_key(&virtual_path, state.services.use_case_sensitive_file_names);
        state.services.project_reference_paths.get(&key).cloned()
    };

    if reference_path.is_none() && is_inside_node_modules(&virtual_path) {
        let mapped_path = state.services.node_modules_main_for(&virtual_path).map(str::to_owned);
        let module_out_path =
            state.services.path_translator.get_import_path(mapped_path.as_deref().unwrap_or(&virtual_path), true);
        get_node_modules_import_parts(state, source_file, module_specifier, &module_out_path, &module_file_name)
    } else {
        let is_reference = reference_path.is_some();
        let mut module_out_path =
            reference_path.unwrap_or_else(|| state.services.path_translator.get_import_path(&virtual_path, false));

        if !is_reference
            && !rbxts_project::setup::is_path_descendant_of(&virtual_path, &state.services.path_translator.root_dir)
        {
            // hand-written declarations and JSON outside the source roots describe files mapped directly by Rojo
            if tsrs_core::tspath::is_declaration_file_name(&virtual_path) {
                let ext = if state.services.data.options.luau { ".luau" } else { ".lua" };
                module_out_path = format!("{}{ext}", &virtual_path[..virtual_path.len() - ".d.ts".len()]);
            } else {
                // TypeScript rejects non-declaration source files outside rootDir before this transform
                assert!(tsrs_ast::is_json_source_file(module_file), "a source outside the root is JSON");
                module_out_path = virtual_path.clone();
            }
        }

        let Some(module_rbx_path) = state.services.rojo_resolver.get_rbx_path_from_file_path(&module_out_path) else {
            let relative = relative_to_project(state, &module_out_path);
            state.add_diagnostic(errors::no_rojo_data(module_specifier, &relative, false));
            return vec![Expr::None];
        };

        get_project_import_parts(state, source_file, module_specifier, &module_out_path, &module_rbx_path)
    }
}

/// rbxts-rs: `createImportExpression` for a module file that no specifier node names (`import_named`), with
/// diagnostics at `at`.
pub fn create_import_expression_of_file(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    module_file: P<SourceFile>,
    at: P<Node>,
) -> Expr {
    let mut parts = get_import_parts_of_file(state, source_file, module_file, at);
    parts.insert(0, globals::script());
    let callee = state.ts(at, "import");
    create::call(callee, parts)
}

/// `createImportExpression`: `TS.import(script, <path>)`.
pub fn create_import_expression(
    state: &mut TransformState<'_>,
    source_file: P<SourceFile>,
    module_specifier: P<Node>,
) -> Expr {
    let mut parts = get_import_parts(state, source_file, module_specifier);
    parts.insert(0, globals::script());
    let callee = state.ts(module_specifier.parent().expect("a module specifier has a parent"), "import");
    create::call(callee, parts)
}
