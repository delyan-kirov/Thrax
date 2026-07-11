/*------------------------------------------------------------------------------
 *\file CRxANF.hpp
 *\ANF -- A-Normal Form over the Core
 *
 * Rewrite a freshly built CR::Term tree into A-normal form before De-Bruijn
 * indexing (CR::assign_id) runs. After this pass:
 *
 *   - every application's operator and operand is an *atom* (a variable,
 *     literal, or lambda); any non-atomic sub-expression is hoisted into a
 *     fresh `let`, so evaluation order is explicit and named;
 *   - every `App` carries `tail`, set iff the call is in genuine program tail
 *     position. A non-tail `case` is bound to a `let` (its branches normalized
 *     as value producers) rather than inlined, so no code is duplicated and
 *     branch calls are marked accurately.
 *
 * This is the standard two-function transform (Flanagan et al., "The Essence of
 * Compiling with Continuations"): `norm` threads a continuation `k` that
 * consumes the produced value-expression and returns the rest; `norm_name`
 * forces that value to be an atom. The interpreter's trampoline reads the
 * resulting shape directly, and the named core is the substrate a compiling
 * backend rewrites against. Fresh binder names are interned into the Core
 * arena.
 *-----------------------------------------------------------------------------*/

#ifndef CRxANF_HEADER_
#define CRxANF_HEADER_

#include "CR.hpp"

namespace CR
{

using Cont = std::function<Term *(Term *)>;

bool is_atom(const Term *e);

struct Anf
{
  AR::Arena &arena;
  size_t     counter = 0;

  explicit Anf(
    AR::Arena &arena)
      : arena{ arena }
  {
  }

  Term *mk(Term t);
  Term *mk_varref(UT::Vu name);
  Term *mk_let(UT::Vu var, Term *val, Term *body);

  // A fresh binder name interned into the arena; the '%' prefix cannot collide
  // with a user identifier.
  UT::Vu fresh();

  // Normalize a whole expression in tail position: its value is the answer.
  Term *term(Term *e);

  // Normalize `e`; `tail` marks tail position (then `k` is the identity that
  // returns the answer). `k` receives the produced value-expression.
  Term *norm(Term *e, bool tail, const Cont &k);

  // Normalize `e` and ensure the result handed to `k` is an atom, hoisting a
  // non-atom into a fresh `let`. Always a non-tail position.
  Term *norm_name(Term *e, const Cont &k);

  // Normalize struct fields left-to-right into atoms, then rebuild the struct.
  Term *norm_struct(Struct s, size_t i, UT::Vec<FieldInit> acc, const Cont &k);
};

} // namespace CR

#endif // CRxANF_HEADER_
