#include "IT.hpp"
#include "FF.hpp"

#include <cstdint>

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

  case LTag::LET:
  {
    auto &v = std::get<Let>(node->as);
    // the bound name is in scope for both the value (recursion) and the body
    env.push_back(v.var.unwrap);
    assign_id(v.val, env);
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

  case LTag::EXTERN:
  {
    auto &v = std::get<Extern>(node->as);
    for (auto &a : v.args) assign_id(a, env);
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

// A foreign type name as written in a signature. Type variables and function
// arguments are opaque, word-sized values, so they marshal as pointers.
std::string
ffi_type_name(
  EX::Ty *t)
{
  switch (t->tag)
  {
  case EX::TyTag::Con: return std::to_string(std::get<EX::TyCon>(t->as).name);
  case EX::TyTag::Var:
  case EX::TyTag::Arrow: return "Ptr";
  }
  return "Int";
}

// Split a signature `A -> B -> R` into argument types [A, B] and result R.
void
flatten_sig(
  EX::Ty *t, std::vector<std::string> &args, std::string &ret)
{
  while (t && EX::TyTag::Arrow == t->tag)
  {
    auto &ar = std::get<EX::TyArrow>(t->as);
    args.push_back(ffi_type_name(ar.from));
    t = ar.to;
  }
  ret = t ? ffi_type_name(t) : "Int";
}

// Marshal the accumulated arguments, perform the foreign call, and wrap the
// result back into an Lm (Str returns copy the C string; everything else is an
// integer-width / pointer word represented as Int).
pLm
call_extern(
  const Extern &e)
{
  std::vector<ssize_t> args;
  args.reserve(e.args.size());
  for (const pLm &a : e.args)
  {
    UT_FAIL_IF(!a);
    if (LTag::INT == a->tag)
    {
      args.push_back(std::get<Int>(a->as).unwrap);
    }
    else if (LTag::STR == a->tag)
    {
      const std::string &s = std::get<Str>(a->as).unwrap;
      args.push_back((ssize_t)(intptr_t)s.c_str());
    }
    else
    {
      UT_FAIL_MSG("FFI argument to '%s' must be Int or Str", e.symbol.c_str());
    }
  }

  ssize_t r = FF::call(UT::String{ e.lib.c_str(), e.lib.size() },
                       UT::String{ e.symbol.c_str(), e.symbol.size() },
                       e.arg_types, e.ret_type, args);

  if ("Str" == e.ret_type)
  {
    const char *p = (const char *)(intptr_t)r;
    return std::make_shared<Lm>(
      Lm{ .tag = LTag::STR, .as = Str{ p ? std::string(p) : std::string() } });
  }
  return std::make_shared<Lm>(Lm{ .tag = LTag::INT, .as = Int{ r } });
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
    auto &lt   = std::get<EX::ExLet>(expr->as);
    pLm   val  = exprs2pLm_helper(lt.val, env);
    pLm   body = exprs2pLm_helper(lt.body, env);
    lm         = { .tag = LTag::LET, .as = Let{ mkVar(lt.var), val, body } };
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

  case EX::ExprTag::Def:
  {
    auto       &gd   = std::get<EX::ExDef>(expr->as);
    std::string name = std::string{ gd.name.m_mem, gd.name.m_len };

    pLm def;
    if (EX::ExprTag::Extern == gd.def->tag)
    {
      // Foreign binding: the call types come from the signature, not the body.
      auto  &ex = std::get<EX::ExExtern>(gd.def->as);
      Extern e;
      e.symbol = std::string{ ex.symbol.m_mem, ex.symbol.m_len };
      e.lib    = std::string{ ex.lib.m_mem, ex.lib.m_len };
      if (gd.sig)
        flatten_sig(gd.sig, e.arg_types, e.ret_type);
      else
        e.ret_type = "Int";
      def = std::make_shared<Lm>(Lm{ .tag = LTag::EXTERN, .as = e });
    }
    else
    {
      def = exprs2pLm_helper(gd.def, env);
    }

    NameStack ns;
    assign_id(def, ns);
    env[name] = def;
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1, {} } };
  }
  break;

  case EX::ExprTag::Extern:
    UT_FAIL_MSG("%s", "@extern is only valid as a global definition");
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
  case LTag::LET:
  {
    auto &v = std::get<Let>(lm->as);
    return pad + "(let " + v.var.unwrap + "\n" + pprint(v.val, level + 1) + "\n"
           + pprint(v.body, level + 1) + ")";
  }
  case LTag::EXTERN:
  {
    auto &v = std::get<Extern>(lm->as);
    return pad + "@extern(" + v.symbol + " in " + v.lib + ")";
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

  case LTag::LET:
  {
    auto &l = std::get<Let>(node->as);
    // Bind the name to a placeholder, evaluate the value with that binding in
    // scope, then back-patch the slot so recursive references resolve. (For a
    // non-recursive let the placeholder is simply never read before patching.)
    pLm slot = std::make_shared<Lm>(
      Lm{ .tag = LTag::UNK, .as = std::monostate{} });
    DynEnv new_env = denv;
    new_env.push_back(slot);
    *slot = *eval(l.val, new_env, senv);
    return eval(l.body, new_env, senv);
  }

  case LTag::APP:
  {
    auto &a = std::get<App>(node->as);

    pLm closure = eval(a.fn, denv, senv);
    pLm val     = eval(a.arg, denv, senv);

    // Foreign function: accumulate the argument, call once saturated.
    if (LTag::EXTERN == closure->tag)
    {
      Extern e = std::get<Extern>(closure->as); // copy and extend
      e.args.push_back(val);
      if (e.args.size() >= e.arg_types.size()) return call_extern(e);
      return std::make_shared<Lm>(Lm{ .tag = LTag::EXTERN, .as = e });
    }

    UT_FAIL_IF(closure->tag != LTag::FUN);
    auto  &fun     = std::get<Fun>(closure->as);
    DynEnv new_env = fun.var.env;
    new_env.push_back(val);
    return eval(fun.body, new_env, senv);
  }

  case LTag::EXTERN:
  {
    auto &e = std::get<Extern>(node->as);
    // A nullary extern is already saturated; otherwise it is a function value.
    if (e.args.size() >= e.arg_types.size()) return call_extern(e);
    return node;
  }

  case LTag::UNK:
    return node;
  }

  return node;
}

} // namespace IT
