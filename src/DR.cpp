#include "DR.hpp"
#include "ER.hpp"
#include "UT.hpp"

namespace DR
{

IT::StatEnv
interpret_file(
  UT::String file)
{
  AR::Arena arena{};

  UT::String content = UT::read_entire_file(file, arena);
  LX::Lexer  lexer{ content, file, arena };
  EX::Parser parser{ lexer };
  parser();

  for (const ER::Diagnostic &d : parser.m_diags)
  {
    std::printf("%s\n", ER::pprint(d, content, file).c_str());
  }

  IT::StatEnv env;
  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    IT::exprs2pLm(&parser.m_exprs[i], env);
  }

  return env;
}

} // namespace DR
