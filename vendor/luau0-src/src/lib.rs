use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Represents the configuration for building Luau artifacts.
pub struct Build {
    out_dir: Option<PathBuf>,
    target: Option<String>,
    host: Option<String>,
    // Max number of Lua stack slots that a C function can use
    max_cstack_size: usize,
    // Use longjmp instead of C++ exceptions
    use_longjmp: bool,
    // Enable code generator (jit)
    enable_codegen: bool,
    // Vector size, must be 3 (default) or 4
    vector_size: usize,
}

/// Represents the artifacts produced by the build process.
pub struct Artifacts {
    lib_dir: PathBuf,
    libs: Vec<String>,
    cpp_stdlib: Option<String>,
    include_dirs: Vec<PathBuf>,
    defines: Vec<(&'static str, String)>,
}

impl Default for Build {
    fn default() -> Self {
        Build {
            out_dir: env::var_os("OUT_DIR").map(PathBuf::from),
            target: env::var("TARGET").ok(),
            host: env::var("HOST").ok(),
            max_cstack_size: 1000000,
            use_longjmp: false,
            enable_codegen: false,
            vector_size: 3,
        }
    }
}

impl Build {
    /// Creates a new `Build` instance with default settings.
    pub fn new() -> Build {
        Build::default()
    }

    /// Sets the output directory for the build artifacts.
    ///
    /// Default is the environment variable `OUT_DIR`.
    pub fn out_dir<P: AsRef<Path>>(&mut self, path: P) -> &mut Build {
        self.out_dir = Some(path.as_ref().to_path_buf());
        self
    }

    /// Sets the target architecture for the build.
    ///
    /// Default is the environment variable `TARGET`.
    pub fn target(&mut self, target: &str) -> &mut Build {
        self.target = Some(target.to_string());
        self
    }

    /// Sets the host architecture for the build.
    ///
    /// Default is the environment variable `HOST`.
    pub fn host(&mut self, host: &str) -> &mut Build {
        self.host = Some(host.to_string());
        self
    }

    /// Sets the maximum number of Lua stack slots that a C function can use.
    ///
    /// Default is 1,000,000.
    pub fn set_max_cstack_size(&mut self, size: usize) -> &mut Build {
        self.max_cstack_size = size;
        self
    }

    /// Sets whether to use longjmp instead of C++ exceptions.
    ///
    /// Default is false.
    pub fn use_longjmp(&mut self, r#use: bool) -> &mut Build {
        self.use_longjmp = r#use;
        self
    }

    /// Sets whether to enable the code generator (JIT).
    ///
    /// Default is false.
    pub fn enable_codegen(&mut self, enable: bool) -> &mut Build {
        self.enable_codegen = enable;
        self
    }

    /// Sets the vector size (3 or 4).
    ///
    /// Default is 3.
    pub fn set_vector_size(&mut self, size: usize) -> &mut Build {
        assert!(size == 3 || size == 4, "vector size must be 3 or 4");
        self.vector_size = size;
        self
    }

