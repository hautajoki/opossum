//! `classes/MacroManager.ts`: the compiler-types symbols the transformer recognises, and the macros by symbol.
//!
//! One per checker: merged global symbols are per checker. The well-known symbols are looked up by name when it is
//! made, and each may be missing. Macros are found from the call site: a symbol's name (and, for a method, its
//! interface's) picks the table entry, which counts only when the global of that name is the symbol's own. A macro
//! whose declaration a project's types lack is never looked for, so it is never an error.

use std::cell::RefCell;

use rustc_hash::FxHashMap;

use luau::Expr;
use tsrs_ast::{Node, Symbol, SymbolFlags};
use tsrs_checker::Checker;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::macros::types::{CallMacro, ConstructorMacro, IdentifierMacro, Macro, PropertyCallMacro};
use crate::macros::{CALL_MACROS, CONSTRUCTOR_MACROS, IDENTIFIER_MACROS, PROPERTY_CALL_MACROS};
use crate::shared::diagnostics::errors;
use crate::util::types::get_first_defined_symbol;

/// `SYMBOL_NAMES`
pub mod symbol_names {
    /// The global `globalThis`.
    pub const GLOBAL_THIS: &str = "globalThis";

    /// The global `ArrayConstructor`.
    pub const ARRAY_CONSTRUCTOR: &str = "ArrayConstructor";
    /// The global `SetConstructor`.
    pub const SET_CONSTRUCTOR: &str = "SetConstructor";
    /// The global `MapConstructor`.
    pub const MAP_CONSTRUCTOR: &str = "MapConstructor";
    /// The global `WeakSetConstructor`.
    pub const WEAK_SET_CONSTRUCTOR: &str = "WeakSetConstructor";
    /// The global `WeakMapConstructor`.
    pub const WEAK_MAP_CONSTRUCTOR: &str = "WeakMapConstructor";
    /// The global `ReadonlyMapConstructor`.
    pub const READONLY_MAP_CONSTRUCTOR: &str = "ReadonlyMapConstructor";
    /// The global `ReadonlySetConstructor`.
    pub const READONLY_SET_CONSTRUCTOR: &str = "ReadonlySetConstructor";

    /// The global `Array`.
    pub const ARRAY: &str = "Array";
    /// The global `Generator`.
    pub const GENERATOR: &str = "Generator";
    /// The global `IterableFunction`.
    pub const ITERABLE_FUNCTION: &str = "IterableFunction";
    /// The global `LuaTuple`.
    pub const LUA_TUPLE: &str = "LuaTuple";
    /// The global `Map`.
    pub const MAP: &str = "Map";
    /// The global `Object`.
    pub const OBJECT: &str = "Object";
    /// The global `ReadonlyArray`.
    pub const READONLY_ARRAY: &str = "ReadonlyArray";
    /// The global `ReadonlyMap`.
    pub const READONLY_MAP: &str = "ReadonlyMap";
    /// The global `ReadonlySet`.
    pub const READONLY_SET: &str = "ReadonlySet";
    /// The global `ReadVoxelsArray`.
    pub const READ_VOXELS_ARRAY: &str = "ReadVoxelsArray";
    /// The global `Set`.
    pub const SET: &str = "Set";
    /// The global `SharedTable`.
    pub const SHARED_TABLE: &str = "SharedTable";
    /// The global `String`.
    pub const STRING: &str = "String";
    /// The global `TemplateStringsArray`.
    pub const TEMPLATE_STRINGS_ARRAY: &str = "TemplateStringsArray";
    /// The global `WeakMap`.
    pub const WEAK_MAP: &str = "WeakMap";
    /// The global `WeakSet`.
    pub const WEAK_SET: &str = "WeakSet";

    /// The global `Iterable`.
    pub const ITERABLE: &str = "Iterable";

    /// The global `$range`.
    pub const RANGE: &str = "$range";
    /// The global `$tuple`.
    pub const TUPLE: &str = "$tuple";

