/*-------------------------------------------------------------------------------
 *\file EX.cpp
 *\info Type and Precedence parsing
 * *----------------------------------------------------------------------------*/

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "EX.hpp"
#include "LX.hpp"
#include "UT.hpp"
#include <map>
#include <vector>

namespace EX
{

/*-------------------------------------------------------------------------------
 *\OPERATOR TABLE
 *------------------------------------------------------------------------------*/

namespace
{

enum class OpTag
{
  App,
  Add,
  Sub,
  Mul,
  Div,
  Mod,
  IsEq,
  Neg,
  Not
};

struct OpEntry
{
  int  prec;
  bool right_assoc;
};

using OpInfoDb = std::map<OpTag, OpEntry>;
using ValStack = std::vector<Expr *>;
using OpStack  = std::vector<OpTag>;

const OpInfoDb op_info_db{
  { OpTag::App, { 5, false } },  { OpTag::Neg, { 4, true } },
  { OpTag::Not, { 4, true } },   { OpTag::Mul, { 3, false } },
  { OpTag::Div, { 3, false } },  { OpTag::Mod, { 3, false } },
  { OpTag::Add, { 2, false } },  { OpTag::Sub, { 2, false } },
  { OpTag::IsEq, { 1, false } },
};

OpEntry
op_lookup(
  OpTag op)
{
  auto result = op_info_db.find(op);
  UT_FAIL_IF(op_info_db.end() == result);
  return result->second;
}

class ShuntState
{
public:
  ValStack   m_vals;
  OpStack    m_op_stk;
  bool       m_last_was_val = false;
  AR::Arena &m_arena;

  ShuntState(
    AR::Arena &arena)
      : m_arena{ arena }
  {
  }

