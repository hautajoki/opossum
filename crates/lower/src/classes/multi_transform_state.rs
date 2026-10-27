//! `classes/MultiTransformState.ts`: state that lives for one compilation, shared by the files of one checker.

use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_luau::{BindingKey, Effects, Expr};
use tsrs_ast::{Node, SourceFile, Symbol};
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::diagnostic_service::ReportOnce;
use crate::util::evaluation::bindings::BindingUsage;
use crate::util::symlink_cache::SymlinkedDirectories;

/// `MultiTransformState`. Type classifications stay valid for the compilation; emitted AST facts can still change.
#[derive(Default)]
pub struct MultiTransformState {
    /// `isPrimitiveTypeCache`
    pub is_primitive_type_cache: FxHashMap<P<Type>, bool>,
    /// `isRobloxTypeCache`
    pub is_roblox_type_cache: FxHashMap<P<Type>, bool>,
    /// rbxts-rs: per file (cleared at each file's start, like `binding_keys`): a symbol another file can name
    /// without an import alias (a script file's global) would otherwise get what the files transformed before it, on
    /// its checker, left here, so a file's Luau would depend on the checker count and on what a rebuild transforms.
    pub function_effects: FxHashMap<P<Symbol>, Arc<Effects>>,
    /// rbxts-rs: per file, like `function_effects`: each binding's key for the evaluation-order analysis.
    pub binding_keys: FxHashMap<P<Symbol>, BindingKey>,
    /// rbxts-rs: each file's binding usage (where each binding is read and written), for the evaluation-order analysis.
    pub binding_usage_by_source_file: FxHashMap<P<SourceFile>, Rc<BindingUsage>>,
    /// `isMethodCache`
    pub is_method_cache: FxHashMap<P<Type>, bool>,
    /// `isReportedByNoUnstableThisTypeCache`: the nodes already reported.
    pub is_reported_by_no_unstable_this_type: FxHashSet<P<Node>>,
    /// `isDefinedAsLetCache`
    pub is_defined_as_let_cache: FxHashMap<P<Symbol>, bool>,
    /// `isAnyOrAnyArrayCache` (`validateNotAnyType`).
    pub is_any_or_any_array_cache: FxHashMap<P<Type>, bool>,
    /// `isReportedByNoAnyCache` and `isReportedByMultipleDefinitionsCache`, by a key every checker computes alike
    /// ([`ReportOnce`]).
    pub reported_once: FxHashSet<ReportOnce>,
    /// `getModuleExportsCache`
    pub get_module_exports_cache: FxHashMap<P<Symbol>, Rc<[P<Symbol>]>>,
    /// `getModuleExportsAliasMapCache`
    pub get_module_exports_alias_map_cache: FxHashMap<P<Symbol>, Rc<FxHashMap<P<Symbol>, Expr>>>,
    /// rbxts-rs: whether a declaration file is under `node_modules/@rbxts/types` (`isRobloxType`, `isBuiltinDeclaration`).
    pub is_rbxts_types_file_cache: FxHashMap<P<SourceFile>, bool>,
    /// rbxts-rs: TypeScript 5's symlinked directories by real path for `guessVirtualPath`, and the program they are
    /// for, made on first use.
    pub symlinked_directories: Option<(&'static tsrs_compiler::Program, Rc<SymlinkedDirectories>)>,
}

impl MultiTransformState {
    /// Empty caches.
    pub fn new() -> MultiTransformState {
        MultiTransformState::default()
    }
}
