/*-------------------------------------------------------------------------------
 *\file DR.hpp
 *\info Header file for the Driver (pipeline orchestration)
 * *----------------------------------------------------------------------------*/

#ifndef DR_HEADER
#define DR_HEADER

#include "EX.hpp"
#include "IT.hpp"
#include "LX.hpp"

namespace DR
{

// Full pipeline: lex -> parse -> interpret. Parse diagnostics are printed as
// they are collected; the good definitions are still interpreted.
IT::StatEnv interpret_file(UT::String file);

} // namespace DR

#endif // DR_HEADER
