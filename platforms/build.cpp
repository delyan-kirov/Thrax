/*-------------------------------------------------------------------------------
 *\file platforms/build.cpp
 *\info Build recipe for the platforms module (the C runtime). The runtime is
 *      not compiled into the amalgamation; it is baked into a generated header
 *      (artifacts/THxRTxAMALG.hpp) that the C backend embeds as a string so
 *      emitted programs are self-contained. Headers first (inclusion order),
 *      sources after.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

BLD::Module
platforms_module()
{
  BLD::Module m;
  m.name     = "platforms";
  m.gen_deps = { "artifacts/THxRTxAMALG.hpp" };
  m.generate = [](const BLD::Ctx &c) {
    BLD::gen_runtime_header(c,
                            c.artifacts + "/THxRTxAMALG.hpp",
                            { "platforms/THxCHECK.h",
                              "platforms/THxVALUE.h",
                              "platforms/THxMEM.h",
                              "platforms/THxRT.h",
                              "platforms/THxK.h",
                              "platforms/THxCHECK.c",
                              "platforms/THxVALUE.c",
                              "platforms/THxMEMBUMP.c",
                              "platforms/THxMEMRC.c",
                              "platforms/THxRT.c",
                              "platforms/THxK.c" });
  };
  return m;
}
