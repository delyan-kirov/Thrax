/*-------------------------------------------------------------------------------
 *\file FF.cpp
 *\info Foreign-function calls via libffi + dlopen. Confined here on purpose.
 * *----------------------------------------------------------------------------*/

#include "FF.hpp"
#include "OP.hpp"
#include "TG.hpp"
#include "UT.hpp"

#ifndef THRAX_3RD_PARTY_ON

// No libffi / dlopen (the `no-ffi` build, and the wasm/browser build where
// neither exists). Foreign calls are served from a COMPILED-IN HOST TABLE of
// the `C`/libm namespace instead: no dynamic loading, but the interpreter can
// still print and compute in the browser. A symbol outside the table fails
// with a clear message (e.g. a program's own `@extern "raylib"`).
//
// Each entry is a tiny adapter that calls its real C function with that
// function's TRUE signature -- essential on wasm, whose indirect calls are
// type-checked, so casting `&free` (returns void) to an int-returning pointer
// would trap. The adapter reads/writes the uniform 64-bit `Slot` ABI that
// FF::call already defines (a pointer is the slot's low word; a Real is the
// slot's double bit-pattern).

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <string>
#include <unistd.h>
#include <unordered_map>

namespace FF
{
namespace
{

using Adapter = Slot (*)(const Slot *a);

// A Real travels as its double bit-pattern in the slot (FF::call's contract);
// word args/results cast through intptr_t (a wasm pointer is a 32-bit word,
// the low bits of the 64-bit slot).
inline double
as_real(
  Slot s)
{
  double d;
  std::memcpy(&d, &s, sizeof d);
  return d;
}
inline Slot
from_real(
  double d)
{
  Slot s = 0;
  std::memcpy(&s, &d, sizeof d);
  return s;
}

const std::unordered_map<std::string, Adapter> &
host_table()
{
  static const std::unordered_map<std::string, Adapter> t = {
    // libc -- the `C` namespace DR generates (app/DR.cpp core_c_source). A
    // FILE* / allocation travels as an Int carrying the pointer.
    { "abort", [](const Slot *) -> Slot { std::abort(); } },
    { "exit", [](const Slot *a) -> Slot { std::exit((int)a[0]); } },
    { "puts",
      [](const Slot *a) -> Slot {
        return (Slot)std::puts((const char *)(intptr_t)a[0]);
      } },
    { "putchar",
      [](const Slot *a) -> Slot { return (Slot)std::putchar((int)a[0]); } },
    { "getchar", [](const Slot *) -> Slot { return (Slot)std::getchar(); } },
    { "malloc",
      [](const Slot *a) -> Slot {
        return (Slot)(intptr_t)std::malloc((size_t)a[0]);
      } },
    { "free",
      [](const Slot *a) -> Slot {
        std::free((void *)(intptr_t)a[0]);
        return 0;
      } },
    { "memset",
      [](const Slot *a) -> Slot {
        return (Slot)(intptr_t)std::memset(
          (void *)(intptr_t)a[0], (int)a[1], (size_t)a[2]);
      } },
    { "strlen",
      [](const Slot *a) -> Slot {
        return (Slot)std::strlen((const char *)(intptr_t)a[0]);
      } },
    { "fopen",
      [](const Slot *a) -> Slot {
        return (Slot)(intptr_t)std::fopen((const char *)(intptr_t)a[0],
                                          (const char *)(intptr_t)a[1]);
      } },
    { "fclose",
      [](const Slot *a) -> Slot {
        return (Slot)std::fclose((FILE *)(intptr_t)a[0]);
      } },
    { "fgetc",
      [](const Slot *a) -> Slot {
        return (Slot)std::fgetc((FILE *)(intptr_t)a[0]);
      } },
    { "fputs",
      [](const Slot *a) -> Slot {
        return (Slot)std::fputs((const char *)(intptr_t)a[0],
                                (FILE *)(intptr_t)a[1]);
      } },
    { "fflush",
      [](const Slot *a) -> Slot {
        return (Slot)std::fflush((FILE *)(intptr_t)a[0]);
      } },
    { "fseek",
      [](const Slot *a) -> Slot {
        return (Slot)std::fseek((FILE *)(intptr_t)a[0], (long)a[1], (int)a[2]);
      } },
    { "ftell",
      [](const Slot *a) -> Slot {
        return (Slot)std::ftell((FILE *)(intptr_t)a[0]);
      } },
    { "write",
      [](const Slot *a) -> Slot {
        return (Slot)write(
          (int)a[0], (const void *)(intptr_t)a[1], (size_t)a[2]);
      } },
    { "remove",
      [](const Slot *a) -> Slot {
        return (Slot)std::remove((const char *)(intptr_t)a[0]);
      } },
    { "getenv",
      [](const Slot *a) -> Slot {
        return (Slot)(intptr_t)std::getenv((const char *)(intptr_t)a[0]);
      } },
    { "time",
      [](const Slot *a) -> Slot {
        return (Slot)(intptr_t)std::time((time_t *)(intptr_t)a[0]);
      } },
    // libm.
    { "sqrt",
      [](const Slot *a) -> Slot {
        return from_real(std::sqrt(as_real(a[0])));
      } },
    { "sin",
      [](const Slot *a) -> Slot {
        return from_real(std::sin(as_real(a[0])));
      } },
    { "cos",
      [](const Slot *a) -> Slot {
        return from_real(std::cos(as_real(a[0])));
      } },
    { "tan",
      [](const Slot *a) -> Slot {
        return from_real(std::tan(as_real(a[0])));
      } },
    { "exp",
      [](const Slot *a) -> Slot {
        return from_real(std::exp(as_real(a[0])));
      } },
    { "log",
      [](const Slot *a) -> Slot {
        return from_real(std::log(as_real(a[0])));
      } },
    { "floor",
      [](const Slot *a) -> Slot {
        return from_real(std::floor(as_real(a[0])));
      } },
    { "ceil",
      [](const Slot *a) -> Slot {
        return from_real(std::ceil(as_real(a[0])));
      } },
    { "round",
      [](const Slot *a) -> Slot {
        return from_real(std::round(as_real(a[0])));
      } },
    { "pow",
      [](const Slot *a) -> Slot {
        return from_real(std::pow(as_real(a[0]), as_real(a[1])));
      } },
    { "fmod",
      [](const Slot *a) -> Slot {
        return from_real(std::fmod(as_real(a[0]), as_real(a[1])));
      } },
    { "atan2",
      [](const Slot *a) -> Slot {
        return from_real(std::atan2(as_real(a[0]), as_real(a[1])));
      } },
  };
  return t;
}

} // namespace

Slot
call(
  UT::Vu lib,
  UT::Vu symbol,
  const std::vector<std::string> &, // types: the adapter knows its own
  const std::string &,
  const std::vector<Slot> &args)
{
  std::string sym{ symbol };
  const auto &t  = host_table();
  auto        it = t.find(sym);
  if (it == t.end())
    UT_FAIL_MSG(
      "FFI: symbol '%s' (library '%s') is not in this build's host table -- "
      "only the C/libm namespace is compiled in (no dynamic loading; see "
      "engines/FF.cpp)",
      sym.c_str(),
      std::string(lib).c_str());

  // Zero-pad so an adapter reads its fixed argument count without a check.
  Slot buf[8] = { 0 };
  for (size_t i = 0; i < args.size() && i < 8; ++i) buf[i] = args[i];
  return it->second(buf);
}

void
add_lib_path(
  const std::string &)
{
}

void
add_preload(
  const std::string &)
{
}

} // namespace FF