    /// Every name, in `Object.values(SYMBOL_NAMES)` order.
    pub const ALL: &[&str] = &[
        GLOBAL_THIS,
        ARRAY_CONSTRUCTOR,
        SET_CONSTRUCTOR,
        MAP_CONSTRUCTOR,
        WEAK_SET_CONSTRUCTOR,
        WEAK_MAP_CONSTRUCTOR,
        READONLY_MAP_CONSTRUCTOR,
        READONLY_SET_CONSTRUCTOR,
        ARRAY,
        GENERATOR,
        ITERABLE_FUNCTION,
        LUA_TUPLE,
        MAP,
        OBJECT,
        READONLY_ARRAY,
        READONLY_MAP,
        READONLY_SET,
        READ_VOXELS_ARRAY,
        SET,
        SHARED_TABLE,
        STRING,
        TEMPLATE_STRINGS_ARRAY,
        WEAK_MAP,
        WEAK_SET,
        ITERABLE,
        RANGE,
        TUPLE,
    ];
}

/// `NOMINAL_LUA_TUPLE_NAME`
pub const NOMINAL_LUA_TUPLE_NAME: &str = "_nominal_LuaTuple";

const MACRO_ONLY_CLASSES: &[&str] = &[
    symbol_names::READONLY_ARRAY,
    symbol_names::ARRAY,
    symbol_names::READONLY_MAP,
    symbol_names::WEAK_MAP,
    symbol_names::MAP,
    symbol_names::READONLY_SET,
    symbol_names::WEAK_SET,
    symbol_names::SET,
    symbol_names::STRING,
];

