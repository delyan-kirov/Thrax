#include "IT.hpp"
#include "FF.hpp"
#include "OP.hpp"

namespace IT
{

namespace
{

Var
mkVar(
  UT::Vu s, size_t idx = 0)
{
  return Var{ std::string{ s }, idx, {} };
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

  case LTag::IF:
  {
    auto &v = std::get<If>(node->as);
    assign_id(v.cond, env);
    assign_id(v.yes, env);
    assign_id(v.no, env);
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

  case LTag::INT:
  case LTag::REAL:
  case LTag::STR:
  case LTag::REC: // a runtime-only cell; never present in the static AST
  case LTag::UNK : break;
  }
}

} // namespace

pLm eval(pLm node, DynEnv denv, StatEnv &senv);

namespace
{

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

// One built-in implementation: its arity and a function over the already
// evaluated, saturated operands.
struct Impl
{
  size_t arity;
  pLm (*fn)(const std::vector<pLm> &);
};

// The dispatch table, keyed by the monomorphic key the type checker resolved
// each overloaded use to (see TC's overload_db). An "@Int" impl only sees Ints;
// an "@Real" impl may see an Int on one side (mixed Int/Real combinations route
// here), so it reads operands via as_num, which coerces an Int to double.
const std::unordered_map<std::string, Impl> impls{
  { OP::mono(OP::ADD, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) + as_int(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) - as_int(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) * as_int(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) / as_int(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) % as_int(a[1]));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) == as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) >= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) <= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) > as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) < as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::ADD, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) + as_num(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) - as_num(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) * as_num(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) / as_num(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(std::fmod(as_num(a[0]), as_num(a[1])));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) == as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) >= as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) <= as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) > as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) < as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::NEG, OP::TY_INT),
    { 1, [](const std::vector<pLm> &a) { return mk_int(-as_int(a[0])); } } },
  { OP::mono(OP::NEG, OP::TY_REAL),
    { 1, [](const std::vector<pLm> &a) { return mk_real(-as_num(a[0])); } } },
  { OP::mono(OP::NOT, OP::TY_INT),
    { 1,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) == 0 ? 1 : 0);
      } } },
};

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

} // namespace

namespace
{

/*------------------------------------------------------------------------------
 *\ANF -- A-Normal Form
 *
 * Rewrite the freshly built Lm tree into A-normal form before De Bruijn
 * indexing (assign_id) runs. After this pass:
 *
 *   - every application's operator and operand is an *atom* (a variable,
 *     literal, or lambda); any non-atomic sub-expression is hoisted into a
 *     fresh `let`, so evaluation order is explicit and named;
 *   - every `App` carries `tail`, set iff the call is in genuine program tail
 *     position. A non-tail `if` is bound to a `let` (its branches normalized as
 *     value producers) rather than inlined into the continuation, so no code is
 *     duplicated and branch calls are marked accurately.
 *
 * This is the standard two-function transform (Flanagan et al., "The Essence of
 * Compiling with Continuations"): `norm` threads a continuation `k` that
 * consumes the produced value-expression and returns the rest; `norm_name`
 * forces that value to be an atom. The trampoline in eval reads the resulting
 * shape directly, and the named core is the substrate a later optimizer (e.g.
 * list fusion over lazy sum types) would rewrite against.
 *-----------------------------------------------------------------------------*/

using Cont = std::function<pLm(pLm)>;

// An atom is a value-expression that needs no further evaluation to name: a
// literal, a variable, a lambda, or an already-opaque runtime value.
bool
is_atom(
  const pLm &e)
{
  switch (e->tag)
  {
  case LTag::INT:
  case LTag::REAL:
  case LTag::STR:
  case LTag::VAR:
  case LTag::FUN:
  case LTag::EXTERN:
  case LTag::BUILTIN:
  case LTag::REC:
  case LTag::UNK    : return true;
  default           : return false;
  }
}

pLm
mk(
  Lm lm)
{
  return std::make_shared<Lm>(std::move(lm));
}
pLm
mk_varref(
  const std::string &name)
{
  return mk(Lm{ .tag = LTag::VAR, .as = Var{ name, 0, {} } });
}
pLm
mk_let(
  const Var &var, pLm val, pLm body)
{
  return mk(Lm{ .tag = LTag::LET, .as = Let{ var, val, body } });
}

struct Anf
{
  size_t counter = 0;

  // A fresh binder name; the '%' prefix cannot collide with a user identifier.
  std::string
  fresh()
  {
    return "%a" + std::to_string(counter++);
  }

