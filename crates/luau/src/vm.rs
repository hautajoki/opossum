//! The Luau VM local tests, benches and exec run in, over `src/vm.cpp`. Everything that touches a `lua_State` lives
//! there behind `noexcept` entry points, so no Luau error, C++ exception or longjmp crosses a Rust frame.
//!
//! A [`Vm`] is one `lua_State` with the standard libraries, `print` captured into [`Vm::take_output`], sandboxed
//! globals and a [`MEMORY_LIMIT`]. Host chunks ([`Vm::eval`]) run on one sandboxed thread. Bytecode is cached per
//! process by source, so a module that many VMs load compiles once.
//!
//! `require` resolves by string from the file system as the `luau` CLI does (`./`, `../`, `@self`, `init.luau`
//! directories, `.luaurc` aliases), each module on its own sandboxed thread with the chunk name `@<path from the
//! root>`, so traces read `src/x.luau:12`. A host chunk `c` stands at `<root>/c`: its `./x` is `<root>/x`. The global
//! `__load(file)` requires a path relative to the root (`__load("src/a.local.spec.luau")`) through the same cache.
//!
//! With codegen on, `--!native` modules and `@native` functions run as machine code, as on Roblox, and nothing else
//! does, so local bench numbers mean what they will in the engine. One code generation context serves every VM in
//! the process, so a native module many VMs load is compiled to machine code once. A VM runs on one thread at a time; it may move
//! between threads, and [`Vm::interrupter`] stops it from another.

use std::ffi::{CString, c_char, c_int};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The most a VM may allocate. Past it an allocation fails and Luau raises `not enough memory`.
pub const MEMORY_LIMIT: usize = 4 << 30;

#[repr(C)]
struct RawVm {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn opossum_vm_new(root: *const c_char, codegen: c_int, limit: usize) -> *mut RawVm;
    fn opossum_vm_eval(
        vm: *mut RawVm,
        chunkname: *const c_char,
        src: *const c_char,
        n: usize,
        out: *mut *mut c_char,
        outsize: *mut usize,
    ) -> c_int;
    fn opossum_vm_interrupt(vm: *mut RawVm);
    fn opossum_vm_native_functions(vm: *mut RawVm) -> usize;
    fn opossum_vm_take_output(vm: *mut RawVm, outsize: *mut usize) -> *mut c_char;
    fn opossum_vm_free(vm: *mut RawVm);
    fn opossum_vm_free_buffer(buffer: *mut c_char);
}

/// One Luau VM. Dropping it closes the state.
pub struct Vm {
    raw: *mut RawVm,
    /// The pointer [`Interrupter`]s reach it by; nulled before the state closes.
    shared: Arc<Mutex<usize>>,
}

// SAFETY: a lua_State belongs to no thread; `&mut self` on every call into it keeps it to one thread at a time.
unsafe impl Send for Vm {}

/// Stops a running [`Vm`] from another thread: every safepoint from then on raises `timed out`, even inside `pcall`,
/// so the running chunk unwinds and [`Vm::eval`] returns an error. An interrupted VM stays interrupted.
#[derive(Clone)]
pub struct Interrupter(Arc<Mutex<usize>>);

impl Interrupter {
    /// Interrupts the VM, or does nothing once it was dropped.
    pub fn interrupt(&self) {
        let raw = *self.0.lock().unwrap_or_else(|e| e.into_inner());
        if raw != 0 {
            // SAFETY: the VM nulls this under the lock before closing, so `raw` is live while we hold it; installing
            // the interrupt callback is the one call lua.h allows from any thread.
            unsafe { opossum_vm_interrupt(raw as *mut RawVm) };
        }
    }
}

impl Vm {
    /// A fresh VM whose `require` resolves from `root`, generating native code when `codegen` is on (and the CPU
    /// supports it).
    pub fn new(root: &Path, codegen: bool) -> Result<Vm, String> {
        Vm::with_limit(root, codegen, MEMORY_LIMIT)
    }

