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

  // Declared effects and their operations (M3 effect rows). `m_effects` is the
  // set of declared `@effect` names (so a `<E>` row annotation can validate its
  // labels); `m_op_effect` maps each operation to its owning effect, so a
  // handler knows which effect its clauses discharge from the ambient row.
  std::unordered_set<std::string>              m_effects;
  std::unordered_map<std::string, std::string> m_op_effect;
  // Declared parameter count (arity) of every nominal type, keyed by name.
  // Populated before any field type is built so applied cons can be
  // arity-checked even across forward references. Built-ins (Int/Str) are
  // absent => arity 0.
  std::unordered_map<std::string, size_t> m_arity;

  // Transparent type aliases: `$ Name : @alias = target` registers Name -> the
  // target's EX type. sig_to_type resolves an alias name to its target wherever
  // it is written, so the two are interchangeable. `m_alias_depth` guards a
  // cyclic alias (A = B, B = A) from recursing forever.
  std::unordered_map<std::string, EX::Ty *> m_aliases;
  size_t                                    m_alias_depth = 0;

  // current diagnostic anchor (the global under check)
  UT::Vu m_anchor;
  size_t m_line = 0;

  // overload resolution: deferred sites collected during an inference scope and
  // resolved at its end (see infer's Var case / resolve_sites).
  ResolveSites m_sites;

  // A deferred user-overload resolution: a use MR left as an overload set. Like
  // an operator site it is typed as a fresh `use` var; resolve_user_sites later
  // picks the one candidate whose (instantiated) type fits and writes its
  // mangled name to `slot` (the EX::ExOverload's `chosen` field).
  struct UserSite
  {
    UT::Vu                *slot;
    Type                  *use;
    const UT::Vec<UT::Vu> *cands;
    std::string            name;
    UT::Vu                 anchor;
  };
  std::vector<UserSite> m_usites;

  // When non-null, unify records every variable it binds here so a speculative
  // unification (a candidate trial in resolve_user_sites) can be rolled back.
  std::vector<Type *> *m_trail = nullptr;

  // An unqualified struct/union literal whose type is inferred from context.
  // Like an overload site, it is typed as a fresh `use` var and settled once
  // unification binds it to a concrete nominal type (see resolve_lit_sites).
  struct LitSite
  {
    bool                                        is_struct;
    Type                                       *use;
    UT::Vu                                      anchor;
    std::vector<std::pair<std::string, Type *>> fields; // (name, value type)
    std::string                                 vtag;   // variant only
    EX::ExStructLit                            *exs = nullptr; // write-back
    EX::ExVariantLit                           *exv = nullptr; // write-back
  };
  std::vector<LitSite> m_lit_sites;

  // A deferred field access `record.field`. Field access needs the receiver's
  // struct type known, but that type may settle only later -- after an overload
  // is resolved, or a parameter's signature type is applied. So a use is typed
  // as a fresh `result` var here and checked once its `record` type is concrete
  // (see resolve_field_sites). Sites are resolved in recording order, which is
  // innermost-first, so a chain `a.b.c` settles `a.b` before `(a.b).c`.
  struct FieldSite
  {
    Type       *record; // receiver type (a var until its context settles it)
    std::string field;
    UT::Vu      anchor;
    Type       *result; // unified with the field's type once resolved
  };
  std::vector<FieldSite> m_field_sites;

  // type constructors
  Type *fresh(bool rigid = false, std::string name = "");
  Type *con(std::string name, std::vector<Type *> args = {});
  Type *arrow(Type *from, Type *to, Type *eff);
  Type *arrow(Type *from, Type *to); // eff defaults to a fresh row variable
  Type *row_empty();
  Type *row_extend(std::string label, Type *rest);
  bool  unify_row(Type *a, Type *b);
  bool  subrow(Type *sub, Type *super);
  bool  rewrite_row(Type *row, const std::string &label, Type *&found_rest);
  Core *core(CoreData as);

  // substitution / nominal instantiation
  Type *subst(Type *t, const Subst &s);
  Subst fresh_subst(const VarIds &params, std::vector<Type *> &args_out);

  // union-find
  Type *prune(Type *t);
  bool  occurs(Type *var, Type *t);
  bool  unify(Type *a, Type *b);
  bool  try_unify(Type *a, Type *b); // speculative: rolled back, never reported

  // schemes
  void   free_vars(Type *t, VarIds &out);
  void   env_free_vars(const Env &env, VarIds &out);
  Scheme generalize(const Env &locals, Type *t);
  Type  *instantiate(const Scheme &s);

  // signatures
  Type *
  sig_to_type(EX::Ty *sig, TyVarEnv &tv, bool rigid, VarIds *order = nullptr);
  Scheme scheme_of_sig(EX::Ty *sig);

  // desugar + inference
  Core *desugar(EX::Expr *e);
  bool  occurs_free(const std::string &name, Core *e);
  Type *infer(Core *e, Env &locals, Type *amb);
  Type *infer_against(Core *e, Type *expected, Env &locals, Type *amb);

  // resolution
  Scheme resolve(const std::string &name);
  void   resolve_sites(size_t from);
  void   resolve_user_sites(size_t from);
  void   resolve_lit_sites(size_t from);
  void   resolve_field_sites(size_t from);
  UT::Vu intern(const std::string &s);

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
  std::string name, std::vector<Type *> args)
{
  m_types.push_back(Type{ TCon{ std::move(name), std::move(args) } });
  return &m_types.back();
}

Type *
Checker::arrow(
  Type *from, Type *to, Type *eff)
{
  m_types.push_back(Type{ TArrow{ from, to, eff } });
  return &m_types.back();
}

// A function type whose latent effect is left open: a fresh row variable. Used
// for shaped/probed arrows and effect-polymorphic primitives -- the variable
// unifies with whatever ambient effect the use site demands.
Type *
Checker::arrow(
  Type *from, Type *to)
{
  return arrow(from, to, fresh());
}

Type *
Checker::row_empty()
{
  m_types.push_back(Type{ TRowEmpty{} });
  return &m_types.back();
}

Type *
Checker::row_extend(
  std::string label, Type *rest)
{
  m_types.push_back(Type{ TRowExtend{ std::move(label), rest } });
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
    return occurs(var, a.from) || occurs(var, a.to) || occurs(var, a.eff);
  }
  if (t->kind() == Kind::Con)
  {
    for (Type *arg : std::get<TCon>(t->as).args)
      if (occurs(var, arg)) return true;
  }
  if (t->kind() == Kind::RowExtend)
    return occurs(var, std::get<TRowExtend>(t->as).rest);
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
    if (m_trail) m_trail->push_back(a);
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
    if (m_trail) m_trail->push_back(b);
    std::get<TVar>(b->as).ref = a;
    return true;
  }

  if (a->kind() == Kind::Con && b->kind() == Kind::Con)
  {
    TCon &ca = std::get<TCon>(a->as);
    TCon &cb = std::get<TCon>(b->as);
    if (ca.name == cb.name && ca.args.size() == cb.args.size())
    {
      bool ok = true;
      for (size_t i = 0; i < ca.args.size(); ++i)
        ok &= unify(ca.args[i], cb.args[i]);
      return ok;
    }
  }

  if (a->kind() == Kind::Arrow && b->kind() == Kind::Arrow)
  {
    TArrow &fa = std::get<TArrow>(a->as);
    TArrow &fb = std::get<TArrow>(b->as);
    bool    l  = unify(fa.from, fb.from);
    bool    r  = unify(fa.to, fb.to);
    bool    e  = unify(fa.eff, fb.eff);
    return l && r && e;
  }

  if (a->kind() == Kind::RowEmpty && b->kind() == Kind::RowEmpty) return true;

  if (a->kind() == Kind::RowExtend || b->kind() == Kind::RowExtend)
    return unify_row(a, b);

  fail(ER::Code::TYPE_MISMATCH,
       m_anchor,
       "type mismatch: expected '%s', got '%s'",
       show(a).c_str(),
       show(b).c_str());
  return false;
}

