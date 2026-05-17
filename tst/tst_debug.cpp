#include "LX.hpp"
#include "UT.hpp"
#include <cstdio>

constexpr UT::String sut_file = "./dat/debug.thx";

int
main()
{
  AR::Arena  arena{};
  UT::String source_code = UT::read_entire_file(sut_file, arena);
  LX::Lexer  l{ source_code, arena, 0, source_code.m_len };

  std::string sb{};
  for (LX::E e = l.next_word(sb); LX::E::OK == e; e = l.next_word(sb))
  {
    std::printf("INFO: %s\n", sb.c_str());
    sb.erase();
  }
}
