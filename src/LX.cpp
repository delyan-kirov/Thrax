/*-------------------------------------------------------------------------------
 *\file LX.cpp
 *\info Streaming lexer implementation.
 * *----------------------------------------------------------------------------*/

#include "LX.hpp"
#include "ER.hpp"
#include "UT.hpp"

#include <cctype>
#include <cerrno>
#include <cstdlib>
#include <string>
#include <unordered_map>

/*------------------------------------------------------------------------------
 *\MACROS
 *-----------------------------------------------------------------------------*/

// Build a root diagnostic anchored at TOK and return it as a failed Result.
#define LX_ERR(CODE, TOK, ...)                                                 \
  return ER::Fail                                                              \
  {                                                                            \
    ER::mk_root(m_arena,                                                       \
                (CODE),                                                        \
                (TOK).str,                                                     \
                (TOK).line,                                                    \
                ER::mk_msg(m_arena, __VA_ARGS__))                              \
  }

namespace LX
{

/*------------------------------------------------------------------------------
 *\INTERNAL UTILS
 *-----------------------------------------------------------------------------*/

namespace
{

bool
is_digit(
  char c)
{
  return c >= '0' && c <= '9';
}

bool
is_ident_start(
  char c)
{
  return std::isalpha((unsigned char)c) || '_' == c;
}

bool
is_ident_cont(
  char c)
{
  return std::isalnum((unsigned char)c) || '_' == c;
}

using Keywords = std::unordered_map<std::string, TokenTag>;
using Symbols  = std::unordered_map<char, TokenTag>;

const Keywords keyword_db{
  { "let", TokenTag::KwLet }, { "in", TokenTag::KwIn },
  { "if", TokenTag::KwIf },   { "else", TokenTag::KwElse },
  { "ext", TokenTag::KwExt },
};

// Single-character symbols. Multi-character ones ('?=', '=>', '->') and the
// standalone '=' / '-' depend on the next character and are handled in
// lex_symbol.
const Symbols symbol_db{
  { '(', TokenTag::LParen },  { ')', TokenTag::RParen },
  { '\\', TokenTag::Lambda }, { '+', TokenTag::Plus },
  { '*', TokenTag::Mult },    { '/', TokenTag::Div },
  { '%', TokenTag::Modulus }, { '!', TokenTag::Not },
  { '$', TokenTag::Dollar },  { ':', TokenTag::Colon },
};

TokenTag
keyword_tag(
  UT::String s)
{
  return UT::lookup_or(keyword_db, std::to_string(s), TokenTag::Word);
}

} // namespace

/*------------------------------------------------------------------------------
 *\PPRINT
 *-----------------------------------------------------------------------------*/

std::string
pprint(
  TokenTag t)
{
  switch (t)
  {
#define X(tag, type)                                                           \
  case TokenTag::tag: return #tag;
    LX_TOKEN_VARIANTS
#undef X
  }
  return "UNKNOWN";
}

std::string
pprint(
  const Token &t)
{
  return pprint(t.tag) + " '" + std::to_string(t.str) + "'";
}

/*------------------------------------------------------------------------------
 *\LOW-LEVEL CURSOR
 *-----------------------------------------------------------------------------*/

char
Lexer::cur() const
{
  return m_cursor < m_input.m_len ? m_input.m_mem[m_cursor] : '\0';
}

char
Lexer::at(
  size_t i) const
{
  return i < m_input.m_len ? m_input.m_mem[i] : '\0';
}

UT::String
Lexer::slice(
  size_t start) const
{
  return UT::String{ m_input.m_mem + start, m_cursor - start };
}

Token
Lexer::mk(
  TokenTag tag, size_t start, size_t line) const
{
  Token t;
  t.tag  = tag;
  t.str  = slice(start);
  t.line = line;
  // `as` is left at its default alternative; only Int/Str are ever read via
  // std::get, and those set it explicitly.
  return t;
}

void
Lexer::skip_ws()
{
  for (;;)
  {
    char c = cur();
    if (' ' == c || '\t' == c || '\r' == c)
    {
      m_cursor += 1;
    }
    else if ('\n' == c)
    {
      m_cursor += 1;
      m_line += 1;
    }
    else
    {
      break;
    }
  }
}

/*------------------------------------------------------------------------------
 *\PER-KIND LEXERS
 *-----------------------------------------------------------------------------*/

R
Lexer::lex_comment(
  size_t start, size_t line)
{
  while ('\n' != cur() && '\0' != cur()) m_cursor += 1;
  return { true, mk(TokenTag::Comment, start, line), {} };
}

R
Lexer::lex_string(
  size_t start, size_t line)
{
  m_cursor += 1; // opening quote
  size_t content_start = m_cursor;

  for (;;)
  {
    char c = cur();
    if ('\0' == c || '\n' == c)
    {
      Token anchor = mk(TokenTag::Str, start, line);
      LX_ERR(ER::Code::QUOTM_UNCLOSED,
             anchor,
             "string literal is not closed with a '\"'");
    }
    if ('"' == c)
    {
      size_t content_end = m_cursor;
      m_cursor += 1; // closing quote
      Token t = mk(TokenTag::Str, start, line);
      t.as    = TkStr{ UT::String{ m_input.m_mem + content_start,
                                content_end - content_start } };
      return { true, t, {} };
    }
    m_cursor += 1;
  }
}

R
Lexer::lex_number(
  size_t start, size_t line)
{
  while (is_digit(cur())) m_cursor += 1;
  UT::String s = slice(start);

  errno       = 0;
  long long v = std::strtoll(s.m_mem, nullptr, 10);
  if (0 != errno)
  {
    Token anchor = mk(TokenTag::Int, start, line);
    LX_ERR(ER::Code::NUMBER_PARSING_FAILURE,
           anchor,
           "could not parse '%s' as an integer",
           std::to_string(s).c_str());
  }

  Token t = mk(TokenTag::Int, start, line);
  t.as    = TkInt{ (ssize_t)v };
  return { true, t, {} };
}

R
Lexer::lex_word(
  size_t start, size_t line)
{
  while (is_ident_cont(cur())) m_cursor += 1;
  UT::String s = slice(start);
  return { true, mk(keyword_tag(s), start, line), {} };
}

R
Lexer::lex_tyvar(
  size_t start, size_t line)
{
  m_cursor += 1; // leading backtick
  if (!is_ident_start(cur()))
  {
    Token anchor = mk(TokenTag::TyVar, start, line);
    LX_ERR(ER::Code::UNKNOWN_SYMBOL,
           anchor,
           "expected a type name after '`' (e.g. `T)");
  }
  while (is_ident_cont(cur())) m_cursor += 1;
  return { true, mk(TokenTag::TyVar, start, line), {} };
}

R
Lexer::lex_symbol(
  size_t start, size_t line)
{
  char     c   = cur();
  TokenTag tag = TokenTag::Eof;
  size_t   len = 1;

  // Multi-character operators (and the '=' that may begin one) first, since
  // they depend on the following character.
  if ('?' == c)
  {
    if ('=' != at(m_cursor + 1))
    {
      Token anchor = mk(TokenTag::Eof, start, line);
      anchor.str   = UT::String{ m_input.m_mem + start, 1 };
      LX_ERR(ER::Code::UNKNOWN_SYMBOL,
             anchor,
             "'?' is only valid as part of the '?=' operator");
    }
    tag = TokenTag::IsEq;
    len = 2;
  }
  else if ('=' == c)
  {
    if ('>' == at(m_cursor + 1))
    {
      tag = TokenTag::FatArrow;
      len = 2;
    }
    else
    {
      tag = TokenTag::Eq;
    }
  }
  else if ('-' == c)
  {
    // '-' is binary subtraction / unary negation; '->' is the type arrow.
    if ('>' == at(m_cursor + 1))
    {
      tag = TokenTag::Arrow;
      len = 2;
    }
    else
    {
      tag = TokenTag::Minus;
    }
  }
  else if (const TokenTag *found = UT::try_lookup(symbol_db, c))
  {
    tag = *found;
  }
  else
  {
    Token anchor = mk(TokenTag::Eof, start, line);
    anchor.str   = UT::String{ m_input.m_mem + start, 1 };
    LX_ERR(ER::Code::UNKNOWN_SYMBOL, anchor, "unexpected character '%c'", c);
  }

  m_cursor += len;
  return { true, mk(tag, start, line), {} };
}

R
Lexer::lex_one()
{
  skip_ws();
  size_t start = m_cursor;
  size_t line  = m_line;
  char   c     = cur();

  if ('\0' == c) return { true, mk(TokenTag::Eof, start, line), {} };
  if ('#' == c) return lex_comment(start, line);
  if ('"' == c) return lex_string(start, line);
  if (is_digit(c)) return lex_number(start, line);
  if ('`' == c) return lex_tyvar(start, line);
  if (is_ident_start(c)) return lex_word(start, line);
  return lex_symbol(start, line);
}

/*------------------------------------------------------------------------------
 *\ITERATOR SURFACE
 *-----------------------------------------------------------------------------*/

R
Lexer::peek(
  size_t n)
{
  size_t i    = m_pos;
  size_t seen = 0;
  for (;;)
  {
    if (i >= m_buffer.size())
    {
      if (!m_buffer.empty() && TokenTag::Eof == m_buffer.back().tag)
        return { true, m_buffer.back(), {} };
      R r = lex_one();
      if (!r.ok) return r;
      m_buffer.push_back(r.value);
    }
    if (TokenTag::Comment == m_buffer[i].tag)
    {
      i += 1;
      continue;
    }
    if (seen == n) return { true, m_buffer[i], {} };
    seen += 1;
    i += 1;
  }
}

R
Lexer::next()
{
  for (;;)
  {
    if (m_pos >= m_buffer.size())
    {
      if (!m_buffer.empty() && TokenTag::Eof == m_buffer.back().tag)
        return { true, m_buffer.back(), {} };
      R r = lex_one();
      if (!r.ok) return r;
      m_buffer.push_back(r.value);
    }
    Token t = m_buffer[m_pos];
    m_pos += 1;
    if (TokenTag::Comment == t.tag) continue;
    return { true, t, {} };
  }
}

/*------------------------------------------------------------------------------
 *\CTOR
 *-----------------------------------------------------------------------------*/

size_t
Lexer::mark() const
{
  return m_pos;
}
void
Lexer::reset(
  size_t m)
{
  m_pos = m;
}

Lexer::Lexer(
  UT::String input, UT::String filename, AR::Arena &arena)
    : m_arena{ arena },
      m_input{ input },
      m_filename{ filename },
      m_cursor{ 0 },
      m_line{ 1 },
      m_buffer{},
      m_pos{ 0 }
{
}

} // namespace LX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
