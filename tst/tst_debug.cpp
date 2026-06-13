#include "EX.hpp"
#include "IT.hpp"
#include "LX.hpp"
#include "UT.hpp"
#include <cstdio>

constexpr UT::String sut_file = "./dat/debug.thx";

IT::StatEnv
interpret_file(
  UT::String file)
{
  AR::Arena  arena{};
  UT::String f = UT::read_entire_file(file, arena);
  LX::Lexer  l{ f, arena, 0, f.m_len };
  LX::Tokens tokens{ arena };

  for (;;)
  {
    LX::Token t{};
    LX::E     e = l.next_global_sym(t);
    switch (e)
    {
    case LX::E::END_OF_FILE: goto DONE_LEX;
    case LX::E::OK: tokens.push(t); break;
    default:
      printf("ERROR: %s\n", LX::pprint(e).c_str());
      return {};
    }
  }
DONE_LEX:
  EX::Parser parser{ tokens, arena, f };
  EX::E      e = parser.run();
  if ((int)e != (int)EX::E::OK)
  {
    printf("ERROR: parser failed with status %d\n", (int)e);
    return {};
  }

  IT::StatEnv env;
  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    IT::exprs2pLm(&parser.m_exprs[i], env);
  }

  return env;
}

int
main()
{
  IT::StatEnv env = interpret_file(sut_file);

  for (auto &[name, def] : env)
  {
    IT::pLm result = IT::eval(def, {}, env);
    printf("%s = %s\n", name.c_str(), IT::pprint(result).c_str());
  }
}
