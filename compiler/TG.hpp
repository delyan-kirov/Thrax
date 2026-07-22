/*-------------------------------------------------------------------------------
 *\file TG.hpp
 *\info The compilation target, as data -- the single source of platform truth
 *      for one compilation (see doc/platform-abstraction.md).
 *
 * HOST (where a stage runs) and TARGET (what the program is for) are different
 * values. The target is constructed at runtime in app/ (default: the host,
 * overridable by --target=) and passed down; no stage below app/ may decide
 * platform policy from preprocessor conditionals of its own build. The one
 * sanctioned exception is `host()` below: detecting the machine the compiler
 * binary runs on has to consult predefined macros somewhere, and this is the
 * only place in compiler/, engines/ or app/ allowed to.
 *
 * Host-bound consumers (the interpreter's dispatch table, FF's marshalling)
 * key off `host()` -- the interpreter is *defined* to run programs for the
 * host, so that is target truth for them, not a shortcut. Target-bound
 * consumers (TC's literal typing, DR's prelude generation, CC) take the
 * threaded Target.
 *-----------------------------------------------------------------------------*/

#ifndef TG_HEADER_
#define TG_HEADER_

#include "OP.hpp"
#include "UT.hpp"

namespace TG
{

enum class Os
{
  Linux,
  Macos,
  Windows,
  Wasi,
};

enum class Arch
{
  X86_64,
  Aarch64,
  X86,
  Arm,
  Wasm32,
};

struct Target
{
  Os   os;
  Arch arch;

  constexpr bool operator==(const Target &) const = default;

  constexpr int
  ptr_bits() const
  {
    switch (arch)
    {
    case Arch::X86_64:
    case Arch::Aarch64: return 64;
    case Arch::X86    :
    case Arch::Arm    :
    case Arch::Wasm32 : return 32;
    }
    return 64;
  }

  // The largest integer LITERAL that fits this target's word.
  // FIXME : use int64_t not long long | better yet define integer aliases in UT
  constexpr long long
  lit_max() const
  {
    return 32 == ptr_bits() ? 0xffffffffLL : 0x7fffffffffffffffLL;
  }

  constexpr long long
  int_max() const
  {
    return 32 == ptr_bits() ? 0x7fffffffLL : 0x7fffffffffffffffLL;
  }
  constexpr long long
  int_min() const
  {
    return 32 == ptr_bits() ? -0x80000000LL : (-0x7fffffffffffffffLL - 1);
  }

  // The canonical spelling the target's `Int`/`Nat` alias to: the target word.
  constexpr const char *
  int_ty() const
  {
    return 32 == ptr_bits() ? OP::TY_INT32 : OP::TY_INT64;
  }
  constexpr const char *
  nat_ty() const
  {
    return 32 == ptr_bits() ? OP::TY_NAT32 : OP::TY_NAT64;
  }

  // The C library's runtime name on this target. Source code never spells
  // this: `@extern` names libraries SYMBOLICALLY ("libc") and consumers
  // resolve through soname() below at the last possible moment
  // (doc/platform-abstraction.md section 3.3).
  constexpr const char *
  libc_soname() const
  {
    switch (os)
    {
    case Os::Linux  : return "libc.so.6";
    case Os::Macos  : return "libSystem.B.dylib";
    case Os::Windows: return "msvcrt.dll";
    case Os::Wasi   : return ""; // static-link only; no runtime loading
    }
    return "";
  }

  // The math library's runtime name. On Linux libm is its own soname (even
  // where glibc >= 2.34 folded the symbols into libc for static linking,
  // distributions like Nix still ship them only in libm.so.6); elsewhere the
  // C library carries the math symbols.
  constexpr const char *
  libm_soname() const
  {
    return Os::Linux == os ? "libm.so.6" : libc_soname();
  }

  constexpr bool
  has_dlopen() const
  {
    return Os::Wasi != os;
  }

  // Resolve a symbolic `@extern` library name to this target's runtime
  // loadable name. "libc"/"libm" map to the known C/math libraries; a name
  // containing '.' or '/' is an explicit soname/path and passes through
  // verbatim (user's responsibility); anything else gets the platform's
  // conventional decoration ("raylib" -> "libraylib.so" / "libraylib.dylib"
  // / "raylib.dll"; a leading "lib" is not doubled). The native backend does
  // NOT use this: it turns the symbolic name into a link-line flag instead.
  std::string
  soname(
    UT::Vu lib) const
  {
    if (lib == "libc" || lib == "c") return libc_soname();
    if (lib == "libm" || lib == "m") return libm_soname();
    if (lib.find('.') != UT::Vu::npos || lib.find('/') != UT::Vu::npos)
      return std::string(lib);
    std::string base{ lib };
    switch (os)
    {
    case Os::Linux:
      return (base.starts_with("lib") ? base : "lib" + base) + ".so";
    case Os::Macos:
      return (base.starts_with("lib") ? base : "lib" + base) + ".dylib";
    case Os::Windows: return base + ".dll";
    case Os::Wasi   : return base; // no runtime loading; never dlopened
    }
    return base;
  }

