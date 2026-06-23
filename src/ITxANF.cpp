#include "ITxANF.hpp"
#include "IT.hpp"

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

namespace IT
{

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
  case LTag::THUNK:
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

std::string
Anf::fresh()
{
  return "%a" + std::to_string(counter++);
}

// Normalize a whole expression in tail position: its value is the answer.
pLm
Anf::term(
  pLm e)
{
  return norm(e, true, [](pLm v) { return v; });
}

// Normalize `e` and ensure the result handed to `k` is an atom, hoisting a
// non-atom into a fresh `let`. Always a non-tail position.
pLm
Anf::norm_name(
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
Anf::norm_struct(
  Struct                                    s,
  size_t                                    i,
  std::vector<std::pair<std::string, pLm> > acc,
  const Cont                               &k)
{
  if (i == s.fields.size())
    return k(mk(Lm{ .tag = LTag::STRUCT, .as = Struct{ s.name, acc } }));
  return norm_name(s.fields[i].second, [this, s, i, acc, k](pLm a) {
    auto acc2 = acc;
    acc2.push_back({ s.fields[i].first, a });
    return norm_struct(s, i + 1, acc2, k);
  });
}

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
  case LTag::THUNK:
  case LTag::UNK    : return k(e);

  case LTag::FUN:
  {
    Fun f  = std::get<Fun>(e->as);
    f.body = term(f.body); // a lambda body is itself in tail position
    return k(mk(Lm{ .tag = LTag::FUN, .as = f }));
  }

  case LTag::VARIANT:
  {
    // A constructor's fields are non-strict: normalize each as its own value
    // producer (like a lambda body) but keep it INLINE -- never hoist it into a
    // `let`, which would force it eagerly and defeat the laziness. eval
    // suspends each into a Thunk.
    Variant v = std::get<Variant>(e->as);
    for (auto &f : v.fields) f = term(f);
    return k(mk(Lm{ .tag = LTag::VARIANT, .as = v }));
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

  case LTag::CASE:
  {
    Case c = std::get<Case>(e->as);
    return norm_name(c.scrut, [this, c, tail, k](pLm s2) mutable {
      // Each alternative body (and the default) sits in the `case`'s own
      // position: in tail position they stay tail, otherwise they each produce
      // the value the `case` is bound to, so the whole `case` is bound via `k`.
      Cont id = [](pLm v) { return v; };
      for (auto &alt : c.alts)
        alt.body = tail ? term(alt.body) : norm(alt.body, false, id);
      c.deflt = tail ? term(c.deflt) : norm(c.deflt, false, id);
      c.scrut = s2;
      return k(mk(Lm{ .tag = LTag::CASE, .as = c }));
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

} // namespace IT
