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

} // namespace DR

#endif // DR_HEADER
