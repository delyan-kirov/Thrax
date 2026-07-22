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

// Emit, compile and run the WHOLE example suite as one native program: every
// example plus the shared driver (tests/MAIN.thx), whose MAIN runs each
// example's `$ test`. A failing test prints its module name; the process exit
// code is the number of failing modules, so one failure never masks the rest.
int
tests_native(
  const BLD::Ctx &c)
{
  namespace fs            = std::filesystem;
  const std::string thrax = "./" + c.artifacts + "/thrax";
  const std::string cfile = c.artifacts + "/native.c";
  const std::string bfile = c.artifacts + "/native.bin";

  // The combined program: all top-level examples (glob does not recurse, so the
  // sub-directory projects with their own MAIN are excluded) + the driver.
  std::vector<std::string> cmd = { thrax, "-c" };
  for (const std::string &f : BLD::glob({ "examples" }, { ".thx" }))
    cmd.push_back(f);
  cmd.push_back("tests/MAIN.thx");

  if (BLD::spawn(cmd, cfile) != 0)
  {
    std::print(
      "native-test: FAIL -- `thrax -c` rejected the combined program\n");
    return 1;
  }
  // -lm: the suite's externs are libc (implicit) + libm; `thrax --build`
  // derives this from the IR, a hand-rolled cc line names it itself.
  BLD::Output cc = BLD::exec({ "cc", "-O2", cfile, "-o", bfile, "-lm" });
  if (cc.code != 0)
  {
    std::print("native-test: FAIL(cc)\n{}{}", cc.out, cc.err);
    return 1;
  }
  BLD::Output rn = BLD::exec({ "./" + bfile });
  std::print("{}{}", rn.out, rn.err); // surface any "MODULE FAILED" lines
  fs::remove(cfile);
  fs::remove(bfile);
  if (rn.code != 0)
  {
    std::print("native-test: FAIL -- {} module(s) failed\n", rn.code);
    return 1;
  }
  std::print("native-test: OK -- all example modules passed\n");
  return 0;
}

int
tests_wasm(
  const BLD::Ctx &c)
{
  namespace fs            = std::filesystem;
  const std::string thrax = "./" + c.artifacts + "/thrax";
  const std::string cfile = c.artifacts + "/wasm.c";
  const std::string wfile = c.artifacts + "/wasm.bin.wasm";

  const char *wasi_cc = std::getenv("WASI_CC");
  if (!wasi_cc || !*wasi_cc)
  {
    std::print("wasm-test: FAIL -- WASI_CC is not set (the nix dev shell "
               "exports it; any wasi-sdk clang works)\n");
    return 1;
  }

  std::vector<std::string> cmd = { thrax, "-c", "--target=wasm32-wasi" };
  for (const std::string &f : BLD::glob({ "examples" }, { ".thx" }))
    cmd.push_back(f);
  cmd.push_back("tests/MAIN.thx");

  if (BLD::spawn(cmd, cfile) != 0)
  {
    std::print("wasm-test: FAIL -- `thrax -c --target=wasm32-wasi` rejected "
               "the combined program\n");
    return 1;
  }
  // -lm as in native-test. The stack flags mirror TG::toolchain: wasm has no
  // guard pages and wasm-ld's default layout puts the 64 KiB shadow stack
  // above the data segment, so a deep C stack silently corrupts globals,
  // stack-first + a native-sized stack instead.
  BLD::Output cc = BLD::exec({ wasi_cc,
                               "-O2",
                               cfile,
                               "-o",
                               wfile,
                               "-lm",
                               "-Wl,--stack-first",
                               "-Wl,-z,stack-size=8388608" });
  if (cc.code != 0)
  {
    std::print("wasm-test: FAIL(cc)\n{}{}", cc.out, cc.err);
    return 1;
  }
  // The suite needs the wasi sandbox opened up: /tmp for the IO example's
  // file round-trip, HOME for its environment check.
  const char *home = std::getenv("HOME");
  BLD::Output rn   = BLD::exec({ "wasmtime",
                                 "--dir=/tmp",
                                 "--env",
                                 std::string("HOME=") + (home ? home : ""),
                                 wfile });
  std::print("{}{}", rn.out, rn.err);
  fs::remove(cfile);
  fs::remove(wfile);
  if (rn.code != 0)
  {
    std::print("wasm-test: FAIL -- {} module(s) failed\n", rn.code);
    return 1;
  }
  std::print("wasm-test: OK -- all example modules passed under wasmtime\n");
  return 0;
}