    /// Builds the Lua artifacts for the specified version.
    ///
    /// opossum: the libraries are compiled once per output directory. `luau-build/.stamp` records the version and
    /// every option they were compiled with; while it matches, the existing libraries are reused, so a rerun of the
    /// dependent's build script (an edit to its own C++) does not recompile Luau.
    pub fn build(&mut self) -> Artifacts {
        let target = &self.target.as_ref().expect("TARGET is not set")[..];
        let host = &self.host.as_ref().expect("HOST is not set")[..];
        let out_dir = self.out_dir.as_ref().expect("OUT_DIR is not set");
        let build_dir = out_dir.join("luau-build");

        let source_base_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let luau_source_dir = source_base_dir.join("luau");

        // Extra compiler flags and include directories for the vendored C++.
        //
        // `LUAU_CXXFLAGS` is split on whitespace and appended to the compiler
        // command; `LUAU_INCLUDE_DIRS` uses the platform path separator. Both
        // are declared with `rerun-if-env-changed`, so changing either forces a
        // rebuild instead of silently reusing an artifact built with the old
        // settings.
        println!("cargo:rerun-if-env-changed=LUAU_CXXFLAGS");
        println!("cargo:rerun-if-env-changed=LUAU_INCLUDE_DIRS");
        let cxxflags = env::var("LUAU_CXXFLAGS").unwrap_or_default();
        let extra_include_dirs = env::var("LUAU_INCLUDE_DIRS").unwrap_or_default();

        // Every define the libraries are compiled with, and so the ones code calling into them must use.
        let mut defines = vec![
            ("LUAI_MAXCSTACK", self.max_cstack_size.to_string()),
            ("LUA_VECTOR_SIZE", self.vector_size.to_string()),
            ("LUA_API", "extern \"C\"".to_string()),
        ];
        if self.use_longjmp {
            defines.push(("LUA_USE_LONGJMP", "1".to_string()));
        }
        if cfg!(debug_assertions) {
            defines.push(("LUAU_ENABLE_ASSERT", "1".to_string()));
        }
        let api_defines = [
            ("LUACODEGEN_API", "extern \"C\"".to_string()),
            ("LUACODE_API", "extern \"C\"".to_string()),
            ("LUAJITINLINER_API", "extern \"C\"".to_string()),
        ];

        let stamp = format!(
            "{};codegen={};vector={};cstack={};longjmp={};assert={};cxxflags={cxxflags};includes={extra_include_dirs}",
            env!("CARGO_PKG_VERSION"),
            self.enable_codegen,
            self.vector_size,
            self.max_cstack_size,
            self.use_longjmp,
            cfg!(debug_assertions),
        );
        let stamp_path = build_dir.join(".stamp");
        if fs::read_to_string(&stamp_path).ok().as_deref() != Some(stamp.as_str()) {
            if build_dir.exists() {
                fs::remove_dir_all(&build_dir).unwrap();
            }
            self.compile_libraries(
                target,
                &luau_source_dir,
                &build_dir,
                &defines,
                &cxxflags,
                &extra_include_dirs,
            );
            fs::write(&stamp_path, &stamp).unwrap();
        }

        let mut libs = vec![
            "luauinliner",
            "luauvm",
            "luaubytecode",
            "luaucompiler",
            "luauast",
            "luaucommon",
            "luauconfig",
            "luaucustom",
            "luaurequire",
        ];
        if self.enable_codegen {
            libs.push("luaucodegen");
        }
        let include_dirs = [
            "Common/include",
            "VM/include",
            "VM/src",
            "Compiler/include",
            "Ast/include",
            "Bytecode/include",
            "CodeGen/include",
            "Require/include",
            "Config/include",
        ];
        defines.extend(api_defines);

        Artifacts {
            lib_dir: build_dir,
            libs: libs.into_iter().map(str::to_string).collect(),
            cpp_stdlib: Self::get_cpp_link_stdlib(target, host),
            include_dirs: include_dirs
                .iter()
                .map(|dir| luau_source_dir.join(dir))
                .collect(),
            defines,
        }
    }

