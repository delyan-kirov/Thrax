/*-------------------------------------------------------------------------------
 *\file build.cpp
 *\info Standalone build for the Thrax raylib demo -- a native-backend FFI
 *      example. No make, no shell: it emits C from MAIN.thx with the Thrax
 *      compiler, compiles/links it (-ldl for the dlopen/dlsym the @extern
 *      bindings use), and symlinks raylib's shared object into bin/ so those
 *      bindings resolve at runtime.
 *
 *      It reuses the repo-root build helpers (utilities/UTxBUILD.hpp) -- the
 *      same no-shell spawner, redirect and self-rebuild as the main build -- so
 *      bootstrap it with that dir on the include path:
 *
 *          clang++ -std=c++23 -I../../utilities build.cpp -o build  # bootstrap
 * once nix develop                                              # raylib
 * ($RAYLIB) + cc + display
 *          ./build run                                              # emit ->
 * compile -> link -> run
 *
 *      Thereafter it self-rebuilds when this file (or those helpers) change.
 *      Subcommands: build (default), run, clean.
 *-----------------------------------------------------------------------------*/

#include "UTxBUILD.hpp" // BLD::spawn / run / die / needs_rebuild / rebuild_self

#include <cstdlib> // getenv

namespace fs = std::filesystem;
using BLD::die;

// The Thrax compiler, produced by the repo-root build (`./build` there).
static const std::string THRAX = "../../artifacts/thrax";

// Build the compiler from the repo root if it is missing. The root build.cpp
// uses relative paths, so it must run with the repo root as cwd.
static void
ensure_compiler()
{
  if (fs::exists(THRAX)) return;
  std::print(
    "build: thrax compiler missing -- building it from the repo root\n");

  fs::path here = fs::current_path();
  fs::current_path("../..");
  int rc
    = fs::exists("./build") ? BLD::run("./build")
      : BLD::run("clang++ -std=c++23 -Iutilities -O0 build.cpp -o build") == 0
        ? BLD::run("./build")
        : 1;
  fs::current_path(here);

  if (rc != 0 || !fs::exists(THRAX)) die("failed to build the thrax compiler");
}

static int
cmd_build()
{
  ensure_compiler();
  fs::create_directories("bin");

  // Emit self-contained C from the Thrax source, then compile/link it. FFI
  // programs need -ldl (dlopen/dlsym); the runtime is baked into the emitted C.
  std::print("thrax -c MAIN.thx > bin/raylib_demo.c\n");
  if (BLD::spawn({ THRAX, "-c", "MAIN.thx" }, "bin/raylib_demo.c") != 0)
    die("thrax failed to emit C");
  const char *cc = std::getenv("CC");
  std::string CC = cc && *cc ? cc : "cc";
  if (BLD::run(CC + " -O2 bin/raylib_demo.c -ldl -o bin/raylib_demo") != 0)
    die("cc failed to compile the emitted C");

  // Symlink raylib's shared object into bin/ so the demo's relative @extern
  // library path resolves at runtime (its own X11/GL deps resolve via the nix
  // rpaths baked into the .so).
  const char *raylib = std::getenv("RAYLIB");
  if (!raylib || !*raylib)
    die("RAYLIB is unset. Enter the dev shell first:  nix develop");
  fs::path        so = fs::path(raylib) / "lib" / "libraylib.so";
  std::error_code ec;
  fs::remove("bin/libraylib.so", ec);
  fs::create_symlink(so, "bin/libraylib.so", ec);
  if (ec) die("failed to symlink " + so.string() + ": " + ec.message());
  return 0;
}

int
main(
  int argc, char **argv)
{
  BLD::rebuild_self(
    argc,
    argv,
    "clang++ -std=c++23 -I../../utilities -O2 build.cpp -o build",
    { "build.cpp", "../../utilities/UTxBUILD.hpp", "../../utilities/UT.hpp" });

  std::string cmd = argc > 1 ? argv[1] : "build";

  if (cmd == "build" || cmd == "all") return cmd_build();
  if (cmd == "run")
  {
    if (int rc = cmd_build(); rc != 0) return rc;
    std::print("./bin/raylib_demo\n");
    return BLD::spawn({ "./bin/raylib_demo" }); // needs a display
  }
  if (cmd == "clean")
  {
    std::error_code ec;
    fs::remove_all("bin", ec);
    return 0;
  }

  std::print(stderr, "usage: build [build|run|clean]\n");
  return 2;
}