    fn with_limit(root: &Path, codegen: bool, limit: usize) -> Result<Vm, String> {
        crate::gate::init();
        let root = std::path::absolute(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let root = CString::new(root.to_string_lossy().as_bytes()).map_err(|_| "the root holds a NUL".to_owned())?;
        // SAFETY: `root` is a valid C string the VM copies.
        let raw = unsafe { opossum_vm_new(root.as_ptr(), c_int::from(codegen), limit) };
        if raw.is_null() {
            return Err("could not create a Luau VM: out of memory".to_owned());
        }
        Ok(Vm { raw, shared: Arc::new(Mutex::new(raw as usize)) })
    }

    /// Runs `src` as the chunk `chunk` (traces read `chunk:line`). The chunk returns one string, or nothing; an error
    /// comes back with its stack.
    pub fn eval(&mut self, chunk: &str, src: &str) -> Result<String, String> {
        let chunk = CString::new(chunk).map_err(|_| "the chunk name holds a NUL".to_owned())?;
        let mut out = std::ptr::null_mut();
        let mut size = 0usize;
        // SAFETY: `self.raw` is live and used by this thread only; `src` is valid for its length; the VM returns
        // null or a malloc'ed buffer of `size` bytes, which `take` copies and frees once.
        let ok =
            unsafe { opossum_vm_eval(self.raw, chunk.as_ptr(), src.as_ptr().cast(), src.len(), &mut out, &mut size) };
        let text = take(out, size).ok_or_else(|| "the VM ran out of memory for its result".to_owned())?;
        if ok == 1 { Ok(text) } else { Err(text) }
    }

    /// A handle that stops this VM from another thread.
    pub fn interrupter(&self) -> Interrupter {
        Interrupter(self.shared.clone())
    }

    /// How many functions of the chunks and modules loaded so far run as native code.
    pub fn native_functions(&self) -> usize {
        // SAFETY: a plain read of a live VM that no other thread runs (`&self` cannot overlap a `&mut self` call).
        unsafe { opossum_vm_native_functions(self.raw) }
    }

    /// Everything `print`ed since the last call, one line per call, arguments tab-separated.
    pub fn take_output(&mut self) -> String {
        let mut size = 0usize;
        // SAFETY: as in `eval`.
        let out = unsafe { opossum_vm_take_output(self.raw, &mut size) };
        take(out, size).unwrap_or_default()
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        // SAFETY: no interrupter can reach the state any more, and nothing else holds it.
        unsafe { opossum_vm_free(self.raw) };
    }
}

/// Copies and frees a buffer the VM returned; `None` for null.
fn take(buffer: *mut c_char, size: usize) -> Option<String> {
    if buffer.is_null() {
        return None;
    }
    // SAFETY: a non-null buffer from the VM is `size` bytes from `malloc`, freed once here by the VM's `free`.
    unsafe {
        let text = String::from_utf8_lossy(std::slice::from_raw_parts(buffer.cast::<u8>(), size)).into_owned();
        opossum_vm_free_buffer(buffer);
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> Vm {
        Vm::new(Path::new("/"), false).unwrap()
    }

    #[test]
    fn returns_strings_and_captures_print() {
        let mut vm = vm();
        assert_eq!(vm.eval("host", "print('a', 1, nil) print() return 'done'"), Ok("done".to_owned()));
        assert_eq!(vm.take_output(), "a\t1\tnil\n\n");
        assert_eq!(vm.take_output(), "");
        assert_eq!(vm.eval("host", "x = 1"), Ok(String::new()));
        // globals a chunk sets stay on the host thread; the libraries are frozen
        assert_eq!(vm.eval("host", "return tostring(x)"), Ok("1".to_owned()));
        assert!(vm.eval("host", "string.x = 1").unwrap_err().contains("readonly"));
        assert!(vm.eval("host", "return 1").unwrap_err().contains("returned a number"));
    }

    #[test]
    fn errors_carry_their_line_and_stack() {
        let error = vm().eval("src/a.luau", "function f()\n  error('boom')\nend\nf()").unwrap_err();
        assert!(error.starts_with("src/a.luau:2: boom\nstack traceback:\n"), "{error}");
        assert!(error.contains("src/a.luau:4"), "{error}");
        assert!(vm().eval("host", "local = 1").unwrap_err().starts_with("host:1: "));
    }

    #[test]
    fn an_interrupt_stops_a_loop_inside_pcall() {
        // native code checks for the interrupt at the same safepoints
        for codegen in [false, true] {
            let mut vm = Vm::new(Path::new("/"), codegen).unwrap();
            let interrupter = vm.interrupter();
            let watchdog = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                interrupter.interrupt();
            });
            let error =
                vm.eval("host", "--!native\nwhile true do pcall(function() while true do end end) end").unwrap_err();
            watchdog.join().unwrap();
            assert!(error.contains("timed out"), "{error}");
            assert_eq!(vm.native_functions() > 0, codegen);
        }
    }

