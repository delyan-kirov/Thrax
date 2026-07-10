#include "LXxDATA.hpp"
#include "LX.hpp"
#include "UT.hpp"

namespace LX
{

/*------------------------------------------------------------------------------
 *\INTERNAL UTILS
 *-----------------------------------------------------------------------------*/

bool
is_digit(
  char c)
{
  return c >= '0' && c <= '9';
}

// Digits valid after a `0x` prefix.
bool
is_hex_digit(
  char c)
{
  return is_digit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

// Digits valid after a `0b` prefix.
bool
is_bin_digit(
  char c)
{
  return '0' == c || '1' == c;
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

TokenTag
keyword_tag(
  UT::Vu s)
{
  return UT::lookup_or(keyword_db, std::string(s), TokenTag::Word);
}

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
  return pprint(t.tag) + " '" + std::string(t.str) + "'";
}

} // namespace LX
