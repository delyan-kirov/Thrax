/*-------------------------------------------------------------------------------
 *\file IT.hpp
 *\info The tree-walking interpreter. It evaluates the Core (see CR.hpp) into
 *      reference-counted runtime values (see ITxDATA.hpp). The Core is the
 *      immutable, arena-owned program; the values eval produces are the
 *      churning runtime heap -- the two are deliberately separate types so the
 *      Core carries no reference counts and a future backend can lower the same
 *      Core to C.
 * *----------------------------------------------------------------------------*/

#ifndef IT_HEADER_
#define IT_HEADER_

#include "CR.hpp"
#include "ITxDATA.hpp"

namespace IT
{

// Evaluate a Core term in the dynamic environment `denv` (the De-Bruijn-indexed
// locals) and the global environment `senv`. Call-by-value, with a trampoline
// for tail calls (see the comment in eval).
pVal eval(const CR::Term *node, ValEnv denv, CR::StatEnv &senv);

} // namespace IT

#endif // IT_HEADER_

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
