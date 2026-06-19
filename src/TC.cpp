/*-------------------------------------------------------------------------------
 *\file TC.cpp
 *\info Hindley-Milner type checker.
 *
 * Pipeline: desugar EX::Expr -> Core (lambda calculus + let), then Algorithm W
 * over a union-find of unification variables. Operators are just overloaded
 * names (see overload_db): a use is typed as a fresh variable and the overload
 * is chosen by resolve_sites once its operands are known, rewriting the EX Var
 * to a monomorphic key. Only `if` is a real primitive in the initial env.
 * *----------------------------------------------------------------------------*/

#include "TC.hpp"
#include "ER.hpp"
#include "UT.hpp"
#include "OP.hpp"

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
  std::string name;            // Con: "Int"/"Str"; Var (rigid): the skolem name
  int         id    = 0;       // Var: identity
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
 *\OVERLOADS
 *
 * The typed overload registry. Each overloaded name (operators today, but the
 * mechanism is not operator-specific) maps to a list of overloads; an overload
 * is a full type signature -- the operand types followed by the result type --
 * plus the monomorphic implementation key the resolver rewrites a use to.
 *
 * The signature shape is arity-agnostic: unary is {operand, result}, binary is
 * {lhs, rhs, result}. The resolver searches by operand types (return-type
 * overloading is a later extension; the result is stored, just not matched on
 * yet). Several rows may share an impl -- every Real-involving combination of
 * '+' runs the one coercing "+@Real". Adding a row is the only extension point.
 *
 * It lives here, not in a shared header, precisely because only TC needs the
 * type signatures; nothing below TC includes it, so there is no circularity.
 *-----------------------------------------------------------------------------*/

struct Overload
{
  std::vector<const char *> sig;  // operand types..., then the result type
  std::string               mono; // implementation key, e.g. "+@Int"
};
using OverloadTable = std::unordered_map<std::string, std::vector<Overload>>;

// Arithmetic: Int x Int -> Int, otherwise Real (mixed operands coerce).
std::vector<Overload>
arith(
  const char *name)
{
  return {
    { { OP::TY_INT, OP::TY_INT, OP::TY_INT }, OP::mono(name, OP::TY_INT) },
    { { OP::TY_REAL, OP::TY_REAL, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_INT, OP::TY_REAL, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_REAL, OP::TY_INT, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
  };
}

// Comparison: any Int/Real combination -> Int.
std::vector<Overload>
cmp(
  const char *name)
{
  return {
    { { OP::TY_INT, OP::TY_INT, OP::TY_INT }, OP::mono(name, OP::TY_INT) },
    { { OP::TY_REAL, OP::TY_REAL, OP::TY_INT }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_INT, OP::TY_REAL, OP::TY_INT }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_REAL, OP::TY_INT, OP::TY_INT }, OP::mono(name, OP::TY_REAL) },
  };
}

const OverloadTable overload_db{
  { OP::ADD, arith(OP::ADD) },
  { OP::SUB, arith(OP::SUB) },
  { OP::MUL, arith(OP::MUL) },
  { OP::DIV, arith(OP::DIV) },
  { OP::MOD, arith(OP::MOD) },
  { OP::ISEQ, cmp(OP::ISEQ) },
  { OP::GEQ, cmp(OP::GEQ) },
  { OP::LEQ, cmp(OP::LEQ) },
  { OP::MORE, cmp(OP::MORE) },
  { OP::LESS, cmp(OP::LESS) },
  { OP::NEG,
    { { { OP::TY_INT, OP::TY_INT }, OP::mono(OP::NEG, OP::TY_INT) },
      { { OP::TY_REAL, OP::TY_REAL }, OP::mono(OP::NEG, OP::TY_REAL) } } },
  { OP::NOT, { { { OP::TY_INT, OP::TY_INT }, OP::mono(OP::NOT, OP::TY_INT) } } },
};

/*------------------------------------------------------------------------------
 *\CORE
 *-----------------------------------------------------------------------------*/

enum class CKind
{
  Var,
  LitInt,
  LitReal,
  LitStr,
  Lam,
  App,
  Let,
  Extern // a foreign binding; its type comes entirely from the signature
};

struct Core
{
  CKind       kind;
  std::string name;          // Var name / Lam param / Let name
  Core       *a    = nullptr; // Lam: body; App: fn; Let: value
  Core       *b    = nullptr; // App: arg; Let: body
  UT::String  anchor;         // source slice for diagnostics (Var only)
  UT::String *slot = nullptr; // overloaded Var: the EX name field to rewrite
};

// A deferred overload resolution. An overloaded name (e.g. `+`) is typed as a
// fresh variable `use` and applied like any function; once the enclosing global
// is fully inferred its operand/result types are settled, and we rewrite `slot`
// (the EX Var name field) to the matching monomorphic key. The whole use type is
// stored -- not split into lhs/rhs -- so resolution is uniform across arities
// and ready to also match on the result type later.
struct ResolveSite
{
  UT::String *slot;   // the EX Var name to rewrite to the resolved impl
  Type       *use;    // the use's type: an arrow chain over the operands
  std::string base;   // the overloaded name to look up in overload_db
  UT::String  anchor; // for diagnostics
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
  } state
    = Unresolved;
  Scheme scheme{};
};