// Unify two effect rows, at least one a `<label | rest>` extension (Var-vs-row
// and empty-vs-empty are handled by unify's earlier arms). Following Leijen's
// scoped-label discipline: pull the head label of `a` out of `b` (rewrite_row),
// then unify the two remaining tails. A label absent from a closed `b` is the
// unhandled-effect error.
bool
Checker::unify_row(
  Type *a, Type *b)
{
  // Normalize so `a` is the extension we decompose.
  if (a->kind() != Kind::RowExtend) std::swap(a, b);
  TRowExtend &ea = std::get<TRowExtend>(a->as);

  Type *b_rest = nullptr;
  if (!rewrite_row(b, ea.label, b_rest))
  {
    fail(ER::Code::TYPE_MISMATCH,
         m_anchor,
         "effect '%s' is performed but not handled (expected '%s', got '%s')",
         ea.label.c_str(),
         show(b).c_str(),
         show(a).c_str());
    return false;
  }
  return unify(ea.rest, b_rest);
}

// Effect subsumption: require `sub` to be a SUBROW of `super` -- every effect
// the callee performs (`sub`) must be permitted by the ambient (`super`). Used
// at a call site so a pure (or smaller-effect) function fits a larger ambient.
//   - empty `sub` fits any `super` (a pure call performs nothing);
//   - a labelled `sub` peels each label out of `super` (which may grow an open
//     tail) and recurses, so a missing label in a closed `super` is the
//     unhandled-effect error;
//   - a variable `sub` (an unknown callee effect) is tied to `super` by
//     equality -- sound, since `super` is a subrow of itself.
bool
Checker::subrow(
  Type *sub, Type *super)
{
  sub = prune(sub);
  if (sub->kind() == Kind::RowEmpty) return true;
  if (sub->kind() == Kind::Var) return unify(sub, super);
  if (sub->kind() == Kind::RowExtend)
  {
    TRowExtend &r          = std::get<TRowExtend>(sub->as);
    Type       *super_rest = nullptr;
    if (!rewrite_row(super, r.label, super_rest))
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "effect '%s' is performed but not handled (it is not in '%s')",
           r.label.c_str(),
           show(super).c_str());
      return false;
    }
    return subrow(r.rest, super_rest);
  }
  return unify(sub, super); // non-row (shouldn't occur); fall back to equality
}

// Bring an occurrence of effect `label` to the head of `row`, yielding the row
// that remains once it is removed (`found_rest`). If `row`'s tail is a variable
// and `label` is absent, extend that variable with `<label | fresh>` and report
// the fresh tail -- this is how an open row grows to accept a new effect.
// Returns false iff `row` is closed and lacks `label`.
bool
Checker::rewrite_row(
  Type *row, const std::string &label, Type *&found_rest)
{
  row = prune(row);
  if (row->kind() == Kind::RowExtend)
  {
    TRowExtend &r = std::get<TRowExtend>(row->as);
    if (r.label == label)
    {
      found_rest = r.rest;
      return true;
    }
    Type *deeper = nullptr;
    if (!rewrite_row(r.rest, label, deeper)) return false;
    // Keep this head, splice the rest found below it.
    found_rest = row_extend(r.label, deeper);
    return true;
  }
  if (row->kind() == Kind::Var && !std::get<TVar>(row->as).rigid)
  {
    Type *tail = fresh();
    if (m_trail) m_trail->push_back(row);
    std::get<TVar>(row->as).ref = row_extend(label, tail);
    found_rest                  = tail;
    return true;
  }
  // Closed row (empty or rigid var) without the label.
  return false;
}

