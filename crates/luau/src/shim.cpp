// `luau_compile` behind a boundary no C++ exception crosses. Luau turns its own parse and compile errors into
// an error buffer, but other exceptions (std::bad_alloc, a container's length_error / out_of_range) would unwind into
// Rust, which is undefined behaviour. Here every exception becomes an error buffer too, in Luau's format: a zero
// byte, then `:<line>: <message>`. The buffer is malloc'ed; free it with opossum_luau_free (the same C runtime).
// Luau's fast flags live here too: the policy that sets them, and a listing for the golden test.

#include "luacode.h"

#include "Luau/Common.h"
#include "Luau/ExperimentalFlags.h"

#include <cstddef>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <string>

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

extern "C" char* opossum_luau_compile(const char* source, size_t size, lua_CompileOptions* options, size_t* outsize) noexcept
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

extern "C" void opossum_luau_free(void* buffer) noexcept
{
    std::free(buffer);
}

// The `luau` CLI's policy (CLI/src/Flags.cpp, setLuauFlagsDefault): every boolean flag named Luau* is on, except the
// analysis flags Luau lists as experimental. Flags are process-global, so this runs once, before any compile.
extern "C" void opossum_luau_init() noexcept
{
    for (Luau::FValue<bool>* flag = Luau::FValue<bool>::list; flag; flag = flag->next)
        if (strncmp(flag->name, "Luau", 4) == 0 && !Luau::isAnalysisFlagExperimental(flag->name))
            flag->value = true;
}

// Every flag linked in, one `name=value` line each in registration order; malloc'ed, `*outsize` bytes, or null.
extern "C" char* opossum_luau_flags(size_t* outsize) noexcept
{
    try
    {
        std::string text;
        for (Luau::FValue<bool>* flag = Luau::FValue<bool>::list; flag; flag = flag->next)
            text += std::string(flag->name) + (flag->value ? "=true\n" : "=false\n");
        for (Luau::FValue<int>* flag = Luau::FValue<int>::list; flag; flag = flag->next)
            text += std::string(flag->name) + "=" + std::to_string(flag->value) + "\n";
        char* out = static_cast<char*>(std::malloc(text.size()));
        if (out)
            std::memcpy(out, text.data(), text.size());
        *outsize = text.size();
        return out;
    }
    catch (...)
    {
        return nullptr;
    }
}