class Checker
{
public:
  Checker(
    AR::Arena &arena, UT::String src)
      : m_arena{ arena },
        m_src{ src }
  {
    seed_primitives();
  }

  void run(EX::Exprs &exprs);

  std::vector<ER::Diagnostic> m_diags;

private:
  AR::Arena                                   &m_arena;
  UT::String                                   m_src;
  std::deque<Type>                             m_types;
  std::deque<Core>                             m_cores;
  int                                          m_next_id = 0;
  Env                                          m_prim;
  std::unordered_map<std::string, GlobalEntry> m_globals;
  std::vector<std::string> m_order; // source order of globals

  // current diagnostic anchor (the global under check)
  UT::String m_anchor;
  size_t     m_line = 0;

  // overload resolution: deferred sites collected during an inference scope and
  // resolved at its end (see infer's Var case / resolve_sites).
  std::vector<ResolveSite> m_sites;

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
  Type  *sig_to_type(EX::Ty                                  *sig,
                     std::unordered_map<std::string, Type *> &tv,
                     bool                                     rigid);
  Scheme scheme_of_sig(EX::Ty *sig);

  // desugar + inference
  Core *desugar(EX::Expr *e);
  bool  occurs_free(const std::string &name, Core *e);
  Type *infer(Core *e, Env &locals);

