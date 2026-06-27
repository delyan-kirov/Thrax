#ifndef LXxDATA_HEADER_
#define LXxDATA_HEADER_

#include "LX.hpp"
#include "OP.hpp"
#include "UT.hpp"

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

bool is_digit(char c);
bool is_hex_digit(char c);
bool is_bin_digit(char c);
bool is_ident_start(char c);
bool is_ident_cont(char c);

TokenTag keyword_tag(UT::Vu s);

using Keywords  = std::unordered_map<std::string, TokenTag>;
using Operators = std::unordered_map<std::string, TokenTag>;
using Delims    = std::unordered_map<char, TokenTag>;
using CharSet   = std::unordered_set<char>;

const Keywords keyword_db{
  { "let", TokenTag::KwLet },   //
  { "in", TokenTag::KwIn },     //
  { "if", TokenTag::KwIf },     //
  { "is", TokenTag::KwIs },     //
  { "then", TokenTag::KwThen }, //
  { "else", TokenTag::KwElse }, //
  { "ext", TokenTag::KwExt },   //
  { "with", TokenTag::KwWith }, //
  { "do", TokenTag::KwDo },     //
  { "ctl", TokenTag::KwCtl },   //
};

// The operator characters: ASCII punctuation minus the delimiters and the lead
// characters of other token kinds. A maximal run of these forms one operator.
const CharSet operator_char_db{
  '!', '$', '%', '&', '*', '+', '-', '/',  ':',
  '<', '=', '>', '?', '^', '|', '~', '\\',
};

// Operators are looked up by their full lexeme. The structural ones ('=', '=>',
// '->', '\\', ':', '$') carry their own tag; the evaluable arithmetic/logical
// operators all share TokenTag::Op, with the lexeme kept in Token::str. EX
// desugars each Op to a Var of that lexeme; see OP.hpp for the name vocabulary.
const Operators operator_db{
  { "\\", TokenTag::Lambda },   //
  { "=", TokenTag::Eq },        //
  { "=>", TokenTag::FatArrow }, //
  { "->", TokenTag::Arrow },    //
  { ":", TokenTag::Colon },     //
  { "$", TokenTag::Dollar },    //
  { OP::ADD, TokenTag::Op },    //
  { OP::SUB, TokenTag::Op },    //
  { OP::MUL, TokenTag::Op },    //
  { OP::DIV, TokenTag::Op },    //
  { OP::MOD, TokenTag::Op },    //
  { OP::BANG, TokenTag::Op },   //
  { OP::ISEQ, TokenTag::Op },   //
  { OP::GEQ, TokenTag::Op },    //
  { OP::LEQ, TokenTag::Op },    //
  { OP::MORE, TokenTag::Op },   //
  { OP::LESS, TokenTag::Op },   //
  // Effect-row delimiters (M3): `A -> <E1, E2 | `e> B`. Op-char runs coalesce,
  // so the opener may arrive as `<`, the empty row as `<>`, or a label-less open
  // row as `<|`; the closing `>` and the tail separator `|` stand alone. Outside
  // a type these would just be unbound variable names.
  { "<", TokenTag::Op },        //
  { ">", TokenTag::Op },        //
  { "|", TokenTag::Op },        //
  { "<>", TokenTag::Op },       //
  { "<|", TokenTag::Op },       //
};

// Single-character delimiters: brackets and the comma separator. Unlike
// operators these never coalesce with neighbouring punctuation.
const Delims delim_db{
  { '(', TokenTag::LParen }, { ')', TokenTag::RParen },
  { ',', TokenTag::Comma },  { '.', TokenTag::Dot },
  { '{', TokenTag::LBrace }, { '}', TokenTag::RBrace },
};

} // namespace LX

#endif // LXxDATA_HEADER_
