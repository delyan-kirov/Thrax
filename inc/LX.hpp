/*-------------------------------------------------------------------------------
 *\file LX.hpp
 *\info Header file for Lexer
 * *----------------------------------------------------------------------------*/

#ifndef LX_HEADER
#define LX_HEADER

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "ER.hpp"
#include "UT.hpp"
#include <variant>

namespace LX
{

/*------------------------------------------------------------------------------
 *\CONSTANTS
 *-----------------------------------------------------------------------------*/

namespace Keyword
{

constexpr UT::String LET{ "let" };
constexpr UT::String IN{ "in" };
constexpr UT::String IF{ "if" };
constexpr UT::String ELSE{ "else" };
constexpr UT::String INT{ "int" };
constexpr UT::String PUB{ "pub" };
constexpr UT::String WHILE{ "while" };
constexpr UT::String EXT{ "ext" };

} // namespace Keyword

/*------------------------------------------------------------------------------
 *\TYPES
 *-----------------------------------------------------------------------------*/

// TODO: Need to have actual error codes here
#define LX_E_ENUM_VARIANTS                                                     \
  X(OK)                                                                        \
  X(END_OF_FILE)                                                               \
  X(ASCII_CTR_CHAR)                                                            \
  X(NON_ASCII_CHAR)                                                            \
  X(QUOTM_UNCLOSED)                                                            \
  X(PARENTHESIS_UNBALANCED)                                                    \
  X(NUMBER_PARSING_FAILURE)                                                    \
  X(FAT_ARROW)                                                                 \
  X(ELSE_KEYWORD)                                                              \
  X(IN_KEYWORD)                                                                \
  X(MATCHED_OPERATOR)                                                          \
  X(PAREN_LEFT)                                                                \
  X(MATCHED_QUOTM)                                                             \
  X(MATCHES_IFELSE)                                                            \
  X(MATCHES_LETIN)                                                             \
  X(MATCHES_OPEN_PAREN)                                                        \
  X(MATCHES_INTEGER)                                                           \
  X(MATCHES_STRING)                                                            \
  X(MATCHES_CONTROL_OPERATOR)                                                  \
  X(MATCHES_COLON)                                                             \
  X(MATCHES_LAMBDA)                                                            \
  X(GLOBAL_DEF_STRUCTURE_MALFORMED)                                            \
  X(EXPECT_EQUALS_AFTER_GLOBAL_SYM_DEF)                                        \
  X(UNEXPECTED_GLOBAL_DEF_SYM_MARKER)                                          \
  X(ILLEGAL_USE_OF_RESERVED_CHAR)                                              \
  X(IF_CONDITION_SEPARATOR_MISSING)                                            \
  X(IF_EXPR_MISSING_ELSE_BRANCH)                                               \
  X(IF_EXPR_ELSE_BRANCH_EMPTY)                                                 \
  X(IF_EXPR_MALFORMED_ELSE_BRANCH)                                             \
  X(LET_EXPR_VAR_NAME_MISSING)                                                 \
  X(LET_EXPR_EMPTY_VAR_NAME)                                                   \
  X(LET_EXPR_VAR_DEF_EMPTY)                                                    \
  X(LET_EXPR_EQ_SYMB_AFTER_VAR_MISSING)                                        \
  X(LET_EXPR_EXPECTED_DEF_AFTER_EQ)                                            \
  X(LET_EXPR_MISSING_IN)                                                       \
  X(LET_EXPR_ERRONEOUS_IN_EXPR)                                                \
  X(LAMBDA_NO_VAR_NAME)                                                        \
  X(LAMBDA_NOTHING_AFTER_VAR)                                                  \
  X(LAMBDA_EQ_EXPECTED_AFTER_VARNAME)                                          \
  X(LAMBDA_EXPECTED_DEF_AFTER_EQ)                                              \
  X(MATCHES_NOTHING)                                                           \
  X(NO_TOKENS)

enum class E
{
#define X(LX_ENUM_VALUE) LX_ENUM_VALUE,
  LX_E_ENUM_VARIANTS
#undef X
};

struct Token;
using Tokens = UT::Vec<Token>;

/*------------------------------------------------------------------------------
 *\TOKEN VARIANT TYPES
 *-----------------------------------------------------------------------------*/

struct TkMin
{
};
struct TkInt
{
  ssize_t value;
};
struct TkPlus
{
};
struct TkMinus
{
};
struct TkDiv
{
};
struct TkModulus
{
};
struct TkMult
{
};
struct TkIsEq
{
};
struct TkGroup
{
  Tokens tokens;
};
struct TkNot
{
};
struct TkStr
{
  UT::String value;
};
struct TkMax
{
};

struct TkLet
{
  UT::String var;
  Tokens     equals;
  Tokens     in;
};

struct TkFn
{
  UT::String param_name;
  Tokens     body;
};

struct TkWord
{
  UT::String value;
};

struct TkIf
{
  Tokens condition;
  Tokens true_branch;
  Tokens else_branch;
};

struct TkIntDef
{
  Tokens     def;
  UT::String name;
};

struct TkPubDef
{
  Tokens     def;
  UT::String name;
};

/*------------------------------------------------------------------------------
 *\TOKEN TAG + VARIANT
 *-----------------------------------------------------------------------------*/

#define LX_TOKEN_VARIANTS                                                      \
  X(Min, TkMin)                                                                \
  X(Int, TkInt)                                                                \
  X(Plus, TkPlus)                                                              \
  X(Minus, TkMinus)                                                            \
  X(Div, TkDiv)                                                                \
  X(Modulus, TkModulus)                                                        \
  X(Mult, TkMult)                                                              \
  X(IsEq, TkIsEq)                                                              \
  X(Group, TkGroup)                                                            \
  X(Let, TkLet)                                                                \
  X(Fn, TkFn)                                                                  \
  X(Word, TkWord)                                                              \
  X(If, TkIf)                                                                  \
  X(IntDef, TkIntDef)                                                          \
  X(PubDef, TkPubDef)                                                          \
  X(Not, TkNot)                                                                \
  X(Str, TkStr)                                                                \
  X(Max, TkMax)

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
  UT::String word;
  TokenData  as;
};

