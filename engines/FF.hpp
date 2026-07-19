/*-------------------------------------------------------------------------------
 *\file FF.hpp
 *\info Foreign-function interface (the only module that touches libffi/dlopen).
 *
 * Keep libffi/dlfcn confined to FF.cpp, so the rest of the project
 * links neither unless FFI is actually used. Building with
 * -DTHRAX_NO_3RD_PARTY drops the dependency entirely (calls then abort).
 *-----------------------------------------------------------------------------*/

#ifndef FF_HEADER_
#define FF_HEADER_

#include "UT.hpp"

namespace FF
{

// One marshalling slot: every argument and result crosses the FFI seam in one
// of these. Fixed at 64 bits (NOT the host word) so a pointer, a full Int, or
// a double's bit pattern always fits, on 32-bit hosts too.
using Slot = int64_t;

// Call `symbol` in shared library `lib`. `arg_types`/`ret_type` are Thrax
// type names (canonical "@int64"/"@str"/... or their bare prelude spellings);
// a Str/Ptr argument is its pointer cast into the slot, a Real its double
// bits. A float return comes back widened to double bits.
Slot call(UT::Vu                          lib,
          UT::Vu                          symbol,
          const std::vector<std::string> &arg_types,
          const std::string              &ret_type,
          const std::vector<Slot>        &args);

} // namespace FF

#endif // FF_HEADER_
