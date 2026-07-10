/*-------------------------------------------------------------------------------
 *\file compiler/build.cpp
 *\info Build recipe for the compiler module (lexer, parser, typecheck, Core,
 *      lowering, IR, module resolution). Pure source contribution to the
 *      library amalgamation -- no generated files, no standalone binaries.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

BLD::Module
compiler_module()
{
  return BLD::Module{ "compiler", { "compiler" }, {}, nullptr };
}
