
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

namespace IT
{

enum class E
{
  OK,
};

struct Lm;
using pLm     = std::shared_ptr<Lm>;
using StatEnv = std::unordered_map<std::string, pLm>;
using DynEnv  = std::vector<pLm>;

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
  DynEnv      env;
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

// A (possibly recursive) local binding. `var` is in scope while `val` is
// evaluated, so a value that refers to itself resolves once `val` is
// back-patched into the binding slot (see eval).
struct Let
{
  Var var;
  pLm val;
  pLm body;
};

// A foreign binding. `arg_types`/`ret_type` are Thrax type names taken from the
// signature; `args` accumulates curried arguments until the function is
// saturated, at which point the C call is made (see eval / call_extern).
struct Extern
{
  std::string              symbol;
  std::string              lib;
  std::vector<std::string> arg_types;
  std::string              ret_type;
  std::vector<pLm>         args;
};

#define IT_L_VARIANTS                                                          \
  X(INT, Int)                                                                  \
  X(STR, Str)                                                                  \
  X(APP, App)                                                                  \
  X(FUN, Fun)                                                                  \
  X(LET, Let)                                                                  \
  X(EXTERN, Extern)                                                            \
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

pLm eval(pLm node, DynEnv denv, StatEnv &senv);

} // namespace IT

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // IT_HEADER
