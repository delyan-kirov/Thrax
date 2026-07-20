/*-------------------------------------------------------------------------------
 *\file library/build.cpp
 *\info Build recipe for the library module (the Thrax standard library) and
 *      the core layer (core/PRELUDE.thx, the language-defined prelude). The
 *      .thx sources of both are baked into a generated header
 *      (artifacts/STDLIBxAMALG.hpp) that DR injects into every compile, so
 *      the thrax binary is self-contained -- no install path or environment
 *      variable is needed to find the standard library. A pure-generation
 *      module, like platforms.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

BLD::Module
library_module()
{
  BLD::Module m;
  m.name     = "library";
  m.gen_deps = { "artifacts/STDLIBxAMALG.hpp" };
  m.generate = [](const BLD::Ctx &c) {
    BLD::gen_stdlib_header(c,
                           c.artifacts + "/STDLIBxAMALG.hpp",
                           BLD::glob({ "core", "library" }, { ".thx" }));
  };
  return m;
}
