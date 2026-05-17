#include "LX.hpp"
#include "UT.hpp"

constexpr UT::String sut_file = "./dat/debug.thx";

int
main()
{
  AR::Arena  arena{};
  UT::String source_code = UT::read_entire_file(sut_file, arena);
  LX::Lexer  l{ source_code, arena, 0, source_code.m_len };
  l.tokenize();
}