// Speculatively unify `a` and `b`, then undo every binding it made and discard
// any diagnostics it produced -- the union-find is left exactly as it was. Used
// to probe whether an overload candidate fits without committing to it. (Path
// compression by prune may shorten already-bound chains; that is harmless,
// since nothing pre-existing points into the freshly-bound vars we roll back.)
bool
Checker::try_unify(
  Type *a, Type *b)
{
  std::vector<Type *>  trail;
  std::vector<Type *> *save  = m_trail;
  size_t               diag0 = m_diags.size();
  m_trail                    = &trail;
  bool ok                    = unify(a, b);
  m_trail                    = save;
  for (Type *v : trail) std::get<TVar>(v->as).ref = nullptr;
  m_diags.resize(diag0);
  return ok;
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
  case Kind::Con:
  {
    for (Type *arg : std::get<TCon>(t->as).args) free_vars(arg, out);
    break;
  }
  case Kind::Arrow:
  {
    TArrow &a = std::get<TArrow>(t->as);
    free_vars(a.from, out);
    free_vars(a.to, out);
    free_vars(a.eff, out);
    break;
  }
  case Kind::RowExtend: free_vars(std::get<TRowExtend>(t->as).rest, out); break;
  case Kind::RowEmpty : break;
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
  for (const UserSite &s : m_usites) free_vars(s.use, ev);
  // A pending field access is likewise monomorphic until its receiver settles.
  for (const FieldSite &s : m_field_sites) free_vars(s.result, ev);

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

// Copy `t`, replacing any type-variable whose id is in `s` with its mapping and
// rebuilding constructors/arrows around the result. Variables not in `s`, and
// constructors with no substituted descendant, are shared structurally.
Type *
Checker::subst(
  Type *t, const Subst &s)
{
  t = prune(t);
  switch (t->kind())
  {
  case Kind::Var:
  {
    auto it = s.find(std::get<TVar>(t->as).id);
    return it == s.end() ? t : it->second;
  }
  case Kind::Con:
  {
    TCon &c = std::get<TCon>(t->as);
    if (c.args.empty()) return t;
    std::vector<Type *> args;
    args.reserve(c.args.size());
    for (Type *a : c.args) args.push_back(subst(a, s));
    return con(c.name, std::move(args));
  }
  case Kind::Arrow:
  {
    TArrow &a = std::get<TArrow>(t->as);
    return arrow(subst(a.from, s), subst(a.to, s), subst(a.eff, s));
  }
  case Kind::RowExtend:
  {
    TRowExtend &r = std::get<TRowExtend>(t->as);
    return row_extend(r.label, subst(r.rest, s));
  }
  case Kind::RowEmpty: return t;
  }
  return t;
}

// Make a fresh unification variable for each parameter id, returning the
// id->var substitution and (in `args_out`) the fresh vars in parameter order --
// exactly the type arguments of the instantiated nominal type.
Subst
Checker::fresh_subst(
  const VarIds &params, std::vector<Type *> &args_out)
{
  Subst sub;
  args_out.clear();
  args_out.reserve(params.size());
  for (int id : params)
  {
    Type *v = fresh();
    sub[id] = v;
    args_out.push_back(v);
  }
  return sub;
}

Type *
Checker::instantiate(
  const Scheme &s)
{
  if (s.vars.empty()) return s.type;

  Subst sub;
  for (int id : s.vars) sub[id] = fresh();
  return subst(s.type, sub);
}

/*------------------------------------------------------------------------------
 *\SIGNATURES
 *-----------------------------------------------------------------------------*/

Type *
Checker::sig_to_type(
  EX::Ty *sig, TyVarEnv &tv, bool rigid, VarIds *order)
{
  switch (sig->tag)
  {
  case EX::TyTag::Con:
  {
    auto       &c    = std::get<EX::TyCon>(sig->as);
    std::string name = std::string(c.name);

    // A transparent alias used without arguments resolves to its target.
    // (Aliases are nullary for now; an alias applied to type arguments is left
    // as an ordinary con, so the arity check below reports it.)
    if (c.args.empty() && m_aliases.count(name) && m_alias_depth < 64)
    {
      m_alias_depth++;
      Type *r = sig_to_type(m_aliases.at(name), tv, rigid, order);
      m_alias_depth--;
      return r;
    }

    size_t arity = m_arity.count(name) ? m_arity.at(name) : 0;
    size_t given = c.args.size();

    // Either spell out every argument, or none (then infer them with fresh
    // holes); any other count is a kind/arity error. Built-ins have arity 0.
    // Reported only on the non-rigid pass so a signature checked twice (its
    // scheme in phase B, then rigidly against the body in phase C) faults once.
    if (!rigid && given != 0 && given != arity)
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "type '%s' expects %zu argument(s), but %zu given",
           name.c_str(),
           arity,
           given);
      return fresh();
    }

    std::vector<Type *> args;
    args.reserve(arity);
    if (given == arity)
      for (EX::Ty *a : c.args) args.push_back(sig_to_type(a, tv, rigid, order));
    else // given == 0, arity > 0: infer each argument
      for (size_t i = 0; i < arity; ++i) args.push_back(fresh());
    return con(std::move(name), std::move(args));
  }
  case EX::TyTag::Var:
  {
    std::string nm = std::string(std::get<EX::TyVar>(sig->as).name);
    auto        it = tv.find(nm);
    if (it != tv.end()) return it->second;
    Type *v = fresh(rigid, nm);
    tv[nm]  = v;
    if (order) order->push_back(std::get<TVar>(v->as).id);
    return v;
  }
  case EX::TyTag::Arrow:
  {
    auto &ar = std::get<EX::TyArrow>(sig->as);
    Type *f  = sig_to_type(ar.from, tv, rigid, order);
    Type *t  = sig_to_type(ar.to, tv, rigid, order);

    // The effect row: the tail (a row variable, shared with type vars via `tv`
    // so it skolemizes/generalizes consistently) or the empty closed row, then
    // each written label extended onto it. A label must name a declared effect.
    Type *eff;
    if (ar.eff_tail.data() && ar.eff_tail.size())
    {
      std::string nm(ar.eff_tail);
      auto        it = tv.find(nm);
      if (it != tv.end())
        eff = it->second;
      else
      {
        eff    = fresh(rigid, nm);
        tv[nm] = eff;
        if (order) order->push_back(std::get<TVar>(eff->as).id);
      }
    }
    else
      eff = row_empty();
    for (UT::Vu lab : ar.eff_labels)
    {
      std::string name(lab);
      if (!rigid && !m_effects.count(name))
        fail(ER::Code::TYPE_UNBOUND,
             m_anchor,
             "unknown effect '%s' in effect row",
             name.c_str());
      eff = row_extend(name, eff);
    }
    return arrow(f, t, eff);
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
  case EX::ExprTag::Unit  : return core(CLitUnit{});
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
    return core(CStructLit{
      std::string(sl.type_name), sl.type_name, std::move(fields), &sl });
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
                             std::move(fields),
                             &vl });
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
  case EX::ExprTag::Overload:
  {
    // A use MR left as an overload set: type it as a fresh var and let
    // resolve_user_sites pick the candidate that fits, writing its name to
    // `chosen` (which the slot points at, and IT later reads).
    auto &ov = std::get<EX::ExOverload>(e->as);
    CVar  v;
    v.name     = std::string(ov.name);
    v.anchor   = ov.anchor;
    v.slot     = &ov.chosen;
    v.overload = &ov.candidates;
    return core(std::move(v));
  }
  // Handlers: type-checking lands in increment 3c (this stub keeps 3a's
  // parser-only milestone honest -- a handler program aborts here until then).
  case EX::ExprTag::Handle:
  {
    auto   &h = std::get<EX::ExHandle>(e->as);
    CHandle ch;
    ch.body = desugar(h.body);
    ch.k    = std::string(h.k);
    for (auto &c : h.clauses)
      ch.clauses.push_back(
        { std::string(c.op), std::string(c.arg), desugar(c.body) });
    if (h.else_body)
    {
      ch.els_var = std::string(h.else_var);
      ch.els     = desugar(h.else_body);
    }
    return core(std::move(ch));
  }
  // Match is removed by the LL pass before type checking; Def / StructDecl /
  // UnionDecl are top-level only; Unknown is never produced as a body.
  case EX::ExprTag::Match:
  case EX::ExprTag::Def:
  case EX::ExprTag::StructDecl:
  case EX::ExprTag::UnionDecl:
  case EX::ExprTag::AliasDecl:
  case EX::ExprTag::EffectDecl:
  case EX::ExprTag::ModDecl:
  case EX::ExprTag::Import:
  case EX::ExprTag::Vis:
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
  case CKind::LitUnit:
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
  case CKind::Handle:
  {
    CHandle &h = std::get<CHandle>(e->as);
    if (occurs_free(name, h.body)) return true;
    if (h.els && h.els_var != name && occurs_free(name, h.els)) return true;
    for (CHandleClause &c : h.clauses)
      if (c.arg != name && h.k != name && occurs_free(name, c.body))
        return true;
    return false;
  }
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