  // Normalize a whole expression in tail position: its value is the answer.
  pLm
  term(
    pLm e)
  {
    return norm(e, true, [](pLm v) { return v; });
  }

  // Normalize `e`; `tail` marks tail position (then `k` is the identity that
  // returns the answer). `k` receives the produced value-expression.
  pLm norm(pLm e, bool tail, const Cont &k);

  // Normalize `e` and ensure the result handed to `k` is an atom, hoisting a
  // non-atom into a fresh `let`. Always a non-tail position.
  pLm
  norm_name(
    pLm e, const Cont &k)
  {
    return norm(e, false, [this, k](pLm e2) {
      if (is_atom(e2)) return k(e2);
      std::string t = fresh();
      return mk_let(Var{ t, 0, {} }, e2, k(mk_varref(t)));
    });
  }

  // Normalize struct fields left-to-right into atoms, then rebuild the struct.
  pLm
  norm_struct(
    Struct                                   s,
    size_t                                   i,
    std::vector<std::pair<std::string, pLm>> acc,
    const Cont                              &k)
  {
    if (i == s.fields.size())
      return k(mk(Lm{ .tag = LTag::STRUCT, .as = Struct{ s.name, acc } }));
    return norm_name(s.fields[i].second, [this, s, i, acc, k](pLm a) {
      auto acc2 = acc;
      acc2.push_back({ s.fields[i].first, a });
      return norm_struct(s, i + 1, acc2, k);
    });
  }
};

pLm
Anf::norm(
  pLm e, bool tail, const Cont &k)
{
  switch (e->tag)
  {
  case LTag::INT:
  case LTag::REAL:
  case LTag::STR:
  case LTag::VAR:
  case LTag::EXTERN:
  case LTag::BUILTIN:
  case LTag::REC:
  case LTag::UNK    : return k(e);

  case LTag::FUN:
  {
    Fun f  = std::get<Fun>(e->as);
    f.body = term(f.body); // a lambda body is itself in tail position
    return k(mk(Lm{ .tag = LTag::FUN, .as = f }));
  }

  case LTag::APP:
  {
    App a = std::get<App>(e->as);
    return norm_name(a.fn, [this, a, tail, k](pLm fn2) {
      return norm_name(a.arg, [fn2, tail, k](pLm arg2) {
        return k(mk(Lm{ .tag = LTag::APP, .as = App{ fn2, arg2, tail } }));
      });
    });
  }

  case LTag::IF:
  {
    If i = std::get<If>(e->as);
    return norm_name(i.cond, [this, i, tail, k](pLm c2) {
      pLm y, n;
      if (tail)
      {
        // Tail `if`: each branch is itself in tail position.
        y = term(i.yes);
        n = term(i.no);
      }
      else
      {
        // Non-tail `if`: each branch produces the value the `if` is bound to,
        // so its calls are not tail. Bind the whole `if` via `k`.
        Cont id = [](pLm v) { return v; };
        y       = norm(i.yes, false, id);
        n       = norm(i.no, false, id);
      }
      return k(mk(Lm{ .tag = LTag::IF, .as = If{ c2, y, n } }));
    });
  }

  case LTag::LET:
  {
    Let l = std::get<Let>(e->as);
    // The bound value is never in tail position; the body inherits this `let`'s
    // position and continuation.
    return norm(l.val, false, [this, l, tail, k](pLm v2) {
      return mk_let(l.var, v2, norm(l.body, tail, k));
    });
  }

  case LTag::FIELD:
  {
    Field fa = std::get<Field>(e->as);
    return norm_name(fa.record, [k, fa](pLm r2) {
      return k(mk(Lm{ .tag = LTag::FIELD, .as = Field{ r2, fa.name } }));
    });
  }

  case LTag::STRUCT: return norm_struct(std::get<Struct>(e->as), 0, {}, k);
  }
  return k(e);
}

// Normalize to ANF, then assign De Bruijn indices -- the finishing steps shared
// by every definition before it is stored or evaluated.
pLm
finalize(
  pLm node)
{
  Anf anf;
  node = anf.term(node);
  NameStack ns;
  assign_id(node, ns);
  return node;
}

} // namespace

