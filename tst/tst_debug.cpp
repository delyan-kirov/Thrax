#include "EX.hpp"
#include "IT.hpp"
#include "LX.hpp"
#include "UT.hpp"
#include <cstdio>

constexpr UT::String sut_file = "./dat/debug.thx";

int
main()
{
  AR::Arena  arena{};
  UT::String f = UT::read_entire_file(sut_file, arena);
  LX::Lexer  l{ f, arena, 0, f.m_len };
  LX::Tokens tokens{ arena };

  for (;;)
  {
    LX::Token t{};
    LX::E     e = l.next_global_sym(t);
    switch (e)
    {
    case LX::E::END_OF_FILE: goto END;
    case LX::E::OK:
      tokens.push(t);
      printf("INFO: %s\n", LX::pprint(t).c_str());
      break;
    default: printf("ERROR: %s\n", LX::pprint(e).c_str()); return (int)e;
    }
  }
END:
  EX::Parser parser{ tokens, arena, f };
  EX::E      e = parser.run();

  printf("\n--- Parsed expressions ---\n");
  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    printf("%s\n", EX::pprint(&parser.m_exprs[i]).c_str());
  }

  printf("\nParser status: %d\n", (int)e);

  IT::StatEnv env;
  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    IT::pLm lm = IT::exprs2pLm(&parser.m_exprs[i], env);
  }

  printf("\n--- Lambda IR ---\n");
  for (auto &[name, def] : env)
  {
    printf("(%s %s)\n", name.c_str(), IT::pprint(def).c_str());
  }
}