// `amb` is the ambient effect row: the effects this expression is allowed to
// perform. Performing an operation injects its effect into `amb` (an App unifies
// the callee's latent effect with `amb`); a handler discharges the effects it
// handles by typing its body under `<handled... | amb>`; a lambda body runs
// under its own fresh ambient (the arrow's latent effect). A top-level body is
// typed under the empty closed row, so an unhandled effect fails to unify.
Type *
Checker::infer(
  Core *e, Env &locals, Type *amb)
{
  switch (e->kind())
  {
  case CKind::LitInt : return con(OP::TY_INT);
  case CKind::LitReal: return con(OP::TY_REAL);
  case CKind::LitStr : return con(OP::TY_STR);
  case CKind::LitUnit: return con(OP::TY_UNIT);
  case CKind::Extern : return fresh(); // unifies with the declared signature

  case CKind::Handle:
  {
    // The handler discharges the effects named by its clauses: the body runs
    // under `<handled... | amb>`, the handle expression itself under `amb`. else
    // x = e: x has the body's type, e has `result`; with no else the body's
    // result IS the handler's. Each clause `is op a = e` for `op : A -> B` binds
    // a : A and k : B ->^amb result (resuming is deep, so k runs under amb).
    CHandle &h      = std::get<CHandle>(e->as);
    Type    *result = fresh();

    // Build the body's ambient: amb extended with each DISTINCT handled effect
    // (several clauses may handle one effect, e.g. get/put both belong to State;
    // adding the label once keeps the row in step with what the body performs).
    Type *inner = amb;
    {
      std::unordered_set<std::string> seen;
      for (CHandleClause &c : h.clauses)
      {
        auto oe = m_op_effect.find(c.op);
        if (oe != m_op_effect.end() && seen.insert(oe->second).second)
          inner = row_extend(oe->second, inner);
      }
    }
    Type *tbody = infer(h.body, locals, inner);
    if (h.els)
    {
      Env e2        = locals;
      e2[h.els_var] = Scheme{ {}, tbody };
      unify(infer(h.els, e2, amb), result);
    }
    else
      unify(tbody, result);
    for (CHandleClause &c : h.clauses)
    {
      auto pit = m_prim.find(c.op);
      if (pit == m_prim.end())
      {
        fail(ER::Code::TYPE_MISMATCH,
             m_anchor,
             "unknown effect operation '%s' in handler",
             c.op.c_str());
        continue;
      }
      Type *opty = instantiate(pit->second);
      Type *A    = fresh();
      Type *B    = fresh();
      unify(opty, arrow(A, B));
      Env e2    = locals;
      e2[c.arg] = Scheme{ {}, A };
      e2[h.k]   = Scheme{ {}, arrow(B, result, amb) };
      unify(infer(c.body, e2, amb), result);
    }
    return result;
  }

  case CKind::Var:
  {
    CVar &v = std::get<CVar>(e->as);

    // An overload set (from MR): type as a fresh var and defer the choice to
    // resolve_user_sites, once the use's type is settled by its context.
    if (v.overload)
    {
      Type *use = fresh();
      m_usites.push_back({ v.slot, use, v.overload, v.name, v.anchor });
      return use;
    }

    auto it = locals.find(v.name);
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
    // Constructing a closure performs no effect (the lambda itself is pure under
    // `amb`); its body runs under a fresh ambient that becomes the arrow's
    // latent effect.
    CLam &l        = std::get<CLam>(e->as);
    Type *pv       = fresh();
    Type *e_body   = fresh();
    Env   inner    = locals;
    inner[l.param] = Scheme{ {}, pv }; // lambda params are monomorphic
    Type *bt       = infer(l.body, inner, e_body);
    return arrow(pv, bt, e_body);
  }

  case CKind::App:
  {
    // The callee may perform AT MOST what the ambient allows: its latent effect
    // must be a SUBROW of the ambient (subsumption), not equal to it. So a pure
    // (or smaller-effect) function is callable in any larger effect context.
    // Argument and result still unify exactly. (Operations are Apps whose callee
    // carries `<L|mu>`, so subsuming it forces L into amb.)
    CApp &ap = std::get<CApp>(e->as);
    Type *ft = infer(ap.fn, locals, amb);
    Type *at = infer(ap.arg, locals, amb);
    Type *rv = fresh();
    Type *ef = fresh(); // the callee's latent effect, extracted then subsumed
    unify(ft, arrow(at, rv, ef));
    subrow(ef, amb);
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
      vt           = infer(lt.val, rec, amb);
      unify(nv, vt);
      vt = nv;
    }
    else
    {
      vt = infer(lt.val, locals, amb);
    }
    if (lt.sig) unify(lt.sig, vt); // pin the value's type (pattern-let subject)
    Scheme s       = generalize(locals, vt);
    Env    inner   = locals;
    inner[lt.name] = s;
    return infer(lt.body, inner, amb);
  }

  case CKind::StructLit:
  {
    CStructLit &sl = std::get<CStructLit>(e->as);
    UT::Vu      at = sl.anchor.data() ? sl.anchor : m_anchor;

    // Bare `.{...}`: defer to a lit site, typed as a fresh var that later
    // unification (an annotation or an enclosing field) must bind to a struct.
    if (sl.type_name.empty())
    {
      LitSite s;
      s.is_struct = true;
      s.use       = fresh();
      s.anchor    = at;
      s.exs       = sl.ex;
      for (auto &init : sl.fields)
        s.fields.push_back({ init.first, infer(init.second, locals, amb) });
      Type *use = s.use;
      m_lit_sites.push_back(std::move(s));
      return use;
    }

    auto sit = m_structs.find(sl.type_name);
    if (sit == m_structs.end())
    {
      fail(ER::Code::TYPE_UNBOUND,
           at,
           "unknown struct type '%s'",
           sl.type_name.c_str());
      return fresh();
    }
    const StructFields &decl = sit->second.fields;

    // Instantiate the struct's parameters fresh for this use; field types are
    // read through this substitution and the result is `con(name, args)`.
    std::vector<Type *> args;
    Subst               sub = fresh_subst(sit->second.params, args);

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
      Type *vt  = infer(init.second, locals, amb);
      unify(subst(decl[idx].second, sub), vt);
    }
    for (size_t k = 0; k < decl.size(); ++k)
      if (!seen[k])
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "struct '%s' is missing field '%s'",
             sl.type_name.c_str(),
             decl[k].first.c_str());

    return con(sl.type_name, args);
  }

  case CKind::VariantLit:
  {
    CVariantLit &vl = std::get<CVariantLit>(e->as);
    UT::Vu       at = vl.anchor.data() ? vl.anchor : m_anchor;

    // Bare `.Tag...`: defer to a lit site keyed by the tag; later unification
    // binds its `use` var to the union the tag belongs to.
    if (vl.type_name.empty())
    {
      LitSite s;
      s.is_struct = false;
      s.use       = fresh();
      s.anchor    = at;
      s.vtag      = vl.vtag;
      s.exv       = vl.ex;
      for (auto &init : vl.fields)
        s.fields.push_back({ init.first, infer(init.second, locals, amb) });
      Type *use = s.use;
      m_lit_sites.push_back(std::move(s));
      return use;
    }

    auto uit = m_unions.find(vl.type_name);
    if (uit == m_unions.end())
    {
      fail(ER::Code::TYPE_UNBOUND,
           at,
           "unknown union type '%s'",
           vl.type_name.c_str());
      return fresh();
    }
    const StructFields *decl = nullptr;
    for (auto &v : uit->second.variants)
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

    // Instantiate the union's parameters fresh; payload types are read through
    // this substitution and the result is `con(name, args)`.
    std::vector<Type *> args;
    Subst               sub = fresh_subst(uit->second.params, args);

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
        unify(subst((*decl)[idx].second, sub), infer(init.second, locals, amb));
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
        unify(subst((*decl)[k].second, sub),
              infer(vl.fields[k].second, locals, amb));
    }

    return con(vl.type_name, args);
  }

  case CKind::Field:
  {
    // Defer: the receiver's type may not be concrete yet (e.g. it is the result
    // of an as-yet-unresolved overload). Type the access as a fresh var and
    // settle it in resolve_field_sites once the receiver is known.
    CField &fld = std::get<CField>(e->as);
    UT::Vu  at  = fld.anchor.data() ? fld.anchor : m_anchor;
    Type   *rt  = infer(fld.record, locals, amb);
    Type   *res = fresh();
    m_field_sites.push_back({ rt, fld.field, at, res });
    return res;
  }

  case CKind::Case:
  {
    CCase &cc = std::get<CCase>(e->as);
    Type  *st = infer(cc.scrut, locals, amb); // the scrutinee
    Type  *rt = fresh(); // every arm and the default share one type
    for (CoreAlt &alt : cc.alts)
    {
      Env   inner = locals;
      Core *body  = nullptr;
      switch (alt.kind())
      {
      case EX::AltKind::Int:
        unify(st, con(OP::TY_INT));
        body = std::get<AltInt>(alt.as).body;
        break;
      case EX::AltKind::Real:
        unify(st, con(OP::TY_REAL));
        body = std::get<AltReal>(alt.as).body;
        break;
      case EX::AltKind::Con:
      {
        // The scrutinee is the constructor's union type; instantiate the union
        // fresh and bind its payload from the matched variant's field types,
        // read through that same instantiation.
        AltCon &ac  = std::get<AltCon>(alt.as);
        auto    uit = m_unions.find(ac.type_name);
        if (uit != m_unions.end() && ac.tag < uit->second.variants.size())
        {
          std::vector<Type *> args;
          Subst               sub = fresh_subst(uit->second.params, args);
          unify(st, con(ac.type_name, args));
          const StructFields &decl = uit->second.variants[ac.tag].fields;
          for (size_t i = 0; i < ac.binders.size() && i < decl.size(); ++i)
            if (!ac.binders[i].empty())
              inner[ac.binders[i]] = Scheme{ {}, subst(decl[i].second, sub) };
        }
        else
          unify(st, con(ac.type_name));
        body = ac.body;
        break;
      }
      }
      unify(rt, infer(body, inner, amb));
    }
    unify(rt, infer(cc.deflt, locals, amb)); // the default arm
    return rt;
  }
  }
  return fresh();
}

