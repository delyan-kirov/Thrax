/*-------------------------------------------------------------------------------
 *\file LL.hpp
 *\info Header file for LL -- the pattern-lowering ("link") layer.
 *
 * LL runs between the parser (EX) and the type checker (TC). It rewrites the
 * pattern sugar the parser produced -- pattern lambdas (`\Person.{x,y} = ...`)
 * and pattern `let` bindings (`let Person.{x,y} = e in ...`) -- into the plain
 * core the rest of the pipeline already understands: ordinary lambdas, `let`
 * bindings, and field accesses. After LL runs no Pattern node remains, so TC
 * and IT need no knowledge of patterns.
 *
 * Lowering is purely structural: a struct pattern names its own type, and field
 * order/names come from the `Struct` declarations, so no type inference is
 * needed here. Each destructured value is bound through a type-pinned `let` so
 * that the generated field accesses are statically typed (TC requires a known
 * receiver type for `.field`).
 *-----------------------------------------------------------------------------*/

#ifndef LL_HEADER_
#define LL_HEADER_

#include "EX.hpp"

namespace LL
{

// Lower every pattern lambda and pattern `let` in `exprs` in place. Returns one
// diagnostic per error (a refutable pattern in an irrefutable binding, an
// unknown struct type or field, a positional arity mismatch); empty on success.
std::vector<ER::Diagnostic> lower(EX::Exprs &exprs, AR::Arena &arena);

} // namespace LL

#endif // LL_HEADER_
