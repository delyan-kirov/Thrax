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
#include "OP.hpp"
#include "TCxDATA.hpp"
#include "UT.hpp"

namespace TC
{

namespace
{

/*------------------------------------------------------------------------------
 *\CHECKER
 *-----------------------------------------------------------------------------*/

class Checker
{
public:
  Checker(
    AR::Arena &arena, UT::Vu src)
      : m_arena{ arena },
        m_src{ src }
  {
    seed_primitives();
  }

  void run(EX::Exprs &exprs);

  Diagnostics m_diags;

private:
  AR::Arena  &m_arena;
  UT::Vu      m_src;
  TypeStore   m_types;
  CoreStore   m_cores;
  int         m_next_id = 0;
  Env         m_prim;
  Globals     m_globals;
  StructTable m_structs;
  UnionTable  m_unions;

  // current diagnostic anchor (the global under check)
  UT::Vu m_anchor;
  size_t m_line = 0;

  // overload resolution: deferred sites collected during an inference scope and
  // resolved at its end (see infer's Var case / resolve_sites).
  ResolveSites m_sites;

  // type constructors
  Type *fresh(bool rigid = false, std::string name = "");
  Type *con(std::string name);
  Type *arrow(Type *from, Type *to);
  Core *core(CoreData as);

  // union-find
  Type *prune(Type *t);
  bool  occurs(Type *var, Type *t);
  bool  unify(Type *a, Type *b);

  // schemes
  void   free_vars(Type *t, VarIds &out);
  void   env_free_vars(const Env &env, VarIds &out);
  Scheme generalize(const Env &locals, Type *t);
  Type  *instantiate(const Scheme &s);

  // signatures
  Type  *sig_to_type(EX::Ty *sig, TyVarEnv &tv, bool rigid);
  Scheme scheme_of_sig(EX::Ty *sig);

  // desugar + inference
  Core *desugar(EX::Expr *e);
  bool  occurs_free(const std::string &name, Core *e);
  Type *infer(Core *e, Env &locals);

  // resolution
  Scheme resolve(const std::string &name);
  void   resolve_sites(size_t from);

  // diagnostics
  size_t      line_of(UT::Vu anchor);
  std::string show(Type *t);
  void        fail(ER::Code code, UT::Vu anchor, const char *fmt, ...)
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
  m_types.push_back(
    Type{ TVar{ m_next_id++, nullptr, rigid, std::move(name) } });
  return &m_types.back();
}

Type *
Checker::con(
  std::string name)
{
  m_types.push_back(Type{ TCon{ std::move(name) } });
  return &m_types.back();
}

Type *
Checker::arrow(
  Type *from, Type *to)
{
  m_types.push_back(Type{ TArrow{ from, to } });
  return &m_types.back();
}

Core *
Checker::core(
  CoreData as)
{
  m_cores.push_back(Core{ std::move(as) });
  return &m_cores.back();
}

/*------------------------------------------------------------------------------
 *\UNION-FIND
 *-----------------------------------------------------------------------------*/

Type *
Checker::prune(
  Type *t)
{
  if (t->kind() == Kind::Var)
  {
    TVar &v = std::get<TVar>(t->as);
    if (v.ref)
    {
      v.ref = prune(v.ref);
      return v.ref;
    }
  }
  return t;
}

bool
Checker::occurs(
  Type *var, Type *t)
{
  t = prune(t);
  if (t == var) return true;
  if (t->kind() == Kind::Arrow)
  {
    TArrow &a = std::get<TArrow>(t->as);
    return occurs(var, a.from) || occurs(var, a.to);
  }
  return false;
}

bool
Checker::unify(
  Type *a, Type *b)
{
  a = prune(a);
  b = prune(b);
  if (a == b) return true;

  if (a->kind() == Kind::Var && !std::get<TVar>(a->as).rigid)
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
    std::get<TVar>(a->as).ref = b;
    return true;
  }
  if (b->kind() == Kind::Var && !std::get<TVar>(b->as).rigid)
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
    std::get<TVar>(b->as).ref = a;
    return true;
  }

  if (a->kind() == Kind::Con && b->kind() == Kind::Con
      && std::get<TCon>(a->as).name == std::get<TCon>(b->as).name)
    return true;

