/*-------------------------------------------------------------------------------
 *\file EX.cpp
 *\info Pratt parser implementation.
 * *----------------------------------------------------------------------------*/

#include "EX.hpp"
#include "ER.hpp"
#include "LX.hpp"
#include "UT.hpp"

#include <unordered_map>
#include <unordered_set>

/*------------------------------------------------------------------------------
 *\MACROS
 *
 * TRY / CTX / EX_ERR are the only error primitives. TRY propagates a failure
 * unchanged; CTX propagates it but first appends a context frame anchored at a
 * token; EX_ERR starts a fresh root cause. All three rely on ER::Fail
 * converting to whatever Result<T> the enclosing method returns. They use the
 * GNU statement-expression form so they can yield the unwrapped value inline.
 *-----------------------------------------------------------------------------*/

#define TRY(EXPR)                                                              \
  ({                                                                           \
    auto _r = (EXPR);                                                          \
    if (!_r.ok) return ER::Fail{ _r.err };                                     \
    _r.value;                                                                  \
  })

#define CTX(EXPR, TOK, ...)                                                    \
  ({                                                                           \
    auto _r = (EXPR);                                                          \
    if (!_r.ok)                                                                \
    {                                                                          \
      ER::push_ctx(m_arena,                                                    \
                   _r.err,                                                     \
                   (TOK).str,                                                  \
                   (TOK).line,                                                 \
                   ER::mk_msg(m_arena, __VA_ARGS__));                          \
      return ER::Fail{ _r.err };                                               \
    }                                                                          \
    _r.value;                                                                  \
  })

#define EX_ERR(CODE, TOK, ...)                                                 \
  return ER::Fail                                                              \
  {                                                                            \
    ER::mk_root(m_arena,                                                       \
                (CODE),                                                        \
                (TOK).str,                                                     \
                (TOK).line,                                                    \
                ER::mk_msg(m_arena, __VA_ARGS__))                              \
  }

