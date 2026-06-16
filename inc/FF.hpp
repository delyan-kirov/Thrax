/*-------------------------------------------------------------------------------
 *\file FF.hpp
 *\info Foreign-function interface (the only module that touches libffi/dlopen).
 *
 * The interface is deliberately libffi-free: callers pass Thrax
 * type-constructor names ("Int", "Nat", "Int32", "Str", "Ptr", ...) and raw
 * machine words, and a machine word comes back. This keeps libffi/dlfcn
 * confined to FF.cpp, so the rest of the project links neither unless FFI is
 * actually used. Building with -DTHRAX_NO_3RD_PARTY drops the dependency
 * entirely (calls then abort).
 *-----------------------------------------------------------------------------*/

#ifndef FF_HEADER
#define FF_HEADER

#include "UT.hpp"

namespace FF
{

// Call `symbol` in shared library `lib` (assumed to be a resolvable path).
// `arg_types`/`ret_type` are Thrax type names; `args` are raw machine words
// (a Str/Ptr argument is its char*/pointer already cast to ssize_t). The result
// is returned widened to a machine word (a Str/Ptr return is the pointer bits).
ssize_t call(UT::String                      lib,
             UT::String                      symbol,
             const std::vector<std::string> &arg_types,
             const std::string              &ret_type,
             const std::vector<ssize_t>     &args);

} // namespace FF

#endif // FF_HEADER