pLm
exprs2pLm_helper(
  EX::Expr *expr, StatEnv &env)
{
  Lm lm;
  switch (expr->tag)
  {
  case EX::ExprTag::If:
  {
    EX::ExIf &ifexpr = std::get<EX::ExIf>(expr->as);
    pLm       cond   = exprs2pLm_helper(ifexpr.cond, env);
    pLm       then   = exprs2pLm_helper(ifexpr.then, env);
    pLm       othw   = exprs2pLm_helper(ifexpr.alt, env);
    lm               = { .tag = LTag::IF, .as = If{ cond, then, othw } };
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
    pLm   fn  = exprs2pLm_helper(app.fn, env);
    pLm   arg = exprs2pLm_helper(app.arg, env);
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

  case EX::ExprTag::Real:
  {
    lm = { .tag = LTag::REAL,
           .as  = Real{ std::get<EX::ExReal>(expr->as).value } };
  }
  break;

  case EX::ExprTag::Def:
  {
    auto       &gd   = std::get<EX::ExDef>(expr->as);
    std::string name = std::string{ gd.name };

    pLm def;
    if (EX::ExprTag::Extern == gd.def->tag)
    {
      // Foreign binding: the call types come from the signature, not the body.
      auto  &ex = std::get<EX::ExExtern>(gd.def->as);
      Extern e;
      e.symbol = std::string{ ex.symbol };
      e.lib    = std::string{ ex.lib };
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

    env[name] = finalize(def);
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1, {} } };
  }
  break;

  case EX::ExprTag::StructLit:
  {
    auto  &sl = std::get<EX::ExStructLit>(expr->as);
    Struct s;
    s.name = std::string{ sl.type_name };
    for (size_t i = 0; i < sl.fields.size(); ++i)
      s.fields.push_back({ std::string{ sl.fields[i].name },
                           exprs2pLm_helper(sl.fields[i].val, env) });
    lm = { .tag = LTag::STRUCT, .as = s };
  }
  break;

  case EX::ExprTag::Field:
  {
    auto &fa = std::get<EX::ExField>(expr->as);
    Field f;
    f.record = exprs2pLm_helper(fa.record, env);
    f.name   = std::string{ fa.field };
    lm       = { .tag = LTag::FIELD, .as = f };
  }
  break;

  // A struct *declaration* is a type-level form with no runtime value.
  case EX::ExprTag::StructDecl:
  {
    lm = { .tag = LTag::UNK, .as = std::monostate{} };
  }
  break;

  case EX::ExprTag::Extern:
    UT_FAIL_MSG("%s", "@extern is only valid as a global definition");
    break;

  case EX::ExprTag::Match:
    UT_FAIL_MSG("%s", "match should have been lowered by LL");
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
  case LTag::IF:
  {
    auto &v = std::get<If>(lm->as);
    return pad + "(if\n" + pprint(v.cond, level + 1) + "\n"
           + pprint(v.yes, level + 1) + "\n" + pprint(v.no, level + 1) + ")";
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
  case LTag::REC: return pad + "<rec>";
  case LTag::UNK: return pad + "?unknown";
  }
  return pad + "?unreachable";
}

pLm
exprs2pLm(
  EX::Expr *expr, StatEnv &env)
{
  return finalize(exprs2pLm_helper(expr, env));
}

pLm
eval(
  pLm node, DynEnv denv, StatEnv &senv)
{
  // Trampoline: a call in tail position rebinds `node`/`denv` and loops instead
  // of recursing, so tail recursion runs in constant C++ stack. Non-tail
  // sub-evaluations (an `if` condition, a `let` value, an application's atoms)
  // still recurse -- their depth is the program's, not the call chain's.
  //
  // `live` keeps recursive `let` closures alive across the loop: such a closure
  // is held only weakly by its own captured environment (the Rec cell, see
  // LET), and the C++ stack used to keep it live across a call. Globals (empty
  // env) and ordinary closures hold no Rec, so they are never added and plain
  // tail recursion stays in constant space.
  std::vector<pLm> live;

  for (;;)
  {
    UT_FAIL_IF(!node);

    switch (node->tag)
    {
    case LTag::INT:
    case LTag::REAL:
    case LTag::STR : return node;

    case LTag::VAR:
    {
      auto &v = std::get<Var>(node->as);

      // A resolved built-in (e.g. "+@Int") is a first-class, curried value.
      auto bit = impls.find(v.unwrap);
      if (bit != impls.end())
        return std::make_shared<Lm>(
          Lm{ .tag = LTag::BUILTIN,
              .as  = Builtin{ v.unwrap, bit->second.arity, {} } });

      // Global definition placeholder
      if (v.idx == (size_t)-1) return node;

      // Local variable (De Bruijn indexed)
      if (v.idx > 0)
      {
        UT_FAIL_IF(v.idx > denv.size());
        pLm entry = denv[denv.size() - v.idx];
        // A `let` binding is held weakly while its value is built (see LET), to
        // keep the closure graph acyclic; lock it to recover the value.
        if (entry && LTag::REC == entry->tag)
        {
          pLm target = std::get<Rec>(entry->as).target.lock();
          UT_FAIL_IF(!target);
          return target;
        }
        return entry;
      }

      // Global variable (idx == 0): jump to its definition in tail position.
      auto it = senv.find(v.unwrap);
      UT_FAIL_IF(it == senv.end());
      node = it->second;
      denv.clear();
      continue;
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
      // Bind the name to a placeholder, then evaluate the value with that
      // binding *weakly* in scope and back-patch the slot, so recursive
      // references resolve without the value's captured environment forming a
      // shared_ptr cycle with it. (For a non-recursive let the placeholder is
      // simply never read before patching.) The body then evaluates in tail
      // position with the binding held strongly.
      pLm value
        = std::make_shared<Lm>(Lm{ .tag = LTag::UNK, .as = std::monostate{} });
      pLm self = std::make_shared<Lm>(
        Lm{ .tag = LTag::REC, .as = Rec{ std::weak_ptr<Lm>(value) } });

      DynEnv val_env = denv;
      val_env.push_back(self);
      *value = *eval(l.val, val_env, senv);

      denv.push_back(value);
      node = l.body;
      continue;
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

      // Built-in: accumulate the argument, run the impl once saturated.
      if (LTag::BUILTIN == closure->tag)
      {
        Builtin b = std::get<Builtin>(closure->as); // copy and extend
        b.args.push_back(val);
        if (b.args.size() >= b.arity) return impls.at(b.impl).fn(b.args);
        return std::make_shared<Lm>(Lm{ .tag = LTag::BUILTIN, .as = b });
      }

      UT_FAIL_IF(closure->tag != LTag::FUN);
      auto  &fun     = std::get<Fun>(closure->as);
      DynEnv new_env = fun.var.env;
      new_env.push_back(val);
      // A recursive `let` value is reachable from a closure's captured
      // environment only weakly, through a Rec cell (see LET). The C++ stack
      // used to keep it alive across a call; with the call trampolined, pin
      // each such value for the loop's lifetime. Globals (empty env) and
      // ordinary closures carry no Rec, so nothing is pinned and plain tail
      // recursion stays in constant space; dedup keeps self-recursion at O(1).
      for (const pLm &slot : fun.var.env)
        if (slot && LTag::REC == slot->tag)
        {
          pLm tgt = std::get<Rec>(slot->as).target.lock();
          if (tgt && (live.empty() || live.back() != tgt)) live.push_back(tgt);
        }
      node = fun.body;
      denv = std::move(new_env);
      continue;
    }

    case LTag::EXTERN:
    {
      auto &e = std::get<Extern>(node->as);
      // A nullary extern is already saturated; otherwise it is a function
      // value.
      if (e.args.size() >= e.arg_types.size()) return call_extern(e);
      return node;
    }

    case LTag::IF:
    {
      auto &i    = std::get<If>(node->as);
      pLm   cond = eval(i.cond, denv, senv);
      UT_FAIL_IF(cond->tag != LTag::INT);
      bool taken = std::get<Int>(cond->as).unwrap != 0;
      node       = taken ? i.yes : i.no;
      continue;
    }

    // An unapplied built-in is already a value.
    case LTag::BUILTIN: return node;

    case LTag::STRUCT:
    {
      // Force every field, building a fully evaluated struct value.
      auto  &s = std::get<Struct>(node->as);
      Struct out;
      out.name = s.name;
      for (auto &f : s.fields)
        out.fields.push_back({ f.first, eval(f.second, denv, senv) });
      return std::make_shared<Lm>(Lm{ .tag = LTag::STRUCT, .as = out });
    }

    case LTag::FIELD:
    {
      auto &fa  = std::get<Field>(node->as);
      pLm   rec = eval(fa.record, denv, senv);
      UT_FAIL_IF(rec->tag != LTag::STRUCT);
      for (auto &f : std::get<Struct>(rec->as).fields)
        if (f.first == fa.name) return f.second;
      UT_FAIL_MSG("no field '%s' on struct '%s'",
                  fa.name.c_str(),
                  std::get<Struct>(rec->as).name.c_str());
      return node;
    }

    // A let binding's weak self-reference (see LET); lock it to the value. eval
    // is never called on one directly -- VAR resolves it -- but handle it for
    // safety.
    case LTag::REC:
    {
      pLm target = std::get<Rec>(node->as).target.lock();
      UT_FAIL_IF(!target);
      return target;
    }

    case LTag::UNK: return node;
    }

    return node;
  }
}

} // namespace IT
