/*-------------------------------------------------------------------------------
 *\file DR.hpp
 *\info Header file for the Driver (pipeline orchestration)
 * *----------------------------------------------------------------------------*/

#ifndef DR_HEADER_
#define DR_HEADER_

#include "IT.hpp"

namespace DR
{

// Expand command-line paths into the list of source files to compile. A path
// that names a directory contributes every `.thx` file directly inside it
// (sorted, NOT recursing into sub-directories, skipping names that start with
// '_'); any other path is taken verbatim (so an explicitly named file is always
// included, even a `_`-prefixed one). The returned strings own the names.
std::vector<std::string> expand_sources(const std::vector<UT::Vu> &paths);

// Full single-file pipeline (LX -> EX -> LL -> MR -> TC -> IT), returning the
// module-resolved global environment. Used by the test harness; does not
// require or invoke an entry point.
IT::StatEnv interpret_file(UT::Vu file);

// Compile every file as one program (modules link across all of them) and run
// its entry point -- the `main` of module `MAIN`. Returns the program's exit
// code, or 1 on a compile error (diagnostics already printed).
int run_program(const std::vector<UT::Vu> &files);

// Lex + parse `file` and print the resulting AST to stdout (parse diagnostics
// go to stderr). Returns false if the file failed to parse.
bool dump_ast(UT::Vu file);

} // namespace DR

#endif // DR_HEADER_
