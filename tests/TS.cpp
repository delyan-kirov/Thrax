#include "TS.hpp"

namespace TS
{

void
tst_file(
  UT::Vu file)
{
  DR::Interp ip = DR::interpret_file(file);

  // A failed pipeline (parse or type error) yields no definitions; the error
  // messages have already been printed by interpret_file.
  if (ip.prog.globals.empty())
  {
    fprintf(stderr, "\033[1;31mFAIL\033[0m [%s]\n", file.data());
    return;
  }

  // Force every global through the reified-K machine so a runtime fault
  // surfaces here. `ip.arena` keeps the IR alive for the duration.
  IT::Machine m{ ip.prog };
  for (const auto &kv : ip.prog.globals) m.glob(UT::Vu{ kv.first });

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
  for (const std::string &path : scan_dir("./examples"))
  {
    tst_file(UT::Vu{ path.c_str(), path.size() });
  }
}

} // namespace TS
