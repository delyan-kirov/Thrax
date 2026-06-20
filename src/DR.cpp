#include "DR.hpp"
#include "ER.hpp"
#include "LL.hpp"
#include "TC.hpp"
#include "UT.hpp"

namespace DR
{

namespace
{
// TODO: Better print messages
// TODO: Better error handling
UT::Vu
read_entire_file(
  UT::Vu file_name, AR::Arena &arena)
{
  const char *file_str = file_name.data();
  size_t      file_len = 0;
  char       *buffer   = nullptr;
  size_t      result   = 0;

  FILE *file_stream = std::fopen(file_str, "rb");
  if (!file_stream)
  {
    std::fprintf(stderr, "ERROR: could not open file: %s\n", file_str);
    goto DEFER_RETURN;
  }

  std::fseek(file_stream, 0, SEEK_END);
  file_len = ftell(file_stream);

  std::rewind(file_stream);
  buffer           = (char *)arena.alloc(sizeof(char) * (file_len + 1));
  buffer[file_len] = 0;

  result = std::fread(buffer, 1, file_len, file_stream);
  if (result != file_len)
  {
    std::fprintf(
      stderr, "ERROR: could not map file %s to memory buffer\n", file_str);

    goto DEFER_RETURN;
  }

DEFER_RETURN:
  if (file_stream) std::fclose(file_stream);
  return UT::Vu{ buffer, file_len };
}
} // namespace

IT::StatEnv
interpret_file(
  UT::Vu file)
{
  AR::Arena arena{};

  IT::StatEnv env;
  UT::Vu      content = read_entire_file(file, arena);
  if (!content.data()) return env; // unreadable file; error already printed

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

  for (size_t i = 0; i < parser.m_exprs.size(); ++i)
  {
    IT::exprs2pLm(&parser.m_exprs[i], env);
  }

  return env;
}

bool
run_file(
  UT::Vu file)
{
  IT::StatEnv env = interpret_file(file);
  if (env.empty()) return false;

  // Force every definition so a runtime fault surfaces here.
  for (auto &kv : env) IT::eval(kv.second, {}, env);
  return true;
}

bool
dump_ast(
  UT::Vu file)
{
  AR::Arena arena{};

  UT::Vu content = read_entire_file(file, arena);
  if (!content.data()) return false; // unreadable file; error already printed

  LX::Lexer  lexer{ content, file, arena };
  EX::Parser parser{ lexer };
  parser();

  for (const ER::Diagnostic &d : parser.m_diags)
  {
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  }
  if (!parser.m_diags.empty()) return false;

  for (size_t i = 0; i < parser.m_exprs.size(); ++i)
  {
    std::printf("%s\n", EX::pprint(&parser.m_exprs[i]).c_str());
  }

  return true;
}

} // namespace DR
