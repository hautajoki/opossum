//! `classes/TransformState.ts`: the state of one file's transform from TypeScript to Luau.

use std::rc::Rc;

use rustc_hash::FxHashMap;

use rbxts_luau::{AnyIdentifier, Expr, Stmt, TemporaryIdentifier, create, globals};
use rbxts_project::{ProjectServices, ProjectType, RelativeRbxPathPart, RojoResolver};
use tsrs_ast::{Diagnostic, Kind, Node, SourceFile, Symbol};
use tsrs_checker::{Checker, EmitResolver, Type};
use tsrs_compiler::Program;
use tsrs_core::{CompilerOptions, P};

use crate::classes::diagnostic_service::DiagnosticService;
use crate::classes::macro_manager::MacroManager;
use crate::classes::multi_transform_state::MultiTransformState;
use crate::shared::constants::PARENT_FIELD;
use crate::shared::diagnostics::{errors, internal_error, warnings};
use crate::util::create_get_service::create_get_service;
use crate::util::expression_chain::property_access_expression_chain;
use crate::util::get_block_comment_text::get_block_comment_text;
use crate::util::symlink_cache::SymlinkedDirectories;
use crate::util::traversal::{get_module_ancestor, skip_upwards};

/// `TryUses`
#[derive(Clone, Copy, Debug, Default)]
pub struct TryUses {
    /// A `return` inside the try statement leaves it.
    pub uses_return: bool,
    /// A `break` inside it leaves it.
    pub uses_break: bool,
    /// A `continue` inside it leaves it.
    pub uses_continue: bool,
}

/// Which exit a try statement uses (`keyof TryUses`).
#[derive(Clone, Copy, Debug)]
pub enum TryUse {
    /// `usesReturn`
    Return,
    /// `usesBreak`
    Break,
    /// `usesContinue`
    Continue,
}

/// `TransformState`: everything the transform of one source file reads and records.
pub struct TransformState<'a> {
    /// The program.
    pub program: &'static Program,
    /// rbxts-rs: the resolver for `$getModuleTree` specifiers no import resolves (`ts.resolveModuleName` with
    /// `ts.sys`): the project's ([`rbxts_project::Project::module_resolver`]).
    pub module_resolver: &'a rbxts_project::ModuleResolver,
    /// The checker this file is transformed on (its owner).
    pub checker: &'a mut Checker,
    /// The project's setup (`data`, `pathTranslator`, the Rojo resolvers, …).
    pub services: &'a ProjectServices,
    /// The program's compiler options.
    pub compiler_options: P<CompilerOptions>,
    /// The macros, by symbol.
    pub macro_manager: &'a MacroManager,
    /// The caches shared by the files of this checker.
    pub multi_transform_state: &'a mut MultiTransformState,
    /// The file transformed.
    pub source_file: P<SourceFile>,
    /// The emit resolver, for import elision on the original program (DESIGN §9).
    pub resolver: P<EmitResolver>,
    /// The file's diagnostics.
    pub diagnostics: DiagnosticService,

    source_file_text: &'static str,
    /// `hasExportEquals`: the file has `export =`.
    pub has_export_equals: bool,
    /// `hasExportFrom`: the file has `export … from`.
    pub has_export_from: bool,
    /// `classIdentifierMap`: the identifier each class is known by inside its own body.
    pub class_identifier_map: FxHashMap<P<Node>, AnyIdentifier>,
    /// `tryUsesStack`: the exits of the enclosing try statements, innermost last.
    pub try_uses_stack: Vec<TryUses>,
    /// `hoistsByStatement`: the identifiers whose declarations hoist before each statement.
    pub hoists_by_statement: FxHashMap<P<Node>, Vec<P<Node>>>,
    /// `isHoisted`: the symbols already hoisted.
    pub is_hoisted: FxHashMap<P<Symbol>, bool>,
    /// rbxts-rs: per switch statement, the flag its `continue`s set (see `create_continue`).
    pub switch_continue_flags: FxHashMap<P<Node>, TemporaryIdentifier>,
    get_type_cache: FxHashMap<P<Node>, P<Type>>,
    /// `usesRuntimeLib`: the file needs the `TS` import.
    pub uses_runtime_lib: bool,
    /// rbxts-rs: how many times `TS` was used, so that an import elided for a plugin at the end of the file can take
    /// back its uses.
    pub runtime_lib_uses: u32,
    module_id_by_symbol: FxHashMap<P<Symbol>, AnyIdentifier>,
    /// `symbolToIdMap`: symbols whose identifier is replaced by a temporary (a `for` loop variable copied per iteration).
    pub symbol_to_id_map: FxHashMap<P<Symbol>, TemporaryIdentifier>,
    class_element_to_object_key_map: FxHashMap<P<Node>, Expr>,
    is_in_replicated_first: bool,
    /// rbxts-rs: the plugins of this checker (DESIGN §11); `None` in a build without plugins.
    pub plugins: Option<&'a crate::plugins::PluginHost>,
    /// rbxts-rs: what this file records for its plugins.
    pub plugin_file: crate::plugins::file_state::PluginFileState,
}

