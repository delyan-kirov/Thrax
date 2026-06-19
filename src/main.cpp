/*-------------------------------------------------------------------------------
 *\file main.cpp
 *\info thrax -- the command-line interpreter. Parses argv and hands each source
 *      file to the driver (DR). With --ast / -a it prints the parsed AST instead
 *      of interpreting.
 *-----------------------------------------------------------------------------*/

#include "DR.hpp"
#include "UT.hpp"

#include <cstdio>
#include <cstring>
#include <vector>

namespace
{

const char *USAGE
  = "usage: thrax [--ast|-a] FILE...\n"
    "  interpret each FILE; with --ast print the parsed AST instead\n";

} // namespace

int
main(
  int argc, char **argv)
{
  bool                    print_ast = false;
  std::vector<UT::String> files;

  for (int i = 1; i < argc; ++i)
  {
    const char *arg = argv[i];
    if (0 == std::strcmp(arg, "--ast") || 0 == std::strcmp(arg, "-a"))
      print_ast = true;
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
      files.push_back(UT::String{ arg, std::strlen(arg) });
  }

  if (files.empty())
  {
    std::fprintf(stderr, "thrax: no input files\n%s", USAGE);
    return 2;
  }

  bool ok = true;
  for (UT::String file : files)
    ok &= print_ast ? DR::dump_ast(file) : DR::run_file(file);

  return ok ? 0 : 1;
}
