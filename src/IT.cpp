#include "IT.hpp"
#include <cstdio>
#include <memory>
#include <string>

namespace IT
{

namespace
{

Int
mkInt(
  ssize_t n)
{
  return Int{ n };
}

Var
mkVar(
  UT::String s, size_t idx = (size_t)-1)
{
  return Var{ std::string{ s.m_mem, s.m_len }, idx };
}

Var
mkVar(
  const char *s, size_t idx = (size_t)-1)
{
  return Var{ std::string{ s }, idx };
}

Str
mkStr(
  UT::String s)
{
  std::string ss = { s.m_mem, s.m_len };
  return Str{ ss };
}

template <typename S>
Fun
mkFun(
  S s, pLm body)
{
  return Fun{ mkVar(s), body };
}

App
mkApp(
  pLm fn, pLm arg)
{
  return App{ fn, arg };
}

Ifc
mkIfc(
  pLm cond, pLm then, pLm othw)
{
  return Ifc{ cond, then, othw };
}

} // namespace

pLm
exprs2pLm(
  EX::Expr *expr, StatEnv &env)
{
  Lm lm;
  switch (expr->m_type)
  {
  case EX::Type::Add:
  case EX::Type::Div:
  case EX::Type::Modulus:
  case EX::Type::Sub:
  case EX::Type::Mult:
  case EX::Type::IsEq:
  {
    const char *op_str;
    switch (expr->m_type)
    {
    case EX::Type::Add    : op_str = "+"; break;
    case EX::Type::Sub    : op_str = "-"; break;
    case EX::Type::Mult   : op_str = "*"; break;
    case EX::Type::Div    : op_str = "/"; break;
    case EX::Type::Modulus: op_str = "%"; break;
    case EX::Type::IsEq   : op_str = "?="; break;
    default               : op_str = "?"; break;
    }
    pLm lhs = exprs2pLm(expr->as.m_pair.begin(), env);
    pLm rhs = exprs2pLm(expr->as.m_pair.last(), env);
    pLm op  = std::make_shared<Lm>(Lm{ .tag = LTag::VAR, .as = mkVar(op_str) });
    pLm inner
      = std::make_shared<Lm>(Lm{ .tag = LTag::APP, .as = mkApp(op, lhs) });
    lm = { .tag = LTag::APP, .as = mkApp(inner, rhs) };
  }
  break;

  case EX::Type::Minus:
  case EX::Type::Not:
  {
    const char *op_str  = (expr->m_type == EX::Type::Minus) ? "neg" : "not";
    pLm         operand = exprs2pLm(expr->as.m_expr, env);
    pLm         op      = std::make_shared<Lm>(
      Lm{ .tag = LTag::VAR,
                       .as  = mkVar(UT::String{ op_str, std::strlen(op_str) }) });
    lm = { .tag = LTag::APP, .as = mkApp(op, operand) };
  }
  break;

  case EX::Type::If:
  {
    EX::If ifexpr = expr->as.m_if;
    Ifc    ifc    = mkIfc(exprs2pLm(ifexpr.m_condition, env),
                    exprs2pLm(ifexpr.m_true_branch, env),
                    exprs2pLm(ifexpr.m_else_branch, env));
    lm            = { .tag = LTag::IFC, .as = ifc };
  }
  break;

  case EX::Type::Let:
  {
    pLm continuation = exprs2pLm(expr->as.m_let.m_continuation, env);
    pLm value        = exprs2pLm(expr->as.m_let.m_value, env);
    Fun fn           = mkFun(expr->as.m_let.m_var_name, continuation);
    pLm fn_lm        = std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = fn });
    lm               = { .tag = LTag::APP, .as = mkApp(fn_lm, value) };
  }
  break;

  case EX::Type::FnApp:
  {
    // FIXME: FnApp should just be a pair of expressions in my opinion
    Fun fn     = mkFun(expr->as.m_fnapp.m_body.m_param,
                   exprs2pLm(expr->as.m_fnapp.m_body.m_body, env));
    pLm result = std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = fn });
    for (size_t i = 0; i < expr->as.m_fnapp.m_param.m_len; ++i)
    {
      result = std::make_shared<Lm>(Lm{
        .tag = LTag::APP,
        .as  = mkApp(result, exprs2pLm(&expr->as.m_fnapp.m_param[i], env)) });
    }
    lm = *result;
  }
  break;

  case EX::Type::FnDef:
  {
    Fun fn = mkFun(expr->as.m_fn.m_param, exprs2pLm(expr->as.m_fn.m_body, env));
    lm     = { .tag = LTag::FUN, .as = fn };
  }
  break;

  case EX::Type::Str:
  {
    lm = { .tag = LTag::STR, .as = mkStr(expr->as.m_string) };
  }
  break;

  case EX::Type::Var:
  {
    lm = { .tag = LTag::VAR, .as = mkVar(expr->as.m_var) };
  }
  break;

  case EX::Type::VarApp:
  {
    // FIXME, should spread out the args
    App app = mkApp(exprs2pLm(expr->as.m_fnapp.m_param.begin(), env),
                    exprs2pLm(expr->as.m_fnapp.m_body.m_body, env));
    lm      = { .tag = LTag::APP, .as = app };
  }
  break;

  case EX::Type::Int:
  {
    lm = { .tag = LTag::INT, .as = mkInt(expr->as.m_int) };
  }
  break;

  case EX::Type::PubDef:
  case EX::Type::IntDef:
  {
    std::string name
      = std::string{ expr->as.m_gdef.name.m_mem, expr->as.m_gdef.name.m_len };
    pLm def   = exprs2pLm(expr->as.m_gdef.def, env);
    env[name] = def;
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1 } };
  }
  break;

  case EX::Type::Unknown:
  {
    lm = { .tag = LTag::UNK, .as = std::monostate{} };
  }
  break;
  }

  return std::make_shared<Lm>(lm);
};

std::string
pprint(
  pLm lm, int level)
{
  if (!lm) return std::string(level * 2, ' ') + "(null)";

  std::string pad(level * 2, ' ');
  switch (lm->tag)
  {
  case LTag::INT:
  {
    auto &v = std::get<Int>(lm->as);
    return pad + std::to_string(v.unwrap);
  }
  case LTag::STR:
  {
    auto &v = std::get<Str>(lm->as);
    return pad + "\"" + v.unwrap + "\"";
  }
  case LTag::VAR:
  {
    auto &v = std::get<Var>(lm->as);
    if (v.idx == (size_t)-1) return pad + v.unwrap;
    return pad + v.unwrap + "[" + std::to_string(v.idx) + "]";
  }
  case LTag::APP:
  {
    auto &v = std::get<App>(lm->as);
    return pad + "(app\n"
         + pprint(v.fn, level + 1) + "\n"
         + pprint(v.arg, level + 1) + ")";
  }
  case LTag::FUN:
  {
    auto &v = std::get<Fun>(lm->as);
    return pad + "(fn " + v.var.unwrap + "\n"
         + pprint(v.body, level + 1) + ")";
  }
  case LTag::IFC:
  {
    auto &v = std::get<Ifc>(lm->as);
    return pad + "(if\n"
         + pprint(v.cond, level + 1) + "\n"
         + pprint(v.then, level + 1) + "\n"
         + pprint(v.othw, level + 1) + ")";
  }
  case LTag::UNK: return pad + "?unknown";
  }
  return pad + "?unreachable";
}

} // namespace IT