  Expr *alloc(Expr e);
  void  combine();
  void  push_op(OpTag op);
  void  push_val(Expr e);
};

Expr *
ShuntState::alloc(
  Expr e)
{
  Expr *p = (Expr *)m_arena.alloc<Expr>(1);
  *p      = e;
  return p;
}

void
ShuntState::combine()
{
  UT_FAIL_IF(m_op_stk.empty());
  OpTag op = m_op_stk.back();
  m_op_stk.pop_back();

  if (op == OpTag::App)
  {
    UT_FAIL_IF(m_vals.size() < 2);
    Expr *arg = m_vals.back();
    m_vals.pop_back();
    Expr *fn = m_vals.back();
    m_vals.pop_back();
    Expr e{ ExprTag::App };
    e.as = ExApp{ fn, arg };
    m_vals.push_back(alloc(e));
  }
  else if (op == OpTag::Neg || op == OpTag::Not)
  {
    UT_FAIL_IF(m_vals.empty());
    Expr *operand = m_vals.back();
    m_vals.pop_back();
    Expr e{ ExprTag::Unop };
    e.as = ExUnop{ op == OpTag::Neg ? UnopTag::Neg : UnopTag::Not, operand };
    m_vals.push_back(alloc(e));
  }
  else
  {
    UT_FAIL_IF(m_vals.size() < 2);
    Expr *rhs = m_vals.back();
    m_vals.pop_back();
    Expr *lhs = m_vals.back();
    m_vals.pop_back();

    BinopTag btag;
    switch (op)
    {
    case OpTag::Add : btag = BinopTag::Add; break;
    case OpTag::Sub : btag = BinopTag::Sub; break;
    case OpTag::Mul : btag = BinopTag::Mul; break;
    case OpTag::Div : btag = BinopTag::Div; break;
    case OpTag::Mod : btag = BinopTag::Mod; break;
    case OpTag::IsEq: btag = BinopTag::IsEq; break;
    default         : UT_FAIL_IF("Not a binop"); btag = BinopTag::Add;
    }

    ExPair pair{ m_arena };
    *pair.begin() = *lhs;
    *pair.last()  = *rhs;
    Expr e{ ExprTag::Binop };
    e.as = ExBinop{ btag, pair };
    m_vals.push_back(alloc(e));
  }
}

void
ShuntState::push_op(
  OpTag op)
{
  OpEntry info = op_lookup(op);
  while (!m_op_stk.empty())
  {
    OpEntry top = op_lookup(m_op_stk.back());
    if (info.right_assoc ? top.prec > info.prec : top.prec >= info.prec)
      combine();
    else
      break;
  }
  m_op_stk.push_back(op);
}

void
ShuntState::push_val(
  Expr e)
{
  if (m_last_was_val) push_op(OpTag::App);
  m_vals.push_back(alloc(e));
  m_last_was_val = true;
}

} // namespace

/*-------------------------------------------------------------------------------
 *\IMPL (EX)
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

// TODO: candidate for removal
Parser::Parser(
  LX::Lexer l)
    : m_arena{ l.m_arena },
      m_events{ *l.m_events },
      m_input{ l.m_input },
      m_tokens{ std::move(l.m_tokens) },
      m_begin{ 0 },
      m_end{ 0 },
      m_exprs{ l.m_arena }
{
  this->m_end = this->m_tokens.m_len;
};

Parser::Parser(LX::Tokens tokens, AR::Arena &arena, const UT::String input)
    : m_arena{ arena },
      m_events{ arena },
      m_input{ input },
      m_tokens{ tokens },
      m_begin{ 0 },
      m_end{ tokens.m_len },
      m_exprs{ arena } {};

Parser::Parser(EX::Parser old, size_t begin, size_t end)
    : m_arena{ old.m_arena },   //
      m_events{ old.m_arena },  //
      m_input{ old.m_input },   //
      m_tokens{ old.m_tokens }, //
      m_begin{ begin },         //
      m_end{ end },             //
      m_exprs{ old.m_arena }    //
{};

Parser::Parser(EX::Parser &parent_parser, const LX::Tokens &t)
    : m_arena{ parent_parser.m_arena },  //
      m_events{ parent_parser.m_arena }, //
      m_input{ parent_parser.m_input },  //
      m_tokens{ t },                     //
      m_begin{ 0 },                      //
      m_end{ t.m_len },                  //
      m_exprs{ parent_parser.m_arena }   //
{};

E
Parser::operator()()
{
  return this->run();
}

E
Parser::run()
{
  ShuntState s{ m_arena };
  E          result = E::OK;

  for (const LX::Token &t : m_tokens) switch (t.tag)
    {
    case LX::TokenTag::Int:
    {
      Expr e{ ExprTag::Int };
      e.as = ExInt{ std::get<LX::TkInt>(t.as).value };
      s.push_val(e);
    }
    break;
    case LX::TokenTag::Str:
    {
      Expr e{ ExprTag::Str };
      e.as = ExStr{ std::get<LX::TkStr>(t.as).value };
      s.push_val(e);
    }
    break;
    case LX::TokenTag::Word:
    {
      Expr e{ ExprTag::Var };
      e.as = ExVar{ std::get<LX::TkWord>(t.as).value };
      s.push_val(e);
    }
    break;
    case LX::TokenTag::Group:
    {
      Parser group_parser{ *this, std::get<LX::TkGroup>(t.as).tokens };
      result = group_parser();
      s.push_val(*group_parser.m_exprs.last());
    }
    break;
    case LX::TokenTag::Let:
    {
      auto  &bnd = std::get<LX::TkLet>(t.as);
      Parser vp{ *this, bnd.equals };
      vp();
      Parser cp{ *this, bnd.in };
      cp();
      Expr e{ ExprTag::Let };
      e.as = ExLet{ bnd.var, vp.m_exprs.last(), cp.m_exprs.last() };
      s.push_val(e);
    }
    break;
    case LX::TokenTag::Fn:
    {
      auto  &lxfn = std::get<LX::TkFn>(t.as);
      Parser bp{ *this, lxfn.body };
      bp();
      ExFnDef fn_def{ lxfn.param_name, m_arena };
      *fn_def.body = *bp.m_exprs.last();
      Expr e{ ExprTag::FnDef };
      e.as = fn_def;
      s.push_val(e);
    }
    break;
    case LX::TokenTag::If:
    {
      auto  &ie = std::get<LX::TkIf>(t.as);
      Parser cp{ *this, ie.condition };
      cp();
      Parser tp{ *this, ie.true_branch };
      tp();
      Parser ep{ *this, ie.else_branch };
      ep();
      Expr e{ ExprTag::If, m_arena };
      *std::get<ExIf>(e.as).cond = *cp.m_exprs.last();
      *std::get<ExIf>(e.as).then = *tp.m_exprs.last();
      *std::get<ExIf>(e.as).alt  = *ep.m_exprs.last();
      s.push_val(e);
    }
    break;
    case LX::TokenTag::IntDef:
    {
      auto  &sd = std::get<LX::TkIntDef>(t.as);
      Parser sp{ *this, sd.def };
      UT_FAIL_IF(E::OK != sp.run());
      Expr e{ ExprTag::IntDef };
      e.as = ExIntDef{ sd.name, sp.m_exprs.last() };
      s.push_val(e);
    }
    break;
    case LX::TokenTag::PubDef:
    {
      auto  &sd = std::get<LX::TkPubDef>(t.as);
      Parser sp{ *this, sd.def };
      UT_FAIL_IF(E::OK != sp.run());
      Expr e{ ExprTag::PubDef };
      e.as = ExPubDef{ sd.name, sp.m_exprs.last() };
      s.push_val(e);
    }
    break;
    case LX::TokenTag::Plus:
    {
      s.m_last_was_val = false;
      s.push_op(OpTag::Add);
    }
    break;
    case LX::TokenTag::Mult:
    {
      s.m_last_was_val = false;
      s.push_op(OpTag::Mul);
    }
    break;
    case LX::TokenTag::Div:
    {
      s.m_last_was_val = false;
      s.push_op(OpTag::Div);
    }
    break;
    case LX::TokenTag::Modulus:
    {
      s.m_last_was_val = false;
      s.push_op(OpTag::Mod);
    }
    break;
    case LX::TokenTag::IsEq:
    {
      s.m_last_was_val = false;
      s.push_op(OpTag::IsEq);
    }
    break;
    case LX::TokenTag::Minus:
    {
      if (!s.m_last_was_val)
        s.push_op(OpTag::Neg);
      else
      {
        s.m_last_was_val = false;
        s.push_op(OpTag::Sub);
      }
    }
    break;
    case LX::TokenTag::Not:
    {
      s.m_last_was_val = false;
      s.push_op(OpTag::Not);
    }
    break;
    case LX::TokenTag::Min:
    case LX::TokenTag::Max:
    default:
      UT_FAIL_MSG("The token type <%s> is unhandled",
                  LX::pprint(t.tag).c_str());
      break;
    }

  while (!s.m_op_stk.empty()) s.combine();
  if (!s.m_vals.empty()) m_exprs.push(*s.m_vals.back());

  return result;
}

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

namespace
{
const char *
binop_str(
  BinopTag op)
{
  switch (op)
  {
  case BinopTag::Add : return " + ";
  case BinopTag::Sub : return " - ";
  case BinopTag::Mul : return " * ";
  case BinopTag::Div : return " / ";
  case BinopTag::Mod : return " % ";
  case BinopTag::IsEq: return " ?= ";
  }
  UT_FAIL_IF("Not a binop");
  return "";
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

} // namespace EX
