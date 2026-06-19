/*-------------------------------------------------------------------------------
 *\file DR.hpp
 *\info Header file for the Driver (pipeline orchestration)
 * *----------------------------------------------------------------------------*/

#ifndef DR_HEADER
#define DR_HEADER

#include "IT.hpp"

namespace DR
{

// Full pipeline: LX -> EX -> IT
IT::StatEnv interpret_file(UT::String file);

// Interpret `file` and force every top-level definition so runtime faults
// surface. Returns false on a parse/type/empty-program error.
bool run_file(UT::String file);

// Lex + parse `file` and print the resulting AST to stdout (parse diagnostics
// go to stderr). Returns false if the file failed to parse.
bool dump_ast(UT::String file);

} // namespace DR

#endif // DR_HEADER
