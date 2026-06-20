#include "DR.hpp"
#include "ER.hpp"
#include "LL.hpp"
#include "TC.hpp"
#include "UT.hpp"

namespace DR
{

IT::StatEnv
interpret_file(
  UT::String file)
{
  AR::Arena arena{};

  IT::StatEnv env;
  UT::String  content = UT::read_entire_file(file, arena);
  if (!content.m_mem) return env; // unreadable file; error already printed

  LX::Lexer  lexer{ content, file, arena };
  EX::Parser parser{ lexer };
  parser();

  for (const ER::Diagnostic &d : parser.m_diags)
  {
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  }

  if (!parser.m_diags.empty()) return env; // do not type-check broken syntax

  // Lower pattern sugar (pattern lambdas / lets) into the plain core before
  // type checking; the rest of the pipeline never sees a Pattern node.
  std::vector<ER::Diagnostic> lower_diags = LL::lower(parser.m_exprs, arena);
  for (const ER::Diagnostic &d : lower_diags)
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  if (!lower_diags.empty()) return env;

  // Type check before interpreting; a type error stops the pipeline.
  std::vector<ER::Diagnostic> type_diags
    = TC::check(parser.m_exprs, arena, content);
  for (const ER::Diagnostic &d : type_diags)
  {
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  }
  if (!type_diags.empty()) return env;

  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    IT::exprs2pLm(&parser.m_exprs[i], env);
  }

  return env;
}

bool
run_file(
  UT::String file)
{
  IT::StatEnv env = interpret_file(file);
  if (env.empty()) return false;

  // Force every definition so a runtime fault surfaces here.
  for (auto &kv : env) IT::eval(kv.second, {}, env);
  return true;
}

bool
dump_ast(
  UT::String file)
{
  AR::Arena arena{};

  UT::String content = UT::read_entire_file(file, arena);
  if (!content.m_mem) return false; // unreadable file; error already printed

  LX::Lexer  lexer{ content, file, arena };
  EX::Parser parser{ lexer };
  parser();

  for (const ER::Diagnostic &d : parser.m_diags)
  {
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  }
  if (!parser.m_diags.empty()) return false;

  for (size_t i = 0; i < parser.m_exprs.m_len; ++i)
  {
    std::printf("%s\n", EX::pprint(&parser.m_exprs[i]).c_str());
  }

  return true;
}

} // namespace DR
