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
#include <map>

namespace TL
{

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
