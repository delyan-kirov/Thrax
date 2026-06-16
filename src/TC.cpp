/*-------------------------------------------------------------------------------
 *\file TC.cpp
 *\info Hindley-Milner type checker.
 *
 * Pipeline: desugar EX::Expr -> Core (lambda calculus + let), then Algorithm W
 * over a union-find of unification variables. Operators and `if` are primitives
 * in the initial environment, so the core stays tiny.
 * *----------------------------------------------------------------------------*/

#include "TC.hpp"
#include "ER.hpp"
#include "UT.hpp"

#include <deque>
#include <functional>
#include <string>
#include <unordered_map>
#include <vector>

namespace TC
{

namespace
{

/*------------------------------------------------------------------------------
 *\TYPES
 *
 * A Type is a type constructor (Int/Str), a unification variable (possibly a
 * rigid skolem from a signature), or an arrow. Variables are linked through
 * `ref` (union-find); `prune` chases the chain.
 *-----------------------------------------------------------------------------*/

enum class Kind
{
  Con,
  Var,
  Arrow
};

struct Type
{
  Kind        kind;
  std::string name;        // Con: "Int"/"Str"; Var (rigid): the skolem name
  int         id    = 0;   // Var: identity
  Type       *ref   = nullptr; // Var: union-find link (bound variable)
  bool        rigid = false;   // Var: a skolem, only unifies with itself
  Type       *from  = nullptr; // Arrow
  Type       *to    = nullptr; // Arrow
};

// A type scheme: forall `vars`. `type`.
struct Scheme
{
  std::vector<int> vars;
  Type            *type;
};

using Env = std::unordered_map<std::string, Scheme>;

/*------------------------------------------------------------------------------
 *\CORE
 *-----------------------------------------------------------------------------*/

enum class CKind
{
  Var,
  LitInt,
  LitStr,
  Lam,
  App,
  Let
};

struct Core
{
  CKind       kind;
  std::string name;            // Var name / Lam param / Let name
  Core       *a = nullptr;     // Lam: body; App: fn; Let: value
  Core       *b = nullptr;     // App: arg; Let: body
  UT::String  anchor;          // source slice for diagnostics (Var only)
};

/*------------------------------------------------------------------------------
 *\CHECKER
 *-----------------------------------------------------------------------------*/

struct GlobalEntry
{
  EX::ExDef *def;
  Core      *body;
  enum
  {
    Unresolved,
    Resolving,
    Resolved
  } state = Unresolved;
  Scheme scheme{};
};

class Checker
{
public:
  Checker(AR::Arena &arena, UT::String src)
      : m_arena{ arena },
        m_src{ src }
  {
    seed_primitives();
  }

  void run(EX::Exprs &exprs);

  std::vector<ER::Diagnostic> m_diags;

private:
  AR::Arena                            &m_arena;
  UT::String                            m_src;
  std::deque<Type>                      m_types;
  std::deque<Core>                      m_cores;
  int                                   m_next_id = 0;
  Env                                   m_prim;
  std::unordered_map<std::string, GlobalEntry> m_globals;
  std::vector<std::string>              m_order; // source order of globals

  // current diagnostic anchor (the global under check)
  UT::String m_anchor;
  size_t     m_line = 0;

  // type constructors
  Type *fresh(bool rigid = false, std::string name = "");
  Type *con(std::string name);
  Type *arrow(Type *from, Type *to);
  Core *core(CKind k);

  // union-find
  Type *prune(Type *t);
  bool  occurs(Type *var, Type *t);
  bool  unify(Type *a, Type *b);

  // schemes
  void   free_vars(Type *t, std::vector<int> &out);
  void   env_free_vars(const Env &env, std::vector<int> &out);
  Scheme generalize(const Env &locals, Type *t);
  Type  *instantiate(const Scheme &s);

  // signatures
  Type *sig_to_type(EX::Ty *sig, std::unordered_map<std::string, Type *> &tv,
                    bool rigid);
  Scheme scheme_of_sig(EX::Ty *sig);

  // desugar + inference
  Core *desugar(EX::Expr *e);
  bool  occurs_free(const std::string &name, Core *e);
  Type *infer(Core *e, Env &locals);

  // resolution
  Scheme resolve(const std::string &name);

  // diagnostics
  size_t      line_of(UT::String anchor);
  std::string show(Type *t);
  void        fail(ER::Code code, UT::String anchor, const char *fmt, ...)
    UT_PRINTF_LIKE(4, 5);

