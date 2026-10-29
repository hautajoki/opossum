luau0-src 0.22.0+luau740 (Luau 0.740) from crates.io, without its README, CI files and test crate. It replaces the
crates.io package through `[patch.crates-io]` in the workspace Cargo.toml, so the `=0.22.0` pin in crates/luau stays
the one place that names the version.

opossum.patch is the whole delta from upstream, all in src/lib.rs. Reapply it after a bump:

1. `Artifacts::include_dirs()`: the include directories of Luau's headers (Common, VM/include, VM/src, Compiler, Ast,
   Bytecode, CodeGen, Require, Config). Upstream exports none, and crates/luau compiles C++ against Luau's C++-only
   API (`Luau::CodeGen::compile` with flags).
2. `Artifacts::defines()`: exactly the defines the libraries are compiled with (`LUA_API`, `LUA_VECTOR_SIZE`,
   `LUAI_MAXCSTACK`, the `extern "C"` API macros, `LUA_USE_LONGJMP`/`LUAU_ENABLE_ASSERT` when on). C++ compiled with
   any other set gets mangled names or a different `lua_State` layout.
3. A build stamp: `build()` writes `OUT_DIR/luau-build/.stamp` with the version and every option, and while it matches
   skips the `remove_dir_all` and every library compile. Without it each rerun of crates/luau's build script (any edit
   to its own C++) recompiles all of Luau, about a minute.

The stamp keys on the version, not the sources: after editing the vendored C++ by hand, `cargo clean -p luau`.
