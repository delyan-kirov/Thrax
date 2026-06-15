/*-------------------------------------------------------------------------------
 *\file LX.hpp
 *\info Header file for the Lexer.
 *
 * The lexer is a forward, buffered iterator over the source: peek(n)/next()
 * hand out one flat token at a time, lexing on demand. It does no structural
 * work -- '(', ')', '\\', '=', keywords, etc. are all ordinary tokens, and the
 * parser (EX) recovers structure. Every token keeps its source view (`str`),
 * its 1-based line, a tag, and a variant payload (only Int/Str carry data).
 * Comments are lexed as tokens but skipped by peek/next.
 *-----------------------------------------------------------------------------*/

#ifndef LX_HEADER
#define LX_HEADER

#include "ER.hpp"
#include "UT.hpp"
#include <variant>
#include <vector>

namespace LX
{

/*------------------------------------------------------------------------------
 *\TOKEN PAYLOADS
 *-----------------------------------------------------------------------------*/

// Only Int and Str carry parsed data; every other kind is fully determined by
// its tag plus its source view (`Token::str`), so its payload is empty. The
// payload structs stay distinct (not one shared type) so the std::variant has
// no duplicate alternatives.
struct TkInt
{
  ssize_t value;
};
struct TkStr
{
  UT::String value; // unescaped contents (Token::str still holds the quotes)
};
struct TkWord
{
};
struct TkComment
{
};
struct TkPlus
{
};
struct TkMinus
{
};
struct TkMult
{
};
struct TkDiv
{
};
struct TkModulus
{
};
struct TkIsEq
{
};
struct TkNot
{
};
struct TkLParen
{
};
struct TkRParen
{
};
struct TkLambda
{
};
struct TkEq
{
};
struct TkFatArrow
{
};
struct TkKwLet
{
};
struct TkKwIn
{
};
struct TkKwIf
{
};
struct TkKwElse
{
};
struct TkKwInt
{
};
struct TkKwPub
{
};
struct TkKwExt
{
};
struct TkEof
{
};

/*------------------------------------------------------------------------------
 *\TOKEN TAG + VARIANT
 *-----------------------------------------------------------------------------*/

#define LX_TOKEN_VARIANTS                                                      \
  X(Int, TkInt)                                                                \
  X(Str, TkStr)                                                                \
  X(Word, TkWord)                                                              \
  X(Comment, TkComment)                                                        \
  X(Plus, TkPlus)                                                              \
  X(Minus, TkMinus)                                                            \
  X(Mult, TkMult)                                                              \
  X(Div, TkDiv)                                                                \
  X(Modulus, TkModulus)                                                        \
  X(IsEq, TkIsEq)                                                              \
  X(Not, TkNot)                                                                \
  X(LParen, TkLParen)                                                          \
  X(RParen, TkRParen)                                                          \
  X(Lambda, TkLambda)                                                          \
  X(Eq, TkEq)                                                                  \
  X(FatArrow, TkFatArrow)                                                      \
  X(KwLet, TkKwLet)                                                            \
  X(KwIn, TkKwIn)                                                              \
  X(KwIf, TkKwIf)                                                              \
  X(KwElse, TkKwElse)                                                          \
  X(KwInt, TkKwInt)                                                            \
  X(KwPub, TkKwPub)                                                            \
  X(KwExt, TkKwExt)                                                            \
  X(Eof, TkEof)

enum class TokenTag
{
#define X(tag, type) tag,
  LX_TOKEN_VARIANTS
#undef X
};

using TokenData =
#define X(tag, type) type,
  std::variant<LX_TOKEN_VARIANTS std::monostate>
#undef X
  ;

struct Token
{
  TokenTag   tag;
  UT::String str;  // lexeme view into the source -> pointer, offset and length
  size_t     line; // 1-based source line
  TokenData  as;
};

/*------------------------------------------------------------------------------
 *\LEXER
 *-----------------------------------------------------------------------------*/

class Lexer
{
public:
  AR::Arena         &m_arena;
  UT::String         m_input;
  UT::String         m_filename;
  size_t             m_cursor; // byte offset of the next char to lex
  size_t             m_line;   // current 1-based line
  std::vector<Token> m_buffer; // all tokens lexed so far (incl. comments)
  size_t             m_pos;    // raw buffer index of the next token to hand out

  Lexer(UT::String input, UT::String filename, AR::Arena &arena);

  // Significant-token surface (Comment tokens are skipped transparently).
  ER::Result<Token> peek(size_t n = 0);
  ER::Result<Token> next();

  // Backtracking: snapshot and restore the read position.
  size_t mark() const { return m_pos; }
  void   reset(size_t m) { m_pos = m; }

private:
  ER::Result<Token> lex_one(); // lex exactly one token from m_cursor

  ER::Result<Token> lex_comment(size_t start, size_t line);
  ER::Result<Token> lex_string(size_t start, size_t line);
  ER::Result<Token> lex_number(size_t start, size_t line);
  ER::Result<Token> lex_word(size_t start, size_t line);
  ER::Result<Token> lex_symbol(size_t start, size_t line);

  void       skip_ws();
  char       cur() const;
  char       at(size_t i) const;
  UT::String slice(size_t start) const;
  Token      mk(TokenTag tag, size_t start, size_t line) const;
};

/*------------------------------------------------------------------------------
 *\UTILS
 *-----------------------------------------------------------------------------*/

std::string pprint(TokenTag t);
std::string pprint(const Token &t);

} // namespace LX

#endif // LX_HEADER