  void seed_primitives();
};

/*------------------------------------------------------------------------------
 *\CONSTRUCTORS
 *-----------------------------------------------------------------------------*/

Type *
Checker::fresh(
  bool rigid, std::string name)
{
  m_types.push_back(Type{ Kind::Var, std::move(name), m_next_id++, nullptr,
                          rigid, nullptr, nullptr });
  return &m_types.back();
}

Type *
Checker::con(
  std::string name)
{
  m_types.push_back(
    Type{ Kind::Con, std::move(name), 0, nullptr, false, nullptr, nullptr });
  return &m_types.back();
}

Type *
Checker::arrow(
  Type *from, Type *to)
{
  m_types.push_back(
    Type{ Kind::Arrow, "", 0, nullptr, false, from, to });
  return &m_types.back();
}

Core *
Checker::core(
  CKind k)
{
  m_cores.push_back(Core{ k, "", nullptr, nullptr, {} });
  return &m_cores.back();
}

/*------------------------------------------------------------------------------
 *\UNION-FIND
 *-----------------------------------------------------------------------------*/

Type *
Checker::prune(
  Type *t)
{
  if (t->kind == Kind::Var && t->ref)
  {
    t->ref = prune(t->ref);
    return t->ref;
  }
  return t;
}

bool
Checker::occurs(
  Type *var, Type *t)
{
  t = prune(t);
  if (t == var) return true;
  if (t->kind == Kind::Arrow)
    return occurs(var, t->from) || occurs(var, t->to);
  return false;
}

bool
Checker::unify(
  Type *a, Type *b)
{
  a = prune(a);
  b = prune(b);
  if (a == b) return true;

  if (a->kind == Kind::Var && !a->rigid)
  {
    if (occurs(a, b))
    {
      fail(ER::Code::TYPE_MISMATCH, m_anchor, "infinite type: '%s' occurs in '%s'",
           show(a).c_str(), show(b).c_str());
      return false;
    }
    a->ref = b;
    return true;
  }
  if (b->kind == Kind::Var && !b->rigid)
  {
    if (occurs(b, a))
    {
      fail(ER::Code::TYPE_MISMATCH, m_anchor, "infinite type: '%s' occurs in '%s'",
           show(b).c_str(), show(a).c_str());
      return false;
    }
    b->ref = a;
    return true;
  }

  if (a->kind == Kind::Con && b->kind == Kind::Con && a->name == b->name)
    return true;

  if (a->kind == Kind::Arrow && b->kind == Kind::Arrow)
  {
    bool l = unify(a->from, b->from);
    bool r = unify(a->to, b->to);
    return l && r;
  }

  fail(ER::Code::TYPE_MISMATCH, m_anchor, "type mismatch: expected '%s', got '%s'",
       show(a).c_str(), show(b).c_str());
  return false;
}

/*------------------------------------------------------------------------------
 *\SCHEMES
 *-----------------------------------------------------------------------------*/

void
Checker::free_vars(
  Type *t, std::vector<int> &out)
{
  t = prune(t);
  switch (t->kind)
  {
  case Kind::Var:
    if (!t->rigid)
      for (int id : out)
        if (id == t->id) return;
    if (!t->rigid) out.push_back(t->id);
    break;
  case Kind::Con: break;
  case Kind::Arrow:
    free_vars(t->from, out);
    free_vars(t->to, out);
    break;
  }
}

void
Checker::env_free_vars(
  const Env &env, std::vector<int> &out)
{
  for (const auto &[name, s] : env)
  {
    std::vector<int> tv;
    free_vars(s.type, tv);
    for (int id : tv)
    {
      bool quantified = false;
      for (int q : s.vars)
        if (q == id) quantified = true;
      if (!quantified) out.push_back(id);
    }
  }
}

Scheme
Checker::generalize(
  const Env &locals, Type *t)
{
  std::vector<int> tv;
  free_vars(t, tv);
  std::vector<int> ev;
  env_free_vars(locals, ev);

  std::vector<int> q;
  for (int id : tv)
  {
    bool in_env = false;
    for (int e : ev)
      if (e == id) in_env = true;
    if (!in_env) q.push_back(id);
  }
  return Scheme{ q, t };
}

Type *
Checker::instantiate(
  const Scheme &s)
{
  if (s.vars.empty()) return s.type;

  std::unordered_map<int, Type *> sub;
  for (int id : s.vars) sub[id] = fresh();

  std::function<Type *(Type *)> go = [&](Type *t) -> Type * {
    t = prune(t);
    switch (t->kind)
    {
    case Kind::Var:
    {
      auto it = sub.find(t->id);
      return it == sub.end() ? t : it->second;
    }
    case Kind::Con: return t;
    case Kind::Arrow: return arrow(go(t->from), go(t->to));
    }
    return t;
  };
  return go(s.type);
}

/*------------------------------------------------------------------------------
 *\SIGNATURES
 *-----------------------------------------------------------------------------*/

Type *
Checker::sig_to_type(
  EX::Ty *sig, std::unordered_map<std::string, Type *> &tv, bool rigid)
{
  switch (sig->tag)
  {
  case EX::TyTag::Con:
    return con(std::to_string(std::get<EX::TyCon>(sig->as).name));
  case EX::TyTag::Var:
  {
    std::string nm = std::to_string(std::get<EX::TyVar>(sig->as).name);
    auto        it = tv.find(nm);
    if (it != tv.end()) return it->second;
    Type *v = fresh(rigid, nm);
    tv[nm]  = v;
    return v;
  }
  case EX::TyTag::Arrow:
  {
    auto &ar = std::get<EX::TyArrow>(sig->as);
    Type *f  = sig_to_type(ar.from, tv, rigid);
    Type *t  = sig_to_type(ar.to, tv, rigid);
    return arrow(f, t);
  }
  }
  return fresh();
}

Scheme
Checker::scheme_of_sig(
  EX::Ty *sig)
{
  std::unordered_map<std::string, Type *> tv;
  Type                                   *t = sig_to_type(sig, tv, false);
  std::vector<int>                        q;
  for (auto &[name, v] : tv) q.push_back(v->id);
  return Scheme{ q, t };
}

/*------------------------------------------------------------------------------
 *\DESUGAR
 *-----------------------------------------------------------------------------*/

Core *
Checker::desugar(
  EX::Expr *e)
{
  switch (e->tag)
  {
  case EX::ExprTag::Int:
    return core(CKind::LitInt);
  case EX::ExprTag::Str:
    return core(CKind::LitStr);
  case EX::ExprTag::Var:
  {
    Core *c   = core(CKind::Var);
    c->anchor = std::get<EX::ExVar>(e->as).name;
    c->name   = std::to_string(c->anchor);
    return c;
  }
  case EX::ExprTag::App:
  {
    auto &ap = std::get<EX::ExApp>(e->as);
    Core *c  = core(CKind::App);
    c->a     = desugar(ap.fn);
    c->b     = desugar(ap.arg);
    return c;
  }
  case EX::ExprTag::FnDef:
  {
    auto &fn = std::get<EX::ExFnDef>(e->as);
    Core *c  = core(CKind::Lam);
    c->name  = std::to_string(fn.param);
    c->a     = desugar(fn.body);
    return c;
  }
  case EX::ExprTag::Let:
  {
    auto &lt = std::get<EX::ExLet>(e->as);
    Core *c  = core(CKind::Let);
    c->name  = std::to_string(lt.var);
    c->a     = desugar(lt.val);
    c->b     = desugar(lt.body);
    return c;
  }
  case EX::ExprTag::Binop:
  {
    auto       &b = std::get<EX::ExBinop>(e->as);
    const char *op;
    switch (b.tag)
    {
    case EX::BinopTag::Add : op = "+";  break;
    case EX::BinopTag::Sub : op = "-";  break;
    case EX::BinopTag::Mul : op = "*";  break;
    case EX::BinopTag::Div : op = "/";  break;
    case EX::BinopTag::Mod : op = "%";  break;
    case EX::BinopTag::IsEq: op = "?="; break;
    default                : op = "+";  break;
    }
    Core *v = core(CKind::Var);
    v->name = op;
    Core *app1 = core(CKind::App);
    app1->a    = v;
    app1->b    = desugar(b.ops.begin());
    Core *app2 = core(CKind::App);
    app2->a    = app1;
    app2->b    = desugar(b.ops.last());
    return app2;
  }
  case EX::ExprTag::Unop:
  {
    auto &u = std::get<EX::ExUnop>(e->as);
    Core *v = core(CKind::Var);
    v->name = (u.tag == EX::UnopTag::Neg) ? "neg" : "not";
    Core *app = core(CKind::App);
    app->a    = v;
    app->b    = desugar(u.op);
    return app;
  }
  case EX::ExprTag::If:
  {
    auto &i = std::get<EX::ExIf>(e->as);
    Core *v = core(CKind::Var);
    v->name = "if";
    Core *a1 = core(CKind::App);
    a1->a    = v;
    a1->b    = desugar(i.cond);
    Core *a2 = core(CKind::App);
    a2->a    = a1;
    a2->b    = desugar(i.then);
    Core *a3 = core(CKind::App);
    a3->a    = a2;
    a3->b    = desugar(i.alt);
    return a3;
  }
  default:
    // Def / Unknown never appear nested in an expression.
    return core(CKind::LitInt);
  }
}

bool
Checker::occurs_free(
  const std::string &name, Core *e)
{
  switch (e->kind)
  {
  case CKind::Var: return e->name == name;
  case CKind::LitInt:
  case CKind::LitStr: return false;
  case CKind::Lam:
    if (e->name == name) return false; // shadowed
    return occurs_free(name, e->a);
  case CKind::App: return occurs_free(name, e->a) || occurs_free(name, e->b);
  case CKind::Let:
    // name shadowed in the body; still visible in the (recursive) value
    if (e->name == name) return occurs_free(name, e->a);
    return occurs_free(name, e->a) || occurs_free(name, e->b);
  }
  return false;
}

/*------------------------------------------------------------------------------
 *\INFERENCE
 *-----------------------------------------------------------------------------*/

Type *
Checker::infer(
  Core *e, Env &locals)
{
  switch (e->kind)
  {
  case CKind::LitInt: return con("Int");
  case CKind::LitStr: return con("Str");

  case CKind::Var:
  {
    auto it = locals.find(e->name);
    if (it != locals.end()) return instantiate(it->second);
    if (m_globals.count(e->name)) return instantiate(resolve(e->name));
    auto pit = m_prim.find(e->name);
    if (pit != m_prim.end()) return instantiate(pit->second);

    fail(ER::Code::TYPE_UNBOUND,
         e->anchor.m_mem ? e->anchor : m_anchor,
         "unbound variable '%s'", e->name.c_str());
    return fresh();
  }

  case CKind::Lam:
  {
    Type *pv = fresh();
    Env   inner = locals;
    inner[e->name] = Scheme{ {}, pv }; // lambda params are monomorphic
    Type *bt       = infer(e->a, inner);
    return arrow(pv, bt);
  }

  case CKind::App:
  {
    Type *ft = infer(e->a, locals);
    Type *at = infer(e->b, locals);
    Type *rv = fresh();
    unify(ft, arrow(at, rv));
    return rv;
  }

  case CKind::Let:
  {
    Type *vt;
    if (occurs_free(e->name, e->a))
    {
      // recursive: bind the name monomorphically while inferring the value
      Type *nv      = fresh();
      Env   rec     = locals;
      rec[e->name]  = Scheme{ {}, nv };
      vt            = infer(e->a, rec);
      unify(nv, vt);
      vt = nv;
    }
    else
    {
      vt = infer(e->a, locals);
    }
    Scheme s        = generalize(locals, vt);
    Env    inner    = locals;
    inner[e->name]  = s;
    return infer(e->b, inner);
  }
  }
  return fresh();
}

/*------------------------------------------------------------------------------
 *\RESOLUTION (phase B)
 *-----------------------------------------------------------------------------*/

Scheme
Checker::resolve(
  const std::string &name)
{
  GlobalEntry &g = m_globals.at(name);
  if (g.state == GlobalEntry::Resolved) return g.scheme;
  if (g.state == GlobalEntry::Resolving)
  {
    fail(ER::Code::TYPE_CYCLE, g.def->name,
         "global '%s' depends on itself but has no type annotation", name.c_str());
    g.scheme = Scheme{ {}, fresh() };
    g.state  = GlobalEntry::Resolved;
    return g.scheme;
  }

  g.state = GlobalEntry::Resolving;

  if (g.def->sig)
  {
    g.scheme = scheme_of_sig(g.def->sig);
  }
  else
  {
    UT::String save_a = m_anchor;
    size_t     save_l = m_line;
    m_anchor          = g.def->name;
    m_line            = line_of(g.def->name);

    Env   empty;
    Type *t = prune(infer(g.body, empty));

    if (t->kind != Kind::Con)
    {
      fail(ER::Code::TYPE_ANNOTATION_REQUIRED, g.def->name,
           "global '%s' needs a type annotation (its type is '%s', not a ground "
           "Int/Str)",
           name.c_str(), show(t).c_str());
      g.scheme = Scheme{ {}, t };
    }
    else
    {
      g.scheme = Scheme{ {}, t };
    }
    m_anchor = save_a;
    m_line   = save_l;
  }

  g.state = GlobalEntry::Resolved;
  return g.scheme;
}

/*------------------------------------------------------------------------------
 *\DRIVER
 *-----------------------------------------------------------------------------*/

void
Checker::run(
  EX::Exprs &exprs)
{
  // Phase A: collect globals.
  for (size_t i = 0; i < exprs.m_len; ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::Def) continue;
    EX::ExDef  &d    = std::get<EX::ExDef>(e->as);
    std::string name = std::to_string(d.name);
    GlobalEntry g;
    g.def  = &d;
    g.body = desugar(d.def);
    m_globals[name] = g;
    m_order.push_back(name);
  }

