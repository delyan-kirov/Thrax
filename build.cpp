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
 *      Subcommands: build (default), test, native-test, clean, check-ascii,
 *      check-format, install-hooks, pre-push, format, tokei, valgrind, env. A
 *      leading ffi/no-ffi option toggles FFI (on by default):
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

// Where the vendored libffi (static lib + headers) is built from the
// external/libffi subtree. Kept out of the root artifacts/ so a plain
// `build clean` never discards it (only `clean-recursive` does).
static const std::string VENDORED_FFI = "external/artifacts";

// Build the vendored libffi from the subtree into VENDORED_FFI. 0 on success,
// non-zero on failure (with a diagnostic on stderr). Defined below; forward
// declared so resolve_ffi can build on demand. Non-fatal so resolve_ffi can
// fall back to nix.
static int build_vendored_ffi();

// libffi resolution. The vendored subtree is the DEFAULT: use it if built, else
// build it on demand (needs the external dev shell's autotools). Only if that
// build *fails* do we warn and fall back to nix / pkg-config. If nothing works,
// error -- pass `no-ffi` to build without FFI.
static void
resolve_ffi(
  Ctx &c, Ffi mode)
{
  if (mode == Ffi::Off) return;

  const std::string vlib = VENDORED_FFI + "/lib/libffi.a";
  const std::string vinc = VENDORED_FFI + "/include";

  // 1. Vendored is the default. If not built yet, build it from the subtree:
  //    build_vendored_ffi pulls autotools from the external flake via
  //    `nix develop ./external` when they aren't already on PATH, so this works
  //    from the plain main shell too. Only a genuine build *failure* warns and
  //    falls through to nix / pkg-config.
  bool built = std::filesystem::exists(vlib);
  if (!built)
  {
    if (build_vendored_ffi() == 0)
      built = std::filesystem::exists(vlib);
    else
      std::print(stderr,
                 "build: warning: failed to build the vendored libffi from "
                 "external/libffi; falling back to nix / pkg-config\n");
  }
  if (built)
  {
    c.ffi_cflags = "-I" + vinc + " -DTHRAX_3RD_PARTY_ON=1";
    c.ffi_libs   = vlib + " -ldl"; // static -- self-contained, no rpath
    return;
  }

  // 2. Fallback: nix ($LIBFFI / $LIBFFI_DEV), then pkg-config.
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

  // 3. Neither the vendored subtree nor nix / pkg-config could provide libffi.
  BLD::die(
    "libffi unavailable: could not build the vendored subtree "
    "(external/libffi) and found no nix ($LIBFFI) or pkg-config libffi. Enter "
    "`nix develop ./external` and run `build ffi-rebuild`, or pass `no-ffi` to "
    "build without FFI.");
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

// Include paths + the invocation banner. FFI resolution (which can build the
// vendored libffi) is deliberately NOT done here -- only compile commands need
// it, so main() calls resolve_ffi/bake_db_flags separately. That keeps
// non-compiling commands (clean, ffi-rebuild, env, ...) from triggering a
// libffi build.
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

// The C/C++ sources clang-format owns (used by both `format` and the pre-push
// format check). glob() skips build.cpp files, so add them and the root
// explicitly.
static std::vector<std::string>
format_files()
{
  std::vector<std::string> f = BLD::glob(
    { "utilities", "compiler", "engines", "app" }, { ".cpp", ".hpp" });
  for (auto &x : BLD::glob({ "platforms" }, { ".c", ".h" })) f.push_back(x);
  for (auto &x : BLD::glob({ "tests" }, { ".cpp", ".hpp" })) f.push_back(x);
  for (const char *d : { "compiler", "engines", "platforms", "app", "tests" })
    f.push_back(std::string(d) + "/build.cpp");
  f.push_back("build.cpp");
  f.push_back("utilities/UTxBUILD.hpp");
  return f;
}

static int
cmd_format()
{
  std::string cmd = "clang-format -i";
  for (auto &x : format_files()) cmd += " " + x;
  return BLD::run(cmd);
}

// Non-mutating counterpart of cmd_format: `--dry-run -Werror` makes
// clang-format exit non-zero if any file would change, without editing it. The
// pre-push hook uses this so a push FAILS on unformatted code instead of
// silently rewriting the tree behind the committer's back.
static int
cmd_check_format()
{
  std::string cmd = "clang-format --dry-run -Werror";
  for (auto &x : format_files()) cmd += " " + x;
  if (BLD::run(cmd) != 0)
  {
    std::print(stderr,
               "build: check-format: some files need formatting; run "
               "`build format`.\n");
    return 1;
  }
  std::print("build: check-format: OK\n");
  return 0;
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

// Like clean, but also removes every nested artifacts/ dir -- module scratch
// and the vendored libffi in external/artifacts -- forcing a libffi rebuild
// next time. Plain `clean` deliberately leaves those alone (libffi is slow to
// build).
static int
cmd_clean_recursive(
  const Ctx &c)
{
  namespace fs = std::filesystem;
  cmd_clean(c);
  std::error_code       ec;
  std::vector<fs::path> targets;
  for (auto it = fs::recursive_directory_iterator(".", ec);
       it != fs::recursive_directory_iterator();
       it.increment(ec))
  {
    const fs::path &p = it->path();
    if (p.filename() == ".git")
    {
      it.disable_recursion_pending();
      continue;
    }
    if (it->is_directory(ec) && p.filename() == "artifacts")
    {
      targets.push_back(p);
      it.disable_recursion_pending(); // don't descend into what we'll delete
    }
  }
  for (const auto &t : targets) fs::remove_all(t, ec);
  std::print("build: clean-recursive removed {} artifacts dir(s)\n",
             targets.size());
  return 0;
}

// Build the vendored libffi from the external/libffi subtree into
// external/artifacts (static lib + public headers). This is the ONLY step that
// compiles libffi: it drives libffi's own autotools build (the single place the
// project shells out to configure/make), and needs autoconf/automake/libtool --
// provided by `nix develop ./external`. Output is version/triplet-independent:
// we search the build tree for the freshly built libffi.a + generated headers.
// Returns 0 on success; on failure it prints why and returns non-zero
// (non-fatal so resolve_ffi can fall back to nix) -- `build ffi-rebuild` turns
// that into a hard error.
static int
build_vendored_ffi()
{
  namespace fs          = std::filesystem;
  const std::string src = "external/libffi";
  if (!fs::exists(src + "/configure.ac"))
  {
    std::print(stderr,
               "build: ffi: external/libffi subtree missing (run: git "
               "subtree add --prefix=external/libffi <fork> master "
               "--squash)\n");
    return 1;
  }

  // Build in a throwaway copy under external/artifacts so the tracked subtree
  // stays pristine: autoreconf would otherwise refresh config.guess/config.sub
  // and scatter generated files into external/libffi, dirtying it for future
  // `git subtree pull`. The copy (and all build scratch) is gitignored.
  std::error_code   ec;
  const std::string bld = VENDORED_FFI + "/build";
  const std::string inc = VENDORED_FFI + "/include";
  const std::string lib = VENDORED_FFI + "/lib";
  fs::remove_all(VENDORED_FFI, ec);
  fs::create_directories(VENDORED_FFI, ec);
  fs::copy(src, bld, fs::copy_options::recursive, ec);
  if (ec)
  {
    std::print(
      stderr, "build: ffi: failed to stage build copy: {}\n", ec.message());
    return 1;
  }

  // libffi's own autotools build, as one shell pipeline. It needs
  // autoconf/automake/libtool: if they are already on PATH (you are in the
  // external dev shell) run it directly; otherwise pull them from the external
  // flake with `nix develop ./external`. Either way the build runs from `bld`.
  const std::string steps
    = "cd " + bld
      + " && autoreconf -fiv && "
        "./configure --enable-static --disable-shared --disable-docs "
        "--disable-dependency-tracking CFLAGS=-fPIC && make";
  std::vector<std::string> cmd;
  if (BLD::exec({ "autoreconf", "--version" }).code == 0)
    cmd = { "bash", "-c", steps };
  else if (BLD::exec({ "nix", "--version" }).code == 0)
  {
    std::print("build: ffi: building libffi via `nix develop ./external`\n");
    cmd = { "nix", "develop", "./external", "--command", "bash", "-c", steps };
  }
  else
  {
    std::print(stderr,
               "build: ffi: need autoconf/automake/libtool (or nix to "
               "provide them via ./external) to build libffi\n");
    return 1;
  }
  if (BLD::spawn(cmd) != 0)
  {
    std::print(stderr, "build: ffi: libffi build failed\n");
    return 1;
  }

  // Collect the static lib + public headers, then drop the build tree so
  // external/artifacts holds only lib/ + include/.
  fs::create_directories(inc, ec);
  fs::create_directories(lib, ec);
  bool a = false, h = false, t = false;
  for (const auto &e : fs::recursive_directory_iterator(bld, ec))
  {
    const fs::path   &p          = e.path();
    const std::string n          = p.filename().string();
    const bool        in_include = p.parent_path().filename() == "include";
    if (!a && n == "libffi.a" && p.string().find(".libs") != std::string::npos)
      a = fs::copy_file(
        p, lib + "/libffi.a", fs::copy_options::overwrite_existing, ec);
    else if (!h && n == "ffi.h" && in_include) // the generated ffi.h, not .in
      h = fs::copy_file(
        p, inc + "/ffi.h", fs::copy_options::overwrite_existing, ec);
    else if (!t && n == "ffitarget.h" && in_include)
      t = fs::copy_file(
        p, inc + "/ffitarget.h", fs::copy_options::overwrite_existing, ec);
  }
  fs::remove_all(bld, ec);
  if (!(a && h && t))
  {
    std::print(stderr,
               "build: ffi: built libffi but could not locate "
               "libffi.a / ffi.h / ffitarget.h in the build tree\n");
    return 1;
  }
  std::print("build: vendored libffi installed to {}/ (lib + include)\n",
             VENDORED_FFI);
  return 0;
}

// `build ffi-rebuild`: force a vendored libffi build; a failure is fatal here.
static int
cmd_ffi_rebuild()
{
  if (build_vendored_ffi() != 0) BLD::die("libffi rebuild failed");
  return 0;
}

// `build check-ascii`: enforce the ASCII-only rule for tracked sources and
// docs. Any byte >= 0x80 in a checked file is a violation, reported as
// file:line:col. A file may opt out by containing the marker below (e.g.
// examples/STRINGS.thx, whose whole point is to lex raw UTF-8). Skips the
// build/vendor/VCS trees (artifacts/, external/, bin/, .git/, .cache/). Returns
// 0 when clean, 1 on any violation. Wired into the git pre-push hook, not into
// `build test`.
//
// The marker is spelled with adjacent string literals so this file -- which
// must name the marker -- does not itself contain the contiguous token, and so
// stays subject to the check rather than exempting itself.
static const char *const ASCII_OPT_OUT = "thrax-allow"
                                         "-nonascii";

static bool
ascii_checked_ext(
  const std::string &ext)
{
  return ext == ".cpp" || ext == ".hpp" || ext == ".c" || ext == ".h"
         || ext == ".md" || ext == ".txt" || ext == ".thx";
}

static int
cmd_check_ascii()
{
  namespace fs = std::filesystem;
  std::error_code ec;
  int             violations = 0, scanned = 0, exempt = 0;
  for (auto it = fs::recursive_directory_iterator(".", ec);
       it != fs::recursive_directory_iterator();
       it.increment(ec))
  {
    const fs::path   &p    = it->path();
    const std::string name = p.filename().string();
    if (it->is_directory(ec))
    {
      if (name == ".git" || name == "artifacts" || name == "external"
          || name == "bin" || name == ".cache")
        it.disable_recursion_pending();
      continue;
    }
    if (!it->is_regular_file(ec) || !ascii_checked_ext(p.extension().string()))
      continue;

    const std::string body = BLD::read_file(p.string());
    if (body.find(ASCII_OPT_OUT) != std::string::npos)
    {
      ++exempt;
      continue;
    }
    ++scanned;
    int line = 1, col = 1;
    for (unsigned char ch : body)
    {
      if (ch == '\n')
      {
        ++line;
        col = 1;
        continue;
      }
      if (ch >= 0x80)
      {
        std::print(stderr,
                   "{}:{}:{}: non-ASCII byte 0x{:02x}\n",
                   p.string(),
                   line,
                   col,
                   static_cast<unsigned>(ch));
        ++violations;
      }
      ++col;
    }
  }
  if (violations)
  {
    std::print(stderr,
               "build: check-ascii: {} non-ASCII byte(s) in tracked sources/"
               "docs (see above). Keep sources and docs ASCII-only, or add the "
               "marker '{}' to a file that must contain non-ASCII bytes.\n",
               violations,
               ASCII_OPT_OUT);
    return 1;
  }
  std::print(
    "build: check-ascii: OK ({} files scanned, {} exempt)\n", scanned, exempt);
  return 0;
}

// `build install-hooks`: install the git pre-push hook. To stay cross-platform
// (Windows included) NONE of the check logic lives in shell -- the hook is a
// one-line POSIX-sh shim that execs `build pre-push`, and git runs hooks
// through its bundled sh on every platform. All real work (ascii + format
// checks, tests) is portable C++ in the build program. Re-run this after
// changing the shim.
static int
cmd_install_hooks()
{
  namespace fs = std::filesystem;
  if (!fs::exists(".git"))
    BLD::die("install-hooks: no .git here -- run from the repo root");
  std::error_code ec;
  const fs::path  hooks = ".git/hooks";
  fs::create_directories(hooks, ec);
  const fs::path hook = hooks / "pre-push";
  BLD::write_file(
    hook.string(),
    "#!/bin/sh\n"
    "# Auto-generated by `build install-hooks`; do not edit. A one-line shim "
    "so\n"
    "# the real pre-push checks stay in the portable C++ build program (they "
    "run\n"
    "# on Windows too, where git executes hooks via its bundled sh). "
    "Bootstrap\n"
    "# the build program on a fresh clone, then hand off to it.\n"
    "[ -x ./build ] || clang++ -std=c++23 -Iutilities build.cpp -o build\n"
    "exec ./build pre-push\n");
  fs::permissions(hook,
                  fs::perms::owner_all | fs::perms::group_read
                    | fs::perms::group_exec | fs::perms::others_read
                    | fs::perms::others_exec,
                  ec);
  if (ec)
    BLD::die("install-hooks: cannot set hook permissions: " + ec.message());
  std::print("build: installed pre-push hook at {} (runs `build pre-push`)\n",
             hook.string());
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
//      nothing to build here, so warn and stop rather than erroring
//      cryptically. Exits.
//
// Returns the module name to build (empty for the whole project) in cases 1/3,
// having chdir'd to the root; otherwise it exits.
static std::string
delegate_local(
  int argc, char **argv)
{
  namespace fs  = std::filesystem;
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
                         + (root / "utilities").string()
                         + " build.cpp -o build";
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
    std::print(
      "build: building the '{}' module (from {})\n", top, root.string());
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
  Ctx         c    = make_ctx(argc, argv);
  auto        mods = modules();

  // Commands that neither compile Thrax nor need libffi are handled first, so
  // they never trigger FFI resolution (which can build the vendored libffi).
  if (cmd == "clean") return cmd_clean(c);
  if (cmd == "clean-recursive") return cmd_clean_recursive(c);
  if (cmd == "check-ascii") return cmd_check_ascii();
  if (cmd == "check-format") return cmd_check_format();
  if (cmd == "install-hooks") return cmd_install_hooks();
  if (cmd == "ffi-rebuild") return cmd_ffi_rebuild();
  if (cmd == "format") return cmd_format();
  if (cmd == "tokei")
    return BLD::run("tokei --exclude artifacts --exclude external");
  if (cmd == "env")
  {
    std::string root = std::filesystem::current_path().string();
    std::print("export THRAX_ROOT={}\n", root);
    std::print("export PATH={}:{}/{}:$PATH\n", root, root, c.artifacts);
    return 0;
  }
  if (cmd == "compile-commands")
  {
    // Precise compile_commands.json for clangd: intercept a full (clean) build
    // with bear so every TU's real command is recorded. clean first so every
    // TU actually recompiles (bear only sees compilers it launches). THRAX_DB
    // tells the child build to bake in the absolute compiler + nix -isystem
    // paths (see bake_db_flags), so the DB is self-contained. The child ./build
    // does its own FFI resolution.
    cmd_clean(c);
    write_clangd_config();
    setenv("THRAX_DB", "1", 1);
    return BLD::spawn({ "bear", "--", "./build" });
  }

  // Everything below compiles Thrax, so resolve libffi now -- this is what may
  // build the vendored libffi on demand (see resolve_ffi).
  resolve_ffi(c, ffi);
  bake_db_flags(c);

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
  if (cmd == "valgrind")
  {
    BLD::build(c, mods);
    return BLD::run("valgrind --leak-check=full --show-leak-kinds=all "
                    "--track-origins=yes ./"
                    + c.artifacts + "/tst_all");
  }
  if (cmd == "pre-push")
  {
    // The gate the git pre-push hook runs (see cmd_install_hooks). Cheap,
    // non-compiling checks first so a bad push fails fast, then build + tests.
    if (int rc = cmd_check_ascii()) return rc;
    if (int rc = cmd_check_format()) return rc;
    BLD::build(c, mods);
    return tests_smoke(c);
  }

  std::print(stderr,
             "usage: build [ffi|no-ffi] [build|test|native-test|clean|"
             "clean-recursive|check-ascii|check-format|install-hooks|"
             "ffi-rebuild|format|compile-commands|tokei|valgrind|env|"
             "pre-push]\n");
  return 2;
}
