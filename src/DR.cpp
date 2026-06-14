#include "DR.hpp"

#include <cstdio>

namespace DR
{

LX::E
lex_sym(
  LX::Lexer &lexer, LX::Token &out)
{
  return lexer.next(out);
}

LX::E
lex_file(
  UT::String  file,
  AR::Arena  &arena,
  LX::Tokens &out_tokens,
  UT::String &out_content)
{
  out_content = UT::read_entire_file(file, arena);
  LX::Lexer lexer{ out_content, file, arena, 0, out_content.m_len };

  for (;;)
  {
    LX::Token t{};
    LX::E     e = lex_sym(lexer, t);
    switch (e)
    {
    case LX::E::END_OF_FILE: return LX::E::OK;
    case LX::E::OK         : out_tokens.push(t); break;
    default:
      std::printf("%s\n", LX::pprint(*lexer.m_events, 0).c_str());
      return e;
    }
  }
}

EX::E
parse_sym(
  LX::Token &token, AR::Arena &arena, UT::String input, EX::Expr &out)
{
  LX::Tokens tokens{ arena };
  tokens.push(token);
  EX::Parser parser{ tokens, arena, input };
  EX::E      e = parser.run();
  if (e == EX::E::OK && !parser.m_exprs.is_empty())
  {
    out = *parser.m_exprs.last();
  }
  return e;
}

EX::E
parse_tokens(
  LX::Tokens        &tokens,
  AR::Arena         &arena,
  UT::String         input,
  UT::Vec<EX::Expr> &out)
{
  for (size_t i = 0; i < tokens.m_len; ++i)
  {
    EX::Expr expr{};
    EX::E    e = parse_sym(tokens[i], arena, input, expr);
    if (e != EX::E::OK)
    {
      return e;
    }
    out.push(expr);
  }
  return EX::E::OK;
}

IT::StatEnv
interpret_file(
  UT::String file)
{
  AR::Arena arena{};

  UT::String content = UT::read_entire_file(file, arena);
  LX::Lexer  lexer{ content, file, arena, 0, content.m_len };

  UT::Vec<EX::Expr> exprs{ arena };

  for (;;)
  {
    LX::Token t{};
    LX::E     le = lex_sym(lexer, t);

    if (le == LX::E::END_OF_FILE)
    {
      break;
    }
    if (le != LX::E::OK)
    {
      std::printf("%s\n", LX::pprint(*lexer.m_events, 0).c_str());
      return {};
    }

    EX::Expr expr{};
    EX::E    pe = parse_sym(t, arena, content, expr);
    if (pe != EX::E::OK)
    {
      std::printf("ERROR: parser failed with status %d\n", (int)pe);
      return {};
    }

    exprs.push(expr);
  }

  IT::StatEnv env;
  for (size_t i = 0; i < exprs.m_len; ++i)
  {
    IT::exprs2pLm(&exprs[i], env);
  }

  return env;
}

} // namespace DR
