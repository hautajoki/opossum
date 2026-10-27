// rbxts-rs: `luau_compile` behind a boundary no C++ exception crosses. Luau turns its own parse and compile errors into
// an error buffer, but other exceptions (std::bad_alloc, a container's length_error / out_of_range) would unwind into
// Rust, which is undefined behaviour. Here every exception becomes an error buffer too, in Luau's format: a zero
// byte, then `:<line>: <message>`. The buffer is malloc'ed; free it with rbxts_luau_free (the same C runtime).

#include <cstddef>
#include <cstdlib>
#include <cstring>
#include <exception>

struct lua_CompileOptions;

extern "C" char* luau_compile(const char* source, size_t size, lua_CompileOptions* options, size_t* outsize);

static char* error_buffer(const char* what, size_t* outsize)
{
    static const char prefix[] = ":0: Luau's compiler failed: ";
    size_t prefix_size = sizeof(prefix) - 1;
    size_t what_size = std::strlen(what);
    char* out = static_cast<char*>(std::malloc(1 + prefix_size + what_size));
    if (!out)
        return nullptr;
    out[0] = 0;
    std::memcpy(out + 1, prefix, prefix_size);
    std::memcpy(out + 1 + prefix_size, what, what_size);
    *outsize = 1 + prefix_size + what_size;
    return out;
}

extern "C" char* rbxts_luau_compile(const char* source, size_t size, lua_CompileOptions* options, size_t* outsize) noexcept
{
    try
    {
        return luau_compile(source, size, options, outsize);
    }
    catch (const std::exception& e)
    {
        return error_buffer(e.what(), outsize);
    }
    catch (...)
    {
        return error_buffer("an unknown C++ exception", outsize);
    }
}

extern "C" void rbxts_luau_free(void* buffer) noexcept
{
    std::free(buffer);
}
