#include "TS.hpp"

namespace TS
{

// Collect every non-directory entry in `dir`, sorted for stable output. The
// sub-directory projects (foo_project, ...) have their own MAIN and are
// skipped.
std::vector<std::string>
scan_dir(
  const char *dir)
{
  std::vector<std::string> files;

  std::error_code                     ec;
  std::filesystem::directory_iterator it(dir, ec);
  if (ec)
  {
    printf(
      "ERROR: could not open directory %s: %s\n", dir, ec.message().c_str());
    return files;
  }

  for (const auto &entry : it)
  {
    if (entry.is_directory(ec)) continue;
    files.push_back(entry.path().string());
  }

  std::sort(files.begin(), files.end());
  return files;
}

// Compile every example plus the combined test driver into ONE program and run
// its MAIN entry (tests/MAIN.thx). The driver runs each example's `$ test`,
// prints the name of any module whose test fails, and the exit code is the
// number of failing modules -- so a single failure never masks the others (no
// `assert`/abort, every test runs). Returns that count (0 = all pass).
int
run_all()
{
  std::vector<std::string> files = scan_dir("./examples");
  files.push_back("./tests/MAIN.thx");

  std::vector<UT::Vu> vus;
  vus.reserve(files.size());
  for (const std::string &f : files)
    vus.push_back(UT::Vu{ f.c_str(), f.size() });

  int code = DR::run_program(vus);
  if (code == 0)
    printf("\033[1;32mOK\033[0m   all example tests passed\n");
  else
    printf("\033[1;31mFAIL\033[0m %d example module(s) failed\n", code);
  return code;
}

} // namespace TS
