/*-------------------------------------------------------------------------------
 *\file web/playground.cpp
 *\info The browser/Node entry point: a string-in pipeline for the playground.
 *      Compiled ONLY by the Emscripten build (`build wasm`); the native CLI
 *      entry is app/main.cpp. `thrax_eval` takes one source string, runs the
 *      real pipeline on it, and lets all output flow to stdout/stderr -- which
 *      Emscripten delivers to the page's `Module.print` / `Module.printErr`,
 *      so no in-C++ capture is needed.
 *
 *      Reuse over a parallel string API: the source is written to a scratch
 *      file in Emscripten's in-memory filesystem (MEMFS) and handed to the
 *      existing file-based driver, so prelude/`C`/`TARGET`/stdlib injection
 *      and every backend behave exactly as on the CLI. The scratch file is
 *      named after the program's `@mod` header (the driver lints that the
 *      filename matches the module).
 *
 *      No Emscripten headers or platform macros here (the file stays portable
 *      and gate-clean); `thrax_eval` is exposed to JS by the linker's
 *      -sEXPORTED_FUNCTIONS, and called with `Module.ccall`.
 *-----------------------------------------------------------------------------*/

#include "UTxAMALG.hpp"

#include <cctype>
#include <cstdio>
#include <cstring>
#include <string>

namespace
{

// The program's module name from its `@mod NAME` header (the scratch file must
// be `NAME.thx`). Defaults to "MAIN" when absent -- the parser then reports the
// missing header, which is the right diagnostic to show.
std::string
module_name(
  const char *src)
{
  const char *p = std::strstr(src, "@mod");
  if (!p) return "MAIN";
  p += 4;
  while (*p == ' ' || *p == '\t') ++p;
  std::string name;
  while (*p && (std::isalnum((unsigned char)*p) || *p == '_')) name += *p++;
  return name.empty() ? "MAIN" : name;
}

// mode: 0 run (interpret), 1 emit C, 2 dump IR, 3 dump AST. The target is the
// host (wasm32) throughout, so what "Emit C"/"IR" shows is exactly what "Run"
// executes.
int
eval(
  const char *src, int mode)
{
  std::string path = module_name(src) + ".thx";
  {
    FILE *f = std::fopen(path.c_str(), "wb");
    if (!f)
    {
      std::fprintf(
        stderr, "playground: cannot create scratch file '%s'\n", path.c_str());
      return 2;
    }
    std::fwrite(src, 1, std::strlen(src), f);
    std::fclose(f);
  }

  std::vector<UT::Vu> files = { UT::Vu{ path.data(), path.size() } };
  switch (mode)
  {
  case 1 : return DR::emit_c(files) ? 0 : 1;
  case 2 : return DR::dump_ir(files) ? 0 : 1;
  case 3 : return DR::dump_ast(files.front()) ? 0 : 1;
  default: return DR::run_program(files);
  }
}

} // namespace

extern "C" int
thrax_eval(
  const char *src, int mode)
{
  return eval(src, mode);
}

// Emscripten expects an entry point; it returns immediately and (with the
// default EXIT_RUNTIME=0) the runtime stays alive so the page can `ccall`
// thrax_eval repeatedly.
int
main()
{
  return 0;
}
