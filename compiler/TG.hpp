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

#include <optional>
#include <string>

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

  // The C library's runtime name on this target. Interim: baked into the
  // generated `C.thx` externs by DR until symbolic library names land
  // (doc/platform-abstraction.md section 3.3), when this becomes a
  // resolve-at-the-edge detail.
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

  constexpr bool
  has_dlopen() const
  {
    return Os::Wasi != os;
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

  std::string
  name() const
  {
    const char *a = "?";
    switch (arch)
    {
    case Arch::X86_64 : a = "x86_64"; break;
    case Arch::Aarch64: a = "aarch64"; break;
    case Arch::X86    : a = "x86"; break;
    case Arch::Arm    : a = "arm"; break;
    case Arch::Wasm32 : a = "wasm32"; break;
    }
    const char *o = "?";
    switch (os)
    {
    case Os::Linux  : o = "linux"; break;
    case Os::Macos  : o = "macos"; break;
    case Os::Windows: o = "windows"; break;
    case Os::Wasi   : o = "wasi"; break;
    }
    return std::string(a) + "-" + o;
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
