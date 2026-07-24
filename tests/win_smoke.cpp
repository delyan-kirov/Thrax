/*-------------------------------------------------------------------------------
 *\file win_smoke.cpp
 *\info Windows runtime smoke for the cross-platform IO module
 *-----------------------------------------------------------------------------*/

#include "UTxIO.hpp"

#include <cstdio>
#include <filesystem>

static int
fail(
  const char *what)
{
  std::printf("win-smoke: FAIL -- %s\n", what);
  return 1;
}

int
main()
{
  // Environment round-trip: set (overwriting), read back, overwrite again.
  if (IO::get_env("THRAX_WIN_SMOKE")) return fail("var unexpectedly preset");
  if (!IO::set_env("THRAX_WIN_SMOKE", "1"))
    return fail("set_env returned false");
  if (IO::get_env("THRAX_WIN_SMOKE").value_or("") != "1")
    return fail("set/get round-trip");
  if (!IO::set_env("THRAX_WIN_SMOKE", "22")) return fail("overwrite set_env");
  if (IO::get_env("THRAX_WIN_SMOKE").value_or("") != "22")
    return fail("overwrite value");

  // File round-trip through the same abstraction.
  const std::string path
    = (std::filesystem::temp_directory_path() / "thrax_win_smoke.txt").string();
  const std::string payload = "hello from windows\n";
  if (!IO::write_to_file(path, payload)) return fail("write_to_file");
  if (IO::read_entire_file(path).value_or("") != payload)
    return fail("read_entire_file round-trip");
  std::filesystem::remove(path);

  std::printf("win-smoke: OK\n");
  return 0;
}