  if (a->kind() == Kind::Arrow && b->kind() == Kind::Arrow)
  {
    TArrow &fa = std::get<TArrow>(a->as);
    TArrow &fb = std::get<TArrow>(b->as);
    bool    l  = unify(fa.from, fb.from);
    bool    r  = unify(fa.to, fb.to);
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
  Type *t, VarIds &out)
{
  t = prune(t);
  switch (t->kind())
  {
  case Kind::Var:
  {
    TVar &v = std::get<TVar>(t->as);
    if (v.rigid) break;
    for (int id : out)
      if (id == v.id) return;
    out.push_back(v.id);
    break;
  }
  case Kind::Con: break;
  case Kind::Arrow:
  {
    TArrow &a = std::get<TArrow>(t->as);
    free_vars(a.from, out);
    free_vars(a.to, out);
    break;
  }
  }
}

void
Checker::env_free_vars(
  const Env &env, VarIds &out)
{
  for (const auto &[name, s] : env)
  {
    VarIds tv;
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
  VarIds tv;
  free_vars(t, tv);
  VarIds ev;
  env_free_vars(locals, ev);

  // Also keep monomorphic any variable an unresolved overload site still
  // constrains: it will be pinned (or defaulted) by resolution, so generalizing
  // it would let one binding be used at two types while a single overload was
  // chosen -- SML's "monomorphism restriction" for overloaded numerics.
  // (Without this, `let square = \x = x*x` would generalize its result variable
  // and the call's result would never be tied back to the resolved operand
  // type.)
  for (const ResolveSite &s : m_sites) free_vars(s.use, ev);

  VarIds q;
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

  Subst sub;
  for (int id : s.vars) sub[id] = fresh();

  std::function<Type *(Type *)> go = [&](Type *t) -> Type * {
    t = prune(t);
    switch (t->kind())
    {
    case Kind::Var:
    {
      auto it = sub.find(std::get<TVar>(t->as).id);
      return it == sub.end() ? t : it->second;
    }
    case Kind::Con: return t;
    case Kind::Arrow:
    {
      TArrow &a = std::get<TArrow>(t->as);
      return arrow(go(a.from), go(a.to));
    }
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
  EX::Ty *sig, TyVarEnv &tv, bool rigid)
{
  switch (sig->tag)
  {
  case EX::TyTag::Con:
    return con(std::string(std::get<EX::TyCon>(sig->as).name));
  case EX::TyTag::Var:
  {
    std::string nm = std::string(std::get<EX::TyVar>(sig->as).name);
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
  TyVarEnv tv;
  Type    *t = sig_to_type(sig, tv, false);
  VarIds   q;
  for (auto &[name, v] : tv) q.push_back(std::get<TVar>(v->as).id);
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
  case EX::ExprTag::Int   : return core(CLitInt{});
  case EX::ExprTag::Real  : return core(CLitReal{});
  case EX::ExprTag::Str   : return core(CLitStr{});
  case EX::ExprTag::Extern: return core(CExtern{});
  case EX::ExprTag::Var:
  {
    auto &v = std::get<EX::ExVar>(e->as);
    // slot is rewritten if this name turns out to be overloaded
    return core(CVar{ std::string(v.name), v.name, &v.name });
  }
  case EX::ExprTag::App:
  {
    auto &ap = std::get<EX::ExApp>(e->as);
    return core(CApp{ desugar(ap.fn), desugar(ap.arg) });
  }
  case EX::ExprTag::FnDef:
  {
    auto &fn = std::get<EX::ExFnDef>(e->as);
    return core(CLam{ std::string(fn.param), desugar(fn.body) });
  }
  case EX::ExprTag::Let:
  {
    auto &lt  = std::get<EX::ExLet>(e->as);
    Type *sig = nullptr;
    if (lt.sig)
    {
      TyVarEnv tv;
      sig = sig_to_type(lt.sig, tv, false);
    }
    return core(
      CLet{ std::string(lt.var), desugar(lt.val), desugar(lt.body), sig });
  }
  case EX::ExprTag::If:
  {
    auto &i  = std::get<EX::ExIf>(e->as);
    Core *v  = core(CVar{ std::string(OP::IF), {}, nullptr });
    Core *a1 = core(CApp{ v, desugar(i.cond) });
    Core *a2 = core(CApp{ a1, desugar(i.then) });
    Core *a3 = core(CApp{ a2, desugar(i.alt) });
    return a3;
  }
  case EX::ExprTag::StructLit:
  {
    auto      &sl = std::get<EX::ExStructLit>(e->as);
    FieldInits fields;
    for (size_t i = 0; i < sl.fields.size(); ++i)
      fields.push_back(
        { std::string(sl.fields[i].name), desugar(sl.fields[i].val) });
    return core(
      CStructLit{ std::string(sl.type_name), sl.type_name, std::move(fields) });
  }
  case EX::ExprTag::VariantLit:
  {
    auto      &vl = std::get<EX::ExVariantLit>(e->as);
    FieldInits fields;
    for (size_t i = 0; i < vl.fields.size(); ++i)
      fields.push_back(
        { std::string(vl.fields[i].name), desugar(vl.fields[i].val) });
    return core(CVariantLit{ std::string(vl.type_name),
                             std::string(vl.tag),
                             vl.anchor,
                             std::move(fields) });
  }
  case EX::ExprTag::Field:
  {
    auto &fa = std::get<EX::ExField>(e->as);
    return core(CField{ desugar(fa.record), std::string(fa.field), fa.field });
  }
  case EX::ExprTag::Case:
  {
    auto    &ec    = std::get<EX::ExCase>(e->as);
    Core    *scrut = desugar(ec.scrut);
    Core    *deflt = desugar(ec.deflt);
    CoreAlts alts;
    for (size_t i = 0; i < ec.alts.size(); ++i)
    {
      EX::CaseAlt &a    = ec.alts[i];
      Core        *body = desugar(a.body);
      CoreAlt      ca;
      switch (a.kind)
      {
      case EX::AltKind::Con:
      {
        AltCon c;
        c.type_name = std::string(a.type_name);
        c.tag       = a.tag;
        for (size_t j = 0; j < a.binders.size(); ++j)
          c.binders.push_back(std::string(a.binders[j]));
        c.body = body;
        ca.as  = std::move(c);
        break;
      }
      case EX::AltKind::Int : ca.as = AltInt{ body }; break;
      case EX::AltKind::Real: ca.as = AltReal{ body }; break;
      }
      alts.push_back(std::move(ca));
    }
    return core(CCase{ scrut, deflt, std::move(alts) });
  }
  // Match is removed by the LL pass before type checking; Def / StructDecl /
  // UnionDecl are top-level only; Unknown is never produced as a body.
  case EX::ExprTag::Match:
  case EX::ExprTag::Def:
  case EX::ExprTag::StructDecl:
  case EX::ExprTag::UnionDecl:
  case EX::ExprTag::Unknown:
    UT_FAIL_MSG("desugar: unexpected node %s (should be lowered by LL)",
                EX::pprint(e->tag).c_str());
  }
  UT_FAIL_MSG("%s", "desugar: unhandled ExprTag");
  return core(CLitInt{});
}

bool
Checker::occurs_free(
  const std::string &name, Core *e)
{
  switch (e->kind())
  {
  case CKind::Var    : return std::get<CVar>(e->as).name == name;
  case CKind::LitInt :
  case CKind::LitReal:
  case CKind::LitStr :
  case CKind::Extern : return false;
  case CKind::Lam:
  {
    CLam &l = std::get<CLam>(e->as);
    if (l.param == name) return false; // shadowed
    return occurs_free(name, l.body);
  }
  case CKind::App:
  {
    CApp &a = std::get<CApp>(e->as);
    return occurs_free(name, a.fn) || occurs_free(name, a.arg);
  }
  case CKind::Let:
  {
    // name shadowed in the body; still visible in the (recursive) value
    CLet &l = std::get<CLet>(e->as);
    if (l.name == name) return occurs_free(name, l.val);
    return occurs_free(name, l.val) || occurs_free(name, l.body);
  }
  case CKind::StructLit:
    for (auto &f : std::get<CStructLit>(e->as).fields)
      if (occurs_free(name, f.second)) return true;
    return false;
  case CKind::VariantLit:
    for (auto &f : std::get<CVariantLit>(e->as).fields)
      if (occurs_free(name, f.second)) return true;
    return false;
  case CKind::Field: return occurs_free(name, std::get<CField>(e->as).record);
  case CKind::Case:
  {
    CCase &c = std::get<CCase>(e->as);
    if (occurs_free(name, c.scrut) || occurs_free(name, c.deflt)) return true;
    for (CoreAlt &alt : c.alts)
    {
      Core *body     = nullptr;
      bool  shadowed = false;
      switch (alt.kind())
      {
      case EX::AltKind::Con:
      {
        AltCon &ac = std::get<AltCon>(alt.as);
        for (auto &b : ac.binders)
          if (b == name) shadowed = true; // bound by this alt's payload
        body = ac.body;
        break;
      }
      case EX::AltKind::Int : body = std::get<AltInt>(alt.as).body; break;
      case EX::AltKind::Real: body = std::get<AltReal>(alt.as).body; break;
      }
      if (!shadowed && occurs_free(name, body)) return true;
    }
    return false;
  }
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
  switch (e->kind())
  {
  case CKind::LitInt : return con("Int");
  case CKind::LitReal: return con("Real");
  case CKind::LitStr : return con("Str");
  case CKind::Extern : return fresh(); // unifies with the declared signature

  case CKind::Var:
  {
    CVar &v  = std::get<CVar>(e->as);
    auto  it = locals.find(v.name);
    if (it != locals.end()) return instantiate(it->second);
    if (m_globals.find(v.name)) return instantiate(resolve(v.name));
    auto pit = m_prim.find(v.name);
    if (pit != m_prim.end()) return instantiate(pit->second);

    // An overloaded name: type it as a fresh variable and defer the choice of
    // overload to resolve_sites, once its operands (and result) are settled.
    if (overload_db.count(v.name))
    {
      Type *use = fresh();
      m_sites.push_back({ v.slot, use, v.name, v.anchor });
      return use;
    }

    fail(ER::Code::TYPE_UNBOUND,
         v.anchor.data() ? v.anchor : m_anchor,
         "unbound variable '%s'",
         v.name.c_str());
    return fresh();
  }

  case CKind::Lam:
  {
    CLam &l        = std::get<CLam>(e->as);
    Type *pv       = fresh();
    Env   inner    = locals;
    inner[l.param] = Scheme{ {}, pv }; // lambda params are monomorphic
    Type *bt       = infer(l.body, inner);
    return arrow(pv, bt);
  }

  case CKind::App:
  {
    CApp &ap = std::get<CApp>(e->as);
    Type *ft = infer(ap.fn, locals);
    Type *at = infer(ap.arg, locals);
    Type *rv = fresh();
    unify(ft, arrow(at, rv));
    return rv;
  }

  case CKind::Let:
  {
    CLet &lt = std::get<CLet>(e->as);
    Type *vt;
    if (occurs_free(lt.name, lt.val))
    {
      // recursive: bind the name monomorphically while inferring the value
      Type *nv     = fresh();
      Env   rec    = locals;
      rec[lt.name] = Scheme{ {}, nv };
      vt           = infer(lt.val, rec);
      unify(nv, vt);
      vt = nv;
    }
    else
    {
      vt = infer(lt.val, locals);
    }
    if (lt.sig) unify(lt.sig, vt); // pin the value's type (pattern-let subject)
    Scheme s       = generalize(locals, vt);
    Env    inner   = locals;
    inner[lt.name] = s;
    return infer(lt.body, inner);
  }

  case CKind::StructLit:
  {
    CStructLit &sl  = std::get<CStructLit>(e->as);
    UT::Vu      at  = sl.anchor.data() ? sl.anchor : m_anchor;
    auto        sit = m_structs.find(sl.type_name);
    if (sit == m_structs.end())
    {
      fail(ER::Code::TYPE_UNBOUND,
           at,
           "unknown struct type '%s'",
           sl.type_name.c_str());
      return fresh();
    }
    const StructFields &decl = sit->second;

    // Every declared field must be initialized exactly once; reject unknown and
    // duplicate fields. Each initializer is checked against its declared type.
    std::vector<bool> seen(decl.size(), false);
    for (auto &init : sl.fields)
    {
      size_t idx = decl.size();
      for (size_t k = 0; k < decl.size(); ++k)
        if (decl[k].first == init.first) idx = k;

      if (idx == decl.size())
      {
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "struct '%s' has no field '%s'",
             sl.type_name.c_str(),
             init.first.c_str());
        continue;
      }
      if (seen[idx])
      {
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "field '%s' is set more than once in '%s'",
             init.first.c_str(),
             sl.type_name.c_str());
        continue;
      }
      seen[idx] = true;
      Type *vt  = infer(init.second, locals);
      unify(decl[idx].second, vt);
    }
    for (size_t k = 0; k < decl.size(); ++k)
      if (!seen[k])
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "struct '%s' is missing field '%s'",
             sl.type_name.c_str(),
             decl[k].first.c_str());

    return con(sl.type_name);
  }

  case CKind::VariantLit:
  {
    CVariantLit &vl  = std::get<CVariantLit>(e->as);
    UT::Vu       at  = vl.anchor.data() ? vl.anchor : m_anchor;
    auto         uit = m_unions.find(vl.type_name);
    if (uit == m_unions.end())
    {
      fail(ER::Code::TYPE_UNBOUND,
           at,
           "unknown union type '%s'",
           vl.type_name.c_str());
      return fresh();
    }
    const StructFields *decl = nullptr;
    for (auto &v : uit->second)
      if (v.tag == vl.vtag) decl = &v.fields;
    if (!decl)
    {
      fail(ER::Code::TYPE_MISMATCH,
           at,
           "union '%s' has no variant '%s'",
           vl.type_name.c_str(),
           vl.vtag.c_str());
      return fresh();
    }

    // Named when any value carries a field name (then all must be set exactly
    // once, any order); positional otherwise (one value per field, in order).
    bool named = false;
    for (auto &init : vl.fields)
      if (!init.first.empty()) named = true;

    if (named)
    {
      std::vector<bool> seen(decl->size(), false);
      for (auto &init : vl.fields)
      {
        size_t idx = decl->size();
        for (size_t k = 0; k < decl->size(); ++k)
          if ((*decl)[k].first == init.first) idx = k;
        if (idx == decl->size())
        {
          fail(ER::Code::TYPE_MISMATCH,
               at,
               "variant '%s.%s' has no field '%s'",
               vl.type_name.c_str(),
               vl.vtag.c_str(),
               init.first.c_str());
          continue;
        }
        if (seen[idx])
        {
          fail(ER::Code::TYPE_MISMATCH,
               at,
               "field '%s' is set more than once in '%s.%s'",
               init.first.c_str(),
               vl.type_name.c_str(),
               vl.vtag.c_str());
          continue;
        }
        seen[idx] = true;
        unify((*decl)[idx].second, infer(init.second, locals));
      }
      for (size_t k = 0; k < decl->size(); ++k)
        if (!seen[k])
          fail(ER::Code::TYPE_MISMATCH,
               at,
               "variant '%s.%s' is missing field '%s'",
               vl.type_name.c_str(),
               vl.vtag.c_str(),
               (*decl)[k].first.c_str());
    }
    else if (vl.fields.size() != decl->size())
    {
      fail(ER::Code::TYPE_MISMATCH,
           at,
           "variant '%s.%s' takes %zu payload field(s), but %zu given",
           vl.type_name.c_str(),
           vl.vtag.c_str(),
           decl->size(),
           vl.fields.size());
    }
    else
    {
      for (size_t k = 0; k < decl->size(); ++k)
        unify((*decl)[k].second, infer(vl.fields[k].second, locals));
    }

    return con(vl.type_name);
  }

  case CKind::Field:
  {
    CField &fld = std::get<CField>(e->as);
    UT::Vu  at  = fld.anchor.data() ? fld.anchor : m_anchor;
    Type   *rt  = prune(infer(fld.record, locals));
    if (rt->kind() != Kind::Con
        || !m_structs.count(std::get<TCon>(rt->as).name))
    {
      fail(ER::Code::TYPE_MISMATCH,
           at,
           "cannot access field '%s': '%s' is not a known struct type",
           fld.field.c_str(),
           show(rt).c_str());
      return fresh();
    }
    const std::string &rname = std::get<TCon>(rt->as).name;
    for (auto &f : m_structs.at(rname))
      if (f.first == fld.field) return f.second;

    fail(ER::Code::TYPE_UNBOUND,
         at,
         "struct '%s' has no field '%s'",
         rname.c_str(),
         fld.field.c_str());
    return fresh();
  }

  case CKind::Case:
  {
    CCase &cc = std::get<CCase>(e->as);
    Type  *st = infer(cc.scrut, locals); // the scrutinee
    Type  *rt = fresh(); // every arm and the default share one type
    for (CoreAlt &alt : cc.alts)
    {
      Env   inner = locals;
      Core *body  = nullptr;
      switch (alt.kind())
      {
      case EX::AltKind::Int:
        unify(st, con("Int"));
        body = std::get<AltInt>(alt.as).body;
        break;
      case EX::AltKind::Real:
        unify(st, con("Real"));
        body = std::get<AltReal>(alt.as).body;
        break;
      case EX::AltKind::Con:
      {
        // The scrutinee is the constructor's union type; bind its payload from
        // the matched variant's declared field types.
        AltCon &ac = std::get<AltCon>(alt.as);
        unify(st, con(ac.type_name));
        auto uit = m_unions.find(ac.type_name);
        if (uit != m_unions.end() && ac.tag < uit->second.size())
        {
          const StructFields &decl = uit->second[ac.tag].fields;
          for (size_t i = 0; i < ac.binders.size() && i < decl.size(); ++i)
            if (!ac.binders[i].empty())
              inner[ac.binders[i]] = Scheme{ {}, decl[i].second };
        }
        body = ac.body;
        break;
      }
      }
      unify(rt, infer(body, inner));
    }
    unify(rt, infer(cc.deflt, locals)); // the default arm
    return rt;
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
  GlobalEntry &g = *m_globals.find(name);
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
    UT::Vu save_a = m_anchor;
    size_t save_l = m_line;
    m_anchor      = g.def->name;
    m_line        = line_of(g.def->name);

    Env    empty;
    size_t base = m_sites.size();
    Type  *t    = infer(g.body, empty);
    resolve_sites(base); // settle this body's overloads before judging its type
    t = prune(t);

    if (t->kind() != Kind::Con)
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

// Resolve the overload sites collected since index `from` (one inference
// scope's worth), then drop them. For each site we force the use type into an
// arrow chain of the operator's arity -- so an under-applied use still exposes
// operand variables -- default any still-unconstrained operand to Int
// (SML-style), then pick the overload whose operand types match and rewrite the
// EX Var name to its implementation key.
void
Checker::resolve_sites(
  size_t from)
{
  for (size_t i = from; i < m_sites.size(); ++i)
  {
    const ResolveSite &s = m_sites[i];

    const std::vector<Overload> *ovs = UT::try_lookup(overload_db, s.base);
    UT_FAIL_IF(!ovs
               || ovs->empty()); // a site is only made for overloaded names
    size_t arity = ovs->front().sig.size() - 1;

    // Shape the use as (a1 -> ... -> ak -> r) so the operands are reachable
    // whether the operator was fully applied, partially applied, or passed
    // bare.
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
      if (a->kind() == Kind::Var && !std::get<TVar>(a->as).rigid)
      {
        std::get<TVar>(a->as).ref = con(OP::TY_INT);
        a                         = prune(a);
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
        if (args[k]->kind() != Kind::Con
            || std::get<TCon>(args[k]->as).name != o.sig[k])
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
    *s.slot = UT::Vu{ hit->mono.c_str(), hit->mono.size() };
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
  // Phase A: register struct types and collect value globals. Structs are
  // registered up front so a global may reference a struct declared later.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::StructDecl) continue;
    EX::ExStructDecl &sd = std::get<EX::ExStructDecl>(e->as);
    StructFields      flds;
    for (size_t f = 0; f < sd.fields.size(); ++f)
    {
      TyVarEnv tv;
      Type    *ft = sig_to_type(sd.fields[f].ty, tv, false);
      flds.push_back({ std::string(sd.fields[f].name), ft });
    }
    m_structs[std::string(sd.name)] = std::move(flds);
  }

  // ... and union types, likewise registered up front for forward references.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::UnionDecl) continue;
    EX::ExUnionDecl &ud = std::get<EX::ExUnionDecl>(e->as);
    Variants         variants;
    for (size_t v = 0; v < ud.variants.size(); ++v)
    {
      StructFields flds;
      for (size_t f = 0; f < ud.variants[v].fields.size(); ++f)
      {
        TyVarEnv tv;
        Type    *ft = sig_to_type(ud.variants[v].fields[f].ty, tv, false);
        flds.push_back({ std::string(ud.variants[v].fields[f].name), ft });
      }
      variants.push_back({ std::string(ud.variants[v].tag), std::move(flds) });
    }
    m_unions[std::string(ud.name)] = std::move(variants);
  }

  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::Def) continue;
    EX::ExDef  &d = std::get<EX::ExDef>(e->as);
    GlobalEntry g;
    g.def  = &d;
    g.body = desugar(d.def);
    m_globals.add(std::move(g));
  }

  // Phase B: resolve every global's type (on-demand, DAG, cycle-checked).
  for (GlobalEntry &g : m_globals) resolve(std::string(g.def->name));

  // Phase C: check every body against its resolved/declared type, then resolve
  // the overloaded uses it contains (their operand types are now known). A
  // signatured global is only inferred here, so this is where its overloads are
  // resolved; a signature-less one was already resolved in Phase B, and is
  // re-resolved to the same impl here, which is idempotent.
  for (GlobalEntry &g : m_globals)
  {
    m_anchor = g.def->name;
    m_line   = line_of(g.def->name);

    Env    empty;
    size_t base = m_sites.size();
    Type  *bt   = infer(g.body, empty);

    if (g.def->sig)
    {
      TyVarEnv tv;
      Type    *rigid = sig_to_type(g.def->sig, tv, true);
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
  UT::Vu anchor)
{
  if (!anchor.data() || !m_src.data()) return 0;
  if (anchor.data() < m_src.data()) return 0;
  size_t off  = (size_t)(anchor.data() - m_src.data());
  size_t line = 1;
  for (size_t i = 0; i < off && i < m_src.size(); ++i)
    if (m_src.data()[i] == '\n') line++;
  return line;
}

std::string
Checker::show(
  Type *t)
{
  t = prune(t);
  switch (t->kind())
  {
  case Kind::Con: return std::get<TCon>(t->as).name;
  case Kind::Var:
  {
    TVar &v = std::get<TVar>(t->as);
    if (v.rigid) return "`" + v.name;
    return "?" + std::to_string(v.id);
  }
  case Kind::Arrow:
  {
    TArrow     &a = std::get<TArrow>(t->as);
    std::string l = show(a.from);
    if (prune(a.from)->kind() == Kind::Arrow) l = "(" + l + ")";
    return l + " -> " + show(a.to);
  }
  }
  return "?";
}

void
Checker::fail(
  ER::Code code, UT::Vu anchor, const char *fmt, ...)
{
  va_list args;
  va_start(args, fmt);
  va_list copy;
  va_copy(copy, args);
  int len = std::vsnprintf(nullptr, 0, fmt, args);
  va_end(args);

  UT::Vu msg{ "<message formatting failed>" };
  if (len >= 0)
  {
    char *mem = (char *)m_arena.alloc((size_t)len + 1);
    std::vsnprintf(mem, (size_t)len + 1, fmt, copy);
    msg = UT::Vu{ mem, (size_t)len };
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
  Type *tv       = fresh();
  m_prim[OP::IF] = Scheme{ { std::get<TVar>(tv->as).id },
                           arrow(con("Int"), arrow(tv, arrow(tv, tv))) };
}

} // namespace

std::vector<ER::Diagnostic>
check(
  EX::Exprs &exprs, AR::Arena &arena, UT::Vu src)
{
  Checker c{ arena, src };
  c.run(exprs);
  return c.m_diags;
}

} // namespace TC
