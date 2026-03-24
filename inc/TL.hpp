/*-------------------------------------------------------------------------------
 *\file TL.hpp
 *\info Header file for the translation layer
 * *----------------------------------------------------------------------------*/

#ifndef TL_HEADER
#define TL_HEADER

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "EX.hpp"
#include "UT.hpp"
#include "ffi.h"
#include <map>
#include <vector>
#include <string>

namespace TL
{

using FnMap_t = std::map<std::string, void *>;
class Dfn
{
public:
  const char    *m_fn_name;
  ffi_type     **m_in_types;
  ffi_type      *m_out_type;
  static void   *m_handle;
  static FnMap_t m_fn_map;

  Dfn(const char *fn_name, ffi_type **in_types, ffi_type *out_type);

  static void init(UT::String lib_path);

  void configure(void);

  static void deinit(void);

  bool call(std::vector<void *> &input, void *output);
};

using Env = std::map<std::string, EX::Expr>;

struct Instance
{
  EX::Expr m_expr;
  Env      m_env;
};

#define TL_TypeEnumVariants                                                    \
  X(IntDef)                                                                    \
  X(PubDef)                                                                    \
  X(ExtDef)

enum class Type
{
#define X(enum) enum,
  TL_TypeEnumVariants
#undef X
};

struct Def
{
  Type       m_type;
  UT::String m_name;
  EX::Expr   m_expr;
};

struct Mod
{
  UT::String   m_name;
  UT::Vec<Def> m_defs;

  Mod(UT::String file_name, AR::Arena &arena);
};

Instance eval(Instance &inst);
} // namespace TL

namespace std
{
inline string
to_string(
  TL::Type type)
{
  switch (type)
  {
#define X(X_enum)                                                              \
  case TL::Type::X_enum: return #X_enum;
    TL_TypeEnumVariants
#undef X
  }
  UT_FAIL_IF("UNREACHABLE");
}

} // namespace std

#endif // TL_HEADER
