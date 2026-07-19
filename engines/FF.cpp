/*-------------------------------------------------------------------------------
 *\file FF.cpp
 *\info Foreign-function calls via libffi + dlopen. Confined here on purpose.
 * *----------------------------------------------------------------------------*/

#include "FF.hpp"
#include "OP.hpp"
#include "TG.hpp"
#include "UT.hpp"

#ifndef THRAX_3RD_PARTY_ON
namespace FF
{
Slot
call(
  UT::Vu,
  UT::Vu,
  const std::vector<std::string> &,
  const std::string &,
  const std::vector<Slot> &)
{
  UT_FAIL_MSG("%s",
              "this build has no FFI support (compiled with NO_3RD_PARTY)");
  return 0;
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

void *
resolve(
  const std::string &lib, const std::string &symbol)
{
  std::string key = lib + "\x1f" + symbol;
  auto       &sc  = symbol_cache();
  auto        it  = sc.find(key);
  if (it != sc.end()) return it->second;

  auto &hc = handle_cache();
  auto  h  = hc.find(lib);
  void *handle;
  if (h != hc.end())
  {
    handle = h->second;
  }
  else
  {
    handle = dlopen(lib.c_str(), RTLD_NOW);
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
  std::string libs{ lib };
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

} // namespace FF
#endif // THRAX_NO_3RD_PARTY