  // Canonical spelling of a base-type name for THIS target: `Int`/`Nat` fold
  // to the target word type, every other spelling through OP::canon. This is
  // the canonicalization FFI consumers must use (OP::canon alone cannot fold
  // `Int` -- its width is target policy, not vocabulary).
  UT::Vu
  canon(
    UT::Vu name) const
  {
    if (name == "Int") return int_ty();
    if (name == "Nat") return nat_ty();
    return OP::canon(name);
  }

  constexpr const char *
  arch_name() const
  {
    switch (arch)
    {
    case Arch::X86_64 : return "x86_64";
    case Arch::Aarch64: return "aarch64";
    case Arch::X86    : return "x86";
    case Arch::Arm    : return "arm";
    case Arch::Wasm32 : return "wasm32";
    }
    return "?";
  }
  constexpr const char *
  os_name() const
  {
    switch (os)
    {
    case Os::Linux  : return "linux";
    case Os::Macos  : return "macos";
    case Os::Windows: return "windows";
    case Os::Wasi   : return "wasi";
    }
    return "?";
  }

  std::string
  name() const
  {
    return std::string(arch_name()) + "-" + os_name();
  }
};

// The machine this binary runs on. The ONE sanctioned use of predefined
// platform macros outside platforms/ (see the header comment).
constexpr Target
host()
{
#if defined(_WIN32)
  constexpr Os os = Os::Windows;
#elif defined(__APPLE__)
  constexpr Os os = Os::Macos;
#elif defined(__wasi__)
  constexpr Os os = Os::Wasi;
#else
  constexpr Os os = Os::Linux;
#endif

#if defined(__x86_64__) || defined(_M_X64)
  constexpr Arch arch = Arch::X86_64;
#elif defined(__aarch64__) || defined(_M_ARM64)
  constexpr Arch arch = Arch::Aarch64;
#elif defined(__wasm32__)
  constexpr Arch arch = Arch::Wasm32;
#elif defined(__i386__) || defined(_M_IX86)
  constexpr Arch arch = Arch::X86;
#elif defined(__arm__) || defined(_M_ARM)
  constexpr Arch arch = Arch::Arm;
#else
#error "TG: unrecognized host architecture -- add it to TG::host()"
#endif

  return Target{ os, arch };
}

// How the host invokes a C compiler for a target: the `cc -O2` call as data
// (doc/platform-abstraction.md section 4). The host target uses the system
// `cc`; a cross target names its driver through an environment variable so
// tool LOCATIONS stay out of the compiler, like `@extern` library locations
// do (the nix flake exports WASI_CC; any wasi-sdk clang works). An empty
// `cc` means no toolchain was found and `hint` says what to set.
struct Toolchain
{
  std::string              cc;     // C compiler driver; empty = missing
  std::string              hint;   // when cc is empty: how to provide one
  std::vector<std::string> cflags; // always passed (optimization level etc.)
  std::string runner;     // wraps execution ("wasmtime"); empty = run directly
  std::string exe_suffix; // "" native, ".wasm" wasi (".exe" windows, later)
  bool        rpath = true; // linker understands -Wl,-rpath (has dyn loading)
};

inline Toolchain
toolchain(
  const Target &t)
{
  Toolchain tc;
  tc.cflags = { "-O2" };
  if (t == host())
  {
    tc.cc = "cc";
    return tc;
  }
  if (Os::Wasi == t.os && Arch::Wasm32 == t.arch)
  {
    const char *cc = std::getenv("WASI_CC");
    tc.cc          = cc ? cc : "";
    tc.hint        = "no C compiler for wasm32-wasi -- set WASI_CC to a "
                     "wasi-sdk clang (the nix dev shell exports it)";
    // Wasm linear memory has NO guard pages, and wasm-ld's default layout
    // puts the (64 KiB) shadow stack ABOVE the data segment: a deep C stack
    // silently tramples the program's globals instead of faulting. The
    // runtime recurses on the C stack when forcing globals (THxRT_glob ->
    // THxK_run_code -> blocks -> THxRT_glob), so give wasm programs the
    // stack a native host gives them, placed FIRST so overflow traps at
    // address zero instead of corrupting data.
    tc.cflags.push_back("-Wl,--stack-first");
    tc.cflags.push_back("-Wl,-z,stack-size=8388608");
    tc.runner     = "wasmtime";
    tc.exe_suffix = ".wasm";
    tc.rpath      = false; // wasm-ld has no rpath; wasi never loads at runtime
    return tc;
  }
  tc.cflags.clear();
  tc.hint = "cross-compilation to '" + t.name()
            + "' is not supported yet "
              "(host is "
            + host().name() + ")";
  return tc;
}

// Parse a `--target=` spelling ("x86_64-linux", "wasm32-wasi", ...): the
// inverse of Target::name(). Empty optional on an unknown spelling.
inline std::optional<Target>
parse(
  UT::Vu s)
{
  constexpr Os   oses[]   = { Os::Linux, Os::Macos, Os::Windows, Os::Wasi };
  constexpr Arch arches[] = {
    Arch::X86_64, Arch::Aarch64, Arch::X86, Arch::Arm, Arch::Wasm32,
  };
  for (Os os : oses)
    for (Arch arch : arches)
    {
      Target t{ os, arch };
      if (t.name() == s) return t;
    }
  return std::nullopt;
}

} // namespace TG

#endif // TG_HEADER_