  // Phase B: resolve every global's type (on-demand, DAG, cycle-checked).
  for (const std::string &name : m_order) resolve(name);

  // Phase C: check every body against its resolved/declared type.
  for (const std::string &name : m_order)
  {
    GlobalEntry &g = m_globals.at(name);
    m_anchor       = g.def->name;
    m_line         = line_of(g.def->name);

    Env   empty;
    Type *bt = infer(g.body, empty);

    if (g.def->sig)
    {
      std::unordered_map<std::string, Type *> tv;
      Type *rigid = sig_to_type(g.def->sig, tv, true);
      unify(bt, rigid);
    }
    else
    {
      unify(bt, g.scheme.type);
    }
  }
}

/*------------------------------------------------------------------------------
 *\DIAGNOSTICS
 *-----------------------------------------------------------------------------*/

size_t
Checker::line_of(
  UT::String anchor)
{
  if (!anchor.m_mem || !m_src.m_mem) return 0;
  if (anchor.m_mem < m_src.m_mem) return 0;
  size_t off  = (size_t)(anchor.m_mem - m_src.m_mem);
  size_t line = 1;
  for (size_t i = 0; i < off && i < m_src.m_len; ++i)
    if (m_src.m_mem[i] == '\n') line++;
  return line;
}

std::string
Checker::show(
  Type *t)
{
  t = prune(t);
  switch (t->kind)
  {
  case Kind::Con: return t->name;
  case Kind::Var:
    if (t->rigid) return "`" + t->name;
    return "?" + std::to_string(t->id);
  case Kind::Arrow:
  {
    std::string l = show(t->from);
    if (prune(t->from)->kind == Kind::Arrow) l = "(" + l + ")";
    return l + " -> " + show(t->to);
  }
  }
  return "?";
}

void
Checker::fail(
  ER::Code code, UT::String anchor, const char *fmt, ...)
{
  va_list args;
  va_start(args, fmt);
  va_list copy;
  va_copy(copy, args);
  int len = std::vsnprintf(nullptr, 0, fmt, args);
  va_end(args);

  UT::String msg{ "<message formatting failed>" };
  if (len >= 0)
  {
    char *mem = (char *)m_arena.alloc((size_t)len + 1);
    std::vsnprintf(mem, (size_t)len + 1, fmt, copy);
    msg = UT::String{ mem, (size_t)len };
  }
  va_end(copy);

  m_diags.push_back(
    ER::mk_root(m_arena, code, anchor, line_of(anchor), msg));
}

void
Checker::seed_primitives()
{
  auto arith = [&] {
    return Scheme{ {}, arrow(con("Int"), arrow(con("Int"), con("Int"))) };
  };
  m_prim["+"]  = arith();
  m_prim["-"]  = arith();
  m_prim["*"]  = arith();
  m_prim["/"]  = arith();
  m_prim["%"]  = arith();
  m_prim["?="] = arith();
  m_prim["neg"] = Scheme{ {}, arrow(con("Int"), con("Int")) };
  m_prim["not"] = Scheme{ {}, arrow(con("Int"), con("Int")) };

  // if : forall T. Int -> T -> T -> T
  Type *tv = fresh();
  m_prim["if"]
    = Scheme{ { tv->id },
              arrow(con("Int"), arrow(tv, arrow(tv, tv))) };
}

} // namespace

std::vector<ER::Diagnostic>
check(
  EX::Exprs &exprs, AR::Arena &arena, UT::String src)
{
  Checker c{ arena, src };
  c.run(exprs);
  return c.m_diags;
}

} // namespace TC
