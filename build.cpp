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
// off (for the no-nix bootstrap); `ffi` requires it (error if libffi is
// absent).
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

// Under nix, `clang++` is a wrapper that adds its real system include paths --
// the C++ stdlib (gcc/include/c++), libffi, glibc, the clang resource dir -- at
// exec time, partly from $NIX_CFLAGS_COMPILE and partly from its own
// resolution. `bear` records none of them, and clangd neither runs the wrapper
// nor reads $NIX_CFLAGS_COMPILE, so on its own it can't find
// <vector>/<print>/ffi.h. When generating the compile DB (THRAX_DB, set by the
// compile-commands step) under nix, ask the driver for its full search list
// (`clang++ -E -x c++ -v`) and bake every entry in as -isystem, so each
// recorded command is self-contained -- working even with an editor launched
// outside `nix develop`. No-op off nix and on normal builds.
static void
bake_db_flags(
  Ctx &c)
{
  if (!std::getenv("THRAX_DB") || !std::getenv("NIX_CFLAGS_COMPILE")) return;
  // The search list is printed to stderr, between these two markers.
  BLD::Output v = BLD::exec({ "sh", "-c", c.cxx + " -E -x c++ -v /dev/null" });
  const std::string beg = "#include <...> search starts here:";
  const std::string end = "End of search list.";
  auto              b   = v.err.find(beg);
  auto              e   = v.err.find(end);
  if (b == std::string::npos || e == std::string::npos) return;
  b += beg.size();
  for (const std::string &ln : BLD::split_lines(v.err.substr(b, e - b)))
  {
    std::string d = BLD::trim(ln);
    if (d.empty()) continue;
    auto paren = d.find(" ("); // strip clang's " (framework directory)" suffix
    if (paren != std::string::npos) d = BLD::trim(d.substr(0, paren));
    c.includes += " -isystem " + d;
  }
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
  bake_db_flags(c);
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

// Emit a .clangd so the editor works even when the DB isn't regenerated: nix's
// clang++ wrapper hides its stdlib -isystem paths behind $NIX_CFLAGS_COMPILE,
// so authorize clangd to query the driver directly and recover them.
// Committable -- the glob is machine-independent. (Requires launching the
// editor from inside `nix develop`; the baked-in DB flags cover the case where
// it isn't.)
static void
write_clangd_config()
{
  BLD::write_file(
    ".clangd",
    "# Generated by `build compile-commands`.\n"
    "#\n"
    "# nix's clang++ is a wrapper that injects the stdlib/libffi\n"
    "# -isystem paths from $NIX_CFLAGS_COMPILE at exec time. Let\n"
    "# clangd query the driver to recover them (works when the\n"
    "# editor is launched inside `nix develop`).\n"
    "CompileFlags:\n"
    "  QueryDriver: /nix/store/*/bin/*clang*\n");
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

// Locate the repo root: the nearest ancestor of `cwd` (inclusive) that holds
// the bootstrap library. Empty if `cwd` is not inside a Thrax checkout.
static std::filesystem::path
find_root(
  std::filesystem::path cwd)
{
  namespace fs = std::filesystem;
  for (fs::path d = cwd;; d = d.parent_path())
  {
    if (fs::exists(d / "utilities" / "UTxBUILD.hpp")) return d;
    if (d == d.parent_path()) return {}; // reached the filesystem root
  }
}

// True if `top` (a top-level directory name under the root) belongs to a module
// the root build knows how to build.
static bool
is_module_dir(
  const std::string &top)
{
  for (const Module &m : modules())
  {
    if (m.name == top) return true;
    for (const std::string &d : m.amalgam_dirs)
      if (d == top) return true;
  }
  return false;
}

// Make `build` do something meaningful no matter which directory it is run from
// (it lives on $PATH via `build env`, so this is the common case). Four
// outcomes, in order:
//
//   1. The repo root: the full build. Returns "".
//   2. A subdirectory with its own *standalone* build.cpp (one with a main(),
//      like examples/raylib_demo) owns its build -- hand the whole invocation
//      off to it, bootstrapping its ./build first if needed. Exits.
//   3. A module directory (compiler/, engines/, platforms/, app/, tests/, ...).
//      The root build assumes the repo root as cwd, so chdir there and return
//      the module name; main() uses it to build just that module.
//   4. Anything else (doc/, a fresh dir, outside the checkout): there is
//      nothing to build here, so warn and stop rather than erroring cryptically.
//      Exits.
//
// Returns the module name to build (empty for the whole project) in cases 1/3,
// having chdir'd to the root; otherwise it exits.
static std::string
delegate_local(
  int argc, char **argv)
{
  namespace fs = std::filesystem;
  fs::path cwd  = fs::current_path();
  fs::path root = find_root(cwd);

  if (root.empty())
  {
    std::print(stderr,
               "build: warning: not inside a Thrax project (no "
               "utilities/UTxBUILD.hpp at or above {}); nothing to build\n",
               cwd.string());
    std::exit(0);
  }
  if (root == cwd) return ""; // at the root already: full build.

  // (2) Standalone local build -- a build.cpp with its own main().
  fs::path local = cwd / "build.cpp";
  if (fs::exists(local)
      && BLD::read_file(local.string()).find("main(") != std::string::npos)
  {
    // Bootstrap ./build if missing; thereafter its own rebuild_self keeps it
    // fresh. Standalone builds include the bootstrap library from the repo-root
    // utilities dir, so that is the only include path the bootstrap needs.
    if (!fs::exists("./build"))
    {
      std::print("build: bootstrapping local build in {}\n", cwd.string());
      std::string boot = "clang++ -std=c++23 -O1 -I"
                       + (root / "utilities").string() + " build.cpp -o build";
      if (BLD::run(boot) != 0) BLD::die("failed to bootstrap local build");
    }
    std::vector<std::string> a = { "./build" };
    for (int i = 1; i < argc; ++i) a.push_back(argv[i]);
    std::fflush(stdout);
    std::exit(BLD::spawn(a));
  }

  // (3) A module directory: build just that module (from the root, whose cwd
  //     the build's relative paths assume). Consumer modules still pull in the
  //     shared core lib; a pure library/generation module builds only its own
  //     contribution. The per-module target applies to a plain `build`; other
  //     subcommands (test, native-test, ...) ignore it and build the project.
  fs::path    rel = fs::relative(cwd, root);
  std::string top = rel.empty() ? "" : rel.begin()->string();
  if (is_module_dir(top))
  {
    std::print("build: building the '{}' module (from {})\n", top, root.string());
    fs::current_path(root);
    return top;
  }

  // (4) Nothing meaningful here.
  std::print(stderr,
             "build: warning: nothing to build in {} -- no standalone "
             "build.cpp, and not a Thrax module directory.\n"
             "build: run `build` from {} to build the project.\n",
             cwd.string(),
             root.string());
  std::exit(0);
}

int
main(
  int argc, char **argv)
{
  // When invoked from a subdirectory, this may chdir to the repo root and hand
  // back the name of the single module to build (empty for the whole project),
  // delegate to a standalone build, or stop with a warning.
  std::string only = delegate_local(argc, argv);
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
    BLD::build(c, mods, only); // `only` is set when run from a module dir
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
    // TU actually recompiles (bear only sees compilers it launches). THRAX_DB
    // tells the child build to bake in the absolute compiler + nix -isystem
    // paths (see bake_db_flags), so the DB is self-contained.
    cmd_clean(c);
    write_clangd_config();
    setenv("THRAX_DB", "1", 1);
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
