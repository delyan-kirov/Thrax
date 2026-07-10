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

#include <ctime>
#include <fstream>
#include <functional>
#include <iostream>
#include <regex>
#include <sys/wait.h>
#include <unistd.h>

namespace BLD
{
namespace fs = std::filesystem;
using std::string;
using std::vector;

// --- Build context: the compiler invocation, as data ------------------------

struct Ctx
{
  string cxx  = "clang++";
  string std_ = "-std=c++20";
  string warn = "-Wall -Wextra -Wimplicit-fallthrough -Werror -g";
  string includes;   // -I flags (filled by build.cpp)
  string ffi_cflags; // libffi cflags when THRAX_3RD_PARTY_ON
  string ffi_libs;   // libffi link flags
  string artifacts  = "artifacts";
  string invocation = "./build"; // how this run was launched (for banners)

  string
  cxxflags() const
  {
    return std_ + " " + warn + " " + includes
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
};

// --- Process / filesystem helpers -------------------------------------------

inline int
run(
  const string &cmd)
{
  std::cout << cmd << "\n";
  int rc = std::system(cmd.c_str());
  return WIFEXITED(rc) ? WEXITSTATUS(rc) : 1;
}

[[noreturn]] inline void
die(
  const string &msg)
{
  std::cerr << "build: error: " << msg << "\n";
  std::exit(1);
}

inline void
run_or_die(
  const string &cmd)
{
  if (run(cmd) != 0) die("command failed: " + cmd);
}

inline string
capture(
  const string &cmd)
{
  string out;
  FILE  *p = popen(cmd.c_str(), "r");
  if (!p) return out;
  char   buf[4096];
  size_t n;
  while ((n = fread(buf, 1, sizeof buf, p)) > 0) out.append(buf, n);
  pclose(p);
  return out;
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

// Files with any of `exts` across `dirs`, excluding per-module build.cpp,
// sorted by filename (stable, matches the historical amalgamation order).
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
      if (e.path().filename() == "build.cpp") continue;
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
  std::cout << "gen " << out << "\n";
  std::ofstream o(out);
  if (!o) die("cannot write " + out);
  o << banner(c, out, "gen_runtime_header", rt)
    << "#ifndef THxRTxAMALG_HEADER_\n#define THxRTxAMALG_HEADER_\n"
    << "namespace CC\n{\n"
    << "inline const char RUNTIME_C[] = R\"THXRTAMALG(\n";
  std::regex skip(R"(^\s*#\s*include\s*"THx)");
  for (const string &f : rt)
  {
    std::ifstream in(f);
    string        line;
    while (std::getline(in, line))
      if (!std::regex_search(line, skip)) o << line << "\n";
  }
  o << ")THXRTAMALG\";\n} // namespace CC\n#endif // THxRTxAMALG_HEADER_\n";
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
    std::cout << "gen " << hpp_out << "\n";
    std::ofstream o(hpp_out);
    o << banner(c, hpp_out, "gen_amalgam", hpps)
      << "#ifndef UTxAMALG_HEADER_\n#define UTxAMALG_HEADER_\n";
    for (const string &h : hpps)
      o << "#include \"" << fs::path(h).filename().string() << "\"\n";
    o << "#endif // UTxAMALG_HEADER_\n";
  }

  if (needs_rebuild(cpp_out, cpps))
  {
    std::cout << "gen " << cpp_out << "\n";
    std::ofstream o(cpp_out);
    o << banner(c, cpp_out, "gen_amalgam", cpps) << "#include \""
      << fs::path(hpp_out).filename().string() << "\"\n\n";
    for (const string &s : cpps)
      o << "#include \"" << fs::path(s).filename().string() << "\"\n";
  }
}

// --- Orchestration ----------------------------------------------------------

inline void
build(
  const Ctx &c, const vector<Module> &mods)
{
  fs::create_directories(c.artifacts);

  // 1. Per-module generation (e.g. the baked runtime header).
  for (const Module &m : mods)
    if (m.generate) m.generate(c);

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
    if (needs_rebuild(o, ins))
      run_or_die(c.cxx + " " + c.cxxflags() + " " + pch_flags + " -c " + src
                 + " -o " + o);
    return o;
  };

  // 5. The amalgamation object.
  vector<string> amalg_deps = cpps;
  amalg_deps.insert(amalg_deps.end(), hpps.begin(), hpps.end());
  string amalg_o = compile(amalg_cpp, amalg_deps);

  // 6. Standalone entry TUs + link each executable against the amalgam.
  for (const Module &m : mods)
    for (const Exe &e : m.exes)
    {
      vector<string> objs;
      for (const string &s : e.srcs)
        objs.push_back(
          compile(s, glob({ fs::path(s).parent_path().string() }, { ".hpp" })));
      objs.push_back(amalg_o);
      const string out = c.artifacts + "/" + e.name;
      if (needs_rebuild(out, objs))
      {
        string cmd = c.cxx + " " + c.cxxflags();
        for (const string &o : objs) cmd += " " + o;
        if (!c.ffi_libs.empty()) cmd += " " + c.ffi_libs;
        run_or_die(cmd + " -o " + out);
      }
    }

  // 7. compile_flags.txt for clangd.
  std::ofstream cf("compile_flags.txt");
  std::regex    ws("\\s+");
  string        flags = c.cxxflags();
  for (std::sregex_token_iterator it(flags.begin(), flags.end(), ws, -1), end;
       it != end;
       ++it)
    if (!string(*it).empty()) cf << string(*it) << "\n";
}

} // namespace BLD

#endif // UTxBUILD_HEADER_
