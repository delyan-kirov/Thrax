#include "IT.hpp"
#include <cstdio>
#include <memory>
#include <string>

namespace IT
{

namespace
{

Var
mkVar(
  UT::String s, size_t idx = 0)
{
  return Var{ std::string{ s.m_mem, s.m_len }, idx, {} };
}

Str
mkStr(
  UT::String s)
{
  std::string ss = { s.m_mem, s.m_len };
  return Str{ ss };
}

Fun
mkFun(
  UT::String s, pLm body)
{
  return Fun{ mkVar(s), body };
}

using NameStack = std::vector<std::string>;
void
assign_id(
  pLm node, NameStack &env)
{
  UT_FAIL_IF(!node);

  switch (node->tag)
  {
  case LTag::VAR:
  {
    auto &v = std::get<Var>(node->as);
    for (size_t i = env.size(); i > 0; --i)
    {
      if (env[i - 1] == v.unwrap)
      {
        v.idx = env.size() - (i - 1);
        break;
      }
    }
    for (auto &child : v.env) assign_id(child, env);
  }
  break;

  case LTag::FUN:
  {
    auto &v = std::get<Fun>(node->as);
    env.push_back(v.var.unwrap);
    assign_id(v.body, env);
    env.pop_back();
  }
  break;

  case LTag::APP:
  {
    auto &v = std::get<App>(node->as);
    assign_id(v.fn, env);
    assign_id(v.arg, env);
  }
  break;

  case LTag::INT:
  case LTag::STR:
  case LTag::UNK: break;
  }
}

} // namespace

pLm
exprs2pLm_helper(
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
    pLm lhs = exprs2pLm_helper(expr->as.m_pair.begin(), env);
    pLm rhs = exprs2pLm_helper(expr->as.m_pair.last(), env);
    lm      = { .tag = LTag::VAR,
                .as  = Var{ std::string{ op_str }, 0, { lhs, rhs } } };
  }
  break;

  case EX::Type::Minus:
  case EX::Type::Not:
  {
    const char *op_str  = (expr->m_type == EX::Type::Minus) ? "neg" : "not";
    pLm         operand = exprs2pLm_helper(expr->as.m_expr, env);
    lm                  = { .tag = LTag::VAR,
                            .as  = Var{ std::string{ op_str }, 0, { operand } } };
  }
  break;

  case EX::Type::If:
  {
    EX::If ifexpr = expr->as.m_if;
    pLm    cond   = exprs2pLm_helper(ifexpr.m_condition, env);
    pLm    then   = exprs2pLm_helper(ifexpr.m_true_branch, env);
    pLm    othw   = exprs2pLm_helper(ifexpr.m_else_branch, env);
    lm = { .tag = LTag::VAR, .as = Var{ "if", 0, { cond, then, othw } } };
  }
  break;

  case EX::Type::Let:
  {
    pLm continuation = exprs2pLm_helper(expr->as.m_let.m_continuation, env);
    pLm value        = exprs2pLm_helper(expr->as.m_let.m_value, env);
    Fun fn           = mkFun(expr->as.m_let.m_var_name, continuation);
    pLm fn_lm        = std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = fn });
    lm               = { .tag = LTag::APP, .as = App{ fn_lm, value } };
  }
  break;

  case EX::Type::FnApp:
  {
    // FIXME: FnApp should just be a pair of expressions in my opinion
    Fun fn     = mkFun(expr->as.m_fnapp.m_body.m_param,
                   exprs2pLm_helper(expr->as.m_fnapp.m_body.m_body, env));
    pLm result = std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = fn });
    for (size_t i = 0; i < expr->as.m_fnapp.m_param.m_len; ++i)
    {
      result = std::make_shared<Lm>(
        Lm{ .tag = LTag::APP,
            .as  = App{ result,
                       exprs2pLm_helper(&expr->as.m_fnapp.m_param[i], env) } });
    }
    lm = *result;
  }
  break;

  case EX::Type::FnDef:
  {
    Fun fn = mkFun(expr->as.m_fn.m_param,
                   exprs2pLm_helper(expr->as.m_fn.m_body, env));
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
    App app = App{ exprs2pLm_helper(expr->as.m_fnapp.m_param.begin(), env),
                   exprs2pLm_helper(expr->as.m_fnapp.m_body.m_body, env) };
    lm      = { .tag = LTag::APP, .as = app };
  }
  break;

  case EX::Type::Int:
  {
    lm = { .tag = LTag::INT, .as = Int{ expr->as.m_int } };
  }
  break;

  case EX::Type::PubDef:
  case EX::Type::IntDef:
  {
    std::string name
      = std::string{ expr->as.m_gdef.name.m_mem, expr->as.m_gdef.name.m_len };
    pLm       def = exprs2pLm_helper(expr->as.m_gdef.def, env);
    NameStack ns;
    assign_id(def, ns);
    env[name] = def;
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1, {} } };
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
    auto       &v = std::get<Var>(lm->as);
    std::string s = pad + v.unwrap;
    if (v.idx != (size_t)-1) s += "[" + std::to_string(v.idx) + "]";
    for (auto &child : v.env) s += "\n" + pprint(child, level + 1);
    return s;
  }
  case LTag::APP:
  {
    auto &v = std::get<App>(lm->as);
    return pad + "(app\n" + pprint(v.fn, level + 1) + "\n"
           + pprint(v.arg, level + 1) + ")";
  }
  case LTag::FUN:
  {
    auto &v = std::get<Fun>(lm->as);
    return pad + "(fn " + v.var.unwrap + "\n" + pprint(v.body, level + 1) + ")";
  }
  case LTag::UNK: return pad + "?unknown";
  }
  return pad + "?unreachable";
}

pLm
exprs2pLm(
  EX::Expr *expr, StatEnv &env)
{
  pLm       node = exprs2pLm_helper(expr, env);
  NameStack ns;
  assign_id(node, ns);
  return node;
}

} // namespace IT
