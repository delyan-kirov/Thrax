#include "LX.hpp"
#include "UT.hpp"
#include <cstdio>
#include <cstring>

constexpr UT::String sut_file = "./dat/debug.thx";

int
main()
{
  AR::Arena  arena{};
  UT::String source_code = UT::read_entire_file(sut_file, arena);
  LX::Lexer  l{ source_code, arena, 0, source_code.m_len };

  UT::SB sb{};
  for (LX::CharType ctype = l.next_word(sb); LX::CharType::INVALID != ctype;
       ctype              = l.next_word(sb))
  {
    switch (ctype)
    {
    case LX::CharType::CONTROL:
    {
      l.m_cursor += 1;
      if (sb.m_len > 0)
      {
        std::printf("WORD: %s\n", sb.vu().m_mem);
        std::memset(sb.m_mem, 0, sb.m_max_len);
        sb.m_len = 0;
      }
      goto END;
    }

    default: break;
    }
    std::printf("WORD: %s\n", sb.vu().m_mem);

  END:
    std::memset(sb.m_mem, 0, sb.m_max_len);
    sb.m_len = 0;
  }
}
