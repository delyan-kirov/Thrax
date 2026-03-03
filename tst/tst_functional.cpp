#include "TL.hpp"
#include "UT.hpp"
#include <cstdio>

constexpr UT::String sut_file_basic  = "./dat/basic.thr";
constexpr UT::String sut_file_raylib = "./dat/raylib.thr";

constexpr bool RUN_BASIC =
#if GIT_ACTION_CTX
  true;
#else
  false;
#endif

constexpr bool RUN_RAYLIB =
#if GIT_ACTION_CTX
  false;
#else
  true;
#endif

int
main()
{
  AR::Arena arena{};

  if (RUN_BASIC)
  {
    TL::Mod mod_basic(sut_file_basic, arena);
  }
  else if (RUN_RAYLIB)
  {
    TL::Mod mod_raylib(sut_file_raylib, arena);
  }
  else
  {
    std::printf("%s -> OK [no target ran]\n", __FILE__);
  }
}
