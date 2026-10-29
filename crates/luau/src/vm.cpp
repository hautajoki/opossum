// The Luau VM local tests, benches and exec run in (src/vm.rs). Everything that touches a lua_State lives here, and
// every entry point is noexcept with a catch-all, so no Luau error, C++ exception or longjmp crosses a Rust frame.
//
// One opossum_vm is one lua_State: the standard libraries, print captured into the VM's output, the `luau` CLI's
// collectgarbage, globals sandboxed, and a memory cap enforced by its allocator. Host chunks run on one sandboxed thread. Bytecode is cached per process
// by source, so a module many VMs load is compiled once.
//
// Native code is generated as on Roblox: only for `--!native` modules and `@native` functions. Every VM uses one
// process-wide code generation context, keyed by bytecode, and one state that is never closed holds every native chunk
// (the context frees a module's machine code when no state's functions use it any more), so a module VMs load one
// after another is compiled to machine code once per process.
//
// require-by-string is Luau's Require library over a file system navigator ported from Luau 0.740's CLI
// (CLI/src/ReplRequirer.cpp and VfsNavigator.cpp, MIT, Copyright (c) 2019-2025 Roblox Corporation), with its
// std::filesystem calls replaced by POSIX stat, open and read: `./`, `../`, `@self`, `init.luau`/`init.lua`
// directories, `.luau`/`.lua` files and `.luaurc`/`.config.luau` aliases resolve exactly as in the `luau` CLI.

#include "Luau/CodeGen.h"
#include "Luau/Require.h"
#include "lua.h"
#include "luacode.h"
#include "lualib.h"

#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>

struct opossum_vm
{
    lua_State* L = nullptr;
    lua_State* host = nullptr; // the sandboxed thread host chunks run on, pinned by a registry ref
    std::string root;   // absolute, normalized, no trailing slash
    std::string output;
    // where require's navigation stands: `node` is the absolute module path without its suffix, `real` the file or
    // directory it resolved to (or `node` itself for a host chunk, which has no file)
    std::string node;
    std::string real;
    size_t used = 0;
    size_t limit = 0;
    bool codegen = false;
    size_t native = 0; // functions bound to native code, over every chunk loaded
};

static opossum_vm* vm_of(lua_State* L)
{
    return static_cast<opossum_vm*>(lua_callbacks(L)->userdata);
}

// Counts every live byte; an allocation that would pass the cap fails, which Luau raises as "not enough memory".
static void* vm_alloc(void* ud, void* ptr, size_t osize, size_t nsize)
{
    opossum_vm* vm = static_cast<opossum_vm*>(ud);
    if (nsize == 0)
    {
        std::free(ptr);
        vm->used -= osize;
        return nullptr;
    }
    if (nsize > osize && vm->used - osize + nsize > vm->limit)
        return nullptr;
    void* block = std::realloc(ptr, nsize);
    if (block)
        vm->used = vm->used - osize + nsize;
    return block;
}

// Bytecode for `source` at -O2, debug level 1 and type info for native code (the `luau` CLI's options); a
// `--!optimize` directive in the source wins, as on Roblox. A compile error comes back as Luau's error bytecode, which
// luau_load turns into `chunk:line: message`.
struct Bytecode
{
    std::string bytes;
    Luau::CodeGen::ModuleId id; // this entry's number in the cache: the native code of equal bytecode is shared
    mutable std::once_flag anchored;
};

static std::mutex cache_mutex;
static std::unordered_map<std::string, std::shared_ptr<const Bytecode>> cache;
static uint64_t cache_entries = 0;

static std::shared_ptr<const Bytecode> compile(const std::string& source)
{
    {
        std::lock_guard<std::mutex> lock(cache_mutex);
        auto it = cache.find(source);
        if (it != cache.end())
            return it->second;
    }
    lua_CompileOptions options = {};
    options.optimizationLevel = 2;
    options.debugLevel = 1;
    options.typeInfoLevel = 1;
    size_t size = 0;
    char* bytes = luau_compile(source.data(), source.size(), &options, &size);
    if (!bytes)
        throw std::bad_alloc();
    auto compiled = std::make_shared<Bytecode>();
    compiled->bytes.assign(bytes, size);
    std::free(bytes);
    std::lock_guard<std::mutex> lock(cache_mutex);
    auto it = cache.find(source);
    if (it != cache.end())
        return it->second; // a racing compile of the same source won
    uint64_t number = ++cache_entries;
    compiled->id = {};
    std::memcpy(compiled->id.data(), &number, sizeof(number));
    cache.emplace(source, compiled);
    return compiled;
}