/// A global's name and meaning.
type GlobalKey = (&'static str, SymbolFlags);

/// `MacroManager`
pub struct MacroManager {
    symbols: FxHashMap<&'static str, P<Symbol>>,
    globals: RefCell<FxHashMap<GlobalKey, Option<P<Symbol>>>>,
    property_calls: RefCell<FxHashMap<P<Symbol>, Option<Macro<PropertyCallMacro>>>>,
}

fn find<F: Copy>(table: &[(&'static str, F)], name: &str) -> Option<(&'static str, F)> {
    table.iter().copied().find(|&(n, _)| n == name)
}

/// The macro of a macro-only class method opossum has no lowering for: reported at the call.
fn missing_property_call(
    state: &mut TransformState<'_>,
    _: &mut Prereqs,
    node: P<Node>,
    _: Expr,
    _: Vec<Expr>,
) -> Expr {
    let callee = node.expression().expect("a call has a callee");
    let t = state.get_type(callee);
    let name = match get_first_defined_symbol(state, t) {
        Some(method) => match method.parent() {
            Some(class) => format!("{}.{}", class.name(), method.name()),
            None => method.name().to_owned(),
        },
        None => crate::util::traversal::get_text(callee).to_owned(),
    };
    state.add_diagnostic(errors::missing_macro(node, &name));
    Expr::None
}

impl MacroManager {
    /// Looks up the well-known symbols (`Array`, `$range`, `LuaTuple`'s nominal brand); any may be missing.
    pub fn new(checker: &mut Checker) -> MacroManager {
        let mut symbols = FxHashMap::default();
        for &name in symbol_names::ALL {
            if let Some(symbol) = checker.resolve_name_exported(name, None, SymbolFlags::All, false) {
                symbols.insert(name, checker.skip_alias(symbol));
            }
        }
        let lua_tuple_alias = symbols
            .get(symbol_names::LUA_TUPLE)
            .and_then(|s| s.declarations().iter().copied().find(|d| d.kind() == tsrs_ast::Kind::TypeAliasDeclaration));
        if let Some(declaration) = lua_tuple_alias {
            let t = checker.get_type_at_location(declaration);
            if let Some(nominal) = checker.get_property_of_type_exported(t, NOMINAL_LUA_TUPLE_NAME) {
                symbols.insert(NOMINAL_LUA_TUPLE_NAME, nominal);
            }
        }
        MacroManager { symbols, globals: RefCell::default(), property_calls: RefCell::default() }
    }

    /// The well-known symbol `name` (one of [`symbol_names`] or [`NOMINAL_LUA_TUPLE_NAME`]), if the project has it.
    pub fn symbol(&self, name: &str) -> Option<P<Symbol>> {
        self.symbols.get(name).copied()
    }

    /// Whether `symbol` is the well-known symbol `name`.
    pub fn is(&self, symbol: P<Symbol>, name: &str) -> bool {
        self.symbol(name) == Some(symbol)
    }

    /// The global `name` with `meaning`, aliases resolved; looked up once.
    fn global(&self, checker: &mut Checker, name: &'static str, meaning: SymbolFlags) -> Option<P<Symbol>> {
        if let Some(&found) = self.globals.borrow().get(&(name, meaning)) {
            return found;
        }
        let found = checker.resolve_name_exported(name, None, meaning, false).map(|s| checker.skip_alias(s));
        self.globals.borrow_mut().insert((name, meaning), found);
        found
    }

    /// `isMacroOnlyClass`
    pub fn is_macro_only_class(&self, symbol: P<Symbol>) -> bool {
        MACRO_ONLY_CLASSES.iter().any(|name| self.is(symbol, name))
    }

    /// `getIdentifierMacro`
    pub fn get_identifier_macro(&self, checker: &mut Checker, symbol: P<Symbol>) -> Option<Macro<IdentifierMacro>> {
        let symbol = checker.skip_alias(symbol);
        let (name, transform) = find(IDENTIFIER_MACROS, symbol.name())?;
        (self.global(checker, name, SymbolFlags::Variable) == Some(symbol)).then_some(Macro { name, transform })
    }

    /// `getCallMacro`
    pub fn get_call_macro(&self, checker: &mut Checker, symbol: P<Symbol>) -> Option<Macro<CallMacro>> {
        let symbol = checker.skip_alias(symbol);
        let (name, transform) = find(CALL_MACROS, symbol.name())?;
        (self.global(checker, name, SymbolFlags::Function) == Some(symbol)).then_some(Macro { name, transform })
    }

    /// The global interface a member symbol belongs to, by name: `None` for a member of any other interface.
    /// Augmented interface members keep their original, unmerged parent symbol, so the parent is merged first.
    fn global_interface_of(
        &self,
        checker: &mut Checker,
        member: P<Symbol>,
        names: &[&'static str],
    ) -> Option<&'static str> {
        let parent = checker.get_merged_symbol_exported(member.parent()?);
        let name = names.iter().copied().find(|&n| n == parent.name())?;
        (self.global(checker, name, SymbolFlags::Interface) == Some(parent)).then_some(name)
    }

    /// `getConstructorMacro`: `symbol` is a construct signature's.
    pub fn get_constructor_macro(&self, checker: &mut Checker, symbol: P<Symbol>) -> Option<Macro<ConstructorMacro>> {
        let names: Vec<&'static str> = CONSTRUCTOR_MACROS.iter().map(|&(n, _)| n).collect();
        let name = self.global_interface_of(checker, symbol, &names)?;
        let (name, transform) = find(CONSTRUCTOR_MACROS, name)?;
        Some(Macro { name, transform })
    }

    /// `getPropertyCallMacro`: the macro of a method symbol. A method of a macro-only class (`Array`, `Map`, …) with
    /// no lowering gets a macro that reports `macro.missing` at the call.
    pub fn get_property_call_macro(
        &self,
        checker: &mut Checker,
        symbol: P<Symbol>,
    ) -> Option<Macro<PropertyCallMacro>> {
        if let Some(&found) = self.property_calls.borrow().get(&symbol) {
            return found;
        }
        let found = self.find_property_call_macro(checker, symbol);
        self.property_calls.borrow_mut().insert(symbol, found);
        found
    }

    fn find_property_call_macro(&self, checker: &mut Checker, symbol: P<Symbol>) -> Option<Macro<PropertyCallMacro>> {
        let classes: Vec<&'static str> =
            PROPERTY_CALL_MACROS.iter().map(|&(n, _)| n).chain(MACRO_ONLY_CLASSES.iter().copied()).collect();
        let class_name = self.global_interface_of(checker, symbol, &classes)?;
        let method = find(PROPERTY_CALL_MACROS, class_name).and_then(|(_, methods)| find(methods, symbol.name()));
        match method {
            Some((method_name, transform)) => {
                Some(Macro { name: property_call_macro_name(class_name, method_name), transform })
            }
            None if MACRO_ONLY_CLASSES.contains(&class_name) => {
                Some(Macro { name: "missing", transform: missing_property_call })
            }
            None => None,
        }
    }
}

/// `Class.method`, made once per process.
fn property_call_macro_name(class_name: &'static str, method_name: &'static str) -> &'static str {
    static NAMES: std::sync::Mutex<Option<FxHashMap<(&'static str, &'static str), &'static str>>> =
        std::sync::Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    names
        .get_or_insert_with(FxHashMap::default)
        .entry((class_name, method_name))
        .or_insert_with(|| Box::leak(format!("{class_name}.{method_name}").into_boxed_str()))
}