  // resolution
  Scheme resolve(const std::string &name);
  void   resolve_sites(size_t from);

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
  m_types.push_back(Type{ Kind::Var,
                          std::move(name),
                          m_next_id++,
                          nullptr,
                          rigid,
                          nullptr,
                          nullptr });
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
  m_types.push_back(Type{ Kind::Arrow, "", 0, nullptr, false, from, to });
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
  if (t->kind == Kind::Arrow) return occurs(var, t->from) || occurs(var, t->to);
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
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "infinite type: '%s' occurs in '%s'",
           show(a).c_str(),
           show(b).c_str());
      return false;
    }
    a->ref = b;
    return true;
  }
  if (b->kind == Kind::Var && !b->rigid)
  {
    if (occurs(b, a))
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "infinite type: '%s' occurs in '%s'",
           show(b).c_str(),
           show(a).c_str());
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

  fail(ER::Code::TYPE_MISMATCH,
       m_anchor,
       "type mismatch: expected '%s', got '%s'",
       show(a).c_str(),
       show(b).c_str());
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

  // Also keep monomorphic any variable an unresolved overload site still
  // constrains: it will be pinned (or defaulted) by resolution, so generalizing
  // it would let one binding be used at two types while a single overload was
  // chosen -- SML's "monomorphism restriction" for overloaded numerics. (Without
  // this, `let square = \x = x*x` would generalize its result variable and the
  // call's result would never be tied back to the resolved operand type.)
  for (const ResolveSite &s : m_sites) free_vars(s.use, ev);

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
    case Kind::Con  : return t;
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
  case EX::ExprTag::Int   : return core(CKind::LitInt);
  case EX::ExprTag::Real  : return core(CKind::LitReal);
  case EX::ExprTag::Str   : return core(CKind::LitStr);
  case EX::ExprTag::Extern: return core(CKind::Extern);
  case EX::ExprTag::Var:
  {
    auto &v   = std::get<EX::ExVar>(e->as);
    Core *c   = core(CKind::Var);
    c->anchor = v.name;
    c->name   = std::to_string(v.name);
    c->slot   = &v.name; // rewritten if this name turns out to be overloaded
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
  case EX::ExprTag::If:
  {
    auto &i  = std::get<EX::ExIf>(e->as);
    Core *v  = core(CKind::Var);
    v->name  = OP::IF;
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
  case CKind::Var    : return e->name == name;
  case CKind::LitInt :
  case CKind::LitReal:
  case CKind::LitStr :
  case CKind::Extern : return false;
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
  case CKind::LitInt : return con("Int");
  case CKind::LitReal: return con("Real");
  case CKind::LitStr : return con("Str");
  case CKind::Extern : return fresh(); // unifies with the declared signature

  case CKind::Var:
  {
    auto it = locals.find(e->name);
    if (it != locals.end()) return instantiate(it->second);
    if (m_globals.count(e->name)) return instantiate(resolve(e->name));
    auto pit = m_prim.find(e->name);
    if (pit != m_prim.end()) return instantiate(pit->second);

    // An overloaded name: type it as a fresh variable and defer the choice of
    // overload to resolve_sites, once its operands (and result) are settled.
    if (overload_db.count(e->name))
    {
      Type *use = fresh();
      m_sites.push_back({ e->slot, use, e->name, e->anchor });
      return use;
    }

    fail(ER::Code::TYPE_UNBOUND,
         e->anchor.m_mem ? e->anchor : m_anchor,
         "unbound variable '%s'",
         e->name.c_str());
    return fresh();
  }

  case CKind::Lam:
  {
    Type *pv       = fresh();
    Env   inner    = locals;
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
      Type *nv     = fresh();
      Env   rec    = locals;
      rec[e->name] = Scheme{ {}, nv };
      vt           = infer(e->a, rec);
      unify(nv, vt);
      vt = nv;
    }
    else
    {
      vt = infer(e->a, locals);
    }
    Scheme s       = generalize(locals, vt);
    Env    inner   = locals;
    inner[e->name] = s;
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
    fail(ER::Code::TYPE_CYCLE,
         g.def->name,
         "global '%s' depends on itself but has no type annotation",
         name.c_str());
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

    Env    empty;
    size_t base = m_sites.size();
    Type  *t    = infer(g.body, empty);
    resolve_sites(base); // settle this body's overloads before judging its type
    t = prune(t);

    if (t->kind != Kind::Con)
    {
      fail(
        ER::Code::TYPE_ANNOTATION_REQUIRED,
        g.def->name,
        "global '%s' needs a type annotation (its type is '%s', not a ground "
        "Int/Str)",
        name.c_str(),
        show(t).c_str());
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

// Resolve the overload sites collected since index `from` (one inference scope's
// worth), then drop them. For each site we force the use type into an arrow chain
// of the operator's arity -- so an under-applied use still exposes operand
// variables -- default any still-unconstrained operand to Int (SML-style), then
// pick the overload whose operand types match and rewrite the EX Var name to its
// implementation key.
void
Checker::resolve_sites(
  size_t from)
{
  for (size_t i = from; i < m_sites.size(); ++i)
  {
    const ResolveSite &s = m_sites[i];

    const std::vector<Overload> *ovs = UT::try_lookup(overload_db, s.base);
    UT_FAIL_IF(!ovs || ovs->empty()); // a site is only made for overloaded names
    size_t arity = ovs->front().sig.size() - 1;

    // Shape the use as (a1 -> ... -> ak -> r) so the operands are reachable
    // whether the operator was fully applied, partially applied, or passed bare.
    std::vector<Type *> args(arity);
    Type               *result = fresh();
    Type               *shape  = result;
    for (size_t k = arity; k-- > 0;)
    {
      args[k] = fresh();
      shape   = arrow(args[k], shape);
    }
    unify(s.use, shape);

    // SML-style defaulting: an operand left unconstrained (e.g. `\x = x * x`)
    // becomes Int rather than an error.
    for (Type *&a : args)
    {
      a = prune(a);
      if (a->kind == Kind::Var && !a->rigid)
      {
        a->ref = con(OP::TY_INT);
        a      = prune(a);
      }
    }

    // Match on operand types. The result type is stored in every signature and
    // unified back in, but is not yet used to discriminate (return-type
    // overloading is the next extension; see Overload).
    const Overload *hit = nullptr;
    for (const Overload &o : *ovs)
    {
      if (o.sig.size() != arity + 1) continue;
      bool ok = true;
      for (size_t k = 0; k < arity; ++k)
        if (args[k]->kind != Kind::Con || args[k]->name != o.sig[k])
        {
          ok = false;
          break;
        }
      if (ok)
      {
        hit = &o;
        break;
      }
    }

    if (!hit)
    {
      std::string operands;
      for (size_t k = 0; k < arity; ++k)
        operands += (k ? ", " : "") + show(args[k]);
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "no overload of '%s' for operand type(s): %s",
           s.base.c_str(),
           operands.c_str());
      continue;
    }

    unify(prune(result), con(hit->sig.back()));
    *s.slot = UT::String{ hit->mono.c_str(), hit->mono.size() };
  }
  m_sites.erase(m_sites.begin() + (long)from, m_sites.end());
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
    g.def           = &d;
    g.body          = desugar(d.def);
    m_globals[name] = g;
    m_order.push_back(name);
  }

  // Phase B: resolve every global's type (on-demand, DAG, cycle-checked).
  for (const std::string &name : m_order) resolve(name);

  // Phase C: check every body against its resolved/declared type, then resolve
  // the overloaded uses it contains (their operand types are now known). A
  // signatured global is only inferred here, so this is where its overloads are
  // resolved; a signature-less one was already resolved in Phase B, and is
  // re-resolved to the same impl here, which is idempotent.
  for (const std::string &name : m_order)
  {
    GlobalEntry &g = m_globals.at(name);
    m_anchor       = g.def->name;
    m_line         = line_of(g.def->name);

    Env    empty;
    size_t base = m_sites.size();
    Type  *bt   = infer(g.body, empty);

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

    resolve_sites(base);
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

  m_diags.push_back(ER::mk_root(m_arena, code, anchor, line_of(anchor), msg));
}

void
Checker::seed_primitives()
{
  // Operators are overloaded and resolved per use (see infer_op / overload_db),
  // so they are not primitives. Only `if`, which is parametric, lives here.
  //
  // if : forall T. Int -> T -> T -> T
  Type *tv = fresh();
  m_prim[OP::IF]
    = Scheme{ { tv->id }, arrow(con("Int"), arrow(tv, arrow(tv, tv))) };
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