// Infer a body in "checking mode" against an expected type. When the term is a
// lambda and the expectation an arrow, the parameter is bound to the arrow's
// domain *before* the body is inferred -- so a type-directed operation on a
// parameter (notably field access, which needs the receiver's struct type known
// at that point) sees its declared type, which a plain synthesize-then-unify
// pass would only learn afterwards. Any other shape falls back to ordinary
// inference unified against the expectation.
Type *
Checker::infer_against(
  Core *e, Type *expected, Env &locals, Type *amb)
{
  Type *exp = prune(expected);
  if (e->kind() == CKind::Lam && exp->kind() == Kind::Arrow)
  {
    // The declared arrow's latent effect becomes the body's ambient, so a
    // signature like `{} -> <State> Int` lets the body perform State.
    CLam   &l      = std::get<CLam>(e->as);
    TArrow &a      = std::get<TArrow>(exp->as);
    Env     inner  = locals;
    inner[l.param] = Scheme{ {}, a.from }; // param pinned to its declared type
    Type *bt       = infer_against(l.body, a.to, inner, a.eff);
    return arrow(a.from, bt, a.eff);
  }
  Type *t = infer(e, locals, amb);
  unify(t, expected);
  return t;
}

/*------------------------------------------------------------------------------
 *\RESOLUTION (phase B)
 *-----------------------------------------------------------------------------*/

// A global's source-name view, for anchoring a diagnostic at its definition.
// (`name` is the mangled `MOD/x`, which is an arena string with no source
// location; `origin` keeps the original view into the file.)
static UT::Vu
def_anchor(
  const EX::ExDef *d)
{
  return d->origin.data() ? d->origin : d->name;
}

// A global's name for a user-facing message: the mangled `MOD/x` shown `MOD.x`.
static std::string
def_label(
  UT::Vu mangled)
{
  std::string s(mangled);
  size_t      slash = s.find('/');
  if (slash != std::string::npos) s[slash] = '.';
  return s;
}

