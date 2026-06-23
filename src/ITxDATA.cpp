#include "ITxDATA.hpp"
#include "FF.hpp"

namespace IT
{

pLm
lookup(
  const StatEnv &env, const std::string &name)
{
  auto it = env.find(name);
  if (it != env.end()) return it->second;
  return std::make_shared<Lm>(Lm{ .tag = LTag::UNK, .as = std::monostate{} });
}

Var
mkVar(
  UT::Vu s)
{
  return Var{ std::string{ s }, 0, {} };
}

Str
mkStr(
  UT::Vu s)
{
  std::string ss{ s };
  return Str{ ss };
}

Fun
mkFun(
  UT::Vu s, pLm body)
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

  case LTag::CASE:
  {
    auto &v = std::get<Case>(node->as);
    assign_id(v.scrut, env);
    for (auto &alt : v.alts)
    {
      for (auto &b : alt.binders) env.push_back(b);
      assign_id(alt.body, env);
      for (size_t i = 0; i < alt.binders.size(); ++i) env.pop_back();
    }
    assign_id(v.deflt, env);
  }
  break;

  case LTag::BUILTIN:
  {
    // Built-in values are produced during evaluation, not here; an unapplied
    // one carries no arguments to index. Handled for completeness.
    auto &v = std::get<Builtin>(node->as);
    for (auto &a : v.args) assign_id(a, env);
  }
  break;

  case LTag::STRUCT:
  {
    auto &v = std::get<Struct>(node->as);
    for (auto &f : v.fields) assign_id(f.second, env);
  }
  break;

  case LTag::FIELD:
  {
    auto &v = std::get<Field>(node->as);
    assign_id(v.record, env);
  }
  break;

  case LTag::VARIANT:
  {
    // A constructor binds nothing; index each payload field in place (it is
    // suspended into a Thunk capturing this same scope at eval time).
    auto &v = std::get<Variant>(node->as);
    for (auto &f : v.fields) assign_id(f, env);
  }
  break;

  case LTag::INT:
  case LTag::REAL:
  case LTag::STR:
  case LTag::REC:   // a runtime-only cell; never present in the static AST
  case LTag::THUNK: // likewise built only during eval
  case LTag::UNK  : break;
  }
}

// Operand extraction / result construction. The type checker resolved the
// overload, so an "@Int" impl only sees Ints. An "@Real" impl may see an Int on
// one side (mixed Int/Real combinations route here), so it reads via `as_num`,
// which coerces an Int operand to double.
ssize_t
as_int(
  const pLm &v)
{
  return std::get<Int>(v->as).unwrap;
}
double
as_num(
  const pLm &v)
{
  return LTag::REAL == v->tag ? std::get<Real>(v->as).unwrap
                              : (double)std::get<Int>(v->as).unwrap;
}
pLm
mk_int(
  ssize_t v)
{
  return std::make_shared<Lm>(Lm{ .tag = LTag::INT, .as = Int{ v } });
}
pLm
mk_real(
  double v)
{
  return std::make_shared<Lm>(Lm{ .tag = LTag::REAL, .as = Real{ v } });
}

// A foreign type name as written in a signature. Type variables and function
// arguments are opaque, word-sized values, so they marshal as pointers.
std::string
ffi_type_name(
  EX::Ty *t)
{
  switch (t->tag)
  {
  case EX::TyTag::Con  : return std::string(std::get<EX::TyCon>(t->as).name);
  case EX::TyTag::Var  :
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

  ssize_t r = FF::call(UT::Vu{ e.lib.c_str(), e.lib.size() },
                       UT::Vu{ e.symbol.c_str(), e.symbol.size() },
                       e.arg_types,
                       e.ret_type,
                       args);

  if ("Str" == e.ret_type)
  {
    const char *p = (const char *)(intptr_t)r;
    return std::make_shared<Lm>(
      Lm{ .tag = LTag::STR, .as = Str{ p ? std::string(p) : std::string() } });
  }
  return std::make_shared<Lm>(Lm{ .tag = LTag::INT, .as = Int{ r } });
}

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
  case LTag::REAL:
  {
    auto &v = std::get<Real>(lm->as);
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
  case LTag::BUILTIN:
  {
    auto       &v = std::get<Builtin>(lm->as);
    std::string s = pad + v.impl;
    for (auto &arg : v.args) s += "\n" + pprint(arg, level + 1);
    return s;
  }
  case LTag::CASE:
  {
    auto       &v    = std::get<Case>(lm->as);
    std::string pad1 = std::string((level + 1) * 2, ' ');
    std::string s    = pad + "(case\n" + pprint(v.scrut, level + 1);
    for (auto &alt : v.alts)
      s += "\n" + pad1 + "alt ->\n" + pprint(alt.body, level + 2);
    return s + "\n" + pad1 + "else ->\n" + pprint(v.deflt, level + 2) + ")";
  }
  case LTag::STRUCT:
  {
    auto       &v = std::get<Struct>(lm->as);
    std::string s = pad + v.name + ".{";
    for (auto &f : v.fields)
      s += "\n" + std::string((level + 1) * 2, ' ') + f.first + " =\n"
           + pprint(f.second, level + 2);
    return s + ")";
  }
  case LTag::FIELD:
  {
    auto &v = std::get<Field>(lm->as);
    return pad + "(field " + v.name + "\n" + pprint(v.record, level + 1) + ")";
  }
  case LTag::VARIANT:
  {
    auto       &v = std::get<Variant>(lm->as);
    std::string s = pad + v.type_name + "." + v.tag + ".{";
    for (auto &f : v.fields) s += "\n" + pprint(f, level + 1);
    return s + ")";
  }
  case LTag::THUNK: return pad + "<thunk>";
  case LTag::REC  : return pad + "<rec>";
  case LTag::UNK  : return pad + "?unknown";
  }
  return pad + "?unreachable";
}

} // namespace IT
