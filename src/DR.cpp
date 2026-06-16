#include "DR.hpp"
#include "ER.hpp"
#include "TC.hpp"
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
  if (!parser.m_diags.empty()) return env; // do not type-check broken syntax

  // Type check before interpreting; a type error stops the pipeline.
  std::vector<ER::Diagnostic> type_diags = TC::check(parser.m_exprs, arena, content);
  for (const ER::Diagnostic &d : type_diags)
  {
    std::printf("%s\n", ER::pprint(d, content, file).c_str());
  }
  if (!type_diags.empty()) return env;

  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    IT::exprs2pLm(&parser.m_exprs[i], env);
  }

  return env;
}

} // namespace DR
