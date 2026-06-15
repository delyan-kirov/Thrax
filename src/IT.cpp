#include "IT.hpp"

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

pLm eval(pLm node, DynEnv denv, StatEnv &senv);

namespace
{

pLm
apply_builtin(
  const Var &v, DynEnv denv, StatEnv &senv)
{
  const auto &op   = v.unwrap;
  const auto &args = v.env;

  // Lazy: evaluate condition first, then only the taken branch
  if (op == "if")
  {
    UT_FAIL_IF(args.size() != 3);
    pLm cond = eval(args[0], denv, senv);
    UT_FAIL_IF(cond->tag != LTag::INT);
    if (std::get<Int>(cond->as).unwrap != 0)
      return eval(args[1], denv, senv);
    else
      return eval(args[2], denv, senv);
  }

  // Unary operators — evaluate the single operand
  if (op == "neg")
  {
    UT_FAIL_IF(args.size() != 1);
    pLm val = eval(args[0], denv, senv);
    UT_FAIL_IF(val->tag != LTag::INT);
    return std::make_shared<Lm>(
      Lm{ .tag = LTag::INT, .as = Int{ -std::get<Int>(val->as).unwrap } });
  }

  if (op == "not")
  {
    UT_FAIL_IF(args.size() != 1);
    pLm val = eval(args[0], denv, senv);
    UT_FAIL_IF(val->tag != LTag::INT);
    ssize_t v = std::get<Int>(val->as).unwrap;
    return std::make_shared<Lm>(
      Lm{ .tag = LTag::INT, .as = Int{ v == 0 ? 1 : 0 } });
  }

  // Binary operators — evaluate both operands
  UT_FAIL_IF(args.size() != 2);
  pLm lhs_node = eval(args[0], denv, senv);
  pLm rhs_node = eval(args[1], denv, senv);
  UT_FAIL_IF(lhs_node->tag != LTag::INT);
  UT_FAIL_IF(rhs_node->tag != LTag::INT);

  ssize_t lhs = std::get<Int>(lhs_node->as).unwrap;
  ssize_t rhs = std::get<Int>(rhs_node->as).unwrap;

  ssize_t result = 0;
  if (op == "+")       result = lhs + rhs;
  else if (op == "-")  result = lhs - rhs;
  else if (op == "*")  result = lhs * rhs;
  else if (op == "/")  { UT_FAIL_IF(rhs == 0); result = lhs / rhs; }
  else if (op == "%")  { UT_FAIL_IF(rhs == 0); result = lhs % rhs; }
  else if (op == "?=") result = (lhs == rhs) ? 1 : 0;
  else                 UT_FAIL_IF(true);

  return std::make_shared<Lm>(Lm{ .tag = LTag::INT, .as = Int{ result } });
}

} // namespace

