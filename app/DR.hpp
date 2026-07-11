/*-------------------------------------------------------------------------------
 *\file DR.hpp
 *\info Header file for the Driver (pipeline orchestration)
 * *----------------------------------------------------------------------------*/

#ifndef DR_HEADER_
#define DR_HEADER_

#include "CR.hpp"
#include "IT.hpp"

namespace DR
{

// A compiled-and-loaded program: the IR program plus the arena that owns the IR
// (and the Core it was lowered from). The front-end arena is gone by the time
// this is returned (its data was copied into the Core), so this arena is the
// only allocation the machine needs alive while it runs. Kept in a unique_ptr
// because AR::Arena is neither movable nor copyable.
struct Interp
{
  std::unique_ptr<AR::Arena> arena;
  IR::Program                prog;
};

// Expand command-line paths into the list of source files to compile. A path
// that names a directory contributes every `.thx` file directly inside it
// (sorted, NOT recursing into sub-directories, skipping names that start with
// '_'); any other path is taken verbatim (so an explicitly named file is always
// included, even a `_`-prefixed one). The returned strings own the names.
std::vector<std::string> expand_sources(const std::vector<UT::Vu> &paths);

// Full single-file pipeline (LX -> EX -> LL -> MR -> TC -> Core), returning the
// module-resolved global environment together with the arena that owns it. Used
// by the test harness; does not require or invoke an entry point. On a pipeline
// failure the returned env is empty (diagnostics already printed).
Interp interpret_file(UT::Vu file);

// Compile every file as one program (modules link across all of them) and run
// its entry point -- the `main` of module `MAIN` -- via the IR + reified-K
// machine. Returns the program's exit code, or 1 on a compile error
// (diagnostics already printed).
int run_program(const std::vector<UT::Vu> &files);

// Lex + parse `file` and print the resulting AST to stdout (parse diagnostics
// go to stderr). Returns false if the file failed to parse.
bool dump_ast(UT::Vu file);

// Run the full front end on `files`, lower the Core to IR (closure conversion),
// and print the IR program to stdout. Returns false on a compile error.
bool dump_ir(const std::vector<UT::Vu> &files);

// Run the full front end on `files`, lower to IR, and emit a standalone C
// translation unit (the native backend) to stdout. Returns false on a compile
// error or if the program uses a feature the backend does not yet support
// (effects, FFI) -- a diagnostic is printed in that case. See CC.hpp.
bool emit_c(const std::vector<UT::Vu> &files);

// Compile a project -- a directory containing a 'MAIN' module. Lowers it to IR
// and the native backend, then invokes the system C compiler, writing all three
// artifacts into <dir>/bin (created if absent): <name>.ir, <name>.c, and the
// executable <name>. Returns false on a compile error or an unsupported feature
// (a diagnostic is printed). See CC.hpp.
bool build_project(UT::Vu dir);

} // namespace DR

#endif // DR_HEADER_
