//! `luau.globals` and `luau.strings`: the Luau globals and string constants roblox-ts emits, as fresh nodes.
//!
//! Rust keywords get a trailing underscore or a raw name: `self_()`, `super_()`, `r#type()`, `r#typeof()`,
//! `coroutine::r#yield()`, `string::r#match()`, `table::r#move()`.
//!
//! Some globals carry evaluation facts, as roblox-ts marks the shared `luau.globals` nodes once
//! (`TSTransformer/util/evaluation/effects.ts`): `self` is a constant reference, and the builtins listed there have
//! known call effects. A plain [`crate::create::id`] of the same name carries none, as in roblox-ts.

use std::sync::Arc;

use crate::ast::Expr;
use crate::create::{id, property, string};
use crate::facts::{Effects, Facts};

fn marked(mut expr: Expr, mark: fn(&mut Facts)) -> Expr {
    if let Some(facts) = expr.facts_mut() {
        mark(facts);
    }
    expr
}

fn no_mark(_: &mut Facts) {}
fn constant(f: &mut Facts) {
    f.set_constant(true);
}
fn pure(f: &mut Facts) {
    f.set_builtin(Arc::new(Effects::none()));
}
fn may_throw(f: &mut Facts) {
    f.set_builtin(Arc::new(Effects::may_throw()));
}
fn read_heap(f: &mut Facts) {
    f.set_builtin(Arc::new(Effects::read_heap()));
}
fn write_heap(f: &mut Facts) {
    f.set_builtin(Arc::new(Effects::write_heap()));
}
fn allocates(f: &mut Facts) {
    f.set_builtin(Arc::new(Effects { throws: true, allocates: true, ..Effects::none() }));
}

macro_rules! ids {
    ($($(#[$meta:meta])* $fn_name:ident = $name:literal $(=> $mark:ident)?),* $(,)?) => {
        $( $(#[$meta])* #[doc = concat!("`", $name, "`")] pub fn $fn_name() -> Expr {
            marked(id($name), ids!(@mark $($mark)?))
        } )*
    };
    (@mark) => { no_mark };
    (@mark $mark:ident) => { $mark };
}

macro_rules! members {
    ($module:ident: $($fn_name:ident = $name:literal $(=> $mark:ident)?),* $(,)?) => {
        #[doc = concat!("Members of `", stringify!($module), "`.")]
        pub mod $module {
            use super::*;
            $( #[doc = concat!("`", stringify!($module), ".", $name, "`")] pub fn $fn_name() -> Expr {
                marked(property(id(stringify!($module)), $name), ids!(@mark $($mark)?))
            } )*
        }
    };
}

ids!(
    _g = "_G",
    ts = "TS",
    assert = "assert" => may_throw,
    bit32 = "bit32",
    error = "error" => may_throw,
    exports = "exports",
    getmetatable = "getmetatable",
    ipairs = "ipairs",
    next = "next" => read_heap,
    pairs = "pairs",
    pcall = "pcall",
    require = "require",
    script = "script",
    select = "select" => may_throw,
    self_ = "self" => constant,
    setmetatable = "setmetatable",
    super_ = "super",
    tostring = "tostring",
    r#type = "type" => pure,
    r#typeof = "typeof" => pure,
    unpack = "unpack" => read_heap,
    // roblox
    game = "game",
);

members!(coroutine: r#yield = "yield");
// The lookup of every string function is stable; calling some can still invoke a callback (gsub).
members!(string:
    byte = "byte" => may_throw, find = "find" => may_throw, format = "format" => constant,
    gmatch = "gmatch" => constant, gsub = "gsub" => constant, lower = "lower" => may_throw,
    r#match = "match" => may_throw, rep = "rep" => may_throw, reverse = "reverse" => may_throw,
    split = "split" => allocates, sub = "sub" => may_throw, upper = "upper" => may_throw,
);
// table.find's lookup is stable, but searching can invoke user equality code.
members!(table:
    clear = "clear" => write_heap, clone = "clone", concat = "concat" => read_heap, create = "create" => allocates,
    find = "find" => constant, freeze = "freeze", insert = "insert" => write_heap, isfrozen = "isfrozen",
    maxn = "maxn", r#move = "move" => write_heap, pack = "pack" => allocates, remove = "remove" => write_heap,
    sort = "sort", unpack = "unpack" => read_heap,
);
members!(utf8: charpattern = "charpattern", codes = "codes");
members!(math: min = "min");

/// `luau.strings`: string literals roblox-ts uses often.
pub mod strings {
    use super::*;

    macro_rules! strings {
        ($($fn_name:ident = $value:literal),* $(,)?) => {
            $( #[doc = concat!("`\"", $value, "\"`")] pub fn $fn_name() -> Expr { string($value) } )*
        };
    }

    strings!(
        // metamethods
        __index = "__index",
        __tostring = "__tostring",
        __mode = "__mode",
        k = "k",
        // types
        number = "number",
        table = "table",
        // opcall
        success = "success",
        value = "value",
        error = "error",
        // ReadonlyArray.join()
        comma_space = ", ",
    );
}
