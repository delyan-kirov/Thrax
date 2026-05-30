#include "TL.hpp"
#include "UT.hpp"

constexpr UT::String sut_file = "./dat/debug.thx";

int
main()
{
  AR::Arena arena{};
  TL::Mod   mod{ sut_file, arena };
}
