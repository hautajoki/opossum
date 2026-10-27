//! Luau's own compiler, to ask whether Luau accepts a rendered file (DESIGN §9: the compiler owns Luau's limits for
//! all code). Some limits can't be predicted from the syntax tree without re-implementing Luau's compiler: whether a
//! `local` takes a register depends on Luau's constant folding, dead-branch elimination and inlining, which change
//! with the optimisation and debug levels and between Luau releases. So the plugin seam compiles every file a plugin
//! emitted into (`rbxts_transform::plugins::spill`) with this compiler at the levels Roblox uses.
//!
//! The Luau release is pinned in `Cargo.toml` (luau0-src, built from source with the C++ toolchain): [`LUAU_VERSION`].
//! The only `unsafe` is the call into Luau ([`compile_raw`]), through `src/shim.cpp`, which turns any C++ exception
//! into an error so that none unwinds into Rust.
#![warn(missing_docs)]

use std::ffi::{c_char, c_int, c_void};

/// The Luau release compiled in, as `0.740`.
pub const LUAU_VERSION: &str = env!("LUAU_VERSION");

/// Luau refused the source: the line it reports (1-based; 0 when the message has none) and its message (`Out of
/// local registers when trying to allocate x: exceeded limit 200`, a syntax error, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    /// The line Luau names, 1-based; 0 when its message names none.
    pub line: u32,
    /// Luau's message, without the line prefix.
    pub message: String,
}

/// Compiles `source` as Roblox loads a script, at `optimization_level` (0–2) and `debug_level` (0–2). A
/// `--!optimize N` directive in the source overrides the optimisation level, as it does in Roblox.
pub fn compile(source: &str, optimization_level: u8, debug_level: u8) -> Result<(), CompileError> {
    let Some(bytecode) = compile_raw(source, c_int::from(optimization_level.min(2)), c_int::from(debug_level.min(2)))
    else {
        return Err(CompileError { line: 0, message: "Luau's compiler ran out of memory".to_owned() });
    };
    match bytecode.split_first() {
        // Luau's error "bytecode": a zero byte, then `:<line>: <message>`
        Some((0, error)) => Err(parse_error(&String::from_utf8_lossy(error))),
        _ => Ok(()),
    }
}

fn parse_error(text: &str) -> CompileError {
    let parsed = text
        .strip_prefix(':')
        .and_then(|rest| rest.split_once(": "))
        .and_then(|(line, message)| Some(CompileError { line: line.parse().ok()?, message: message.to_owned() }));
    parsed.unwrap_or_else(|| CompileError { line: 0, message: text.to_owned() })
}

/// `lua_CompileOptions` of the pinned release's `Compiler/include/luacode.h`, field for field.
#[repr(C)]
struct CompileOptions {
    optimization_level: c_int,
    debug_level: c_int,
    type_info_level: c_int,
    coverage_level: c_int,
    vector_lib: *const c_char,
    vector_ctor: *const c_char,
    vector_type: *const c_char,
    vector_precision: c_int,
    mutable_globals: *const *const c_char,
    userdata_types: *const *const c_char,
    libraries_with_known_members: *const *const c_char,
    library_member_type_cb: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> c_int>,
    library_member_constant_cb: Option<unsafe extern "C" fn(*const c_char, *const c_char, *mut *mut c_void)>,
    disabled_builtins: *const *const c_char,
}

unsafe extern "C" {
    /// `src/shim.cpp`: `luau_compile` of `luacode.h` inside a `try` (`noexcept`): returns a `malloc`ed buffer of
    /// `*outsize` bytes (bytecode, or an error, also for a C++ exception), or null when even that allocation failed.
    fn rbxts_luau_compile(
        source: *const c_char,
        size: usize,
        options: *mut CompileOptions,
        outsize: *mut usize,
    ) -> *mut c_char;
    /// `src/shim.cpp`: `free` of the C runtime Luau's `malloc` came from.
    fn rbxts_luau_free(buffer: *mut c_void);
}

/// Calls `luau_compile` (through the shim) and copies its output; `None` when there was no memory for it.
fn compile_raw(source: &str, optimization_level: c_int, debug_level: c_int) -> Option<Vec<u8>> {
    let mut options = CompileOptions {
        optimization_level,
        debug_level,
        type_info_level: 0,
        coverage_level: 0,
        vector_lib: std::ptr::null(),
        vector_ctor: std::ptr::null(),
        vector_type: std::ptr::null(),
        vector_precision: 0,
        mutable_globals: std::ptr::null(),
        userdata_types: std::ptr::null(),
        libraries_with_known_members: std::ptr::null(),
        library_member_type_cb: None,
        library_member_constant_cb: None,
        disabled_builtins: std::ptr::null(),
    };
    let mut size = 0usize;
    // SAFETY: `source` is valid for `source.len()` bytes (Luau copies it and needs no terminator); `options` matches
    // `lua_CompileOptions` of the pinned release, with every pointer null (Luau's defaults) and no callbacks; `size`
    // is a valid out-pointer. The shim is `noexcept` and catches every C++ exception (Luau reports its own parse and
    // compile errors as output already), so nothing unwinds across the boundary. A non-null buffer is `size` bytes
    // from `malloc`: it is read once, copied, and freed exactly once by the shim's `free` (the same C runtime).
    // Luau's compiler keeps no global mutable state, so threads may call this at the same time.
    unsafe {
        let out = rbxts_luau_compile(source.as_ptr().cast(), source.len(), &mut options, &mut size);
        if out.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(out.cast::<u8>(), size).to_vec();
        rbxts_luau_free(out.cast());
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `local x0 = 1` … `local x<count - 1> = 1`
    fn constants(count: usize) -> String {
        (0..count).map(|i| format!("local x{i} = 1\n")).collect()
    }

    #[test]
    fn compiles_at_the_levels_asked() {
        assert_eq!(compile("local a = 1\nprint(a)\n", 1, 1), Ok(()));
        // 201 constants: folded at optimisation level 1+ and debug level 1 only
        let source = constants(201);
        let refused = compile(&source, 0, 1).unwrap_err();
        assert_eq!(refused.line, 201);
        assert!(refused.message.starts_with("Out of local registers when trying to allocate x200"), "{refused:?}");
        assert_eq!(compile(&source, 1, 1), Ok(()));
        assert_eq!(compile(&source, 2, 1), Ok(()));
        assert!(compile(&source, 1, 2).is_err());
        // the source's directive wins, as in Roblox
        assert!(compile(&format!("--!optimize 0\n{source}"), 1, 1).is_err());
    }

    #[test]
    fn reports_syntax_errors_with_their_line() {
        let error = compile("local a = 1\nlocal = 2\n", 1, 1).unwrap_err();
        assert_eq!(error.line, 2);
        assert!(!error.message.is_empty());
    }

    #[test]
    fn names_its_version() {
        assert!(LUAU_VERSION.starts_with("0."), "{LUAU_VERSION}");
    }
}
