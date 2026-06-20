/*-------------------------------------------------------------------------------
 *\file TC.hpp
 *\info Header file for the Type Checker.
 *
 * TC runs between the parser (EX) and the interpreter (IT). It lowers the EX
 * tree into a minimal typed Core (lambda calculus + let; operators and `if` are
 * ordinary primitives held in the initial type environment) and runs
 * Hindley-Milner inference over a union-find of unification variables.
 *
 * Globals must carry a signature unless their body has a ground, non-arrow type
 * (Int or Str). Annotated signatures may be polymorphic via `T type variables.
 *-----------------------------------------------------------------------------*/

#ifndef TC_HEADER
#define TC_HEADER

#include "EX.hpp"

namespace TC
{

// Type-check every top-level global in `exprs`. The returned vector is empty
// when the program is well typed; otherwise it holds one diagnostic per error,
// anchored into `src` for rendering with ER::pprint.
std::vector<ER::Diagnostic>
check(EX::Exprs &exprs, AR::Arena &arena, UT::Vu src);

} // namespace TC

#endif // TC_HEADER
