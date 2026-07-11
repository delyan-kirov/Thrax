/*-------------------------------------------------------------------------------
 *\file CC.hpp
 *\info The C-code generation backend. Lowers a closure-converted IR::Program to
 *      a single self-contained C translation unit (the runtime, in src/THx*.c +
 *      inc/THx*.h, is baked into the compiler and emitted inline) to produce a
 *      native executable. This is the compiled counterpart of the IT
 *      interpreter: it consumes exactly the same IR (DR::run_program feeds IT;
 *      DR::emit_c feeds CC).
 *
 * Lowers the strict subset, C FFI (`@extern` -- direct typed foreign calls
 * resolved via dlsym; see emit_externs), AND algebraic effects (handlers /
 * operations / resumptions / `defer`), which run on the CEK driver in the
 * runtime (src/THxK.c): the IR becomes block functions whose continuation is an
 * explicit heap stack, so a handler can capture and splice the delimited
 * continuation. A program that makes foreign calls links with `-ldl` (see
 * `uses_ffi`). `unsupported` is now a vestigial seam (returns std::nullopt).
 * See doc/native-backend.md for the supported surface and the ref-counting
 * seam.
 *-----------------------------------------------------------------------------*/

#ifndef CC_HEADER_
#define CC_HEADER_

#include "IR.hpp"
#include "UT.hpp"

#include <optional>
#include <string>

namespace CC
{

// If `prog` uses a feature the native backend cannot yet compile, return a
// human-readable reason; otherwise std::nullopt. The backend now lowers the
// whole IR (subset + FFI + effects), so this currently always returns nullopt;
// it is retained as the fallback seam and DR still honours a non-null reason.
std::optional<std::string> unsupported(const IR::Program &prog);

// True if `prog` makes any foreign (`@extern`) call. The emitted program then
// needs `-ldl` at link time (for dlopen/dlsym).
bool uses_ffi(const IR::Program &prog);

// Emit the whole program as C source. `entry` is the mangled entry global (its
// `Int` result becomes the process exit code); it may be empty, in which case
// the program simply forces every global (matching the test harness) and exits
// 0. `entry_takes_arg` is true for a `Str -> Int` entry (`main ""`).
//
// Precondition: `unsupported(prog)` returned std::nullopt.
std::string emit(const IR::Program &prog, UT::Vu entry, bool entry_takes_arg);

} // namespace CC

#endif // CC_HEADER_
