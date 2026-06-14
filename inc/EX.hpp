/*-------------------------------------------------------------------------------
 *\file XX.hpp
 *\info Header file for Parser
 * *----------------------------------------------------------------------------*/

#ifndef EX_HEADER
#define EX_HEADER

#include "LX.hpp"
#include "UT.hpp"
#include <variant>

namespace EX
{

/*------------------------------------------------------------------------------
 *\TYPES
 *-----------------------------------------------------------------------------*/

#define EX_Type_EnumVariants                                                   \
  X(Unknown)                                                                   \
  X(PubDef)                                                                    \
  X(IntDef)                                                                    \
  X(Int)                                                                       \
  X(Minus)                                                                     \
  X(Add)                                                                       \
  X(Sub)                                                                       \
  X(Mult)                                                                      \
  X(Div)                                                                       \
  X(Modulus)                                                                   \
  X(Let)                                                                       \
  X(IsEq)                                                                      \
  X(FnDef)                                                                     \
  X(FnApp)                                                                     \
  X(VarApp)                                                                    \
  X(Var)                                                                       \
  X(If)                                                                        \
  X(Not)                                                                       \
  X(Str)

enum class Type
{
#define X(X_enum) X_enum,
  EX_Type_EnumVariants
#undef X
};

struct Expr;
using Exprs = UT::Vec<Expr>;

struct Gdef
{
  UT::String name;
  Expr      *def;
};

struct FnDef
{
  UT::String m_param;
  Expr      *m_body;

  FnDef() = default;
  FnDef(UT::String param, AR::Arena &arena);
};

struct If
{
  Expr *m_condition;
  Expr *m_true_branch;
  Expr *m_else_branch;
};

enum class E
{
  MIN = -1,
  OK,
  MAX
};

// FIXME: This should be Expr and Exprs
struct FnApp
{
  FnDef m_body;
  Exprs m_param;
};

struct VarApp
{
  UT::String m_fn_name;
  Exprs      m_param;
};

struct Let
{
  UT::String m_var_name;
  Expr      *m_value;
  Expr      *m_continuation;
};

struct While
{
  Expr *m_condition;
  Expr *m_body;
};

struct Expr
{
  Type m_type;
  union
  {
    Gdef           m_gdef;
    FnDef          m_fn;
    FnApp          m_fnapp;
    VarApp         m_varapp;
    UT::String     m_var;
    UT::String     m_string;
    UT::Pair<Expr> m_pair;
    Expr          *m_expr;
    ssize_t        m_int = 0;
    If             m_if;
    Let            m_let;
    While          m_while;
  } as;

  Expr() = default;
  Expr(Type type);
  Expr(Type type, AR::Arena &arena);
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

  Parser(EX::Parser &parent_parser, LX::Tokens &t);

  E run();

  E operator()();

  E parse_binop(EX::Type type, size_t start, size_t end);

  E parse_max_precedence_arithmetic_op(EX::Type, size_t &idx);

  E parse_min_precedence_arithmetic_op(EX::Type, size_t &idx);

  bool match_token_type(size_t start, const LX::TokenTag type);

  template <typename... Args>
  bool
  match_token_type(
    size_t start, Args &&...args)
  {
    static_assert((std::is_same_v<std::decay_t<Args>, LX::TokenTag> && ...),
                  "[TYPE-ERROR] All extra arguments must be LX::TokenTag");
    return (... || this->match_token_type(start, args));
  }
};

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string pprint(Type t, int level = 0);
std::string pprint(Expr *e, int level = 0);

} // namespace EX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // EX_HEADER
