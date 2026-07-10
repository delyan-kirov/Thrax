/*-------------------------------------------------------------------------------
 *\file tests/build.cpp
 *\info Build recipe for the tests module. TS.cpp + tst_all.cpp are standalone
 *      TUs linked against the amalgam into artifacts/tst_all -- not part of the
 *      library. This module also owns the run logic for the interpreter smoke
 *      test and the native-backend test.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

BLD::Module
tests_module()
{
  BLD::Module m;
  m.name = "tests";
  m.exes = { { "tst_all", { "tests/tst_all.cpp", "tests/TS.cpp" } } };
  return m;
}

// Interpreter smoke test: evaluate every example through the reified-K machine.
int
tests_smoke(
  const BLD::Ctx &c)
{
  return BLD::run("./" + c.artifacts + "/tst_all");
}

// Emit, compile and run every example through the native backend; each must
// exit 0 (the generated THx_force_all forces every global, so a fault
// surfaces). A program is only SKIPped if `thrax -c` itself declines it.
int
tests_native(
  const BLD::Ctx &c)
{
  namespace fs                   = std::filesystem;
  const std::string        thrax = "./" + c.artifacts + "/thrax";
  const std::string        cfile = c.artifacts + "/native.c";
  const std::string        bfile = c.artifacts + "/native.bin";
  std::vector<std::string> files = BLD::glob({ "examples" }, { ".thx" });
  int                      pass = 0, skip = 0, fail = 0;
  for (const std::string &f : files)
  {
    if (BLD::run(thrax + " -c '" + f + "' > " + cfile + " 2>/dev/null") != 0)
    {
      std::cout << "SKIP  " << f << "\n";
      ++skip;
      continue;
    }
    if (BLD::run("cc -O2 " + cfile + " -o " + bfile + " 2>/dev/null") != 0)
    {
      std::cout << "FAIL(cc)  " << f << "\n";
      ++fail;
      continue;
    }
    if (BLD::run("./" + bfile + " >/dev/null 2>&1") != 0)
    {
      std::cout << "FAIL(run) " << f << "\n";
      ++fail;
      continue;
    }
    std::cout << "PASS  " << f << "\n";
    ++pass;
  }
  fs::remove(cfile);
  fs::remove(bfile);
  std::cout << "native-test: pass=" << pass << " skip=" << skip
            << " fail=" << fail << "\n";
  return fail == 0 ? 0 : 1;
}
