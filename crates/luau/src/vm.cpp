// The Luau VM local tests, benches and exec run in (src/vm.rs). Everything that touches a lua_State lives here, and
// every entry point is noexcept with a catch-all, so no Luau error, C++ exception or longjmp crosses a Rust frame.
//
// One opossum_vm is one lua_State: the standard libraries, print captured into the VM's output, globals sandboxed,
// and a memory cap enforced by its allocator. Host chunks run on one sandboxed thread. Bytecode is cached per process
// by source, so a module many VMs load is compiled once.

#include "lua.h"
#include "luacode.h"
#include "lualib.h"

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
    std::string root;
    std::string output;
    size_t used = 0;
    size_t limit = 0;
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
};

static std::mutex cache_mutex;
static std::unordered_map<std::string, std::shared_ptr<const Bytecode>> cache;

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
    return cache.emplace(source, std::move(compiled)).first->second; // a racing compile of the same source loses
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

extern "C" opossum_vm* opossum_vm_new(const char* root, size_t limit) noexcept
{
    opossum_vm* vm = new (std::nothrow) opossum_vm();
    if (!vm)
        return nullptr;
    try
    {
        vm->root = root;
        vm->limit = limit;
        vm->L = lua_newstate(vm_alloc, vm);
        if (!vm->L)
        {
            delete vm;
            return nullptr;
        }
        lua_State* L = vm->L;
        lua_callbacks(L)->userdata = vm;
        // order matters: luaL_sandbox freezes the globals, so the host's globals go in before it
        luaL_openlibs(L);
        lua_pushcfunction(L, vm_print, "print");
        lua_setglobal(L, "print");
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
        int status = luau_load(T, name.c_str(), bytecode->bytes.data(), bytecode->bytes.size(), 0);
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
