//! `classes/MacroManager.ts`: the compiler-types symbols the transformer recognises, and the macros by symbol.
//!
//! Built once per checker: merged global symbols are per checker (DESIGN §5), so the tables are too.

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_project::ProjectError;
use tsrs_ast::{Kind, Symbol, SymbolFlags};
use tsrs_checker::Checker;
use tsrs_core::P;

use crate::macros::types::{CallMacro, ConstructorMacro, IdentifierMacro, Macro, PropertyCallMacro};
use crate::macros::{CALL_MACROS, CONSTRUCTOR_MACROS, IDENTIFIER_MACROS, PROPERTY_CALL_MACROS};
use crate::util::traversal::skip_upwards;

const TYPES_NOTICE: &str = "\nYou may need to update your @rbxts/compiler-types!";

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

fn project_error(message: String) -> ProjectError {
    ProjectError::new(message + TYPES_NOTICE)
}

fn get_global_symbol_by_name_or_throw(
    checker: &mut Checker,
    name: &str,
    meaning: SymbolFlags,
) -> Result<P<Symbol>, ProjectError> {
    match checker.resolve_name_exported(name, None, meaning, false) {
        Some(symbol) => Ok(checker.skip_alias(symbol)),
        None => Err(project_error(format!("MacroManager could not find symbol for {name}"))),
    }
}

fn get_constructor_symbol(checker: &mut Checker, name: &str) -> Result<P<Symbol>, ProjectError> {
    let symbol = get_global_symbol_by_name_or_throw(checker, name, SymbolFlags::Interface)?;
    let interface = symbol
        .declarations()
        .iter()
        .copied()
        .find(|d| d.kind() == Kind::InterfaceDeclaration)
        .ok_or_else(|| project_error(format!("MacroManager could not find interface for {name}")))?;
    interface
        .members()
        .iter()
        .find(|m| m.kind() == Kind::ConstructSignature)
        .map(|m| m.symbol().expect("a construct signature has a symbol"))
        .ok_or_else(|| project_error(format!("MacroManager could not find constructor for {name}")))
}

/// `MacroManager`
pub struct MacroManager {
    symbols: FxHashMap<&'static str, P<Symbol>>,
    macro_only_classes: FxHashSet<P<Symbol>>,
    identifier_macros: FxHashMap<P<Symbol>, Macro<IdentifierMacro>>,
    call_macros: FxHashMap<P<Symbol>, Macro<CallMacro>>,
    constructor_macros: FxHashMap<P<Symbol>, Macro<ConstructorMacro>>,
    property_call_macros: FxHashMap<P<Symbol>, Macro<PropertyCallMacro>>,
}

/// `Class.method`, made once per process (every checker of every build makes a macro manager).
fn property_call_macro_name(class_name: &'static str, method_name: &'static str) -> &'static str {
    static NAMES: std::sync::Mutex<Option<FxHashMap<(&'static str, &'static str), &'static str>>> =
        std::sync::Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    names
        .get_or_insert_with(FxHashMap::default)
        .entry((class_name, method_name))
        .or_insert_with(|| Box::leak(format!("{class_name}.{method_name}").into_boxed_str()))
}

