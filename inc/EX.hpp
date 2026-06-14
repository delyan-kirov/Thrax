/*-------------------------------------------------------------------------------
 *\file EX.hpp
 *\info Header file for Parser
 * *----------------------------------------------------------------------------*/

#ifndef EX_HEADER
#define EX_HEADER

#include "ER.hpp"
#include "LX.hpp"
#include "UT.hpp"
#include <variant>

namespace EX
{

/*------------------------------------------------------------------------------
 *\TYPES
 *-----------------------------------------------------------------------------*/

struct Expr;
using Exprs  = UT::Vec<Expr>;
using ExPair = UT::Pair<Expr>;

struct ExUnknown
{
};
struct ExPubDef
{
  UT::String name;
  Expr      *def;
};
struct ExIntDef
{
  UT::String name;
  Expr      *def;
};
struct ExInt
{
  ssize_t value;
};
struct ExVar
{
  UT::String name;
};
struct ExStr
{
  UT::String value;
};

enum class BinopTag
{
  Add,
  Sub,
  Mul,
  Div,
  Mod,
  IsEq
};
enum class UnopTag
{
  Neg,
  Not
};

struct ExBinop
{
  BinopTag tag;
  ExPair   ops;
};
struct ExUnop
{
  UnopTag tag;
  Expr   *op;
};

struct ExFnDef
{
  UT::String param;
  Expr      *body;

  ExFnDef() = default;
  ExFnDef(UT::String param, AR::Arena &arena);
};

struct ExIf
{
  Expr *cond;
  Expr *then;
  Expr *alt;
};

struct ExApp
{
  Expr *fn;
  Expr *arg;
};

struct ExLet
{
  UT::String var;
  Expr      *val;
  Expr      *body;
};

#define EX_EXPR_VARIANTS                                                       \
  X(Unknown, ExUnknown)                                                        \
  X(PubDef, ExPubDef)                                                          \
  X(IntDef, ExIntDef)                                                          \
  X(Int, ExInt)                                                                \
  X(Unop, ExUnop)                                                              \
  X(Binop, ExBinop)                                                            \
  X(Let, ExLet)                                                                \
  X(FnDef, ExFnDef)                                                            \
  X(App, ExApp)                                                                \
  X(Var, ExVar)                                                                \
  X(If, ExIf)                                                                  \
  X(Str, ExStr)

enum class ExprTag
{
#define X(tag, type) tag,
  EX_EXPR_VARIANTS
#undef X
};

using ExprData =
#define X(tag, type) type,
  std::variant<EX_EXPR_VARIANTS std::monostate>
#undef X
  ;

enum class E
{
  MIN = -1,
  OK,
  MAX
};

struct Expr
{
  ExprTag  tag;
  ExprData as;

  Expr() = default;
  Expr(ExprTag tag);
  Expr(ExprTag tag, AR::Arena &arena);
};

/*-------------------------------------------------------------------------------
 *\CLASSES
 *------------------------------------------------------------------------------*/

class Parser
{
public:
  AR::Arena       &m_arena;
  ER::Events       m_events;
  const UT::String m_input;
  const LX::Tokens m_tokens;
  size_t           m_begin;
  size_t           m_end;
  Exprs            m_exprs;

  Parser(LX::Lexer l);
  Parser(LX::Tokens tokens, AR::Arena &arena, const UT::String input);
  Parser(EX::Parser old, size_t begin, size_t end);
  Parser(EX::Parser &parent_parser, const LX::Tokens &t);

  E run();
  E operator()();
};

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string pprint(ExprTag t, int level = 0);
std::string pprint(Expr *e, int level = 0);

} // namespace EX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // EX_HEADER
