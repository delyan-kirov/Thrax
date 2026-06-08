
/*-------------------------------------------------------------------------------
 *\file IT.hpp
 *\info Header file for the interpreter
 * *----------------------------------------------------------------------------*/

#ifndef IT_HEADER
#define IT_HEADER

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/
#include "EX.hpp"
#include <memory>
#include <string>
#include <unordered_map>
#include <variant>
#include <vector>

namespace IT
{

enum class E
{
  OK,
};

struct Lm;
using pLm = std::shared_ptr<Lm>;

struct Int
{
  ssize_t unwrap;
};

struct App
{
  pLm fn;
  pLm arg;
};

struct Var
{
  std::string unwrap;
  size_t      idx;
};

struct Str
{
  std::string unwrap;
};

struct Fun
{
  Var var;
  pLm body;
};

struct Ifc
{
  pLm cond;
  pLm then;
  pLm othw;
};

#define IT_L_VARIANTS                                                          \
  X(INT, Int)                                                                  \
  X(IFC, Ifc)                                                                  \
  X(STR, Str)                                                                  \
  X(APP, App)                                                                  \
  X(FUN, Fun)                                                                  \
  X(VAR, Var)

enum class LTag
{
#define X(tag, variant) tag,
  IT_L_VARIANTS
#undef X
  UNK,
};

using LmUnion =
#define X(tag, variant) variant,
  std::variant<IT_L_VARIANTS std::monostate>
#undef X
  ;

struct Lm
{
  LTag    tag;
  LmUnion as;
};

using StatEnv = std::unordered_map<std::string, pLm>;
using DynEnv  = std::vector<pLm>;

inline pLm
lookup(
  const StatEnv &env, const std::string &name)
{
  auto it = env.find(name);
  if (it != env.end()) return it->second;
  return std::make_shared<Lm>(Lm{ .tag = LTag::UNK, .as = std::monostate{} });
}

pLm exprs2pLm(EX::Expr *expr, StatEnv &env);

std::string pprint(pLm lm, int level = 0);

} // namespace IT

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // IT_HEADER
