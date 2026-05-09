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

  UT::SB sb{};
  for (std::string w = l.next_word();; w = l.next_word())
  {
    std::printf("INFO: %s\n", w.c_str());
    if (w == LX::NEXT_WORD_ERROR_ASCII_CTR
        || w == LX::NEXT_WORD_ERROR_NON_ASCII_CHAR
        || w == LX::NEXT_WORD_INFO_END
        || w == LX::NEXT_WORD_ERROR_UNEXPECTED_END_OF_TEXT
        || w == LX::NEXT_WORD_ERROR_UNCLOSED_QUOTMARK
        || w == LX::NEXT_WORD_ERROR_QESTION_REQUIRES_EQ || w == "")
    {
      return 0;
    };
  }
}
