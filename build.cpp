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
 *      valgrind, env. A leading ffi/no-ffi option toggles FFI (on by default):
 *      `./build no-ffi test`, `./build ffi`.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp"

#include "app/build.cpp"
#include "compiler/build.cpp"
#include "engines/build.cpp"
#include "platforms/build.cpp"
#include "tests/build.cpp"

using BLD::Ctx;
using BLD::Module;

// FFI is on by default (the nix dev shell provides libffi). `no-ffi` forces it
// off (for the no-nix bootstrap); `ffi` requires it (error if libffi is absent).
enum class Ffi
{
  Default,
  On,
  Off
};

// libffi layered resolution: env (nix) -> pkg-config -> vendored (not wired).
static void
resolve_ffi(
  Ctx &c, Ffi mode)
{
  if (mode == Ffi::Off) return;

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
    = BLD::trim(BLD::exec({ "pkg-config", "--libs", "libffi" }).out);
  std::string cf
    = BLD::trim(BLD::exec({ "pkg-config", "--cflags", "libffi" }).out);
  if (!l.empty())
  {
    c.ffi_cflags = cf + " -DTHRAX_3RD_PARTY_ON=1";
    c.ffi_libs   = l + " -ldl";
    return;
  }

  const std::string miss
    = "libffi not found: enter `nix develop` (sets $LIBFFI / $LIBFFI_DEV), "
      "install libffi (pkg-config), or vendor external/libffi";
  // An explicit `ffi` must be satisfiable; the default is best-effort so the
  // no-nix bootstrap still builds (without @extern support at runtime).
  if (mode == Ffi::On) BLD::die(miss);
  std::print(stderr,
             "build: {}\nbuild: building WITHOUT FFI (@extern unavailable at "
             "runtime; pass `ffi` to require it)\n",
             miss);
}

static Ctx
make_ctx(
  int argc, char **argv, Ffi ffi)
{
  Ctx c;
  c.includes
    = "-I. -Iutilities -Icompiler -Iengines -Iplatforms -Iapp -I" + c.artifacts;
  std::string inv = "./build";
  for (int i = 1; i < argc; ++i) inv += " " + std::string(argv[i]);
  c.invocation = inv;
  resolve_ffi(c, ffi);
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
  f.push_back("utilities/UTxBUILD.hpp");
  std::string cmd = "clang-format -i";
  for (auto &x : f) cmd += " " + x;
  return BLD::run(cmd);
}

// Remove build artifacts and scratch files (no shell -- pure std::filesystem).
static int
cmd_clean(
  const Ctx &c)
{
  namespace fs = std::filesystem;
  std::error_code ec;
  fs::remove_all(c.artifacts, ec);
  fs::remove("compile_flags.txt", ec);
  for (const auto &e : fs::directory_iterator("."))
  {
    std::string n    = e.path().filename().string();
    bool        orig = n.size() > 5 && n.compare(n.size() - 5, 5, ".orig") == 0;
    if (n.rfind("tmp.", 0) == 0 || n.rfind("vgcore", 0) == 0 || orig)
      fs::remove_all(e.path(), ec);
  }
  return 0;
}

// This project's sources + flags for the shared self-rebuild (BLD::rebuild_self
// does the timestamp check and portable re-exec).
static void
rebuild_self(
  int argc, char **argv)
{
  std::vector<std::string> srcs = { "build.cpp",
                                    "utilities/UTxBUILD.hpp",
                                    "utilities/UT.hpp",
                                    "utilities/AR.hpp" };
  for (const char *d : { "compiler", "engines", "platforms", "app", "tests" })
    srcs.push_back(std::string(d) + "/build.cpp");
  BLD::rebuild_self(
    argc, argv, "clang++ -std=c++23 -O1 -Iutilities build.cpp -o build", srcs);
}

int
main(
  int argc, char **argv)
{
  rebuild_self(argc, argv);

  // Optional leading FFI option: `./build [ffi|no-ffi] <cmd> ...`.
  int argi = 1;
  Ffi ffi  = Ffi::Default;
  if (argi < argc && std::string(argv[argi]) == "no-ffi")
  {
    ffi = Ffi::Off;
    ++argi;
  }
  else if (argi < argc && std::string(argv[argi]) == "ffi")
  {
    ffi = Ffi::On;
    ++argi;
  }

  std::string cmd  = argi < argc ? argv[argi] : "build";
  Ctx         c    = make_ctx(argc, argv, ffi);
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
  if (cmd == "clean") return cmd_clean(c);
  if (cmd == "format") return cmd_format();
  if (cmd == "compile-commands")
  {
    // Precise compile_commands.json for clangd: intercept a full (clean) build
    // with bear so every TU's real command is recorded. clean first so every
    // TU actually recompiles (bear only sees compilers it launches).
    cmd_clean(c);
    return BLD::spawn({ "bear", "--", "./build" });
  }
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
    std::print("export THRAX_ROOT={}\n", root);
    std::print("export PATH={}:{}/{}:$PATH\n", root, root, c.artifacts);
    return 0;
  }

  std::print(stderr,
             "usage: build [ffi|no-ffi] [build|test|native-test|clean|format|"
             "compile-commands|tokei|valgrind|env]\n");
  return 2;
}
