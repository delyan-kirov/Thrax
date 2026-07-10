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
    // Emit the C to cfile (its stdout is the artifact); thrax's stderr flows
    // live. cc and the compiled program are run captured -- nothing dropped --
    // and their output is surfaced only when the step fails.
    if (BLD::spawn({ thrax, "-c", f }, cfile) != 0)
    {
      std::print("SKIP  {}\n", f);
      ++skip;
      continue;
    }
    BLD::Output cc = BLD::exec({ "cc", "-O2", cfile, "-o", bfile });
    if (cc.code != 0)
    {
      std::print("FAIL(cc)  {}\n{}{}", f, cc.out, cc.err);
      ++fail;
      continue;
    }
    BLD::Output rn = BLD::exec({ "./" + bfile });
    if (rn.code != 0)
    {
      std::print("FAIL(run) {}\n{}{}", f, rn.out, rn.err);
      ++fail;
      continue;
    }
    std::print("PASS  {}\n", f);
    ++pass;
  }
  fs::remove(cfile);
  fs::remove(bfile);
  std::print("native-test: pass={} skip={} fail={}\n", pass, skip, fail);
  return fail == 0 ? 0 : 1;
}