impl MacroManager {
    /// Resolves every macro and well-known symbol. A missing one is roblox-ts's `ProjectError` (outdated
    /// compiler-types).
    pub fn new(checker: &mut Checker) -> Result<MacroManager, ProjectError> {
        let mut manager = MacroManager {
            symbols: FxHashMap::default(),
            macro_only_classes: FxHashSet::default(),
            identifier_macros: FxHashMap::default(),
            call_macros: FxHashMap::default(),
            constructor_macros: FxHashMap::default(),
            property_call_macros: FxHashMap::default(),
        };

        for &(name, transform) in IDENTIFIER_MACROS {
            let symbol = get_global_symbol_by_name_or_throw(checker, name, SymbolFlags::Variable)?;
            manager.identifier_macros.insert(symbol, Macro { name, transform });
        }

        for &(name, transform) in CALL_MACROS {
            let symbol = get_global_symbol_by_name_or_throw(checker, name, SymbolFlags::Function)?;
            manager.call_macros.insert(symbol, Macro { name, transform });
        }

        for &(class_name, transform) in CONSTRUCTOR_MACROS {
            let construct_symbol = get_constructor_symbol(checker, class_name)?;
            manager.constructor_macros.insert(construct_symbol, Macro { name: class_name, transform });
        }

        for &(class_name, methods) in PROPERTY_CALL_MACROS {
            let symbol = get_global_symbol_by_name_or_throw(checker, class_name, SymbolFlags::Interface)?;
            let mut method_map: FxHashMap<&'static str, P<Symbol>> = FxHashMap::default();
            for &declaration in symbol.declarations() {
                if declaration.kind() != Kind::InterfaceDeclaration {
                    continue;
                }
                for &member in declaration.members() {
                    if member.kind() == Kind::MethodSignature
                        && let Some(name) = member.name()
                        && name.kind() == Kind::Identifier
                    {
                        let t = checker.get_type_at_location(skip_upwards(member));
                        let method_symbol = t.symbol().expect("a method signature's type has a symbol");
                        method_map.insert(name.text(), method_symbol);
                    }
                }
            }
            for &(method_name, transform) in methods {
                let method_symbol = method_map.get(method_name).copied().ok_or_else(|| {
                    project_error(format!("MacroManager could not find method for {class_name}.{method_name}"))
                })?;
                // rbxts-rs: the name is kept for plugins (`Array.push`); roblox-ts only stores the function
                let name = property_call_macro_name(class_name, method_name);
                manager.property_call_macros.insert(method_symbol, Macro { name, transform });
            }
        }

        for &symbol_name in symbol_names::ALL {
            let symbol = get_global_symbol_by_name_or_throw(checker, symbol_name, SymbolFlags::All)?;
            manager.symbols.insert(symbol_name, symbol);
            if MACRO_ONLY_CLASSES.contains(&symbol_name) {
                manager.macro_only_classes.insert(symbol);
            }
        }

        let lua_tuple = manager.get_symbol_or_throw(symbol_names::LUA_TUPLE);
        let lua_tuple_type_dec =
            lua_tuple.declarations().iter().copied().find(|d| d.kind() == Kind::TypeAliasDeclaration);
        if let Some(declaration) = lua_tuple_type_dec {
            let t = checker.get_type_at_location(declaration);
            if let Some(nominal) = checker.get_property_of_type_exported(t, NOMINAL_LUA_TUPLE_NAME) {
                manager.symbols.insert(NOMINAL_LUA_TUPLE_NAME, nominal);
            }
        }

        Ok(manager)
    }

    /// `getSymbolOrThrow`
    pub fn get_symbol_or_throw(&self, name: &str) -> P<Symbol> {
        *self.symbols.get(name).unwrap_or_else(|| panic!("MacroManager has no symbol {name}"))
    }

    /// The symbol, if it was found (`NOMINAL_LUA_TUPLE_NAME` may be missing).
    pub fn get_symbol(&self, name: &str) -> Option<P<Symbol>> {
        self.symbols.get(name).copied()
    }

    /// `isMacroOnlyClass`
    pub fn is_macro_only_class(&self, symbol: P<Symbol>) -> bool {
        self.macro_only_classes.contains(&symbol)
    }

    /// `getIdentifierMacro`
    pub fn get_identifier_macro(&self, checker: &mut Checker, symbol: P<Symbol>) -> Option<Macro<IdentifierMacro>> {
        self.identifier_macros.get(&checker.skip_alias(symbol)).copied()
    }

    /// `getCallMacro`
    pub fn get_call_macro(&self, checker: &mut Checker, symbol: P<Symbol>) -> Option<Macro<CallMacro>> {
        self.call_macros.get(&checker.skip_alias(symbol)).copied()
    }

    /// `getConstructorMacro`
    pub fn get_constructor_macro(&self, symbol: P<Symbol>) -> Option<Macro<ConstructorMacro>> {
        self.constructor_macros.get(&symbol).copied()
    }

    /// `getPropertyCallMacro`
    pub fn get_property_call_macro(
        &self,
        checker: &mut Checker,
        symbol: P<Symbol>,
    ) -> Option<Macro<PropertyCallMacro>> {
        let found = self.property_call_macros.get(&symbol).copied();
        if found.is_none()
            && let Some(parent) = symbol.parent()
        {
            // augmented interface members retain their original, unmerged parent symbol
            let parent = checker.get_merged_symbol_exported(parent);
            assert!(
                !self.is_macro_only_class(parent),
                "Macro {}.{}() is not implemented!",
                parent.name(),
                symbol.name()
            );
        }
        found
    }
}
