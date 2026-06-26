/*-------------------------------------------------------------------------------
 *\file main.cpp
 *\info thrax -- the command-line interpreter. Parses argv into source paths and
 *      hands them to the driver (DR). A path may be a `.thx` file or a
 * directory (every `.thx` directly inside it, non-recursive, skipping
 * `_`-prefixed names); with no path the current directory is used. All
 * resulting files form one program. With --ast / -a it prints the parsed AST
 * instead.
 *-----------------------------------------------------------------------------*/

#include "UTxAMALG.hpp"

namespace
{

const char *USAGE
  = "usage: thrax [--ast|-a] [--ir|-i] [PATH...]\n"
    "  PATH is a .thx file or a directory (every .thx in it, non-recursive,\n"
    "  skipping _*.thx). With no PATH the current directory is used. All "
    "files\n"
    "  form one program; with --ast print the parsed AST, with --ir the\n"
    "  closure-converted IR, instead of running.\n";

} // namespace

int
main(
  int argc, char **argv)
{
  bool                print_ast = false;
  bool                print_ir  = false;
  std::vector<UT::Vu> paths;

  for (int i = 1; i < argc; ++i)
  {
    const char *arg = argv[i];
    if (0 == std::strcmp(arg, "--ast") || 0 == std::strcmp(arg, "-a"))
      print_ast = true;
    else if (0 == std::strcmp(arg, "--ir") || 0 == std::strcmp(arg, "-i"))
      print_ir = true;
    else if (0 == std::strcmp(arg, "--help") || 0 == std::strcmp(arg, "-h"))
    {
      std::printf("%s", USAGE);
      return 0;
    }
    else if ('-' == arg[0])
    {
      std::fprintf(stderr, "thrax: unknown flag '%s'\n%s", arg, USAGE);
      return 2;
    }
    else
      paths.push_back(UT::Vu{ arg, std::strlen(arg) });
  }

  // No path given: compile the current directory.
  if (paths.empty()) paths.push_back(UT::Vu{ ".", 1 });

  // Expand directories to their `.thx` files. `names` owns the strings; `files`
  // are views into them, valid for the rest of main.
  std::vector<std::string> names = DR::expand_sources(paths);
  if (names.empty())
  {
    std::fprintf(stderr, "thrax: no .thx source files found\n%s", USAGE);
    return 2;
  }

  std::vector<UT::Vu> files;
  files.reserve(names.size());
  for (const std::string &n : names)
    files.push_back(UT::Vu{ n.data(), n.size() });

  if (print_ast)
  {
    bool ok = true;
    for (UT::Vu file : files) ok &= DR::dump_ast(file);
    return ok ? 0 : 1;
  }

  if (print_ir) return DR::dump_ir(files) ? 0 : 1;

  // All files form one program; modules link across them and the entry point is
  // the `main` of module MAIN. The program's exit code is main's Int result.
  return DR::run_program(files);
}
