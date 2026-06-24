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

#ifndef ITxANF_HEADER_
#define ITxANF_HEADER_

#include "IT.hpp"

namespace IT
{

using NameStack = std::vector<std::string>;
using Cont      = std::function<pLm(pLm)>;

bool is_atom(const pLm &e);
pLm  mk(Lm lm);
pLm  mk_varref(const std::string &name);
pLm  mk_let(const Var &var, pLm val, pLm body);

struct Anf
{
  size_t counter = 0;

  // A fresh binder name; the '%' prefix cannot collide with a user identifier.
  std::string fresh();

  // Normalize a whole expression in tail position: its value is the answer.
  pLm term(pLm e);

  // Normalize `e`; `tail` marks tail position (then `k` is the identity that
  // returns the answer). `k` receives the produced value-expression.
  pLm norm(pLm e, bool tail, const Cont &k);

  // Normalize `e` and ensure the result handed to `k` is an atom, hoisting a
  // non-atom into a fresh `let`. Always a non-tail position.
  pLm norm_name(pLm e, const Cont &k);

  // Normalize struct fields left-to-right into atoms, then rebuild the struct.
  pLm norm_struct(Struct                                   s,
                  size_t                                   i,
                  std::vector<std::pair<std::string, pLm>> acc,
                  const Cont                              &k);
};

} // namespace IT

#endif // ITxANF_HEADER_