Scheme
Checker::resolve(
  const std::string &name)
{
  GlobalEntry &g = *m_globals.find(name);
  if (g.state == GlobalEntry::Resolved) return g.scheme;
  if (g.state == GlobalEntry::Resolving)
  {
    fail(ER::Code::TYPE_CYCLE,
         def_anchor(g.def),
         "global '%s' depends on itself but has no type annotation",
         def_label(g.def->name).c_str());
    g.scheme = Scheme{ {}, fresh() };
    g.state  = GlobalEntry::Resolved;
    return g.scheme;
  }

  g.state = GlobalEntry::Resolving;

  if (g.def->sig)
  {
    UT::Vu save_a = m_anchor;
    size_t save_l = m_line;
    m_anchor      = def_anchor(g.def);
    m_line        = line_of(m_anchor);
    g.scheme      = scheme_of_sig(g.def->sig);
    m_anchor      = save_a;
    m_line        = save_l;
  }
  else
  {
    UT::Vu save_a = m_anchor;
    size_t save_l = m_line;
    m_anchor      = def_anchor(g.def);
    m_line        = line_of(m_anchor);

    Env    empty;
    size_t base  = m_sites.size();
    size_t ubase = m_usites.size();
    size_t fbase = m_field_sites.size();
    size_t lbase = m_lit_sites.size();
    Type  *t     = infer(g.body, empty, row_empty()); // top level is pure
    resolve_sites(base); // settle this body's overloads before judging its type
    resolve_user_sites(ubase);  // ... and its user overloads (now constrained)
    resolve_field_sites(fbase); // ... then field accesses (receivers now known)
    resolve_lit_sites(lbase);   // ... and any unqualified literals (no context)
    t = prune(t);

    if (t->kind() != Kind::Con)
    {
      fail(
        ER::Code::TYPE_ANNOTATION_REQUIRED,
        def_anchor(g.def),
        "global '%s' needs a type annotation (its type is '%s', not a ground "
        "Int/Str)",
        def_label(g.def->name).c_str(),
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

// Resolve the user-overload sites collected since `from`. By now the use's type
// is fixed by its surrounding context, so for each site we probe every
// candidate global -- instantiating its scheme and speculatively unifying it
// against the use (try_unify rolls the trial back) -- and keep the ones that
// fit. For an operator site the built-in meanings (overload_db) are folded in
// too: the use is shaped into an arrow chain, unconstrained operands default to
// Int (as for a plain built-in operator), and a matching built-in counts as a
// candidate. Exactly one fit is the answer: we commit it and write the chosen
// name to the slot (the EX::ExOverload's `chosen`) -- a mangled global for a
// user overload, or a "+@Int"-style key for a built-in, both of which IT reads.
// None or several is an error.
void
Checker::resolve_user_sites(
  size_t from)
{
  for (size_t i = from; i < m_usites.size(); ++i)
  {
    const UserSite              &s   = m_usites[i];
    Type                        *use = prune(s.use);
    const std::vector<Overload> *ovs = UT::try_lookup(overload_db, s.name);

    // For an operator, shape the use as an arrow chain and default any
    // unconstrained operand to Int, so the built-in candidates can be matched
    // by operand type (and `\x = x + x` still defaults to the Int built-in).
    std::vector<Type *> args;
    Type               *result = nullptr;
    if (ovs)
    {
      size_t arity = ovs->front().sig.size() - 1;
      result       = fresh();
      Type *shape  = result;
      args.resize(arity);
      for (size_t k = arity; k-- > 0;)
      {
        args[k] = fresh();
        shape   = arrow(args[k], shape);
      }
      unify(use, shape);
      for (Type *&a : args)
      {
        a = prune(a);
        if (a->kind() == Kind::Var && !std::get<TVar>(a->as).rigid)
        {
          std::get<TVar>(a->as).ref = con(OP::TY_INT);
          a                         = prune(a);
        }
      }
      use = prune(use);
    }

    // The matching built-in overload, if any (operators only).
    const Overload *bhit = nullptr;
    if (ovs)
      for (const Overload &o : *ovs)
      {
        if (o.sig.size() != args.size() + 1) continue;
        bool ok = true;
        for (size_t k = 0; k < args.size(); ++k)
          if (args[k]->kind() != Kind::Con
              || std::get<TCon>(args[k]->as).name != o.sig[k])
          {
            ok = false;
            break;
          }
        if (ok)
        {
          bhit = &o;
          break;
        }
      }

    // The fitting user candidates. A candidate may be an effect operation
    // (its scheme is in m_prim, keyed by `Effect.op`) rather than a global.
    std::vector<size_t> fits;
    for (size_t k = 0; k < s.cands->size(); ++k)
    {
      std::string cname(std::string((*s.cands)[k]));
      auto        pit  = m_prim.find(cname);
      Type       *cand = instantiate(pit != m_prim.end() ? pit->second
                                                         : resolve(cname));
      if (try_unify(use, cand)) fits.push_back(k);
    }

    size_t total = fits.size() + (bhit ? 1u : 0u);
    if (total == 0)
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "no overload of '%s' has type '%s'",
           s.name.c_str(),
           show(use).c_str());
      continue;
    }
    if (total > 1)
    {
      std::string cands;
      if (bhit) cands += "built-in " + bhit->mono;
      for (size_t f : fits)
        cands += (cands.empty() ? "" : ", ") + def_label((*s.cands)[f]);
      fail(ER::Code::AMBIGUOUS_NAME,
           s.anchor,
           "ambiguous overload of '%s' at type '%s'; candidates: %s",
           s.name.c_str(),
           show(use).c_str(),
           cands.c_str());
      continue;
    }

    if (bhit)
    {
      unify(prune(result), con(bhit->sig.back()));
      *s.slot = UT::Vu{ bhit->mono.c_str(), bhit->mono.size() };
      continue;
    }

    UT::Vu      chosen = (*s.cands)[fits.front()];
    std::string cname(chosen);
    auto        pit = m_prim.find(cname); // an operation candidate lives here
    unify(use,
          instantiate(pit != m_prim.end() ? pit->second : resolve(cname)));
    *s.slot = chosen;
  }
  m_usites.erase(m_usites.begin() + (long)from, m_usites.end());
}

// Settle the field accesses collected since `from`. By now each receiver's type
// has been fixed by its context (an overload resolved, a signature applied,
// ...), so we can look the field up on its struct and unify its type into the
// access's result var. Resolved in recording order (innermost-first), so
// `a.b.c` settles `a.b` -- giving the receiver of `.c` its struct type --
// before `(a.b).c`.
void
Checker::resolve_field_sites(
  size_t from)
{
  for (size_t i = from; i < m_field_sites.size(); ++i)
  {
    const FieldSite &s  = m_field_sites[i];
    Type            *rt = prune(s.record);
    if (rt->kind() != Kind::Con
        || !m_structs.count(std::get<TCon>(rt->as).name))
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "cannot access field '%s': '%s' is not a known struct type",
           s.field.c_str(),
           show(rt).c_str());
      continue;
    }
    const std::string &rname = std::get<TCon>(rt->as).name;
    const StructDef   &sdef  = m_structs.at(rname);
    // Bind the struct's parameters to the receiver's actual type arguments, so
    // the field type comes back at the receiver's instantiation.
    Subst                      sub;
    const std::vector<Type *> &actual = std::get<TCon>(rt->as).args;
    for (size_t k = 0; k < sdef.params.size() && k < actual.size(); ++k)
      sub[sdef.params[k]] = actual[k];

    bool found = false;
    for (auto &f : sdef.fields)
      if (f.first == s.field)
      {
        unify(s.result, subst(f.second, sub));
        found = true;
        break;
      }
    if (!found)
      fail(ER::Code::TYPE_UNBOUND,
           s.anchor,
           "struct '%s' has no field '%s'",
           rname.c_str(),
           s.field.c_str());
  }
  m_field_sites.erase(m_field_sites.begin() + (long)from, m_field_sites.end());
}

UT::Vu
Checker::intern(
  const std::string &s)
{
  char *mem = (char *)m_arena.alloc(s.size() ? s.size() : 1);
  std::memcpy(mem, s.data(), s.size());
  return UT::Vu{ mem, s.size() };
}