namespace EX
{

/*-------------------------------------------------------------------------------
 *\PRECEDENCE
 *
 * (lbp, rbp) binding powers. A left-associative operator of precedence p gets
 * lbp = 10p, rbp = 10p + 1, so equal precedence folds left. Application
 * (juxtaposition) binds tightest; unary - and ! are prefix.
 *------------------------------------------------------------------------------*/

namespace
{

struct Bp
{
  int l;
  int r;
};

struct InfixInfo
{
  Bp       bp;    // binding powers
  BinopTag binop; // node this operator builds
};

using InfixTable = std::unordered_map<LX::TokenTag, InfixInfo>;
using UnopTable  = std::unordered_map<LX::TokenTag, UnopTag>;
using OperandSet = std::unordered_set<LX::TokenTag>;

const Bp  APP_BP{ 50, 51 };
const int PREFIX_BP = 40; // unary - and !

// Binary operators. Presence in this table is exactly "is an infix operator",
// and it carries both the binding power and the node to build.
const InfixTable infix_db{
  { LX::TokenTag::IsEq, { { 10, 11 }, BinopTag::IsEq } },
  { LX::TokenTag::Plus, { { 20, 21 }, BinopTag::Add } },
  { LX::TokenTag::Minus, { { 20, 21 }, BinopTag::Sub } },
  { LX::TokenTag::Mult, { { 30, 31 }, BinopTag::Mul } },
  { LX::TokenTag::Div, { { 30, 31 }, BinopTag::Div } },
  { LX::TokenTag::Modulus, { { 30, 31 }, BinopTag::Mod } },
};

// Tokens that can begin an operand -> juxtaposition is application.
const OperandSet operand_starters{
  LX::TokenTag::Int,    LX::TokenTag::Str,   LX::TokenTag::Word,
  LX::TokenTag::LParen, LX::TokenTag::KwLet, LX::TokenTag::KwIf,
  LX::TokenTag::Lambda,
};

// Prefix (unary) operators.
const UnopTable unop_db{
  { LX::TokenTag::Minus, UnopTag::Neg },
  { LX::TokenTag::Not, UnopTag::Not },
};

const char *
tok_desc(
  const LX::Token &t)
{
  return LX::TokenTag::Eof == t.tag ? "end of input" : nullptr;
}

} // namespace

/*-------------------------------------------------------------------------------
 *\EXPR CTORS 
 *------------------------------------------------------------------------------*/

ExFnDef::ExFnDef(
  UT::String param, AR::Arena &arena)
    : param{ param }
{
  this->body = (EX::Expr *)arena.alloc<EX::Expr>(1);
}

Expr::Expr(ExprTag tag)
    : tag{ tag } {};

Expr::Expr(
  ExprTag tag, AR::Arena &arena)
    : tag{ tag }
{
  switch (tag)
  {
  case ExprTag::FnDef:
  {
    ExFnDef fndef;
    fndef.body = (EX::Expr *)arena.alloc<EX::Expr>(1);
    this->as   = fndef;
  }
  break;
  case ExprTag::If:
  {
    this->as = ExIf{ (Expr *)arena.alloc<Expr>(1),
                     (Expr *)arena.alloc<Expr>(1),
                     (Expr *)arena.alloc<Expr>(1) };
  }
  break;
  case ExprTag::Binop:
    this->as = ExBinop{ BinopTag::Add, ExPair{ arena } };
    break;
  case ExprTag::Unop:
    this->as = ExUnop{ UnopTag::Neg, (Expr *)arena.alloc<Expr>(1) };
    break;
  default: UT_FAIL_IF("Invalid tag for this constructor");
  }
};

/*-------------------------------------------------------------------------------
 *\NODE BUILDERS
 *------------------------------------------------------------------------------*/

Expr *
Parser::alloc(
  Expr e)
{
  Expr *p = (Expr *)m_arena.alloc<Expr>(1);
  *p      = e;
  return p;
}

Expr *
Parser::mk_int(
  const LX::Token &t)
{
  Expr e{ ExprTag::Int };
  e.as = ExInt{ std::get<LX::TkInt>(t.as).value };
  return alloc(e);
}

Expr *
Parser::mk_str(
  const LX::Token &t)
{
  Expr e{ ExprTag::Str };
  e.as = ExStr{ std::get<LX::TkStr>(t.as).value };
  return alloc(e);
}

Expr *
Parser::mk_var(
  const LX::Token &t)
{
  Expr e{ ExprTag::Var };
  e.as = ExVar{ t.str };
  return alloc(e);
}

Expr *
Parser::mk_app(
  Expr *fn, Expr *arg)
{
  Expr e{ ExprTag::App };
  e.as = ExApp{ fn, arg };
  return alloc(e);
}

Expr *
Parser::mk_unop(
  LX::TokenTag op, Expr *operand)
{
  Expr e{ ExprTag::Unop };
  e.as = ExUnop{ UT::lookup(unop_db, op), operand };
  return alloc(e);
}

Expr *
Parser::mk_binop(
  BinopTag bt, Expr *lhs, Expr *rhs)
{
  ExPair pair{ m_arena };
  *pair.begin() = *lhs;
  *pair.last()  = *rhs;
  Expr e{ ExprTag::Binop };
  e.as = ExBinop{ bt, pair };
  return alloc(e);
}

Expr *
Parser::mk_if(
  Expr *cond, Expr *then, Expr *alt)
{
  Expr e{ ExprTag::If };
  e.as = ExIf{ cond, then, alt };
  return alloc(e);
}

Expr *
Parser::mk_let(
  UT::String var, Expr *val, Expr *body)
{
  Expr e{ ExprTag::Let };
  e.as = ExLet{ var, val, body };
  return alloc(e);
}

Expr *
Parser::mk_fndef(
  UT::String param, Expr *body)
{
  ExFnDef fn;
  fn.param = param;
  fn.body  = body;
  Expr e{ ExprTag::FnDef };
  e.as = fn;
  return alloc(e);
}

Expr *
Parser::mk_intdef(
  UT::String name, Expr *def)
{
  Expr e{ ExprTag::IntDef };
  e.as = ExIntDef{ name, def };
  return alloc(e);
}

Expr *
Parser::mk_pubdef(
  UT::String name, Expr *def)
{
  Expr e{ ExprTag::PubDef };
  e.as = ExPubDef{ name, def };
  return alloc(e);
}

/*-------------------------------------------------------------------------------
 *\PARSE
 *------------------------------------------------------------------------------*/

LX::R
Parser::expect(
  LX::TokenTag tag, const char *what)
{
  LX::Token t = TRY(m_lex.peek());
  if (t.tag != tag)
  {
    EX_ERR(ER::Code::UNEXPECTED_TOKEN, t, "%s", what);
  }
  LX::Token consumed = TRY(m_lex.next());
  return { true, consumed, {} };
}

R
Parser::parse_primary()
{
  LX::Token t = TRY(m_lex.peek());
  switch (t.tag)
  {
  case LX::TokenTag::Int   : m_lex.next(); return { true, mk_int(t), {} };
  case LX::TokenTag::Str   : m_lex.next(); return { true, mk_str(t), {} };
  case LX::TokenTag::Word  : m_lex.next(); return { true, mk_var(t), {} };
  case LX::TokenTag::LParen: return parse_group();
  case LX::TokenTag::KwLet : return parse_let();
  case LX::TokenTag::KwIf  : return parse_if();
  case LX::TokenTag::Lambda: return parse_closure();
  default:
  {
    const char *desc = tok_desc(t);
    if (desc)
      EX_ERR(ER::Code::EXPECTED_OPERAND,
             t,
             "expected an expression, found %s",
             desc);
    EX_ERR(ER::Code::EXPECTED_OPERAND,
           t,
           "expected an expression, found '%s'",
           std::to_string(t.str).c_str());
  }
  }
}

R
Parser::parse_prefix()
{
  LX::Token t = TRY(m_lex.peek());
  if (LX::TokenTag::Minus == t.tag || LX::TokenTag::Not == t.tag)
  {
    m_lex.next();
    Expr *operand = TRY(parse_expr(PREFIX_BP));
    return { true, mk_unop(t.tag, operand), {} };
  }
  return parse_primary();
}

R
Parser::parse_expr(
  int min_bp)
{
  Expr *lhs = TRY(parse_prefix());

  for (;;)
  {
    LX::Token t = TRY(m_lex.peek());

    if (const InfixInfo *infix = UT::try_lookup(infix_db, t.tag))
    {
      if (infix->bp.l < min_bp) break;
      TRY(m_lex.next()); // consume the operator
      Expr *rhs = TRY(parse_expr(infix->bp.r));
      lhs       = mk_binop(infix->binop, lhs, rhs);
    }
    else if (operand_starters.count(t.tag))
    {
      if (APP_BP.l < min_bp) break;
      Expr *rhs = TRY(parse_expr(APP_BP.r));
      lhs       = mk_app(lhs, rhs);
    }
    else
    {
      break;
    }
  }

  return { true, lhs, {} };
}

R
Parser::parse_group()
{
  LX::Token lp = TRY(m_lex.next()); // '('
  Expr     *e  = CTX(parse_expr(0), lp, "in this parenthesized group");

  LX::Token t = TRY(m_lex.peek());
  if (LX::TokenTag::RParen != t.tag)
  {
    EX_ERR(ER::Code::PARENTHESIS_UNBALANCED,
           lp,
           "expected a closing ')' to match this '('");
  }
  m_lex.next();
  return { true, e, {} };
}

R
Parser::parse_let()
{
  LX::Token kw = TRY(m_lex.next()); // 'let'
  LX::Token name
    = TRY(expect(LX::TokenTag::Word, "expected a variable name after 'let'"));
  TRY(expect(LX::TokenTag::Eq, "expected '=' after the 'let' variable"));

  Expr *val = CTX(parse_expr(0),
                  name,
                  "in the binding of 'let %s'",
                  std::to_string(name.str).c_str());

  TRY(expect(LX::TokenTag::KwIn, "expected 'in' after the 'let' binding"));

  Expr *body = CTX(parse_expr(0), kw, "in the body of this 'let'");
  return { true, mk_let(name.str, val, body), {} };
}

R
Parser::parse_if()
{
  LX::Token kw   = TRY(m_lex.next()); // 'if'
  Expr     *cond = CTX(parse_expr(0), kw, "in the condition of this 'if'");

  TRY(expect(LX::TokenTag::FatArrow, "expected '=>' after the 'if' condition"));

  Expr *then = CTX(parse_expr(0), kw, "in the then-branch of this 'if'");

  LX::Token els = TRY(
    expect(LX::TokenTag::KwElse, "expected 'else' after the then-branch"));

  Expr *alt = CTX(parse_expr(0), els, "in the else branch of this 'if'");
  return { true, mk_if(cond, then, alt), {} };
}

R
Parser::parse_closure()
{
  LX::Token bs = TRY(m_lex.next()); // '\'
  LX::Token nm
    = TRY(expect(LX::TokenTag::Word, "expected a parameter name after '\\'"));
  TRY(expect(LX::TokenTag::Eq, "expected '=' after the lambda parameter"));

  Expr *body = CTX(parse_expr(0), bs, "while parsing this closure");
  return { true, mk_fndef(nm.str, body), {} };
}

R
Parser::parse_global()
{
  LX::Token marker = TRY(m_lex.peek());

  bool is_pub;
  switch (marker.tag)
  {
  case LX::TokenTag::KwInt: is_pub = false; break;
  case LX::TokenTag::KwPub: is_pub = true; break;
  case LX::TokenTag::KwExt:
    EX_ERR(
      ER::Code::UNSUPPORTED, marker, "'ext' definitions are not supported yet");
  default:
  {
    const char *desc = tok_desc(marker);
    if (desc)
      EX_ERR(ER::Code::EXPECTED_GLOBAL,
             marker,
             "expected a global definition ('int' or 'pub'), found %s",
             desc);
    EX_ERR(ER::Code::EXPECTED_GLOBAL,
           marker,
           "expected a global definition ('int' or 'pub'), found '%s'",
           std::to_string(marker.str).c_str());
  }
  }
  m_lex.next(); // consume the marker

  LX::Token name = TRY(
    expect(LX::TokenTag::Word, "expected a name after the global marker"));
  TRY(expect(LX::TokenTag::Eq, "expected '=' after the global name"));

  Expr *body = CTX(parse_expr(0),
                   name,
                   "in the definition of global '%s'",
                   std::to_string(name.str).c_str());

  return { true,
           is_pub ? mk_pubdef(name.str, body) : mk_intdef(name.str, body),
           {} };
}

void
Parser::recover()
{
  for (;;)
  {
    LX::R pk = m_lex.peek();
    if (!pk.ok) return;
    LX::TokenTag tag = pk.value.tag;
    if (LX::TokenTag::Eof == tag || LX::TokenTag::KwInt == tag
        || LX::TokenTag::KwPub == tag || LX::TokenTag::KwExt == tag)
      return;
    if (!m_lex.next().ok) return;
  }
}

void
Parser::operator()()
{
  for (;;)
  {
    LX::R pk = m_lex.peek();
    if (!pk.ok)
    {
      m_diags.push_back(pk.err);
      return; // lexer error: cannot make progress, stop
    }
    if (LX::TokenTag::Eof == pk.value.tag) return;

    size_t before = m_lex.mark();
    R      r      = parse_global();
    if (r.ok)
    {
      m_exprs.push(*r.value);
      continue;
    }

    m_diags.push_back(r.err);
    recover();
    if (m_lex.mark() == before)
    {
      // No progress (e.g. an unrecoverable lexer error): bail out.
      if (!m_lex.next().ok) return;
    }
  }
}

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

namespace
{
using BinopNames = std::unordered_map<BinopTag, const char *>;

const BinopNames binop_str_db{
  { BinopTag::Add, " + " }, { BinopTag::Sub, " - " },
  { BinopTag::Mul, " * " }, { BinopTag::Div, " / " },
  { BinopTag::Mod, " % " }, { BinopTag::IsEq, " ?= " },
};

const char *
binop_str(
  BinopTag op)
{
  return UT::lookup(binop_str_db, op);
}
} // namespace

std::string
pprint(
  ExprTag t, int level)
{
  std::string pad(level * 2, ' ');
  switch (t)
  {
#define X(tag, type)                                                           \
  case ExprTag::tag: return pad + #tag;
    EX_EXPR_VARIANTS
#undef X
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

std::string
pprint(
  Expr *e, int level)
{
  if (!e) return std::string(level * 2, ' ') + "(null)";

  std::string pad(level * 2, ' ');
  switch (e->tag)
  {
  case ExprTag::Int: return pad + std::to_string(std::get<ExInt>(e->as).value);
  case ExprTag::Var: return pad + std::to_string(std::get<ExVar>(e->as).name);
  case ExprTag::Str:
    return pad + "\"" + std::to_string(std::get<ExStr>(e->as).value) + "\"";
  case ExprTag::Unop:
  {
    auto &u = std::get<ExUnop>(e->as);
    return pad + (u.tag == UnopTag::Neg ? "-" : "not ") + pprint(u.op, 0);
  }
  case ExprTag::Binop:
  {
    auto &b = std::get<ExBinop>(e->as);
    return pad + pprint(b.ops.begin(), 0) + binop_str(b.tag)
           + pprint(b.ops.last(), 0);
  }
  case ExprTag::Let:
    return pad + "let " + std::to_string(std::get<ExLet>(e->as).var) + " =\n"
           + pprint(std::get<ExLet>(e->as).val, level + 1) + "\n" + pad + "in\n"
           + pprint(std::get<ExLet>(e->as).body, level + 1);
  case ExprTag::If:
    return pad + "if " + pprint(std::get<ExIf>(e->as).cond, 0) + " then\n"
           + pprint(std::get<ExIf>(e->as).then, level + 1) + "\n" + pad
           + "else\n" + pprint(std::get<ExIf>(e->as).alt, level + 1);
  case ExprTag::FnDef:
    return pad + "\\" + std::to_string(std::get<ExFnDef>(e->as).param) + " =\n"
           + pprint(std::get<ExFnDef>(e->as).body, level + 1);
  case ExprTag::App:
  {
    auto &app = std::get<ExApp>(e->as);
    return pad + "(app\n" + pprint(app.fn, level + 1) + "\n"
           + pprint(app.arg, level + 1) + ")";
  }
  case ExprTag::PubDef:
    return pad + "pub " + std::to_string(std::get<ExPubDef>(e->as).name)
           + " =\n" + pprint(std::get<ExPubDef>(e->as).def, level + 1);
  case ExprTag::IntDef:
    return pad + "int " + std::to_string(std::get<ExIntDef>(e->as).name)
           + " =\n" + pprint(std::get<ExIntDef>(e->as).def, level + 1);
  case ExprTag::Unknown: return pad + "?unknown";
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

/*-------------------------------------------------------------------------------
 *\CTOR
 *------------------------------------------------------------------------------*/

Parser::Parser(
  LX::Lexer &lex)
    : m_arena{ lex.m_arena },
      m_lex{ lex },
      m_exprs{ lex.m_arena },
      m_diags{}
{
}

} // namespace EX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
