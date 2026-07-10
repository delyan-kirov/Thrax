/*-------------------------------------------------------------------------------
 *\file engines/build.cpp
 *\info Build recipe for the engines module (interpreter IT, C backend CC, FFI
 *      seam FF). Pure source contribution to the library amalgamation.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

BLD::Module
engines_module()
{
  return BLD::Module{ "engines", { "engines" }, {}, nullptr };
}
