#include "TL.hpp"
#include "UT.hpp"

constexpr UT::String sut_file_basic  = "./dat/basic.thr";
constexpr UT::String sut_file_raylib = "./dat/raylib.thr";

constexpr bool RUN_RAYLIB =
#if GIT_ACTION_CTX
  false;
#else
  true;
#endif

int
main()
{

  {
    AR::Arena arena{};
    TL::Mod   mod_basic(sut_file_basic, arena);
  }

  if (RUN_RAYLIB)
  {
    AR::Arena arena{};
    TL::Mod   mod_raylib(sut_file_raylib, arena);
  }
}