// Created on first use and never destroyed: it must outlive every VM, and VMs live until the process ends.
static Luau::CodeGen::SharedCodeGenContext* shared_codegen()
{
    static Luau::CodeGen::SharedCodeGenContext* context = Luau::CodeGen::createSharedCodeGenContext().release();
    return context;
}

// Compiles a chunk's native code into the anchor state, which keeps it alive for every VM to bind.
static void anchor(const Bytecode* bytecode)
{
    static std::mutex mutex;
    static lua_State* L = nullptr;
    std::lock_guard<std::mutex> lock(mutex);
    if (!L)
    {
        L = luaL_newstate();
        Luau::CodeGen::create(L, shared_codegen());
    }
    if (luau_load(L, "=anchor", bytecode->bytes.data(), bytecode->bytes.size(), 0) == 0)
    {
        Luau::CodeGen::CompilationResult result = Luau::CodeGen::compile(bytecode->id, L, -1, Luau::CodeGen::CodeGen_OnlyNativeModules);
        if (result.result == Luau::CodeGen::CodeGenCompilationResult::Success)
            lua_ref(L, -1); // a plain chunk is left to the collector
    }
    lua_settop(L, 0);
}

// luau_load, then native code for what asks for it. Returns luau_load's status; on 0 the function is on the stack.
static int load_chunk(opossum_vm* vm, lua_State* L, const char* chunkname, const Bytecode& bytecode)
{
    int status = luau_load(L, chunkname, bytecode.bytes.data(), bytecode.bytes.size(), 0);
    if (status == 0 && vm->codegen)
    {
        std::call_once(bytecode.anchored, anchor, &bytecode);
        Luau::CodeGen::CompilationStats stats;
        Luau::CodeGen::compile(bytecode.id, L, -1, Luau::CodeGen::CodeGen_OnlyNativeModules, &stats);
        vm->native += stats.functionsBound;
    }
    return status;
}

static bool is_file(const std::string& path)
{
    struct stat st;
    return stat(path.c_str(), &st) == 0 && S_ISREG(st.st_mode);
}

static bool is_directory(const std::string& path)
{
    struct stat st;
    return stat(path.c_str(), &st) == 0 && S_ISDIR(st.st_mode);
}

