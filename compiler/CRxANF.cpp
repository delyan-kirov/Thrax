#include "CRxANF.hpp"

namespace CR
{

// An atom is a value-expression that needs no further evaluation to name: a
// literal, a variable, a lambda, an extern binding, or the empty Unk node.
bool
is_atom(
  const Term *e)
{
  switch (kind(e))
  {
  case Kind::Int:
  case Kind::Real:
  case Kind::Str:
  case Kind::Var:
  case Kind::Fun:
  case Kind::Extern:
  case Kind::Unk   : return true;
  default          : return false;
  }
}

Term *
Anf::mk(
  Term t)
{
  return alloc(arena, std::move(t));
}

Term *
Anf::mk_varref(
  UT::Vu name)
{
  return mk(Term{ Var{ name, 0 } });
}

Term *
Anf::mk_let(
  UT::Vu var, Term *val, Term *body)
{
  return mk(Term{ Let{ var, val, body } });
}

UT::Vu
Anf::fresh()
{
  return UT::strdup(arena, "%a" + std::to_string(counter++));
}

Term *
Anf::term(
  Term *e)
{
  return norm(e, true, [](Term *v) { return v; });
}

Term *
Anf::norm_name(
  Term *e, const Cont &k)
{
  return norm(e, false, [this, k](Term *e2) {
    if (is_atom(e2)) return k(e2);
    UT::Vu t = fresh();
    return mk_let(t, e2, k(mk_varref(t)));
  });
}

Term *
Anf::norm_struct(
  Struct s, size_t i, UT::Vec<FieldInit> acc, const Cont &k)
{
  if (i == s.fields.size())
  {
    Struct out{ s.name, acc };
    return k(mk(Term{ out }));
  }
  return norm_name(s.fields[i].val, [this, s, i, acc, k](Term *a) {
    UT::Vec<FieldInit> acc2 = acc;
    acc2.push(FieldInit{ s.fields[i].name, a });
    return norm_struct(s, i + 1, acc2, k);
  });
}

Term *
Anf::norm(
  Term *e, bool tail, const Cont &k)
{
  switch (kind(e))
  {
  case Kind::Int:
  case Kind::Real:
  case Kind::Str:
  case Kind::Var:
  case Kind::Extern:
  case Kind::Unk   : return k(e);

  case Kind::Fun:
  {
    Fun f  = std::get<Fun>(e->as);
    f.body = term(f.body); // a lambda body is itself in tail position
    return k(mk(Term{ f }));
  }

  case Kind::Variant:
  {
    // A constructor's fields are non-strict: normalize each as its own value
    // producer (like a lambda body) but keep it INLINE -- never hoist it into a
    // `let`, which would force it eagerly and defeat the laziness. The
    // interpreter suspends each into a thunk.
    Variant v = std::get<Variant>(e->as);
    for (Term *&f : v.fields) f = term(f);
    return k(mk(Term{ v }));
  }

  case Kind::App:
  {
    App a = std::get<App>(e->as);
    return norm_name(a.fn, [this, a, tail, k](Term *fn2) {
      return norm_name(a.arg, [this, fn2, tail, k](Term *arg2) {
        return k(mk(Term{ App{ fn2, arg2, tail } }));
      });
    });
  }

  case Kind::Case:
  {
    Case c = std::get<Case>(e->as);
    return norm_name(c.scrut, [this, c, tail, k](Term *s2) mutable {
      // Each alternative body (and the default) sits in the `case`'s own
      // position: in tail position they stay tail, otherwise they each produce
      // the value the `case` is bound to, so the whole `case` is bound via `k`.
      Cont id = [](Term *v) { return v; };
      for (Alt &alt : c.alts)
        alt.body = tail ? term(alt.body) : norm(alt.body, false, id);
      c.deflt = tail ? term(c.deflt) : norm(c.deflt, false, id);
      c.scrut = s2;
      return k(mk(Term{ c }));
    });
  }

  case Kind::Let:
  {
    Let l = std::get<Let>(e->as);
    // The bound value is never in tail position; the body inherits this `let`'s
    // position and continuation.
    return norm(l.val, false, [this, l, tail, k](Term *v2) {
      return mk_let(l.var, v2, norm(l.body, tail, k));
    });
  }

  case Kind::Field:
  {
    Field fa = std::get<Field>(e->as);
    return norm_name(fa.record, [this, k, fa](Term *r2) {
      return k(mk(Term{ Field{ r2, fa.name } }));
    });
  }

  case Kind::Struct:
    return norm_struct(
      std::get<Struct>(e->as), 0, UT::Vec<FieldInit>{ arena }, k);

  case Kind::Handle:
  {
    // The body sits in the prompt's tail position; the clause/els lambdas
    // normalize like any other Fun. The handler itself is a computation, so a
    // non-atom caller (norm_name) hoists it into a let.
    Handle h = std::get<Handle>(e->as);
    h.body   = term(h.body);
    for (HClause &c : h.clauses) c.fn = term(c.fn);
    h.els = term(h.els);
    return k(mk(Term{ h }));
  }
  }
  UT_FAIL_MSG("%s", "unreachable: unhandled CR::Kind in Anf::norm");
  return k(e);
}

} // namespace CR
