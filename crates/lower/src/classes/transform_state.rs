//! `classes/TransformState.ts`: the state of one file's transform from TypeScript to Luau.

use std::rc::Rc;

use rustc_hash::FxHashMap;

use luau::{AnyIdentifier, Expr, Stmt, TemporaryIdentifier, create, globals};
use tsrs_ast::{Diagnostic, Kind, Node, SourceFile, Symbol};
use tsrs_checker::{Checker, EmitResolver, Type};
use tsrs_compiler::Program;
use tsrs_core::{CompilerOptions, P};

use crate::classes::diagnostic_service::DiagnosticService;
use crate::classes::macro_manager::MacroManager;
use crate::classes::multi_transform_state::MultiTransformState;
use crate::locate::{Location, Locator};
use crate::macros::compile_time::Input;
use crate::options::Options;
use crate::program::Project;
use crate::shared::diagnostics::{errors, internal_error};
use crate::util::get_block_comment_text::get_block_comment_text;
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
    /// The project directory.
    pub root: &'a str,
    /// The checker this file is transformed on (its owner).
    pub checker: &'a mut Checker,
    /// Where every module lives once routed.
    pub locator: &'a Locator,
    /// What the caller turned on.
    pub options: &'a Options,
    /// Where this file lives; `None` when the locator does not route it (`route.unrouted` at the file, and none of
    /// its requires is made).
    pub location: Option<&'a Location>,
    /// The program's compiler options.
    pub compiler_options: P<CompilerOptions>,
    /// The macros, by symbol.
    pub macro_manager: &'a MacroManager,
    /// The caches shared by the files of this checker.
    pub multi_transform_state: &'a mut MultiTransformState,
    /// The file transformed.
    pub source_file: P<SourceFile>,
    /// The emit resolver, for import elision.
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
    module_id_by_symbol: FxHashMap<P<Symbol>, AnyIdentifier>,
    /// `symbolToIdMap`: symbols whose identifier is replaced by a temporary (a `for` loop variable copied per iteration).
    pub symbol_to_id_map: FxHashMap<P<Symbol>, TemporaryIdentifier>,
    class_element_to_object_key_map: FxHashMap<P<Node>, Expr>,
    /// Project bindings named after a Luau keyword or reserved global, by declaration
    /// (`transform_identifier::renamed_binding`).
    pub renames: FxHashMap<P<Node>, TemporaryIdentifier>,
    /// What this file's compile-time macros read ([`crate::macros::compile_time`]).
    pub macro_inputs: Vec<Input>,
}

impl<'a> TransformState<'a> {
    /// The state for transforming `source_file` on `checker`.
    pub fn new(
        project: &'a Project,
        locator: &'a Locator,
        options: &'a Options,
        checker: &'a mut Checker,
        macro_manager: &'a MacroManager,
        multi_transform_state: &'a mut MultiTransformState,
        source_file: P<SourceFile>,
    ) -> TransformState<'a> {
        let program = project.program;
        let resolver = checker.get_emit_resolver();
        TransformState {
            program,
            root: &project.root,
            checker,
            locator,
            options,
            location: locator.get(source_file.get().file_name()),
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
            module_id_by_symbol: FxHashMap::default(),
            symbol_to_id_map: FxHashMap::default(),
            class_element_to_object_key_map: FxHashMap::default(),
            renames: FxHashMap::default(),
            macro_inputs: Vec::new(),
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
    pub fn ts(&mut self, name: &str) -> Expr {
        self.uses_runtime_lib = true;
        create::property(globals::ts(), name)
    }

    /// `local TS = require(<runtime library>)` for the file, or `None` after reporting a runtime library this file
    /// cannot require.
    pub fn create_runtime_lib_import(&mut self) -> Option<Stmt> {
        let node = self.source_file.as_node();
        let Some(runtime) = self.locator.runtime_lib.as_ref() else {
            self.add_diagnostic(errors::unrouted(node, "RuntimeLib"));
            return None;
        };
        let from = self.location?;
        if luau::spell(&from.module, &runtime.module).is_none() {
            self.add_diagnostic(errors::route_boundary(node, from.realm.name(), runtime.realm.name()));
            return None;
        }
        Some(create::local(create::any_id("TS"), Some(create::require(runtime.module.clone()))))
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
            let type_only_declaration = self.checker.get_type_only_alias_declaration(export_symbol);
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
/// A copy of tsrs's private one (`tsrs_ast::utilities_2`).
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