    fn compile_libraries(
        &self,
        target: &str,
        luau_source_dir: &Path,
        build_dir: &Path,
        defines: &[(&str, String)],
        cxxflags: &str,
        extra_include_dirs: &str,
    ) {
        let common_include_dir = luau_source_dir.join("Common").join("include");
        let vm_source_dir = luau_source_dir.join("VM").join("src");
        let vm_include_dir = luau_source_dir.join("VM").join("include");

        // Configure C++
        let mut config = cc::Build::new();
        config
            .warnings(false)
            .cargo_metadata(false)
            .std("c++17")
            .cpp(true);

        for flag in cxxflags.split_whitespace() {
            config.flag(flag);
        }
        if !extra_include_dirs.is_empty() {
            for dir in env::split_paths(extra_include_dirs) {
                config.include(dir);
            }
        }

        if target.ends_with("emscripten") {
            // cc-rs unconditionally adds `-fno-exceptions` for wasm32.
            // We need to re-enable exceptions and switch to the
            // wasm-native EH ABI that Rust's emscripten target uses.
            config.flag_if_supported("-fexceptions");
            config.flag_if_supported("-fwasm-exceptions");
        }

        // Common defines
        for (name, value) in defines {
            config.define(name, value.as_str());
        }

        if !cfg!(debug_assertions) {
            // this flag allows compiler to lower sqrt() into a single CPU instruction
            config.flag_if_supported("-fno-math-errno");
        }

        config.include(&common_include_dir);

        // Build `Ast` library
        let ast_source_dir = luau_source_dir.join("Ast").join("src");
        let ast_include_dir = luau_source_dir.join("Ast").join("include");
        config
            .clone()
            .include(&ast_include_dir)
            .add_files_by_ext_sorted(&ast_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luauast");

        // Build `CodeGen` library
        let codegen_source_dir = luau_source_dir.join("CodeGen").join("src");
        let codegen_include_dir = luau_source_dir.join("CodeGen").join("include");
        if self.enable_codegen {
            if target.ends_with("emscripten") {
                panic!("codegen (jit) is not supported on emscripten");
            }

            config
                .clone()
                .include(&codegen_include_dir)
                .include(&vm_include_dir)
                .include(&vm_source_dir)
                .define("LUACODEGEN_API", "extern \"C\"")
                .add_files_by_ext_sorted(&codegen_source_dir, "cpp")
                .out_dir(build_dir)
                .compile("luaucodegen");
        }

        // Build `Common` library
        let common_source_dir = luau_source_dir.join("Common").join("src");
        config
            .clone()
            .add_files_by_ext_sorted(&common_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luaucommon");

        // Build `Bytecode` library
        let bytecode_source_dir = luau_source_dir.join("Bytecode").join("src");
        let bytecode_include_dir = luau_source_dir.join("Bytecode").join("include");
        config
            .clone()
            .include(&bytecode_include_dir)
            .add_files_by_ext_sorted(&bytecode_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luaubytecode");

        // Build `Inliner` library
        let inliner_source_dir = luau_source_dir.join("Inliner").join("src");
        let inliner_include_dir = luau_source_dir.join("Inliner").join("include");
        config
            .clone()
            .include(&inliner_include_dir)
            .include(&bytecode_include_dir)
            .include(&bytecode_source_dir)
            .include(&vm_include_dir)
            .include(&vm_source_dir)
            .define("LUAJITINLINER_API", "extern \"C\"")
            .add_files_by_ext_sorted(&inliner_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luauinliner");

        // Build `Compiler` library
        let compiler_source_dir = luau_source_dir.join("Compiler").join("src");
        let compiler_include_dir = luau_source_dir.join("Compiler").join("include");
        config
            .clone()
            .include(&compiler_include_dir)
            .include(&bytecode_include_dir)
            .include(&ast_include_dir)
            .define("LUACODE_API", "extern \"C\"")
            .add_files_by_ext_sorted(&compiler_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luaucompiler");

        // Build `Config` library
        let config_source_dir = luau_source_dir.join("Config").join("src");
        let config_include_dir = luau_source_dir.join("Config").join("include");
        config
            .clone()
            .include(&config_include_dir)
            .include(&ast_include_dir)
            .include(&compiler_include_dir)
            .include(&vm_include_dir)
            .add_files_by_ext_sorted(&config_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luauconfig");

        // Build customization library
        let custom_source_dir = luau_source_dir.join("Custom").join("src");
        config
            .clone()
            .include(&vm_include_dir)
            .include(&vm_source_dir)
            .add_files_by_ext_sorted(&custom_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luaucustom");

        // Build `Require` library
        let require_source_dir = luau_source_dir.join("Require").join("src");
        let require_include_dir = luau_source_dir.join("Require").join("include");
        config
            .clone()
            .include(&require_include_dir)
            .include(&ast_include_dir)
            .include(&config_include_dir)
            .include(&vm_include_dir)
            .add_files_by_ext_sorted(&require_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luaurequire");

        // Build VM
        config
            .clone()
            .include(&vm_include_dir)
            .add_files_by_ext_sorted(&vm_source_dir, "cpp")
            .out_dir(build_dir)
            .compile("luauvm");
    }

    /// Returns the C++ standard library:
    /// 1) Uses `CXXSTDLIB` environment variable if set
    /// 2) The default `c++` for OS X and BSDs
    /// 3) `c++_shared` for Android
    /// 4) `None` for MSVC
    /// 5) `stdc++` for anything else.
    ///
    /// Inspired by the `cc` crate.
    fn get_cpp_link_stdlib(target: &str, host: &str) -> Option<String> {
        // Try to get value from the `CXXSTDLIB` env variable
        let kind = if host == target { "HOST" } else { "TARGET" };
        let res = env::var(format!("CXXSTDLIB_{target}"))
            .or_else(|_| env::var(format!("CXXSTDLIB_{}", target.replace('-', "_"))))
            .or_else(|_| env::var(format!("{kind}_CXXSTDLIB")))
            .or_else(|_| env::var("CXXSTDLIB"))
            .ok();
        if res.is_some() {
            return res;
        }

        if target.contains("msvc") {
            None
        } else if target.contains("apple") | target.contains("freebsd") | target.contains("openbsd")
        {
            Some("c++".to_string())
        } else if target.contains("android") {
            Some("c++_shared".to_string())
        } else {
            Some("stdc++".to_string())
        }
    }
}

impl Artifacts {
    pub fn lib_dir(&self) -> &Path {
        &self.lib_dir
    }

    pub fn libs(&self) -> &[String] {
        &self.libs
    }

    /// opossum: the include directories of Luau's public headers and of the VM's internals (`VM/src`), for C++
    /// compiled against these libraries.
    pub fn include_dirs(&self) -> &[PathBuf] {
        &self.include_dirs
    }

    /// opossum: exactly the defines the libraries were compiled with, including the `extern "C"` API macros. C++
    /// compiled against the libraries must use the same set, or it gets mangled names or a different `lua_State`.
    pub fn defines(&self) -> &[(&'static str, String)] {
        &self.defines
    }

    pub fn print_cargo_metadata(&self) {
        println!("cargo:rustc-link-search=native={}", self.lib_dir.display());
        for lib in &self.libs {
            println!("cargo:rustc-link-lib=static={lib}");
        }
        if let Some(ref cpp_stdlib) = self.cpp_stdlib {
            println!("cargo:rustc-link-lib={cpp_stdlib}");
        }
        if let Some(version) = self.version() {
            println!("cargo:rustc-env=LUAU_VERSION={version}");
        }
    }

    pub fn version(&self) -> Option<String> {
        let pkg_version = env!("CARGO_PKG_VERSION");
        let (_, luau_version) = pkg_version.split_once("+luau")?;
        Some(format!("0.{luau_version}"))
    }
}

trait AddFilesByExt {
    fn add_files_by_ext_sorted(&mut self, dir: &Path, ext: &str) -> &mut Self;
}

impl AddFilesByExt for cc::Build {
    // It's important to keep the order of the files to get consistent builds between machines
    // if the order is not always the same, the final binary produces a different SHA256 which
    // might cause issues if one needs to verify which binary is being executed
    fn add_files_by_ext_sorted(&mut self, dir: &Path, ext: &str) -> &mut Self {
        let mut sources: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension() == Some(ext.as_ref()))
            .map(|e| e.path())
            .collect();

        // Sort for determinism
        sources.sort();

        for source in sources {
            self.file(source);
        }

        self
    }
}