pLm
exprs2pLm_helper(
  EX::Expr *expr, StatEnv &env)
{
  Lm lm;
  switch (expr->tag)
  {
  case EX::ExprTag::Binop:
  {
    auto       &b      = std::get<EX::ExBinop>(expr->as);
    const char *op_str = nullptr;
    switch (b.tag)
    {
    case EX::BinopTag::Add : op_str = "+";  break;
    case EX::BinopTag::Sub : op_str = "-";  break;
    case EX::BinopTag::Mul : op_str = "*";  break;
    case EX::BinopTag::Div : op_str = "/";  break;
    case EX::BinopTag::Mod : op_str = "%";  break;
    case EX::BinopTag::IsEq: op_str = "?="; break;
    }
    pLm lhs = exprs2pLm_helper(b.ops.begin(), env);
    pLm rhs = exprs2pLm_helper(b.ops.last(), env);
    lm      = { .tag = LTag::VAR,
                .as  = Var{ std::string{ op_str }, 0, { lhs, rhs } } };
  }
  break;

  case EX::ExprTag::Unop:
  {
    auto       &u      = std::get<EX::ExUnop>(expr->as);
    const char *op_str = (u.tag == EX::UnopTag::Neg) ? "neg" : "not";
    pLm         operand = exprs2pLm_helper(u.op, env);
    lm                  = { .tag = LTag::VAR,
                            .as  = Var{ std::string{ op_str }, 0, { operand } } };
  }
  break;

  case EX::ExprTag::If:
  {
    EX::ExIf &ifexpr = std::get<EX::ExIf>(expr->as);
    pLm     cond   = exprs2pLm_helper(ifexpr.cond, env);
    pLm     then   = exprs2pLm_helper(ifexpr.then, env);
    pLm     othw   = exprs2pLm_helper(ifexpr.alt, env);
    lm = { .tag = LTag::VAR, .as = Var{ "if", 0, { cond, then, othw } } };
  }
  break;

  case EX::ExprTag::Let:
  {
    auto &lt         = std::get<EX::ExLet>(expr->as);
    pLm continuation = exprs2pLm_helper(lt.body, env);
    pLm value        = exprs2pLm_helper(lt.val, env);
    Fun fn           = mkFun(lt.var, continuation);
    pLm fn_lm        = std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = fn });
    lm               = { .tag = LTag::APP, .as = App{ fn_lm, value } };
  }
  break;

  case EX::ExprTag::App:
  {
    auto &app = std::get<EX::ExApp>(expr->as);
    pLm  fn   = exprs2pLm_helper(app.fn, env);
    pLm  arg  = exprs2pLm_helper(app.arg, env);
    lm        = { .tag = LTag::APP, .as = App{ fn, arg } };
  }
  break;

  case EX::ExprTag::FnDef:
  {
    auto &fn = std::get<EX::ExFnDef>(expr->as);
    Fun   f  = mkFun(fn.param, exprs2pLm_helper(fn.body, env));
    lm       = { .tag = LTag::FUN, .as = f };
  }
  break;

  case EX::ExprTag::Str:
  {
    lm = { .tag = LTag::STR, .as = mkStr(std::get<EX::ExStr>(expr->as).value) };
  }
  break;

  case EX::ExprTag::Var:
  {
    lm = { .tag = LTag::VAR, .as = mkVar(std::get<EX::ExVar>(expr->as).name) };
  }
  break;

  case EX::ExprTag::Int:
  {
    lm = { .tag = LTag::INT, .as = Int{ std::get<EX::ExInt>(expr->as).value } };
  }
  break;

  case EX::ExprTag::PubDef:
  {
    auto       &gd   = std::get<EX::ExPubDef>(expr->as);
    std::string name = std::string{ gd.name.m_mem, gd.name.m_len };
    pLm         def  = exprs2pLm_helper(gd.def, env);
    NameStack   ns;
    assign_id(def, ns);
    env[name] = def;
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1, {} } };
  }
  break;

  case EX::ExprTag::IntDef:
  {
    auto       &gd   = std::get<EX::ExIntDef>(expr->as);
    std::string name = std::string{ gd.name.m_mem, gd.name.m_len };
    pLm         def  = exprs2pLm_helper(gd.def, env);
    NameStack   ns;
    assign_id(def, ns);
    env[name] = def;
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1, {} } };
  }
  break;

  case EX::ExprTag::Unknown:
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

pLm
eval(
  pLm node, DynEnv denv, StatEnv &senv)
{
  UT_FAIL_IF(!node);

  switch (node->tag)
  {
  case LTag::INT:
  case LTag::STR:
    return node;

  case LTag::VAR:
  {
    auto &v = std::get<Var>(node->as);

    // Builtin operator or if-expression
    if (!v.env.empty()) return apply_builtin(v, denv, senv);

    // Global definition placeholder
    if (v.idx == (size_t)-1) return node;

    // Local variable (De Bruijn indexed)
    if (v.idx > 0)
    {
      UT_FAIL_IF(v.idx > denv.size());
      return denv[denv.size() - v.idx];
    }

    // Global variable (idx == 0, env empty)
    auto it = senv.find(v.unwrap);
    UT_FAIL_IF(it == senv.end());
    return eval(it->second, {}, senv);
  }

  case LTag::FUN:
  {
    auto &f = std::get<Fun>(node->as);
    Fun   closure;
    closure.var  = Var{ f.var.unwrap, f.var.idx, denv };
    closure.body = f.body;
    return std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = closure });
  }

  case LTag::APP:
  {
    auto &a = std::get<App>(node->as);

    pLm closure = eval(a.fn, denv, senv);
    UT_FAIL_IF(closure->tag != LTag::FUN);

    pLm val = eval(a.arg, denv, senv);

    auto  &fun     = std::get<Fun>(closure->as);
    DynEnv new_env = fun.var.env;
    new_env.push_back(val);
    return eval(fun.body, new_env, senv);
  }

  case LTag::UNK:
    return node;
  }

  return node;
}

} // namespace IT
