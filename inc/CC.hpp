/*-------------------------------------------------------------------------------
 *\file CC.hpp
 *\info The C-code generation backend. Lowers a closure-converted IR::Program to
 *      a single self-contained C translation unit (the runtime, in src/THx*.c +
 *      inc/THx*.h, is baked into the compiler and emitted inline) to produce a
 *      native executable. This is the compiled counterpart of the IT
 *      interpreter: it consumes exactly the same IR (DR::run_program feeds IT;
 *      DR::emit_c feeds CC).
 *
 * Supports the strict subset plus C FFI (`@extern` -- direct typed foreign
 * calls resolved via dlsym; see emit_externs). Programs using algebraic effects
 * (handlers / operations / resumptions / `defer`) are rejected up front by
 * `unsupported`, which names the offending feature so the driver can point the
 * user back at the interpreter. A program that makes foreign calls links with
 * `-ldl` (see `uses_ffi`). See doc/native-backend.md for the supported surface
 * and the seams for ref counting and effects.
 *-----------------------------------------------------------------------------*/

#ifndef CC_HEADER_
#define CC_HEADER_

#include "IR.hpp"
#include "UT.hpp"

#include <optional>
#include <string>

namespace CC
{

// If `prog` uses a feature the native backend cannot yet compile (effects,
// FFI), return a human-readable reason; otherwise std::nullopt. The driver
// prints the reason and falls back to suggesting the interpreter.
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