// Settle the bare struct/union literals collected since `from`. Each was typed
// as a fresh `use` var; by now unification (an annotation, an enclosing field,
// ...) should have bound it to a concrete nominal type. We check the payload
// against that type's (instantiated) fields and patch the EX node's type name
// so the interpreter sees a resolved literal. Resolved outermost-first (reverse
// of the post-order in which they were recorded) so an outer literal binds an
// inner one's `use` before the inner is judged.
void
Checker::resolve_lit_sites(
  size_t from)
{
  for (size_t i = m_lit_sites.size(); i-- > from;)
  {
    LitSite &s = m_lit_sites[i];
    Type    *u = prune(s.use);
    if (u->kind() != Kind::Con)
    {
      fail(ER::Code::TYPE_ANNOTATION_REQUIRED,
           s.anchor,
           "cannot infer the type of this %s literal; add a type annotation",
           s.is_struct ? "struct" : "variant");
      continue;
    }
    TCon              &uc = std::get<TCon>(u->as);
    const std::string &nm = uc.name;

    // Bind the nominal type's parameters to the receiver's actual arguments, so
    // declared field types are read at this literal's instantiation.
    auto bind = [&](const VarIds &params) {
      Subst sub;
      for (size_t k = 0; k < params.size() && k < uc.args.size(); ++k)
        sub[params[k]] = uc.args[k];
      return sub;
    };

    if (s.is_struct)
    {
      auto it = m_structs.find(nm);
      if (it == m_structs.end())
      {
        fail(ER::Code::TYPE_MISMATCH,
             s.anchor,
             "'%s' is not a struct type",
             nm.c_str());
        continue;
      }
      const StructFields &decl = it->second.fields;
      Subst               sub  = bind(it->second.params);

      std::vector<bool> seen(decl.size(), false);
      for (auto &fv : s.fields)
      {
        size_t idx = decl.size();
        for (size_t k = 0; k < decl.size(); ++k)
          if (decl[k].first == fv.first) idx = k;
        if (idx == decl.size())
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "struct '%s' has no field '%s'",
               nm.c_str(),
               fv.first.c_str());
          continue;
        }
        if (seen[idx])
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "field '%s' is set more than once in '%s'",
               fv.first.c_str(),
               nm.c_str());
          continue;
        }
        seen[idx] = true;
        unify(subst(decl[idx].second, sub), fv.second);
      }
      for (size_t k = 0; k < decl.size(); ++k)
        if (!seen[k])
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "struct '%s' is missing field '%s'",
               nm.c_str(),
               decl[k].first.c_str());

      if (s.exs) s.exs->type_name = intern(nm); // patch for the interpreter
      continue;
    }

    // Variant.
    auto it = m_unions.find(nm);
    if (it == m_unions.end())
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "'%s' is not a union type",
           nm.c_str());
      continue;
    }
    const StructFields *decl = nullptr;
    for (auto &v : it->second.variants)
      if (v.tag == s.vtag) decl = &v.fields;
    if (!decl)
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "union '%s' has no variant '%s'",
           nm.c_str(),
           s.vtag.c_str());
      continue;
    }
    Subst sub = bind(it->second.params);

    bool named = false;
    for (auto &fv : s.fields)
      if (!fv.first.empty()) named = true;

    if (named)
    {
      std::vector<bool> seen(decl->size(), false);
      for (auto &fv : s.fields)
      {
        size_t idx = decl->size();
        for (size_t k = 0; k < decl->size(); ++k)
          if ((*decl)[k].first == fv.first) idx = k;
        if (idx == decl->size())
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "variant '%s.%s' has no field '%s'",
               nm.c_str(),
               s.vtag.c_str(),
               fv.first.c_str());
          continue;
        }
        if (seen[idx])
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "field '%s' is set more than once in '%s.%s'",
               fv.first.c_str(),
               nm.c_str(),
               s.vtag.c_str());
          continue;
        }
        seen[idx] = true;
        unify(subst((*decl)[idx].second, sub), fv.second);
      }
      for (size_t k = 0; k < decl->size(); ++k)
        if (!seen[k])
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "variant '%s.%s' is missing field '%s'",
               nm.c_str(),
               s.vtag.c_str(),
               (*decl)[k].first.c_str());
    }
    else if (s.fields.size() != decl->size())
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "variant '%s.%s' takes %zu payload field(s), but %zu given",
           nm.c_str(),
           s.vtag.c_str(),
           decl->size(),
           s.fields.size());
    }
    else
    {
      for (size_t k = 0; k < decl->size(); ++k)
        unify(subst((*decl)[k].second, sub), s.fields[k].second);
    }

    if (s.exv)
    {
      s.exv->type_name = intern(nm); // patch for the interpreter
      // A named bare literal was left unnormalized by LL (it had no type then);
      // reorder its payload into declared field order, dropping the names, so
      // IT (which builds positionally) and a `case` agree.
      if (named && s.exv->fields.size() == decl->size())
      {
        UT::Vec<EX::FieldInit> out{ m_arena };
        bool                   ok = true;
        for (size_t k = 0; k < decl->size(); ++k)
        {
          EX::Expr *val = nullptr;
          for (size_t j = 0; j < s.exv->fields.size(); ++j)
            if (std::string(s.exv->fields[j].name) == (*decl)[k].first)
              val = s.exv->fields[j].val;
          if (!val)
          {
            ok = false;
            break;
          }
          out.push(EX::FieldInit{ UT::Vu{}, val });
        }
        if (ok) s.exv->fields = out;
      }
    }
  }
  m_lit_sites.erase(m_lit_sites.begin() + (long)from, m_lit_sites.end());
}

/*------------------------------------------------------------------------------
 *\DRIVER
 *-----------------------------------------------------------------------------*/

// Collect the distinct type-variable names free in `t`, in order of first
// appearance, appending to `out`. A nominal type's implicit parameters are
// exactly these over its whole declaration body.
static void
collect_tyvars(
  EX::Ty *t, std::vector<std::string> &out)
{
  switch (t->tag)
  {
  case EX::TyTag::Var:
  {
    std::string nm = std::string(std::get<EX::TyVar>(t->as).name);
    for (const std::string &s : out)
      if (s == nm) return;
    out.push_back(nm);
    break;
  }
  case EX::TyTag::Con:
    for (EX::Ty *a : std::get<EX::TyCon>(t->as).args) collect_tyvars(a, out);
    break;
  case EX::TyTag::Arrow:
  {
    auto &ar = std::get<EX::TyArrow>(t->as);
    collect_tyvars(ar.from, out);
    collect_tyvars(ar.to, out);
    break;
  }
  }
}