/*-------------------------------------------------------------------------------
 *\CLASSES
 *------------------------------------------------------------------------------*/

class Lexer
{
public:
  AR::Arena       &m_arena;
  ER::Events      *m_events;
  const UT::String m_input;
  const UT::String m_filename;
  Tokens           m_tokens;
  size_t           m_cursor;
  size_t           m_begin;
  size_t           m_end;

  Lexer(const UT::String input,
        const UT::String filename,
        AR::Arena       &arena,
        size_t           begin,
        size_t           end);

  Lexer(Lexer const &l, size_t begin, size_t end);

  ~Lexer() {}

  char next_char();

  char peek_char();

  UT_NODISCARD E next_valid_char(char & /*out*/ c);

  UT_NODISCARD E next_word(UT::String &sb);

  UT_NODISCARD E next(Token &t);

  UT_NODISCARD E matches_quotm(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_operator(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_ifelse(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_letin(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_open_paren(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_integer(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_string(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_control_operator(UT::Vu<UT::String> &words);

  UT_NODISCARD E matches_lambda(UT::Vu<UT::String> &words);

  UT_NODISCARD E next_sym(Token &t);

  UT_NODISCARD LX::E tokenize(UT::Vu<UT::String> &words);

  UT::String get_word(size_t idx);

  UT::String pop_word(UT::Vu<UT::String> &words);

  void subsume(const Lexer &l);

  void strip_white_space(size_t idx);

  void strip_line(size_t idx);
};

/*-------------------------------------------------------------------------------
 *\ UTILS
 *------------------------------------------------------------------------------*/

std::string pprint(E e, size_t level = 0);
std::string pprint(TokenTag t, size_t level = 0);
std::string pprint(Token t, size_t level = 0);
std::string pprint(Tokens ts, size_t level = 0);
std::string pprint(ER::Events events, size_t level = 0);

} // namespace LX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // LX_HEADER
