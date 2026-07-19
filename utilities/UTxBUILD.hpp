/*-------------------------------------------------------------------------------
 *\file UTxBUILD.hpp
 *\info Build-support library. Included by build.cpp and every module's
 *      build.cpp -- NOT by the compiler amalgamation. It lives in utilities
 *      because utilities bootstraps the build: build.cpp is compiled with
 *      `-Iutilities` and this header carries all the build logic (process
 *      helpers, incremental-rebuild checks, and the generators that emit the
 *      amalgamation and the baked runtime header into artifacts/).
 *
 *      Pulls the project's std aggregation from UT.hpp rather than
 * re-including.
 *-----------------------------------------------------------------------------*/

#ifndef UTxBUILD_HEADER_
#define UTxBUILD_HEADER_

#include "UT.hpp" // std aggregation (<filesystem> <string> <vector> ...) + helpers

#include <cstdio> // fopen/fread/fwrite/fflush -- file I/O without iostreams
#include <ctime>
#include <functional>
#include <print>
#include <regex>

// Platform shim: all OS-specific process handling is isolated here. The rest of
// the build spawns programs directly from an argv vector -- no std::system, no
// shell -- so there is no quoting/injection surface and command semantics don't
// depend on /bin/sh vs cmd.exe. POSIX uses posix_spawn; Windows uses
// CreateProcess. (The Windows path is a deferred fallback -- Windows is
// primarily supported via generated projects; see doc/architecture.md.)
#if defined(_WIN32)
#include <windows.h>
#else
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
extern char **environ;
#endif

namespace BLD
{
namespace fs = std::filesystem;
using std::string;
using std::vector;

// --- Build context: the compiler invocation, as data ------------------------

struct Ctx
{
  string cxx  = "clang++";
  string std_ = "-std=c++23";
  string warn = "-Wall -Wextra -Wimplicit-fallthrough -Werror -g";
  string includes;   // -I flags (filled by build.cpp)
  string ffi_cflags; // libffi cflags when THRAX_3RD_PARTY_ON
  string ffi_libs;   // libffi link flags
  string artifacts  = "artifacts";
  string invocation = "./build"; // how this run was launched (for banners)

