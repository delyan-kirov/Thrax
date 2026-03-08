/*-------------------------------------------------------------------------------
 *\file XX.hpp
 *\info Header file for Parser
 * *----------------------------------------------------------------------------*/

#ifndef EX_HEADER
#define EX_HEADER

#include "LX.hpp"
#include "UT.hpp"

namespace EX
{

/*------------------------------------------------------------------------------
 *\TYPES
 *-----------------------------------------------------------------------------*/

#define EX_Type_EnumVariants                                                   \
  X(Unknown)                                                                   \
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
  X(While)                                                                     \
  X(Str)

enum class Type
{
#define X(X_enum) X_enum,
  EX_Type_EnumVariants
#undef X
};

struct Expr;
using Exprs = UT::Vec<Expr>;

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
  // TODO: use UT::String, not const char*
public:
  AR::Arena       &m_arena;
  ER::Events       m_events;
  const char      *m_input;
  const LX::Tokens m_tokens;
  size_t           m_begin;
  size_t           m_end;
  Exprs            m_exprs;

  Parser(LX::Lexer l);

  Parser(LX::Tokens tokens, AR::Arena &arena, const char *input);

  Parser(EX::Parser old, size_t begin, size_t end);

  Parser(EX::Parser &parent_parser, LX::Tokens &t);

  E run();

  E operator()();

  E parse_binop(EX::Type type, size_t start, size_t end);

  E parse_max_precedence_arithmetic_op(EX::Type, size_t &idx);

  E parse_min_precedence_arithmetic_op(EX::Type, size_t &idx);

  bool match_token_type(size_t start, const LX::Type type);

  template <typename... Args>
  bool
  match_token_type(
    size_t start, Args &&...args)
  {
    static_assert((std::is_same_v<std::decay_t<Args>, LX::Type> && ...),
                  "[TYPE-ERROR] All extra arguments must be LX::Type");
    return (... || this->match_token_type(start, args));
  }
};

} // namespace EX

/*-------------------------------------------------------------------------------
 *\UTILS
 *------------------------------------------------------------------------------*/

namespace std
{

inline string
to_string(
  EX::Type expr_type)
{
  switch (expr_type)
  {
#define X(X_enum)                                                              \
  case EX::Type::X_enum: return #X_enum;
    EX_Type_EnumVariants
  }
#undef X

  UT_FAIL_IF("UNREACHABLE");
}

inline string to_string(EX::FnDef fndef);

inline string
to_string(
  EX::Expr expr)
{
  string s{ "" };

  switch (expr.m_type)
  {
  case EX::Type::Int:
  {
    s = std::to_string(expr.as.m_int);
  }
  break;
  case EX::Type::Add:
  {
    s += "(";
    s += to_string(expr.as.m_pair.first());
    s += " + ";
    s += to_string(expr.as.m_pair.second());
    s += ")";
  }
  break;
  case EX::Type::Minus:
  {
    s += "-(";
    s += to_string(*expr.as.m_expr);
    s += ")";
  }
  break;
  case EX::Type::Sub:
  {
    s += "(";
    s += to_string(expr.as.m_pair.first());
    s += " - ";
    s += to_string(expr.as.m_pair.second());
    s += ")";
  }
  break;
  case EX::Type::Mult:
  {
    s += "(";
    s += to_string(expr.as.m_pair.first());
    s += " * ";
    s += to_string(expr.as.m_pair.second());
    s += ")";
  }
  break;
  case EX::Type::Div:
  {
    s += "(";
    s += to_string(expr.as.m_pair.first());
    s += " / ";
    s += to_string(expr.as.m_pair.second());
    s += ")";
  }
  break;
  case EX::Type::Modulus:
  {
    s += "(";
    s += to_string(expr.as.m_pair.first());
    s += " % ";
    s += to_string(expr.as.m_pair.second());
    s += ")";
  }
  break;
  case EX::Type::IsEq:
  {
    s += "(";
    s += to_string(expr.as.m_pair.first());
    s += " ?= ";
    s += to_string(expr.as.m_pair.second());
    s += ")";
  }
  break;
  case EX::Type::FnDef:
  {
    s += "( \\" + to_string(expr.as.m_fn.m_param) + " = "
         + to_string(*expr.as.m_fn.m_body) + " )";
  }
  break;
  case EX::Type::FnApp:
  {
    s += to_string(expr.as.m_fnapp.m_body) + " (" + " ";
    for (size_t i = 0; i < expr.as.m_fnapp.m_param.m_len; ++i)
    {
      auto &param = expr.as.m_fnapp.m_param[i];
      if (i != expr.as.m_fnapp.m_param.m_len - 1)
      {
        s += to_string(param) + ", ";
      }
      else
      {
        s += to_string(param);
      }
    }
    s += " )";
  }
  break;
  case EX::Type::VarApp:
  {
    s += to_string(expr.as.m_varapp.m_fn_name) + " (" + " "
         + to_string(*expr.as.m_varapp.m_param.last()) + " )";
  }
  break;
  case EX::Type::Var:
  {
    s += "Var (" + std::to_string(expr.as.m_var) + ")";
  }
  break;
  case EX::Type::If:
  {
    s += "if " + std::to_string(*expr.as.m_if.m_condition) +    //
         " => " + std::to_string(*expr.as.m_if.m_true_branch) + //
         " else " + std::to_string(*expr.as.m_if.m_else_branch);
  }
  break;
  case EX::Type::Unknown:
  {
    s += "EX::T::Unknown";
  }
  break;
  case EX::Type::Let:
  {
    s += "let " + to_string(expr.as.m_let.m_var_name) + " = "
         + to_string(*expr.as.m_let.m_value) + " in "
         + to_string(*expr.as.m_let.m_continuation);
  }
  break;
  case EX::Type::Not:
  {
    s += "neg ( " + to_string(*expr.as.m_expr) + " )";
  }
  break;
  case EX::Type::Str:
  {
    s += "\"" + to_string(expr.as.m_string) + "\"";
  }
  break;
  default:
  {
    // TODO: Don't use default case here, fail under switch
    UT_FAIL_MSG("UNREACHABLE %d", expr.m_type);
  }
  break;
  }

  return s;
}
inline string
to_string(
  EX::FnDef fndef)
{
  return "(\\" + to_string(fndef.m_param) + " = " + to_string(*fndef.m_body)
         + ")";
}
} // namespace std

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // EX_HEADER
