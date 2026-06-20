
/*-------------------------------------------------------------------------------
 *\file IT.hpp
 *\info Header file for the interpreter
 * *----------------------------------------------------------------------------*/

#ifndef IT_HEADER_
#define IT_HEADER_

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

struct Real
{
  double unwrap;
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

// A built-in operation as a first-class, curried value. `impl` is the key the
// type checker resolved an overloaded use to (e.g. "+@Int"); `args` accumulates
// applied arguments until it reaches `arity`, at which point the implementation
// runs (see eval). Because it is an ordinary value, a built-in can be partially
// applied and passed around like any function.
struct Builtin
{
  std::string      impl;
  size_t           arity;
  std::vector<pLm> args;
};

// A conditional. `if` is not a function (it must not evaluate both branches),
// so it stays a dedicated lazy node rather than a builtin: eval forces `cond`,
// then only the taken branch.
struct If
{
  pLm cond;
  pLm yes;
  pLm no;
};

// A self-reference cell for a `let` binding. A binding's value (typically a
// closure) captures the environment the binding lives in, which would otherwise
// hold a shared_ptr back to the value -- a cycle that leaks. While the value is
// evaluated the binding is therefore held *weakly* through one of these; eval's
// VAR lookup locks `target` to recover it. The value is always live when
// locked: the body keeps it strongly in scope, and a call site keeps the
// closure alive.
struct Rec
{
  std::weak_ptr<Lm> target;
};

// A struct value: the type name plus its fields in declaration order. Built
// from a `Type.{...}` literal; eval forces every field.
struct Struct
{
  std::string                              name;
  std::vector<std::pair<std::string, pLm>> fields;
};

// Field access `record.field`. eval forces the record (a Struct) and returns
// the named field; the type checker guarantees the field exists.
struct Field
{
  pLm         record;
  std::string name;
};

#define IT_L_VARIANTS                                                          \
  X(INT, Int)                                                                  \
  X(REAL, Real)                                                                \
  X(STR, Str)                                                                  \
  X(APP, App)                                                                  \
  X(FUN, Fun)                                                                  \
  X(LET, Let)                                                                  \
  X(EXTERN, Extern)                                                            \
  X(BUILTIN, Builtin)                                                          \
  X(IF, If)                                                                    \
  X(REC, Rec)                                                                  \
  X(STRUCT, Struct)                                                            \
  X(FIELD, Field)                                                              \
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

#undef IT_L_VARIANTS
#undef X

#endif // IT_HEADER_

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