#else

#include <dlfcn.h>
#include <ffi.h>

namespace FF
{

namespace
{

// How a Thrax type marshals across the C ABI.
struct Desc
{
  ffi_type *type;
  int       width; // bytes (ignored for pointers)
  bool      is_signed;
  bool      is_ptr;
};

Desc
desc_of(
  const std::string &name0)
{
  UT::Vu name = TG::host().canon(name0);

  if (name == OP::TY_INT8) return { &ffi_type_sint8, 1, true, false };
  if (name == OP::TY_INT16) return { &ffi_type_sint16, 2, true, false };
  if (name == OP::TY_INT32) return { &ffi_type_sint32, 4, true, false };
  if (name == OP::TY_INT64) return { &ffi_type_sint64, 8, true, false };
  if (name == OP::TY_NAT8) return { &ffi_type_uint8, 1, false, false };
  if (name == OP::TY_NAT16) return { &ffi_type_uint16, 2, false, false };
  if (name == OP::TY_NAT32) return { &ffi_type_uint32, 4, false, false };
  if (name == OP::TY_NAT64) return { &ffi_type_uint64, 8, false, false };
  // A double argument travels as its bit pattern in the 64-bit slot; a float
  // is narrowed into the slot by `call` below.
  if (name == OP::TY_REAL64) return { &ffi_type_double, 8, true, false };
  if (name == OP::TY_REAL32) return { &ffi_type_float, 4, true, false };
  if (name == OP::TY_STR || name == OP::TY_PTR || name == OP::TY_ARRAY)
    return { &ffi_type_pointer, (int)sizeof(void *), false, true };

  // Any other type name (including type variables) is treated as word-sized,
  // since the runtime represents everything as an integer word.
  return { &ffi_type_slong, (int)sizeof(long), true, false };
}

// dlopen handles cached by library path; resolved symbols by lib+symbol.
std::unordered_map<std::string, void *> &
handle_cache()
{
  static std::unordered_map<std::string, void *> c;
  return c;
}

std::unordered_map<std::string, void *> &
symbol_cache()
{
  static std::unordered_map<std::string, void *> c;
  return c;
}

// BUILD-directive state (`@run BUILD.lib_path` / `BUILD.lib`, applied by DR
// before the program runs): extra dlopen search prefixes, and libraries to
// preload with RTLD_GLOBAL so their symbols are available to later loads.
std::vector<std::string> &
lib_paths()
{
  static std::vector<std::string> v;
  return v;
}

std::vector<std::string> &
pending_preloads()
{
  static std::vector<std::string> v;
  return v;
}

// dlopen `soname`, trying each BUILD lib_path prefix first (a name that is
// already a path skips the prefixes). Null on total failure.
void *
open_lib(
  const std::string &soname)
{
  if (soname.find('/') == std::string::npos)
    for (const std::string &dir : lib_paths())
      if (void *h = dlopen((dir + "/" + soname).c_str(), RTLD_NOW)) return h;
  return dlopen(soname.c_str(), RTLD_NOW);
}

void *
resolve(
  const std::string &lib, const std::string &symbol)
{
  std::string key = lib + "\x1f" + symbol;
  auto       &sc  = symbol_cache();
  auto        it  = sc.find(key);
  if (it != sc.end()) return it->second;

  for (const std::string &p : pending_preloads())
  {
    std::string soname = TG::host().soname(p);
    if (!dlopen(soname.c_str(), RTLD_NOW | RTLD_GLOBAL))
      UT_FAIL_MSG("FFI: cannot preload library '%s' (BUILD.lib \"%s\"): %s",
                  soname.c_str(),
                  p.c_str(),
                  dlerror());
  }
  pending_preloads().clear();

  auto &hc = handle_cache();
  auto  h  = hc.find(lib);
  void *handle;
  if (h != hc.end())
  {
    handle = h->second;
  }
  else
  {
    handle = open_lib(lib);
    if (!handle)
      UT_FAIL_MSG("FFI: cannot open library '%s': %s", lib.c_str(), dlerror());
    hc[lib] = handle;
  }

  void *fn = dlsym(handle, symbol.c_str());
  if (!fn)
    UT_FAIL_MSG(
      "FFI: symbol '%s' not found in '%s'", symbol.c_str(), lib.c_str());
  sc[key] = fn;
  return fn;
}

Slot
extend(
  const Desc &d, uint64_t raw)
{
  if (d.is_ptr) return (Slot)raw;
  switch (d.width)
  {
  case 1 : return d.is_signed ? (Slot)(int8_t)raw : (Slot)(uint8_t)raw;
  case 2 : return d.is_signed ? (Slot)(int16_t)raw : (Slot)(uint16_t)raw;
  case 4 : return d.is_signed ? (Slot)(int32_t)raw : (Slot)(uint32_t)raw;
  default: return d.is_signed ? (Slot)raw : (Slot)(uint64_t)raw;
  }
}

} // namespace

Slot
call(
  UT::Vu                          lib,
  UT::Vu                          symbol,
  const std::vector<std::string> &arg_types,
  const std::string              &ret_type,
  const std::vector<Slot>        &args)
{
  // The interpreter runs programs for the host, so the host target resolves
  // the symbolic library name ("libc" -> "libc.so.6" here and now -- the
  // name never appears in source or IR as a soname).
  std::string libs = TG::host().soname(lib);
  std::string syms{ symbol };

  size_t n = args.size();

  std::vector<ffi_type *> in_types(n);
  std::vector<uint64_t>   storage(n); // one 64-bit slot of backing per argument
  std::vector<void *>     argp(n);
  for (size_t i = 0; i < n; ++i)
  {
    Desc d      = desc_of(arg_types[i]);
    in_types[i] = d.type;
    storage[i]  = (uint64_t)args[i]; // low `width` bytes are read by libffi
    if (d.type == &ffi_type_float)
    {
      // The slot carries double bits (the caller's Real); libffi reads a
      // float, so narrow in place.
      double dv;
      std::memcpy(&dv, &storage[i], sizeof(dv));
      float fv = (float)dv;
      std::memcpy(&storage[i], &fv, sizeof(fv));
    }
    argp[i] = &storage[i];
  }

  Desc      rd = desc_of(ret_type);
  ffi_cif   cif;
  ffi_type *ret = rd.type;
  if (ffi_prep_cif(
        &cif, FFI_DEFAULT_ABI, (unsigned)n, ret, n ? in_types.data() : nullptr)
      != FFI_OK)
    UT_FAIL_MSG("FFI: failed to prepare call to '%s'", syms.c_str());

  void *fn = resolve(libs, syms);

  // The return buffer must be as wide as the widest return libffi may write;
  // a union keeps each read correctly typed. Integral returns narrower than
  // ffi_arg are widened by libffi into `a`.
  union
  {
    ffi_arg  a;
    uint64_t u64;
    double   d;
    float    f;
  } result;
  result.u64 = 0;
  ffi_call(&cif, FFI_FN(fn), &result, n ? argp.data() : nullptr);

  if (rd.type == &ffi_type_double || rd.type == &ffi_type_float)
  {
    // Hand back double bits in the slot; widen a float return first.
    double   dv = (rd.type == &ffi_type_float) ? (double)result.f : result.d;
    uint64_t bits;
    std::memcpy(&bits, &dv, sizeof(bits));
    return (Slot)bits;
  }
  return extend(rd, result.u64);
}

void
add_lib_path(
  const std::string &p)
{
  lib_paths().push_back(p);
}

void
add_preload(
  const std::string &lib)
{
  pending_preloads().push_back(lib);
}

} // namespace FF
#endif // THRAX_NO_3RD_PARTY
