/*-------------------------------------------------------------------------------
 *\file build.cpp
 *\info The Thrax build program -- a thin orchestrator. All the build logic
 *      lives in utilities/UTxBUILD.hpp (the bootstrap library) and in each
 *      module's build.cpp; this file just wires them together, resolves the
 *      build context, and dispatches subcommands. It self-rebuilds when any
 *      build source changes (nob-style).
 *
 *      Bootstrap (Tier 0, no nix):  clang++ -std=c++20 -Iutilities build.cpp -o
 * build && ./build With nix:                    `nix develop` bootstraps
 * ./build for you.
 *
 *      Subcommands: build (default), test, native-test, clean, format, tokei,
 *      valgrind, env.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

#include "app/build.cpp"
#include "compiler/build.cpp"
#include "engines/build.cpp"
#include "platforms/build.cpp"
#include "tests/build.cpp"

using BLD::Ctx;
using BLD::Module;

// libffi layered resolution: env (nix) -> pkg-config -> vendored (not wired).
static void
resolve_ffi(
  Ctx &c)
{
  if (!std::getenv("THRAX_3RD_PARTY_ON")) return;
  const char *dev = std::getenv("LIBFFI_DEV");
  const char *lib = std::getenv("LIBFFI");
  if (dev && lib)
  {
    c.ffi_cflags = "-I" + std::string(dev) + "/include -DTHRAX_3RD_PARTY_ON=1";
    c.ffi_libs   = "-L" + std::string(lib) + "/lib -lffi -ldl -Wl,-rpath,"
                 + std::string(lib) + "/lib";
    return;
  }
  std::string l
    = BLD::trim(BLD::capture("pkg-config --libs libffi 2>/dev/null"));
  std::string cf
    = BLD::trim(BLD::capture("pkg-config --cflags libffi 2>/dev/null"));
  if (!l.empty())
  {
    c.ffi_cflags = cf + " -DTHRAX_3RD_PARTY_ON=1";
    c.ffi_libs   = l + " -ldl";
    return;
  }
  BLD::die("libffi requested (THRAX_3RD_PARTY_ON) but not found: set $LIBFFI "
           "and $LIBFFI_DEV (nix), install libffi (pkg-config), or vendor "
           "external/libffi");
}

static Ctx
make_ctx(
  int argc, char **argv)
{
  Ctx c;
  c.includes
    = "-I. -Iutilities -Icompiler -Iengines -Iplatforms -Iapp -I" + c.artifacts;
  std::string inv = "./build";
  for (int i = 1; i < argc; ++i) inv += " " + std::string(argv[i]);
  c.invocation = inv;
  resolve_ffi(c);
  return c;
}

static std::vector<Module>
modules()
{
  // utilities has no build.cpp: it bootstraps the build (UTxBUILD.hpp lives
  // there). Its sources join the amalgamation directly.
  Module util{ "utilities", { "utilities" }, {}, nullptr };
  return {
    util,         compiler_module(), engines_module(), platforms_module(),
    app_module(), tests_module()
  };
}

static int
cmd_format()
{
  std::vector<std::string> f = BLD::glob(
    { "utilities", "compiler", "engines", "app" }, { ".cpp", ".hpp" });
  for (auto &x : BLD::glob({ "platforms" }, { ".c", ".h" })) f.push_back(x);
  for (auto &x : BLD::glob({ "tests" }, { ".cpp", ".hpp" })) f.push_back(x);
  // glob() skips build.cpp files; add them (and the root) explicitly.
  for (const char *d : { "compiler", "engines", "platforms", "app", "tests" })
    f.push_back(std::string(d) + "/build.cpp");
  f.push_back("build.cpp");
  std::string cmd = "clang-format -i";
  for (auto &x : f) cmd += " " + x;
  return BLD::run(cmd);
}

// Recompile this program if any build source changed, then re-exec.
static void
rebuild_self(
  char **argv)
{
  std::vector<std::string> srcs = { "build.cpp",
                                    "utilities/UTxBUILD.hpp",
                                    "utilities/UT.hpp",
                                    "utilities/AR.hpp" };
  for (const char *d : { "compiler", "engines", "platforms", "app", "tests" })
    srcs.push_back(std::string(d) + "/build.cpp");
  if (std::filesystem::exists("./build") && BLD::needs_rebuild("./build", srcs))
  {
    std::cout << "build: rebuilding self\n";
    if (BLD::run("clang++ -std=c++20 -Iutilities -O0 build.cpp -o build") != 0)
      BLD::die("failed to rebuild self");
    std::cout.flush(); // execv replaces the image without flushing iostreams
    execv("./build", argv);
    BLD::die("failed to re-exec self");
  }
}

int
main(
  int argc, char **argv)
{
  rebuild_self(argv);

  std::string cmd  = argc > 1 ? argv[1] : "build";
  Ctx         c    = make_ctx(argc, argv);
  auto        mods = modules();

  if (cmd == "build" || cmd == "all")
  {
    BLD::build(c, mods);
    return 0;
  }
  if (cmd == "test")
  {
    BLD::build(c, mods);
    return tests_smoke(c);
  }
  if (cmd == "native-test")
  {
    BLD::build(c, mods);
    return tests_native(c);
  }
  if (cmd == "clean")
    return BLD::run("rm -rf " + c.artifacts
                    + " tmp.* vgcore* *.orig compile_flags.txt");
  if (cmd == "format") return cmd_format();
  if (cmd == "tokei")
    return BLD::run("tokei --exclude artifacts --exclude external");
  if (cmd == "valgrind")
  {
    BLD::build(c, mods);
    return BLD::run("valgrind --leak-check=full --show-leak-kinds=all "
                    "--track-origins=yes ./"
                    + c.artifacts + "/tst_all");
  }
  if (cmd == "env")
  {
    std::string root = std::filesystem::current_path().string();
    std::cout << "export THRAX_ROOT=" << root << "\n";
    std::cout << "export PATH=" << root << ":" << root << "/" << c.artifacts
              << ":$PATH\n";
    return 0;
  }

  std::cerr << "usage: build "
               "[build|test|native-test|clean|format|tokei|valgrind|env]\n";
  return 2;
}
