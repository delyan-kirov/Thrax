/*-------------------------------------------------------------------------------
 *\file LX.cpp
 *\info Streaming lexer implementation.
 * *----------------------------------------------------------------------------*/

#include "LX.hpp"
#include "ER.hpp"
#include "LXxDATA.hpp"
#include "UT.hpp"

namespace LX
{
/*------------------------------------------------------------------------------
 *\LOW-LEVEL CURSOR
 *-----------------------------------------------------------------------------*/

char
Lexer::cur() const
{
  return m_cursor < m_input.size() ? m_input.data()[m_cursor] : '\0';
}

char
Lexer::at(
  size_t i) const
{
  return i < m_input.size() ? m_input.data()[i] : '\0';
}

void
Lexer::scan(
  bool (*member)(char))
{
  while (member(cur())) m_cursor += 1;
}

UT::Vu
Lexer::slice(
  size_t start) const
{
  return UT::Vu{ m_input.data() + start, m_cursor - start };
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

RToken
Lexer::lex_comment(
  size_t start, size_t line)
{
  while ('\n' != cur() && '\0' != cur()) m_cursor += 1;
  return { true, mk(TokenTag::Comment, start, line), {} };
}

RToken
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
      t.as    = TkStr{ UT::Vu{ m_input.data() + content_start,
                            content_end - content_start } };
      return { true, t, {} };
    }
    m_cursor += 1;
  }
}

// Parse the lexeme [start, cursor) as an integer in `base`, reading from `num`
// (which may skip a radix prefix), and emit an Int token.
RToken
Lexer::emit_int(
  size_t start, size_t line, const char *num, int base)
{
  errno       = 0;
  long long v = std::strtoll(num, nullptr, base);
  if (0 != errno)
  {
    Token anchor = mk(TokenTag::Int, start, line);
    LX_ERR(ER::Code::NUMBER_PARSING_FAILURE,
           anchor,
           "could not parse '%s' as an integer",
           std::string(slice(start)).c_str());
  }
  Token t = mk(TokenTag::Int, start, line);
  t.as    = TkInt{ (ssize_t)v };
  return { true, t, {} };
}

// Parse the lexeme [start, cursor) as a 64-bit Real and emit a Real token.
RToken
Lexer::emit_real(
  size_t start, size_t line)
{
  errno    = 0;
  double d = std::strtod(slice(start).data(), nullptr);
  if (0 != errno)
  {
    Token anchor = mk(TokenTag::Real, start, line);
    LX_ERR(ER::Code::NUMBER_PARSING_FAILURE,
           anchor,
           "could not parse '%s' as a Real",
           std::string(slice(start)).c_str());
  }
  Token t = mk(TokenTag::Real, start, line);
  t.as    = TkReal{ d };
  return { true, t, {} };
}

// A radix literal: a `0x` / `0b` prefix (already at the cursor) followed by at
// least one digit of `member`. `skip` is how many leading chars strtoll should
// ignore (0 for hex -- base 16 eats `0x`; 2 for binary).
RToken
Lexer::lex_radix(
  size_t start, size_t line, bool (*member)(char), int base, size_t skip)
{
  m_cursor += 2; // the '0x' / '0b' prefix
  size_t digits_at = m_cursor;
  scan(member);
  if (m_cursor == digits_at)
  {
    Token anchor = mk(TokenTag::Int, start, line);
    LX_ERR(ER::Code::NUMBER_PARSING_FAILURE,
           anchor,
           "expected digits after '%s'",
           std::string(slice(start)).c_str());
  }
  return emit_int(start, line, slice(start).data() + skip, base);
}

RToken
Lexer::lex_number(
  size_t start, size_t line)
{
  if ('0' == cur() && ('x' == at(m_cursor + 1) || 'X' == at(m_cursor + 1)))
    return lex_radix(start, line, is_hex_digit, 16, 0);
  if ('0' == cur() && ('b' == at(m_cursor + 1) || 'B' == at(m_cursor + 1)))
    return lex_radix(start, line, is_bin_digit, 2, 2);

  // Decimal: an integer, unless a '.' fraction or an exponent makes it a Real.
  bool is_real = false;
  scan(is_digit);
  if ('.' == cur())
  {
    is_real = true;
    m_cursor += 1;
    scan(is_digit);
  }
  if ('e' == cur() || 'E' == cur())
  {
    is_real = true;
    m_cursor += 1;
    if ('+' == cur() || '-' == cur()) m_cursor += 1;
    scan(is_digit);
  }

  return is_real ? emit_real(start, line)
                 : emit_int(start, line, slice(start).data(), 10);
}

RToken
Lexer::lex_word(
  size_t start, size_t line)
{
  while (is_ident_cont(cur())) m_cursor += 1;
  UT::Vu s = slice(start);
  return { true, mk(keyword_tag(s), start, line), {} };
}

RToken
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

RToken
Lexer::lex_at(
  size_t start, size_t line)
{
  m_cursor += 1; // leading '@'
  if (!is_ident_start(cur()))
  {
    Token anchor = mk(TokenTag::At, start, line);
    LX_ERR(ER::Code::UNKNOWN_SYMBOL,
           anchor,
           "expected an intrinsic name after '@' (e.g. @extern)");
  }
  while (is_ident_cont(cur())) m_cursor += 1;
  return { true, mk(TokenTag::At, start, line), {} };
}

RToken
Lexer::lex_symbol(
  size_t start, size_t line)
{
  char c = cur();

  // Delimiters stand alone -- they never join a neighbouring operator run.
  if (const TokenTag *found = UT::try_lookup(delim_db, c))
  {
    m_cursor += 1;
    return { true, mk(*found, start, line), {} };
  }

  // Otherwise consume a maximal run of operator characters and look the whole
  // lexeme up in the operator table.
  if (operator_char_db.count(c))
  {
    while (operator_char_db.count(cur())) m_cursor += 1;
    UT::Vu      s   = slice(start);
    std::string key = std::string(s);
    if (const TokenTag *found = UT::try_lookup(operator_db, key))
    {
      return { true, mk(*found, start, line), {} };
    }
    Token anchor = mk(TokenTag::Eof, start, line);
    anchor.str   = s;
    LX_ERR(
      ER::Code::UNKNOWN_SYMBOL, anchor, "unknown operator '%s'", key.c_str());
  }

  Token anchor = mk(TokenTag::Eof, start, line);
  anchor.str   = UT::Vu{ m_input.data() + start, 1 };
  LX_ERR(ER::Code::UNKNOWN_SYMBOL, anchor, "unexpected character '%c'", c);
}

RToken
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
  if ('@' == c) return lex_at(start, line);
  if (is_ident_start(c)) return lex_word(start, line);
  return lex_symbol(start, line);
}

/*------------------------------------------------------------------------------
 *\ITERATOR SURFACE
 *-----------------------------------------------------------------------------*/

RToken
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
      RToken r = lex_one();
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

RToken
Lexer::next()
{
  for (;;)
  {
    if (m_pos >= m_buffer.size())
    {
      if (!m_buffer.empty() && TokenTag::Eof == m_buffer.back().tag)
        return { true, m_buffer.back(), {} };
      RToken r = lex_one();
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
  UT::Vu input, UT::Vu filename, AR::Arena &arena)
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
