/*-------------------------------------------------------------------------------
 *\file EX.hpp
 *\info Header file for the Parser.
 *
 * EX is a Pratt (precedence-climbing) parser that pulls flat tokens from a
 * streaming LX::Lexer and builds the Expr tree below. Operator precedence and
 * structure (groups, let, if, lambda) interleave through one recursion, so the
 * C++ call stack mirrors the grammar -- which is what lets errors carry a
 * meaningful context chain (see ER::Diagnostic). The Expr types are unchanged
 * from before; only the parsing engine is new.
 *-----------------------------------------------------------------------------*/

#ifndef EX_HEADER
#define EX_HEADER

#include "ER.hpp"
#include "LX.hpp"
#include "UT.hpp"
#include <variant>
#include <vector>

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

struct Expr
{
  ExprTag  tag;
  ExprData as;

  Expr() = default;
  Expr(ExprTag tag);
  Expr(ExprTag tag, AR::Arena &arena);
};

/*-------------------------------------------------------------------------------
 *\PARSER
 *------------------------------------------------------------------------------*/

class Parser
{
public:
  AR::Arena                  &m_arena;
  LX::Lexer                  &m_lex;
  Exprs                       m_exprs; // successfully parsed top-level defs
  std::vector<ER::Diagnostic> m_diags; // one chain per failed definition

  Parser(LX::Lexer &lex);

  // Parse every top-level definition. Good defs land in m_exprs; each failed
  // definition contributes one chain to m_diags and parsing recovers to the
  // next definition.
  void run();

private:
  ER::Result<Expr *>     parse_global();
  ER::Result<Expr *>     parse_expr(int min_bp);
  ER::Result<Expr *>     parse_prefix();
  ER::Result<Expr *>     parse_primary();
  ER::Result<Expr *>     parse_group();
  ER::Result<Expr *>     parse_let();
  ER::Result<Expr *>     parse_if();
  ER::Result<Expr *>     parse_closure();
  ER::Result<LX::Token>  expect(LX::TokenTag tag, const char *what);
  void                   recover();

  Expr *alloc(Expr e);
  Expr *mk_int(const LX::Token &t);
  Expr *mk_str(const LX::Token &t);
  Expr *mk_var(const LX::Token &t);
  Expr *mk_app(Expr *fn, Expr *arg);
  Expr *mk_unop(LX::TokenTag op, Expr *operand);
  Expr *mk_binop(LX::TokenTag op, Expr *lhs, Expr *rhs);
  Expr *mk_if(Expr *cond, Expr *then, Expr *alt);
  Expr *mk_let(UT::String var, Expr *val, Expr *body);
  Expr *mk_fndef(UT::String param, Expr *body);
  Expr *mk_intdef(UT::String name, Expr *def);
  Expr *mk_pubdef(UT::String name, Expr *def);
};

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string pprint(ExprTag t, int level = 0);
std::string pprint(Expr *e, int level = 0);

} // namespace EX

#endif // EX_HEADER