    /// A project in a fresh temporary directory, from `(path, contents)` pairs.
    fn project(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("opossum-vm-{name}-{}", std::process::id()));
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    #[test]
    fn requires_resolve_like_the_luau_cli() {
        let root = project(
            "require",
            &[
                // an alias declared but never used may point nowhere
                (".luaurc", r#"{ "aliases": { "lib": "./src/lib", "std": "~/.lute/typedefs/std" } }"#),
                ("src/a/init.luau", "return { b = require('@self/b'), lib = require('@lib/c') }"),
                ("src/a/b.luau", "return 'b'"),
                ("src/lib/c.lua", "return 'c'"),
                ("src/d.local.spec.luau", "local a = require('./a') return a.b .. a.lib"),
            ],
        );
        let mut vm = Vm::new(&root, true).unwrap();
        let result = vm.eval("host", "return __load('src/d.local.spec.luau') .. require('./src/a').b");
        let missing = vm.eval("host", "require('./src/nope')");
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result, Ok("bcb".to_owned()));
        assert!(missing.unwrap_err().contains("could not resolve child component \"nope\""));
    }

    #[test]
    fn an_error_deep_in_requires_names_its_file_and_line() {
        let root = project(
            "deep",
            &[
                ("src/a.luau", "return require('./b')"),
                ("src/b.luau", "return require('./c')"),
                ("src/c.luau", "local x = 1\nerror('boom')\nreturn x"),
            ],
        );
        let error = Vm::new(&root, true).unwrap().eval("host", "require('./src/a')").unwrap_err();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(error.contains("src/c.luau:2: boom"), "{error}");
    }

    #[test]
    fn only_native_modules_and_functions_get_native_code() {
        let root = project(
            "native",
            &[
                ("native.luau", "--!native\nlocal function f(x) return x + 1 end\nreturn f"),
                ("plain.luau", "local function f(x) return x + 1 end\nreturn f"),
                ("attribute.luau", "local g = 0\n@native local function f(x) g += x return g end\nreturn f"),
            ],
        );
        let count = |module: &str, codegen: bool| {
            let mut vm = Vm::new(&root, codegen).unwrap();
            vm.eval("host", &format!("require('./{module}')")).unwrap();
            vm.native_functions()
        };
        let counts = [count("native", true), count("plain", true), count("attribute", true), count("native", false)];
        std::fs::remove_dir_all(&root).unwrap();
        assert!(counts[0] > 0 && counts[2] > 0, "{counts:?}");
        assert_eq!((counts[1], counts[3]), (0, 0));
    }

    #[test]
    fn the_memory_cap_fails_allocations() {
        let mut vm = Vm::with_limit(Path::new("/"), false, 16 << 20).unwrap();
        let error = vm.eval("host", "local t = {} for i = 1, 1e8 do t[i] = i end").unwrap_err();
        assert!(error.starts_with("not enough memory"), "{error}");
        // the VM survives it
        assert_eq!(vm.eval("host", "return 'ok'"), Ok("ok".to_owned()));
    }
}
