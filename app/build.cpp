/*-------------------------------------------------------------------------------
 *\file app/build.cpp
 *\info Build recipe for the app module (the thrax CLI). DR.cpp (the pipeline
 *      driver) joins the library amalgamation; main.cpp is the standalone entry
 *      point, compiled on its own and linked against the amalgam into
 *      artifacts/thrax.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

BLD::Module
app_module()
{
  BLD::Module m;
  m.name         = "app";
  m.amalgam_dirs = { "app" };
  m.exes         = { { "thrax", { "app/main.cpp" } } };
  return m;
}