impl<'a> TransformState<'a> {
    /// The state for transforming `source_file` on `checker`.
    pub fn new(
        project: &'a rbxts_project::Project,
        checker: &'a mut Checker,
        services: &'a ProjectServices,
        macro_manager: &'a MacroManager,
        multi_transform_state: &'a mut MultiTransformState,
        source_file: P<SourceFile>,
    ) -> TransformState<'a> {
        let program = project.program;
        let resolver = checker.get_emit_resolver();
        let is_in_replicated_first = services.file_info(source_file.get().file_name()).in_replicated_first;
        TransformState {
            program,
            module_resolver: project.module_resolver(),
            checker,
            services,
            compiler_options: program.options(),
            macro_manager,
            multi_transform_state,
            source_file,
            resolver,
            diagnostics: DiagnosticService::default(),
            source_file_text: source_file.get().text(),
            has_export_equals: false,
            has_export_from: false,
            class_identifier_map: FxHashMap::default(),
            try_uses_stack: Vec::new(),
            hoists_by_statement: FxHashMap::default(),
            is_hoisted: FxHashMap::default(),
            switch_continue_flags: FxHashMap::default(),
            get_type_cache: FxHashMap::default(),
            uses_runtime_lib: false,
            runtime_lib_uses: 0,
            module_id_by_symbol: FxHashMap::default(),
            symbol_to_id_map: FxHashMap::default(),
            class_element_to_object_key_map: FxHashMap::default(),
            is_in_replicated_first,
            plugins: None,
            plugin_file: Default::default(),
        }
    }

    // --- diagnostics (DiagnosticService) ---

    /// `DiagnosticService.addDiagnostic`
    pub fn add_diagnostic(&mut self, diagnostic: P<Diagnostic>) {
        self.diagnostics.add_diagnostic(diagnostic);
    }


    /// rbxts-rs: a compiler bug at `node`; the file is not written.
    pub fn internal_error(&mut self, node: P<Node>, message: &str) {
        self.diagnostics.add_diagnostic(internal_error(node, message));
    }

    /// rbxts-rs: a plugin's misuse of the seam (or a hook that failed) at `node`, an internal error the build
    /// counts as the plugin's (`compile::first_error_cut`); the file is not written.
    pub fn plugin_internal_error(&mut self, node: P<Node>, message: &str) {
        let diagnostic = internal_error(node, message);
        self.diagnostics.add_diagnostic(diagnostic);
        self.plugin_file.plugin_failures.push(diagnostic);
    }

    // --- try statements ---

    /// `pushTryUsesStack`
    pub fn push_try_uses_stack(&mut self) {
        self.try_uses_stack.push(TryUses::default());
    }

    /// `markTryUses`: the current try statement exits with return, break or continue.
    pub fn mark_try_uses(&mut self, property: TryUse) {
        if let Some(top) = self.try_uses_stack.last_mut() {
            match property {
                TryUse::Return => top.uses_return = true,
                TryUse::Break => top.uses_break = true,
                TryUse::Continue => top.uses_continue = true,
            }
        }
    }

    /// `popTryUsesStack`
    pub fn pop_try_uses_stack(&mut self) -> Option<TryUses> {
        self.try_uses_stack.pop()
    }

    // --- comments, types ---

    /// `getLeadingComments`: the comments before `pos` (a node's full start), as Luau comments.
    pub fn get_leading_comments_at(&self, pos: u32) -> Vec<Stmt> {
        tsrs_scanner::get_leading_comment_ranges(self.source_file_text, pos)
            .map(|range| {
                let (start, end) = (
                    range.text_range.pos().expect_source("comment") as usize,
                    range.text_range.end().expect_source("comment") as usize,
                );
                let source = &self.source_file_text[start..end];
                create::comment(if range.kind == Kind::SingleLineCommentTrivia {
                    source[2..].to_owned()
                } else {
                    get_block_comment_text(source)
                })
            })
            .collect()
    }

    /// `getLeadingComments`
    pub fn get_leading_comments(&self, node: P<Node>) -> Vec<Stmt> {
        match node.pos().source_offset() {
            Some(pos) => self.get_leading_comments_at(pos),
            None => Vec::new(),
        }
    }

    /// `getType`: the type at the outermost wrapper around `node` (`skipUpwards`), cached per node.
    pub fn get_type(&mut self, node: P<Node>) -> P<Type> {
        if let Some(&t) = self.get_type_cache.get(&node) {
            return t;
        }
        let t = self.checker.get_type_at_location(skip_upwards(node));
        self.get_type_cache.insert(node, t);
        t
    }

    // --- runtime library ---

    /// `TS`: `TS.<name>`, recording that the file uses the runtime library.
    pub fn ts(&mut self, node: P<Node>, name: &str) -> Expr {
        self.uses_runtime_lib = true;
        self.runtime_lib_uses += 1;
        if self.services.project_type == ProjectType::Game && self.is_in_replicated_first {
            self.add_diagnostic(warnings::runtime_lib_used_in_replicated_first(node));
        }
        create::property(globals::ts(), name)
    }

    /// `createRuntimeLibImport`: `local TS = …` for the file.
    pub fn create_runtime_lib_import(&mut self, source_file: P<SourceFile>) -> Stmt {
        let left = create::any_id("TS");
        let Some(runtime_lib_rbx_path) = self.services.runtime_lib_rbx_path.clone() else {
            // packages receive the runtime library through `_G[script] = TS`
            return create::local(left, Some(create::computed_index(globals::_g(), globals::script())));
        };
        if self.services.project_type == ProjectType::Game {
            // the service the runtime library is in, then a WaitForChild per instance below it
            let service_name = runtime_lib_rbx_path.first().expect("the runtime library path names a service");
            let mut expression = create_get_service(service_name);
            for name in &runtime_lib_rbx_path[1..] {
                expression = create::method_call(expression, "WaitForChild", vec![create::string(name.clone())]);
            }
            create::local(left, Some(create::call(globals::require(), vec![expression])))
        } else {
            let source_out_path = self.services.path_translator.get_output_path(source_file.get().file_name());
            let Some(rbx_path) = self.services.rojo_resolver.get_rbx_path_from_file_path(&source_out_path) else {
                let relative = rbxts_project::nodepath::relative(&self.services.data.project_path, &source_out_path);
                self.add_diagnostic(errors::no_rojo_data(source_file.as_node(), &relative, false));
                return create::local(left, Some(Expr::None));
            };
            let parts: Vec<String> = RojoResolver::relative(&rbx_path, &runtime_lib_rbx_path)
                .into_iter()
                .map(|part| match part {
                    RelativeRbxPathPart::Parent => PARENT_FIELD.to_owned(),
                    RelativeRbxPathPart::Child(name) => name,
                })
                .collect();
            let chain = property_access_expression_chain(globals::script(), &parts);
            create::local(left, Some(create::call(globals::require(), vec![chain])))
        }
    }

    // --- module exports ---

    /// `getModuleExports`: the module's exports, without erased (type-only) aliases.
    pub fn get_module_exports(&mut self, module_symbol: P<Symbol>) -> Rc<[P<Symbol>]> {
        if let Some(exports) = self.multi_transform_state.get_module_exports_cache.get(&module_symbol) {
            return Rc::clone(exports);
        }
        let verbatim = self.compiler_options.verbatim_module_syntax.is_true();
        let mut all = self.checker.get_exports_of_module_exported(module_symbol);
        // rbxts-rs: TypeScript 7's `getExportsOfModule` (the API, as Go's map iteration has no order) returns the
        // exports sorted by `compareSymbols`, i.e. in declaration order; tsrs's in-process call returns the symbol
        // table's insertion order, which is TypeScript 5's. Sorted here as TypeScript 7 does.
        let checker = &mut *self.checker;
        all.sort_by(|a, b| checker.compare_symbols_exported(Some(*a), Some(*b)));
        let mut exports = Vec::with_capacity(all.len());
        for export_symbol in all {
            let type_only_declaration = self.checker.get_type_only_alias_declaration_exported(export_symbol);
            // erased aliases must not redirect mutable locals to properties on the exports table
            if type_only_declaration.is_none_or(|d| verbatim && !is_type_only_export_declaration(d)) {
                exports.push(export_symbol);
            }
        }
        let exports: Rc<[P<Symbol>]> = exports.into();
        self.multi_transform_state.get_module_exports_cache.insert(module_symbol, Rc::clone(&exports));
        exports
    }

    /// `getModuleExportsAliasMap`: original symbol → the key it is exported under.
    pub fn get_module_exports_alias_map(&mut self, module_symbol: P<Symbol>) -> Rc<FxHashMap<P<Symbol>, Expr>> {
        if let Some(map) = self.multi_transform_state.get_module_exports_alias_map_cache.get(&module_symbol) {
            return Rc::clone(map);
        }
        let mut alias_map = FxHashMap::default();
        for &export_symbol in self.get_module_exports(module_symbol).iter() {
            let original_symbol = self.checker.skip_alias(export_symbol);
            let declaration = export_symbol.declarations().first().copied();
            match declaration {
                Some(declaration) if declaration.kind() == Kind::ExportSpecifier => {
                    let name = declaration.as_export_specifier().name;
                    alias_map.insert(original_symbol, crate::nodes::transform_property_name::property_name_key(name));
                }
                _ => {
                    alias_map.insert(original_symbol, create::string(export_symbol.name()));
                }
            }
        }
        let map = Rc::new(alias_map);
        self.multi_transform_state.get_module_exports_alias_map_cache.insert(module_symbol, Rc::clone(&map));
        map
    }

    fn get_module_symbol_from_node(&mut self, node: P<Node>) -> P<Symbol> {
        let module_ancestor = get_module_ancestor(node);
        let location = if module_ancestor.kind() == Kind::SourceFile {
            module_ancestor
        } else {
            module_ancestor.name().expect("a module declaration has a name")
        };
        self.checker.get_symbol_at_location_exported(location).expect("a module has a symbol")
    }

    fn get_module_id_from_symbol(&self, module_symbol: P<Symbol>) -> AnyIdentifier {
        self.module_id_by_symbol.get(&module_symbol).cloned().expect("the module's id was set")
    }

    /// `setModuleIdBySymbol`
    pub fn set_module_id_by_symbol(&mut self, module_symbol: P<Symbol>, module_id: AnyIdentifier) {
        self.module_id_by_symbol.insert(module_symbol, module_id);
    }

    /// `getModuleIdFromNode`: the exports table of the module containing `node`.
    pub fn get_module_id_from_node(&mut self, node: P<Node>) -> AnyIdentifier {
        let module_symbol = self.get_module_symbol_from_node(node);
        self.get_module_id_from_symbol(module_symbol)
    }

    /// `getModuleIdPropertyAccess`: `exports.name` for an exported symbol of this file's modules.
    pub fn get_module_id_property_access(&mut self, id_symbol: P<Symbol>) -> Option<Expr> {
        let value_declaration = id_symbol.value_declaration()?;
        let module_symbol = self.get_module_symbol_from_node(value_declaration);
        let alias = self.get_module_exports_alias_map(module_symbol).get(&id_symbol).cloned()?;
        Some(create::computed_index(self.get_module_id_from_symbol(module_symbol).into(), alias))
    }

    /// `guessVirtualPath`: the path a file has through a symlinked (or junctioned) directory the program resolved
    /// through, from its real path. Module resolution gives packages under a linked `node_modules`, or linked packages
    /// inside it, their real paths; imports need the `node_modules` ones.
    ///
    /// rbxts-rs: the map is TypeScript 5's, rebuilt from the program's resolutions ([`SymlinkedDirectories`]), not
    /// tsrs's TypeScript 7 symlink cache.
    pub fn guess_virtual_path(&mut self, fs_path: &str) -> Option<String> {
        use rbxts_project::nodepath;
        let program = self.program;
        let reverse_symlink_map = match &self.multi_transform_state.symlinked_directories {
            Some((for_program, map)) if std::ptr::eq(*for_program, program) => Rc::clone(map),
            _ => {
                let map = Rc::new(SymlinkedDirectories::new(program));
                self.multi_transform_state.symlinked_directories = Some((program, Rc::clone(&map)));
                map
            }
        };
        let cwd = program.get_current_directory();
        let case_sensitive = program.use_case_sensitive_file_names();

        let original = fs_path;
        let mut fs_path = fs_path.to_owned();
        loop {
            // reverseSymlinkMap always has trailing slashes, as it is constructed from `SymlinkedDirectory.real`
            let parent = tsrs_core::tspath::ensure_trailing_directory_separator(nodepath::dirname(&fs_path));
            if fs_path == parent {
                break;
            }
            fs_path = parent;
            let key = tsrs_core::tspath::to_path(&fs_path, cwd, case_sensitive);
            if let Some(symlink) = reverse_symlink_map.first_symlink(&key) {
                return Some(nodepath::join(&[symlink, &nodepath::relative(&fs_path, original)]));
            }
        }
        None
    }

    /// `setClassElementObjectKey`
    pub fn set_class_element_object_key(&mut self, class_element: P<Node>, identifier: Expr) {
        let previous = self.class_element_to_object_key_map.insert(class_element, identifier);
        assert!(previous.is_none(), "a class element's object key is set once");
    }

    /// `getClassElementObjectKey`
    pub fn get_class_element_object_key(&self, class_element: P<Node>) -> Option<Expr> {
        self.class_element_to_object_key_map.get(&class_element).cloned()
    }
}

/// `ts.isTypeOnlyExportDeclaration`.
///
/// rbxts-rs: tsrs has it as `pub(crate)` (`tsrs_ast::utilities_2`); this is a copy until the fork makes it public.
fn is_type_only_export_declaration(node: P<Node>) -> bool {
    match node.kind() {
        Kind::ExportSpecifier => {
            node.is_type_only() || node.parent().and_then(|p| p.parent()).is_some_and(|d| d.is_type_only())
        }
        Kind::ExportDeclaration => {
            let d = node.as_export_declaration();
            d.is_type_only && d.module_specifier.is_some() && d.export_clause.is_none()
        }
        Kind::NamespaceExport => node.parent().is_some_and(|p| p.is_type_only()),
        _ => false,
    }
}