  // -fPIC so the amalgam object can feed both the executables and the shared
  // libthrax.so. It lives in the shared flags (not just on the amalgam compile)
  // because the PCH is force-included into every TU and must be compiled with
  // matching codegen flags.
  string
  cxxflags() const
  {
    return std_ + " " + warn + " -fPIC " + includes
           + (ffi_cflags.empty() ? "" : " " + ffi_cflags);
  }
};

// --- Module descriptor: what a module contributes to the build --------------

// An executable = standalone TUs linked against the library amalgamation.
struct Exe
{
  string         name; // artifacts/<name>
  vector<string> srcs; // compiled as their own TUs (not part of the amalgam)
};

struct Module
{
  string         name;
  vector<string> amalgam_dirs; // .cpp/.hpp here join the amalgamation
  vector<Exe>    exes;         // standalone binaries
  std::function<void(const Ctx &)> generate; // optional pre-build generation
  vector<string> gen_deps;
};

// --- Process / filesystem helpers -------------------------------------------

[[noreturn]] inline void
die(
  const string &msg)
{
  std::print(stderr, "build: error: {}\n", msg);
  std::exit(1);
}

// Split a whitespace-separated command into argv tokens. There is no shell and
// no quote handling: our build inputs are space-free (nix store paths and the
// compiler flags), so this is exact for our use.
inline vector<string>
split_ws(
  const string &s)
{
  vector<string> out;
  string         cur;
  for (char ch : s)
  {
    if (ch == ' ' || ch == '\t' || ch == '\n')
    {
      if (!cur.empty())
      {
        out.push_back(cur);
        cur.clear();
      }
    }
    else
      cur += ch;
  }
  if (!cur.empty()) out.push_back(cur);
  return out;
}

// Spawn a program directly -- NO shell. argv[0] is the program (searched on
// PATH when it contains no slash). `out_file`/`err_file`, if given, redirect
// the child's stdout/stderr to those files; otherwise each stream is inherited
// (live on the terminal). Nothing is ever discarded. Returns the child's exit
// code, or -1 if it could not be spawned.
inline int
spawn(
  const vector<string> &argv,
  const string         &out_file = "",
  const string         &err_file = "")
{
#if defined(_WIN32)
  string cl; // Windows wants one command line, not an argv array.
  for (size_t i = 0; i < argv.size(); ++i)
    cl += (i ? " \"" : "\"") + argv[i] + "\"";
  SECURITY_ATTRIBUTES sa{};
  sa.nLength        = sizeof sa;
  sa.bInheritHandle = TRUE;
  HANDLE       fo = INVALID_HANDLE_VALUE, fe = INVALID_HANDLE_VALUE;
  STARTUPINFOA si{};
  si.cb         = sizeof si;
  si.dwFlags    = STARTF_USESTDHANDLES;
  si.hStdInput  = GetStdHandle(STD_INPUT_HANDLE);
  si.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
  si.hStdError  = GetStdHandle(STD_ERROR_HANDLE);
  if (!out_file.empty())
  {
    fo            = CreateFileA(out_file.c_str(),
                     GENERIC_WRITE,
                     0,
                     &sa,
                     CREATE_ALWAYS,
                     FILE_ATTRIBUTE_NORMAL,
                     nullptr);
    si.hStdOutput = fo;
  }
  if (!err_file.empty())
  {
    fe           = CreateFileA(err_file.c_str(),
                     GENERIC_WRITE,
                     0,
                     &sa,
                     CREATE_ALWAYS,
                     FILE_ATTRIBUTE_NORMAL,
                     nullptr);
    si.hStdError = fe;
  }
  vector<char> cmd(cl.begin(), cl.end());
  cmd.push_back('\0');
  PROCESS_INFORMATION pi{};
  DWORD               code = (DWORD)-1;
  if (CreateProcessA(nullptr,
                     cmd.data(),
                     nullptr,
                     nullptr,
                     TRUE,
                     0,
                     nullptr,
                     nullptr,
                     &si,
                     &pi))
  {
    WaitForSingleObject(pi.hProcess, INFINITE);
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
  }
  if (fo != INVALID_HANDLE_VALUE) CloseHandle(fo);
  if (fe != INVALID_HANDLE_VALUE) CloseHandle(fe);
  return (int)code;
#else
  vector<char *> a;
  a.reserve(argv.size() + 1);
  for (const string &s : argv) a.push_back(const_cast<char *>(s.c_str()));
  a.push_back(nullptr);
  posix_spawn_file_actions_t fa;
  posix_spawn_file_actions_init(&fa);
  if (!out_file.empty())
    posix_spawn_file_actions_addopen(
      &fa, 1, out_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (!err_file.empty())
    posix_spawn_file_actions_addopen(
      &fa, 2, err_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
  pid_t pid;
  int   rc = posix_spawnp(&pid, a[0], &fa, nullptr, a.data(), environ);
  posix_spawn_file_actions_destroy(&fa);
  if (rc != 0) return -1;
  int st;
  waitpid(pid, &st, 0);
  return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
#endif
}

// Echo and run a whitespace-split command (no shell), inheriting both streams
// live. For redirection use spawn(); to capture output use exec().
inline int
run(
  const string &cmd)
{
  std::print("{}\n", cmd);
  std::fflush(stdout); // keep the echo ahead of the child's inherited output
  return spawn(split_ws(cmd));
}

inline void
run_or_die(
  const string &cmd)
{
  if (run(cmd) != 0) die("command failed: " + cmd);
}

inline string
read_file(
  const string &path)
{
  string data;
  FILE  *f = std::fopen(path.c_str(), "rb");
  if (!f) return data;
  char   buf[4096];
  size_t n;
  while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) data.append(buf, n);
  std::fclose(f);
  return data;
}

inline void
write_file(
  const string &path, const string &data)
{
  FILE *f = std::fopen(path.c_str(), "wb");
  if (!f) die("cannot write " + path);
  std::fwrite(data.data(), 1, data.size(), f);
  std::fclose(f);
}

// Split text into lines (newline stripped), like reading with getline.
inline vector<string>
split_lines(
  const string &s)
{
  vector<string> lines;
  size_t         start = 0;
  while (start <= s.size())
  {
    size_t nl = s.find('\n', start);
    if (nl == string::npos)
    {
      if (start < s.size()) lines.push_back(s.substr(start));
      break;
    }
    lines.push_back(s.substr(start, nl - start));
    start = nl + 1;
  }
  return lines;
}

// The full result of running a program: exit code plus BOTH captured streams.
// Nothing is discarded -- the caller decides what to show.
struct Output
{
  int    code;
  string out;
  string err;
};

// Run a program (no shell) and capture stdout AND stderr (via temp files, so
// large output never deadlocks a pipe). Returns {code, out, err}.
inline Output
exec(
  const vector<string> &argv)
{
  static unsigned long seq = 0;
  fs::path             dir = fs::temp_directory_path();
  const string of = (dir / ("thrax-build-o" + std::to_string(seq))).string();
  const string ef = (dir / ("thrax-build-e" + std::to_string(seq++))).string();
  Output       r;
  r.code = spawn(argv, of, ef);
  r.out  = read_file(of);
  r.err  = read_file(ef);
  std::error_code ec;
  fs::remove(of, ec);
  fs::remove(ef, ec);
  return r;
}

inline string
trim(
  string s)
{
  const char *ws = " \t\r\n";
  size_t      a  = s.find_first_not_of(ws);
  if (a == string::npos) return "";
  return s.substr(a, s.find_last_not_of(ws) - a + 1);
}

inline bool
needs_rebuild(
  const string &out, const vector<string> &inputs)
{
  if (!fs::exists(out)) return true;
  auto ot = fs::last_write_time(out);
  for (const string &in : inputs)
    if (fs::exists(in) && fs::last_write_time(in) > ot) return true;
  return false;
}

// The nob-style self-update every build.cpp needs: if ./build is stale relative
// to any of `srcs`, recompile it with `compile_cmd`, then re-exec the fresh
// binary with the same args and exit with its code. `compile_cmd` and `srcs`
// are the only things that vary per project (include paths, source set); the
// mechanism -- timestamp check, portable re-exec (spawn, not execv) -- is
// shared. A no-op on the first run (fresh ./build, nothing newer), so it never
// loops.
[[maybe_unused]] inline void
rebuild_self(
  int argc, char **argv, const string &compile_cmd, const vector<string> &srcs)
{
  if (!fs::exists("./build") || !needs_rebuild("./build", srcs)) return;
  std::print("build: rebuilding self\n");
  if (run(compile_cmd) != 0) die("failed to rebuild self");
  vector<string> a = { "./build" };
  for (int i = 1; i < argc; ++i) a.push_back(argv[i]);
  std::fflush(stdout); // order our log before the child's inherited output
  std::exit(spawn(a));
}

// Files with any of `exts` across `dirs`, excluding build-system files (which
// live in module dirs but are not library sources), sorted by filename (stable,
// matches the historical amalgamation order).
inline vector<string>
glob(
  const vector<string> &dirs, const vector<string> &exts)
{
  vector<string> out;
  for (const string &d : dirs)
  {
    if (!fs::is_directory(d)) continue;
    for (const auto &e : fs::directory_iterator(d))
    {
      if (!e.is_regular_file()) continue;
      string name = e.path().filename().string();
      if (name == "build.cpp" || name == "UTxBUILD.hpp") continue;
      string ext = e.path().extension().string();
      for (const string &want : exts)
        if (ext == want)
        {
          out.push_back(e.path().string());
          break;
        }
    }
  }
  std::sort(out.begin(), out.end(), [](const string &a, const string &b) {
    return fs::path(a).filename() < fs::path(b).filename();
  });
  return out;
}

// --- Generated-file banner --------------------------------------------------

inline string
banner(
  const Ctx            &c,
  const string         &produced,
  const string         &by,
  const vector<string> &inputs)
{
  std::time_t t = std::time(nullptr);
  char        when[64];
  std::strftime(when, sizeof when, "%Y-%m-%d %H:%M:%S", std::localtime(&t));
  string s;
  s += "/* "
       "======================================================================="
       "===\n";
  s += " * GENERATED FILE -- DO NOT EDIT. Regenerating overwrites any "
       "changes.\n";
  s += " * produced   : " + produced + "\n";
  s += " * by         : build.cpp :: BLD::" + by + "\n";
  s += " * command    : " + c.invocation + "\n";
  s += " * when       : " + string(when) + "\n";
  s += " * inputs     : " + std::to_string(inputs.size()) + " file(s)\n";
  for (const string &i : inputs) s += " *              " + i + "\n";
  s += " * regenerate : ./build   (or `./build clean && ./build`)\n";
  s += " * "
       "======================================================================="
       "===*/\n";
  return s;
}

// --- Generators -------------------------------------------------------------

// Bake the C runtime into a header: wrap the runtime files (inter-runtime
// `#include "THx*.h"` stripped -- everything is inline; system includes kept)
// in a raw string CC::RUNTIME_C so emitted programs are self-contained.
inline void
gen_runtime_header(
  const Ctx &c, const string &out, const vector<string> &rt)
{
  if (!needs_rebuild(out, rt)) return;
  fs::create_directories(fs::path(out).parent_path());
  std::print("gen {}\n", out);
  string body = banner(c, out, "gen_runtime_header", rt);
  body += "#ifndef THxRTxAMALG_HEADER_\n#define THxRTxAMALG_HEADER_\n";
  body += "namespace CC\n{\n";
  body += "inline const char RUNTIME_C[] = R\"THXRTAMALG(\n";
  std::regex skip(R"(^\s*#\s*include\s*"THx)");
  for (const string &f : rt)
    for (const string &line : split_lines(read_file(f)))
      if (!std::regex_search(line, skip)) body += line + "\n";
  body += ")THXRTAMALG\";\n} // namespace CC\n#endif // THxRTxAMALG_HEADER_\n";
  write_file(out, body);
}

inline void
gen_stdlib_header(
  const Ctx &c, const string &out, const vector<string> &srcs)
{
  if (!needs_rebuild(out, srcs)) return;
  fs::create_directories(fs::path(out).parent_path());
  std::print("gen {}\n", out);
  string body = banner(c, out, "gen_stdlib_header", srcs);
  body += "#ifndef STDLIBxAMALG_HEADER_\n#define STDLIBxAMALG_HEADER_\n";
  body += "namespace DR\n{\n";
  body += "struct StdlibUnit\n{\n  const char *name;\n  const char *src;\n};\n";
  body += "inline const StdlibUnit STDLIB_UNITS[] = {\n";
  for (const string &f : srcs)
    body += "  { \"" + f + "\",\n    R\"THXSTDLIB(" + read_file(f)
            + ")THXSTDLIB\" },\n";
  body += "};\n} // namespace DR\n#endif // STDLIBxAMALG_HEADER_\n";
  write_file(out, body);
}

// Emit the project amalgamation: a header including every module header, and a
// TU including that header then every module .cpp. Includes are by bare
// filename (resolved via the per-module -I paths).
inline void
gen_amalgam(
  const Ctx            &c,
  const string         &cpp_out,
  const string         &hpp_out,
  const vector<string> &cpps,
  const vector<string> &hpps)
{
  fs::create_directories(fs::path(cpp_out).parent_path());

  if (needs_rebuild(hpp_out, hpps))
  {
    std::print("gen {}\n", hpp_out);
    string h = banner(c, hpp_out, "gen_amalgam", hpps);
    h += "#ifndef UTxAMALG_HEADER_\n#define UTxAMALG_HEADER_\n";
    for (const string &x : hpps)
      h += "#include \"" + fs::path(x).filename().string() + "\"\n";
    h += "#endif // UTxAMALG_HEADER_\n";
    write_file(hpp_out, h);
  }

  if (needs_rebuild(cpp_out, cpps))
  {
    std::print("gen {}\n", cpp_out);
    string s = banner(c, cpp_out, "gen_amalgam", cpps);
    s += "#include \"" + fs::path(hpp_out).filename().string() + "\"\n\n";
    for (const string &x : cpps)
      s += "#include \"" + fs::path(x).filename().string() + "\"\n";
    write_file(cpp_out, s);
  }
}

// --- Orchestration ----------------------------------------------------------

// Build the project. With `only` empty this is the full build: the compiler
// core (libthrax.a + libthrax.so) plus every module's executables. With `only`
// naming a module it builds just that module -- but the core library is shared,
// so consumer modules (those with executables: app, tests) still build the core
// first and then link only their own exes against it. A pure-generation module
// (platforms: no library code, no exes) is fully built by its generate step.
inline void
build(
  const Ctx &c, const vector<Module> &mods, const string &only = "")
{
  fs::create_directories(c.artifacts);

  // 0. Flag stamp: the compile flags that live in no source file -- notably the
  // FFI -D/-l toggle. Rewritten only when it changes, which bumps its mtime so
  // every TU that lists it as an input recompiles; an unchanged flag set leaves
  // it untouched and triggers nothing. This is what lets `build ffi` /
  // `build no-ffi` switch cleanly, with no manual clean.
  const string flag_stamp = c.artifacts + "/.flags";
  {
    const string want = c.cxxflags() + " || " + c.ffi_libs;
    if (read_file(flag_stamp) != want) write_file(flag_stamp, want);
  }

  // 1. Per-module generation (e.g. the baked runtime header).
  for (const Module &m : mods)
    if (m.generate) m.generate(c);

  // A pure-generation module (no library code, no executables -- platforms) is
  // done once its header is generated; there is nothing to compile or link.
  if (!only.empty())
    for (const Module &m : mods)
      if (m.name == only && m.amalgam_dirs.empty() && m.exes.empty()) return;

  // 2. Gather amalgam sources (module code minus the standalone entry TUs).
  vector<string> dirs;
  vector<string> entries;
  for (const Module &m : mods)
  {
    for (const string &d : m.amalgam_dirs) dirs.push_back(d);
    for (const Exe &e : m.exes)
      for (const string &s : e.srcs) entries.push_back(s);
  }
  vector<string> cpps;
  for (const string &s : glob(dirs, { ".cpp" }))
    if (std::find(entries.begin(), entries.end(), s) == entries.end())
      cpps.push_back(s);
  vector<string> hpps = glob(dirs, { ".hpp" });

  // 3. Generate the amalgamation into artifacts/.
  const string amalg_cpp = c.artifacts + "/UTxAMALG.cpp";
  const string amalg_hpp = c.artifacts + "/UTxAMALG.hpp";
  gen_amalgam(c, amalg_cpp, amalg_hpp, cpps, hpps);

  // 4. PCH: precompile the amalgam header, force-included into every TU.
  const string   pch       = c.artifacts + "/UTxAMALG.hpp.pch";
  const string   pch_flags = "-Xclang -include-pch -Xclang " + pch;
  vector<string> pch_ins   = hpps;
  pch_ins.push_back(amalg_hpp);
  pch_ins.push_back(flag_stamp);
  if (needs_rebuild(pch, pch_ins))
    run_or_die(c.cxx + " " + c.cxxflags() + " -x c++-header " + amalg_hpp
               + " -o " + pch);

  auto obj_of = [&](const string &src) {
    return c.artifacts + "/" + fs::path(src).stem().string() + ".o";
  };
  auto compile = [&](const string &src, const vector<string> &deps) {
    string         o   = obj_of(src);
    vector<string> ins = deps;
    ins.push_back(src);
    ins.push_back(pch);
    ins.push_back(flag_stamp);
    if (needs_rebuild(o, ins))
      run_or_die(c.cxx + " " + c.cxxflags() + " " + pch_flags + " -c " + src
                 + " -o " + o);
    return o;
  };

  // 5. Amalgamation object
  vector<string> amalg_deps = cpps;
  amalg_deps.insert(amalg_deps.end(), hpps.begin(), hpps.end());
  for (const Module &m : mods)
    amalg_deps.insert(amalg_deps.end(), m.gen_deps.begin(), m.gen_deps.end());
  string amalg_o = compile(amalg_cpp, amalg_deps);

  // 6. The compiler core as libraries: a static archive (what the executables
  // link, so they stay self-contained) and a shared object (emitted alongside
  // so the core can be consumed dynamically). Both derive from the single
  // amalgam object, so a module can build against libthrax without recompiling
  // the compiler.
  const string liba  = c.artifacts + "/libthrax.a";
  const string libso = c.artifacts + "/libthrax.so";
  if (needs_rebuild(liba, { amalg_o }))
    run_or_die("ar rcs " + liba + " " + amalg_o);
  if (needs_rebuild(libso, { amalg_o }))
  {
    string cmd = c.cxx + " -shared " + amalg_o + " -o " + libso;
    if (!c.ffi_libs.empty()) cmd += " " + c.ffi_libs;
    run_or_die(cmd);
  }

  // 7. Standalone entry TUs, each linked against the static core lib. When
  // `only` names a module, build just that module's executables.
  for (const Module &m : mods)
  {
    if (!only.empty() && m.name != only) continue;
    for (const Exe &e : m.exes)
    {
      vector<string> objs;
      for (const string &s : e.srcs)
        objs.push_back(
          compile(s, glob({ fs::path(s).parent_path().string() }, { ".hpp" })));
      const string   out = c.artifacts + "/" + e.name;
      vector<string> ins = objs;
      ins.push_back(liba);
      if (needs_rebuild(out, ins))
      {
        string cmd = c.cxx + " " + c.cxxflags();
        for (const string &o : objs) cmd += " " + o;
        cmd += " " + liba;
        if (!c.ffi_libs.empty()) cmd += " " + c.ffi_libs;
        run_or_die(cmd + " -o " + out);
      }
    }
  }
}

} // namespace BLD

#endif // UTxBUILD_HEADER_
