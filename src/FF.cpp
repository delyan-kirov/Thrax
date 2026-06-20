/*-------------------------------------------------------------------------------
 *\file FF.cpp
 *\info Foreign-function calls via libffi + dlopen. Confined here on purpose.
 * *----------------------------------------------------------------------------*/

#include "FF.hpp"
#include "UT.hpp"

namespace FF
{

#ifdef THRAX_NO_3RD_PARTY

ssize_t
call(
  UT::Vu,
  UT::Vu,
  const std::vector<std::string> &,
  const std::string &,
  const std::vector<ssize_t> &)
{
  UT_FAIL_MSG("%s",
              "this build has no FFI support (compiled with NO_3RD_PARTY)");
  return 0;
}

#else

} // namespace FF

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
  const std::string &name)
{
  // Int/Nat are the platform word (ssize_t/size_t == long/unsigned long here).
  if (name == "Int") return { &ffi_type_slong, 8, true, false };
  if (name == "Nat") return { &ffi_type_ulong, 8, false, false };
  if (name == "Int8") return { &ffi_type_sint8, 1, true, false };
  if (name == "Int16") return { &ffi_type_sint16, 2, true, false };
  if (name == "Int32") return { &ffi_type_sint32, 4, true, false };
  if (name == "Int64") return { &ffi_type_sint64, 8, true, false };
  if (name == "Nat8") return { &ffi_type_uint8, 1, false, false };
  if (name == "Nat16") return { &ffi_type_uint16, 2, false, false };
  if (name == "Nat32") return { &ffi_type_uint32, 4, false, false };
  if (name == "Nat64") return { &ffi_type_uint64, 8, false, false };
  if (name == "Str" || name == "Ptr")
    return { &ffi_type_pointer, 8, false, true };

  if (name == "Int128" || name == "Nat128")
    UT_FAIL_MSG("FFI: 128-bit type '%s' is not supported yet", name.c_str());

  // Any other type name (including type variables) is treated as word-sized,
  // since the runtime represents everything as an ssize_t.
  return { &ffi_type_slong, 8, true, false };
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

ssize_t
extend(
  const Desc &d, ffi_arg raw)
{
  if (d.is_ptr) return (ssize_t)raw;
  switch (d.width)
  {
  case 1 : return d.is_signed ? (ssize_t)(int8_t)raw : (ssize_t)(uint8_t)raw;
  case 2 : return d.is_signed ? (ssize_t)(int16_t)raw : (ssize_t)(uint16_t)raw;
  case 4 : return d.is_signed ? (ssize_t)(int32_t)raw : (ssize_t)(uint32_t)raw;
  default: return d.is_signed ? (ssize_t)(int64_t)raw : (ssize_t)(uint64_t)raw;
  }
}

} // namespace

ssize_t
call(
  UT::Vu                          lib,
  UT::Vu                          symbol,
  const std::vector<std::string> &arg_types,
  const std::string              &ret_type,
  const std::vector<ssize_t>     &args)
{
  std::string libs{ lib };
  std::string syms{ symbol };

  size_t n = args.size();

  std::vector<ffi_type *> in_types(n);
  std::vector<uint64_t>   storage(n); // one word of backing per argument
  std::vector<void *>     argp(n);
  for (size_t i = 0; i < n; ++i)
  {
    in_types[i] = desc_of(arg_types[i]).type;
    storage[i]  = (uint64_t)args[i]; // low `width` bytes are read by libffi
    argp[i]     = &storage[i];
  }

  Desc      rd = desc_of(ret_type);
  ffi_cif   cif;
  ffi_type *ret = rd.type;
  if (ffi_prep_cif(
        &cif, FFI_DEFAULT_ABI, (unsigned)n, ret, n ? in_types.data() : nullptr)
      != FFI_OK)
    UT_FAIL_MSG("FFI: failed to prepare call to '%s'", syms.c_str());

  void *fn = resolve(libs, syms);

  ffi_arg result = 0;
  ffi_call(&cif, FFI_FN(fn), &result, n ? argp.data() : nullptr);

  return extend(rd, result);
}

#endif // THRAX_NO_3RD_PARTY

} // namespace FF