void
Checker::run(
  EX::Exprs &exprs)
{
  // The built-in base types are nullary type constructors, known up front. They
  // share the nominal-type arity table so a sized type used with arguments
  // (e.g. `Int32 Foo`) is still an arity error like any other nullary type.
  for (const char *t : OP::base_types) m_arity[t] = 0;

  // Register transparent type aliases up front, so sig_to_type can resolve an
  // alias used by any (possibly earlier) declaration.
  for (EX::Expr &e : exprs)
    if (e.tag == EX::ExprTag::AliasDecl)
    {
      auto &ad                        = std::get<EX::ExAliasDecl>(e.as);
      m_aliases[std::string(ad.name)] = ad.target;
    }

  // Record every declared effect name up front, so an effect-row annotation
  // `<E>` in any (possibly earlier) signature can validate its labels.
  for (EX::Expr &e : exprs)
    if (e.tag == EX::ExprTag::EffectDecl)
      m_effects.insert(std::string(std::get<EX::ExEffectDecl>(e.as).name));

  // Phase A0: record every nominal type's arity (its distinct free type vars)
  // before any field type is built, so applied cons can be arity-checked even
  // when they reference a type declared later.
  for (EX::Expr &e : exprs)
  {
    std::vector<std::string> ps;
    if (e.tag == EX::ExprTag::StructDecl)
    {
      auto &sd = std::get<EX::ExStructDecl>(e.as);
      for (auto &f : sd.fields) collect_tyvars(f.ty, ps);
      m_arity[std::string(sd.name)] = ps.size();
    }
    else if (e.tag == EX::ExprTag::UnionDecl)
    {
      auto &ud = std::get<EX::ExUnionDecl>(e.as);
      for (auto &v : ud.variants)
        for (auto &f : v.fields) collect_tyvars(f.ty, ps);
      m_arity[std::string(ud.name)] = ps.size();
    }
  }

  // Phase A: register struct types and collect value globals. Structs are
  // registered up front so a global may reference a struct declared later. One
  // TyVarEnv per declaration ties every `T in the body to the same parameter;
  // `order` records the parameter ids in appearance order.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::StructDecl) continue;
    EX::ExStructDecl &sd = std::get<EX::ExStructDecl>(e->as);
    StructFields      flds;
    TyVarEnv          tv;
    VarIds            params;
    m_anchor = sd.name;
    for (size_t f = 0; f < sd.fields.size(); ++f)
    {
      Type *ft = sig_to_type(sd.fields[f].ty, tv, false, &params);
      flds.push_back({ std::string(sd.fields[f].name), ft });
    }
    m_structs[std::string(sd.name)]
      = StructDef{ std::move(params), std::move(flds) };
  }

  // ... and union types, likewise registered up front for forward references.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::UnionDecl) continue;
    EX::ExUnionDecl &ud = std::get<EX::ExUnionDecl>(e->as);
    Variants         variants;
    TyVarEnv         tv;
    VarIds           params;
    m_anchor = ud.name;
    for (size_t v = 0; v < ud.variants.size(); ++v)
    {
      StructFields flds;
      for (size_t f = 0; f < ud.variants[v].fields.size(); ++f)
      {
        Type *ft = sig_to_type(ud.variants[v].fields[f].ty, tv, false, &params);
        flds.push_back({ std::string(ud.variants[v].fields[f].name), ft });
      }
      variants.push_back({ std::string(ud.variants[v].tag), std::move(flds) });
    }
    m_unions[std::string(ud.name)]
      = UnionDef{ std::move(params), std::move(variants) };
  }

  // Effect declarations: register each operation as a typed name (a primitive
  // scheme) keyed by its canonical `Effect.op` identity (the same identity MR
  // rewrites uses and clause heads to), so a use `op a` type-checks against its
  // declared signature. Generalize over the signature's free type variables, so
  // a generic effect's operation is polymorphic per use. Same-named operations
  // from different effects coexist -- their identities differ.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::EffectDecl) continue;
    EX::ExEffectDecl &ed = std::get<EX::ExEffectDecl>(e->as);
    for (auto &op : ed.ops)
    {
      std::string key
        = std::string(ed.name) + "." + std::string(op.name); // Effect.op
      m_anchor         = op.name;
      m_op_effect[key] = std::string(ed.name);
      TyVarEnv tv;
      VarIds   params;
      Type    *t = sig_to_type(op.ty, tv, false, &params);

      // The performing arrow carries an open-tailed effect row `<Eff | mu>`
      // (mu fresh and quantified): performing the operation forces `Eff` into
      // the ambient and fits any ambient that already contains more effects.
      if (t->kind() == Kind::Arrow)
      {
        Type *mu = fresh();
        params.push_back(std::get<TVar>(mu->as).id);
        std::get<TArrow>(t->as).eff = row_extend(std::string(ed.name), mu);
      }
      m_prim[key] = Scheme{ std::move(params), t };
    }
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
    m_anchor = def_anchor(g.def);
    m_line   = line_of(m_anchor);

    Env    empty;
    size_t base  = m_sites.size();
    size_t ubase = m_usites.size();
    size_t fbase = m_field_sites.size();
    size_t lbase = m_lit_sites.size();

    if (g.def->sig)
    {
      // Check the body against its declared type so parameters carry their
      // signature types into the body (see infer_against).
      TyVarEnv tv;
      Type    *rigid = sig_to_type(g.def->sig, tv, true);
      infer_against(g.body, rigid, empty, row_empty()); // top level is pure
    }
    else
    {
      Type *bt = infer(g.body, empty, row_empty()); // top level is pure
      unify(bt, g.scheme.type);
    }

    resolve_sites(base);
    resolve_user_sites(ubase);
    resolve_field_sites(fbase);
    // Bare literals last: their type comes from the unification just done.
    resolve_lit_sites(lbase);
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
  case Kind::Con:
  {
    TCon       &c   = std::get<TCon>(t->as);
    std::string out = c.name;
    for (Type *arg : c.args)
    {
      Type *pa = prune(arg);
      bool  nest
        = pa->kind() == Kind::Arrow
          || (pa->kind() == Kind::Con && !std::get<TCon>(pa->as).args.empty());
      std::string a = show(arg);
      out += " " + (nest ? "(" + a + ")" : a);
    }
    return out;
  }
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
    Type       *eff = prune(a.eff);
    std::string e   = eff->kind() == Kind::RowEmpty ? "" : show(eff) + " ";
    return l + " -> " + e + show(a.to);
  }
  case Kind::RowEmpty: return "<>";
  case Kind::RowExtend:
  {
    std::string out = "<";
    Type       *cur = t;
    bool        first = true;
    for (; prune(cur)->kind() == Kind::RowExtend;)
    {
      TRowExtend &r = std::get<TRowExtend>(prune(cur)->as);
      if (!first) out += ", ";
      out += r.label;
      first = false;
      cur   = r.rest;
    }
    Type *tail = prune(cur);
    if (tail->kind() != Kind::RowEmpty) out += " | " + show(tail);
    return out + ">";
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
  // if : forall T e1 e2 e3. Int ->e1 T ->e2 T ->e3 T. The arrows' latent
  // effects are fresh row variables that generalize, so `if` is effect-
  // polymorphic and adapts to whatever ambient its use site demands.
  Type *tv = fresh();
  Type *ift
    = arrow(con(OP::TY_INT), arrow(tv, arrow(tv, tv)));
  m_prim[OP::IF] = generalize(Env{}, ift);

  // %array : Int -> Array -- the byte-block allocator that `@array.{n}`
  // desugars to (see EX::parse_array).
  Type *arrt = arrow(con(OP::TY_INT), con(OP::TY_ARRAY));
  m_prim[OP::ARR_ALLOC] = generalize(Env{}, arrt);

  // %defer : forall a e. ({} ->e a) -> ({} ->e {}) ->e a -- the cleanup
  // intrinsic the `defer` keyword desugars to: run the action, running the
  // cleanup when its scope exits (normal completion or discard). Both thunks and
  // the running arrows share one effect row `e`, so the action's effects must fit
  // the call's ambient (subsumption), and it is effect-polymorphic. `%`-prefixed
  // => not user-writable; reached only via `defer`.
  Type *e    = fresh(); // the shared effect row
  Type *a    = fresh();
  Type *unit = con(OP::TY_UNIT);
  Type *act  = arrow(unit, a, e);    // {} ->e a
  Type *cln  = arrow(unit, unit, e); // {} ->e {}
  Type *fin  = arrow(act, arrow(cln, a, e), e);
  m_prim[OP::DEFER] = generalize(Env{}, fin);
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
