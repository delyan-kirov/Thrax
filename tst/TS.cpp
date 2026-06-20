#include "TS.hpp"

namespace TS
{

void
tst_file(
  UT::Vu file)
{
  IT::StatEnv env = DR::interpret_file(file);

  // A failed pipeline (parse or type error) yields no definitions; the error
  // messages have already been printed by interpret_file.
  if (env.empty())
  {
    fprintf(stderr, "\033[1;31mFAIL\033[0m [%s]\n", file.data());
    return;
  }

  // Force every definition so a runtime fault surfaces here.
  for (auto &kv : env) IT::eval(kv.second, {}, env);

  printf("\033[1;32mOK\033[0m   [%s]\n", file.data());
}

// Collect every non-directory entry in `dir`, sorted for stable output.
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

// Scan ./dat and interpret every file in it, one by one.
void
run_all()
{
  for (const std::string &path : scan_dir("./dat"))
  {
    tst_file(UT::Vu{ path.c_str(), path.size() });
  }
}

} // namespace TS
