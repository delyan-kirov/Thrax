/*-------------------------------------------------------------------------------
 *\file CC.hpp
 *\info The C-code generation backend. Lowers a closure-converted IR::Program to
 *      a single self-contained C translation unit (the runtime, in src/THx*.c +
 *      inc/THx*.h, is baked into the compiler and emitted inline) to produce a
 *      native executable. This is the compiled counterpart of the IT
 *      interpreter: it consumes exactly the same IR (DR::run_program feeds IT;
 *      DR::emit_c feeds CC).
 *
 * Lowers the strict subset, C FFI (`@extern` -- DIRECT foreign calls the
 * system linker resolves, declared via asm labels; see emit_externs), AND
 * algebraic effects (handlers / operations / resumptions / `defer`), which
 * run on the CEK driver in the runtime (src/THxK.c): the IR becomes block
 * functions whose continuation is an explicit heap stack, so a handler can
 * capture and splice the delimited continuation. A program that makes foreign
 * calls links the libraries its externs name (`extern_libs` reports them; DR
 * turns them into link flags). `unsupported` is now a vestigial seam
 * (returns std::nullopt).
 * See doc/native-backend.md for the supported surface and the ref-counting
 * seam.
 *-----------------------------------------------------------------------------*/

#ifndef CC_HEADER_
#define CC_HEADER_

#include "IR.hpp"
#include "TG.hpp"
#include "UT.hpp"

#include <optional>
#include <string>
#include <vector>

namespace CC
{

// If `prog` uses a feature the native backend cannot yet compile, return a
// human-readable reason; otherwise std::nullopt. The backend now lowers the
// whole IR (subset + FFI + effects), so this currently always returns nullopt;
// it is retained as the fallback seam and DR still honours a non-null reason.
std::optional<std::string> unsupported(const IR::Program &prog);

// The link-line flags for every library `prog`'s `@extern`s name, in first
// appearance order: "libc" is implicit (no flag), a symbolic name becomes
// -l<name> (a leading "lib" stripped), an explicit path/soname passes
// verbatim. Generated programs never dlopen; the system linker resolves.
std::vector<std::string> link_flags(const IR::Program &prog);

// Precondition: `unsupported(prog)` returned std::nullopt.
std::string emit(const IR::Program &prog,
                 UT::Vu             entry,
                 bool               entry_takes_arg,
                 const TG::Target  &tg);

} // namespace CC

#endif // CC_HEADER_