static bool read_file(const std::string& path, std::string& out)
{
    int fd = open(path.c_str(), O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return false;
    char buffer[65536];
    ssize_t n;
    while ((n = read(fd, buffer, sizeof(buffer))) > 0)
        out.append(buffer, size_t(n));
    close(fd);
    return n == 0;
}

static bool ends_with(const std::string& text, const char* suffix)
{
    size_t n = std::strlen(suffix);
    return text.size() >= n && text.compare(text.size() - n, n, suffix) == 0;
}

// An absolute path with `.`, `..` and repeated slashes resolved lexically; `..` above the root stays at the root.
static std::string normalize(const std::string& path)
{
    std::string out;
    size_t i = 0;
    while (i < path.size())
    {
        size_t end = path.find('/', i);
        if (end == std::string::npos)
            end = path.size();
        std::string part = path.substr(i, end - i);
        if (part == "..")
        {
            size_t slash = out.rfind('/');
            out.erase(slash == std::string::npos ? 0 : slash);
        }
        else if (!part.empty() && part != ".")
            out += "/" + part;
        i = end + 1;
    }
    return out.empty() ? "/" : out;
}

static const char* const MODULE_SUFFIXES[] = {".luau", ".lua"};
static const char* const INIT_SUFFIXES[] = {"/init.luau", "/init.lua"};

// The module path a file stands for: `a/init.luau` and `a.luau` are both the module `a`.
static std::string module_path(std::string path)
{
    for (const char* suffix : INIT_SUFFIXES)
        if (ends_with(path, suffix))
            return path.substr(0, path.size() - std::strlen(suffix));
    for (const char* suffix : MODULE_SUFFIXES)
        if (ends_with(path, suffix))
            return path.substr(0, path.size() - std::strlen(suffix));
    return path;
}

// VfsNavigator's getRealPath: the file or directory a module path names. A file and a directory of the same name, or
// both suffixes, are ambiguous; a directory without an init file is a directory, not a module.
static luarequire_NavigateResult resolve(const std::string& node, std::string& real)
{
    bool found = false;
    std::string suffix;
    if (node.substr(node.rfind('/') + 1) != "init")
    {
        for (const char* candidate : MODULE_SUFFIXES)
        {
            if (!is_file(node + candidate))
                continue;
            if (found)
                return NAVIGATE_AMBIGUOUS;
            suffix = candidate;
            found = true;
        }
    }
    if (is_directory(node))
    {
        if (found)
            return NAVIGATE_AMBIGUOUS;
        for (const char* candidate : INIT_SUFFIXES)
        {
            if (!is_file(node + candidate))
                continue;
            if (found)
                return NAVIGATE_AMBIGUOUS;
            suffix = candidate;
            found = true;
        }
        found = true;
    }
    if (!found)
        return NAVIGATE_NOT_FOUND;
    real = node + suffix;
    return NAVIGATE_SUCCESS;
}

static luarequire_NavigateResult navigate_to(opossum_vm* vm, const std::string& node)
{
    vm->node = node;
    return resolve(node, vm->real);
}

static luarequire_WriteResult write_string(const std::string& text, char* buffer, size_t size, size_t* outsize)
{
    *outsize = text.size();
    if (text.size() > size)
        return WRITE_BUFFER_TOO_SMALL;
    std::memcpy(buffer, text.data(), text.size());
    return WRITE_SUCCESS;
}

static bool is_require_allowed(lua_State*, void*, const char* chunkname)
{
    return chunkname[0] == '@';
}

// A chunk name is `@` and a path relative to the root (or absolute outside it). A host chunk names no file: its
// module path still stands where the name puts it, so `./x` from the host chunk `@exec` is `<root>/x`.
static luarequire_NavigateResult reset(lua_State*, void* ctx, const char* chunkname)
{
    opossum_vm* vm = static_cast<opossum_vm*>(ctx);
    std::string path = chunkname + 1;
    std::string node = module_path(normalize(path[0] == '/' ? path : vm->root + "/" + path));
    luarequire_NavigateResult result = navigate_to(vm, node);
    if (result == NAVIGATE_NOT_FOUND)
        vm->real = node;
    return result == NAVIGATE_AMBIGUOUS ? NAVIGATE_AMBIGUOUS : NAVIGATE_SUCCESS;
}

// Only an absolute alias path can be jumped to; others (a project's `~/.lute/...`) fail when, and only when, used.
static luarequire_NavigateResult jump_to_alias(lua_State*, void* ctx, const char* path)
{
    if (path[0] != '/')
        return NAVIGATE_NOT_FOUND;
    return navigate_to(static_cast<opossum_vm*>(ctx), module_path(normalize(path)));
}

static luarequire_NavigateResult to_parent(lua_State*, void* ctx)
{
    opossum_vm* vm = static_cast<opossum_vm*>(ctx);
    if (vm->node == "/")
        return NAVIGATE_NOT_FOUND;
    luarequire_NavigateResult result = navigate_to(vm, normalize(vm->node + "/.."));
    return result == NAVIGATE_AMBIGUOUS ? NAVIGATE_SUCCESS : result; // going up is never ambiguous
}

static luarequire_NavigateResult to_child(lua_State*, void* ctx, const char* name)
{
    if (std::strcmp(name, ".config") == 0)
        return NAVIGATE_NOT_FOUND;
    opossum_vm* vm = static_cast<opossum_vm*>(ctx);
    return navigate_to(vm, normalize(vm->node + "/" + name));
}

static bool is_module_present(lua_State*, void* ctx)
{
    return is_file(static_cast<opossum_vm*>(ctx)->real);
}

// `@src/x.luau` for a file under the root, so traces read `src/x.luau:12`.
static luarequire_WriteResult get_chunkname(lua_State*, void* ctx, char* buffer, size_t size, size_t* outsize)
{
    opossum_vm* vm = static_cast<opossum_vm*>(ctx);
    const std::string& real = vm->real;
    bool inside = real.size() > vm->root.size() && real.compare(0, vm->root.size(), vm->root) == 0 && real[vm->root.size()] == '/';
    return write_string("@" + (inside ? real.substr(vm->root.size() + 1) : real), buffer, size, outsize);
}

static luarequire_WriteResult get_real_path(lua_State*, void* ctx, char* buffer, size_t size, size_t* outsize)
{
    return write_string(static_cast<opossum_vm*>(ctx)->real, buffer, size, outsize);
}

static luarequire_ConfigStatus get_config_status(lua_State*, void* ctx)
{
    const std::string& node = static_cast<opossum_vm*>(ctx)->node;
    bool json = is_file(node + "/.luaurc");
    bool luau = is_file(node + "/.config.luau");
    if (json && luau)
        return CONFIG_AMBIGUOUS;
    return json ? CONFIG_PRESENT_JSON : luau ? CONFIG_PRESENT_LUAU : CONFIG_ABSENT;
}

static luarequire_WriteResult get_config(lua_State*, void* ctx, char* buffer, size_t size, size_t* outsize)
{
    const std::string& node = static_cast<opossum_vm*>(ctx)->node;
    std::string text;
    if (!read_file(node + "/.luaurc", text) && !read_file(node + "/.config.luau", text))
        return WRITE_FAILURE;
    return write_string(text, buffer, size, outsize);
}

// Runs a module on its own sandboxed thread, created on the main thread so it never inherits a caller's globals.
static int load(lua_State* L, void* ctx, const char*, const char* chunkname, const char* loadname)
{
    std::string source;
    if (!read_file(loadname, source))
        luaL_error(L, "could not read file '%s'", loadname);
    std::shared_ptr<const Bytecode> bytecode = compile(source);
    lua_State* GL = lua_mainthread(L);
    lua_State* ML = lua_newthread(GL);
    lua_xmove(GL, L, 1);
    luaL_sandboxthread(ML);
    if (load_chunk(static_cast<opossum_vm*>(ctx), ML, chunkname, *bytecode) != 0)
    {
        lua_xmove(ML, L, 1);
        lua_error(L);
    }
    int status = lua_resume(ML, L, 0);
    if (status == LUA_YIELD)
        luaL_error(L, "module can not yield");
    if (status != LUA_OK)
        luaL_error(L, "error while running module: %s", lua_isstring(ML, -1) ? lua_tostring(ML, -1) : "(error object is not a string)");
    int results = lua_gettop(ML);
    if (results == 0)
        luaL_error(L, "module must return a value");
    lua_xmove(ML, L, results);
    lua_remove(L, -results - 1);
    return results;
}

static void require_config(luarequire_Configuration* config)
{
    config->is_require_allowed = is_require_allowed;
    config->reset = reset;
    config->jump_to_alias = jump_to_alias;
    config->to_parent = to_parent;
    config->to_child = to_child;
    config->is_module_present = is_module_present;
    config->get_chunkname = get_chunkname;
    config->get_loadname = get_real_path;
    config->get_cache_key = get_real_path;
    config->get_config_status = get_config_status;
    config->get_config = get_config;
    config->load = load;
}

// `__load("src/a/b.local.spec.luau")`: requires a project path as if from a chunk at the root, through the same
// cache as require. Upvalues: Luau's proxyrequire and the requirer's chunk name (kept alive by the closure).
static int load_project_file(lua_State* L)
{
    std::string path = "./" + module_path(luaL_checkstring(L, 1));
    lua_pushvalue(L, lua_upvalueindex(1));
    lua_pushlstring(L, path.data(), path.size());
    lua_pushvalue(L, lua_upvalueindex(2));
    lua_call(L, 2, 1);
    return 1;
}

static int vm_print(lua_State* L)
{
    opossum_vm* vm = vm_of(L);
    int n = lua_gettop(L);
    for (int i = 1; i <= n; i++)
    {
        size_t size = 0;
        const char* text = luaL_tolstring(L, i, &size);
        if (i > 1)
            vm->output += '\t';
        vm->output.append(text, size);
        lua_pop(L, 1);
    }
    vm->output += '\n';
    return 0;
}

// The `luau` CLI's collectgarbage, which local suites measure allocations with: "collect" runs a full cycle, "count"
// is the heap in KB.
static int vm_collectgarbage(lua_State* L)
{
    const char* option = luaL_optstring(L, 1, "collect");
    if (std::strcmp(option, "collect") == 0)
    {
        lua_gc(L, LUA_GCCOLLECT, 0);
        return 0;
    }
    if (std::strcmp(option, "count") == 0)
    {
        lua_pushnumber(L, lua_gc(L, LUA_GCCOUNT, 0));
        return 1;
    }
    luaL_error(L, "collectgarbage must be called with 'count' or 'collect'");
}

// Raises at every safepoint once installed, so the VM unwinds even through pcall. Null until a deadline passes:
// Luau calls a non-null callback at every loop back edge and call, which would skew benches.
static void vm_interrupted(lua_State* L, int gc)
{
    if (gc >= 0)
        return; // a GC step, not a safepoint that may raise
    luaL_error(L, "timed out");
}

// The error handler of host chunks: the message and the stack where it was raised.
static int vm_traceback(lua_State* L)
{
    std::string message = lua_isstring(L, 1) ? lua_tostring(L, 1) : std::string("(error object is a ") + luaL_typename(L, 1) + " value)";
    const char* trace = lua_debugtrace(L);
    const char* below = std::strchr(trace, '\n'); // the first frame is this handler
    message += "\nstack traceback:\n";
    message += below ? below + 1 : trace;
    lua_pushlstring(L, message.data(), message.size());
    return 1;
}

static char* copy_out(const std::string& text, size_t* outsize)
{
    char* out = static_cast<char*>(std::malloc(text.size() + 1));
    if (!out)
        return nullptr;
    std::memcpy(out, text.data(), text.size());
    out[text.size()] = 0;
    *outsize = text.size();
    return out;
}

extern "C" opossum_vm* opossum_vm_new(const char* root, int codegen, size_t limit) noexcept
{
    opossum_vm* vm = new (std::nothrow) opossum_vm();
    if (!vm)
        return nullptr;
    try
    {
        vm->root = normalize(root);
        vm->limit = limit;
        vm->L = lua_newstate(vm_alloc, vm);
        if (!vm->L)
        {
            delete vm;
            return nullptr;
        }
        lua_State* L = vm->L;
        lua_callbacks(L)->userdata = vm;
        vm->codegen = codegen && Luau::CodeGen::isSupported();
        if (vm->codegen)
            Luau::CodeGen::create(L, shared_codegen());
        // order matters: luaL_sandbox freezes the globals, so the host's globals go in before it
        luaL_openlibs(L);
        lua_pushcfunction(L, vm_print, "print");
        lua_setglobal(L, "print");
        lua_pushcfunction(L, vm_collectgarbage, "collectgarbage");
        lua_setglobal(L, "collectgarbage");
        luarequire_pushproxyrequire(L, require_config, vm);
        lua_pushliteral(L, "@__load");
        lua_pushcclosure(L, load_project_file, "__load", 2);
        lua_setglobal(L, "__load");
        luaopen_require(L, require_config, vm);
        luaL_sandbox(L);
        vm->host = lua_newthread(L);
        luaL_sandboxthread(vm->host);
        lua_ref(L, -1);
        lua_pop(L, 1);
        return vm;
    }
    catch (...)
    {
        if (vm->L)
            lua_close(vm->L);
        delete vm;
        return nullptr;
    }
}

// Runs `src` as the host chunk `@chunkname`. Returns 1 with the string it returned (nothing or nil is ""), or 0 with
// the error and its stack. `*out` is malloc'ed (free with opossum_vm_free_buffer), or null when no memory was left.
extern "C" int opossum_vm_eval(opossum_vm* vm, const char* chunkname, const char* src, size_t n, char** out, size_t* outsize) noexcept
{
    int ok = 0;
    std::string result;
    try
    {
        std::shared_ptr<const Bytecode> bytecode = compile(std::string(src, n));
        lua_State* T = vm->host;
        lua_settop(T, 0);
        lua_pushcfunction(T, vm_traceback, "traceback");
        std::string name = std::string("@") + chunkname;
        int status = load_chunk(vm, T, name.c_str(), *bytecode);
        if (status == 0)
            status = lua_pcall(T, 0, 1, 1);
        if (status != 0)
            result = lua_isstring(T, -1) ? lua_tostring(T, -1) : "(error object is not a string)";
        else if (lua_callbacks(vm->L)->interrupt)
            result = "timed out"; // the chunk swallowed every raise and returned: an interrupted run still failed
        else if (lua_isnil(T, -1))
            ok = 1;
        else if (lua_type(T, -1) == LUA_TSTRING)
        {
            size_t size = 0;
            const char* text = lua_tolstring(T, -1, &size);
            result.assign(text, size);
            ok = 1;
        }
        else
            result = std::string("the chunk returned a ") + luaL_typename(T, -1) + ", not a string";
        lua_settop(T, 0);
    }
    catch (const std::exception& e)
    {
        ok = 0;
        result = std::string("the VM failed: ") + e.what();
    }
    catch (...)
    {
        ok = 0;
        result = "the VM failed: an unknown C++ exception";
    }
    *out = copy_out(result, outsize);
    return ok;
}

// Thread-safe (lua.h allows setting interrupt from any thread): every safepoint from here on raises "timed out".
extern "C" void opossum_vm_interrupt(opossum_vm* vm) noexcept
{
    lua_callbacks(vm->L)->interrupt = vm_interrupted;
}

extern "C" size_t opossum_vm_native_functions(opossum_vm* vm) noexcept
{
    return vm->native;
}

extern "C" char* opossum_vm_take_output(opossum_vm* vm, size_t* outsize) noexcept
{
    char* out = copy_out(vm->output, outsize);
    vm->output.clear();
    return out;
}

extern "C" void opossum_vm_free(opossum_vm* vm) noexcept
{
    lua_close(vm->L);
    delete vm;
}

extern "C" void opossum_vm_free_buffer(char* buffer) noexcept
{
    std::free(buffer);
}
