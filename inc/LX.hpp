/*-------------------------------------------------------------------------------
 *\file LX.hpp
 *\info Header file for Lexer
 * *----------------------------------------------------------------------------*/

#ifndef LX_HEADER
#define LX_HEADER

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "UT.hpp"

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
  X(UNRECOGNIZED_STRING)                                                       \
  X(OPERATOR_MATCH_FAILURE)                                                    \
  X(UNREACHABLE_CASE_REACHED)                                                  \
  X(FAT_ARROW)                                                                 \
  X(ELSE_KEYWORD)                                                              \
  X(IN_KEYWORD)                                                                \
  X(CONTROL_STRUCTURE_ERROR)                                                   \
  X(WORD_NOT_FOUND)                                                            \
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
  X(MATCHES_LAMBDA)

enum class E
{
#define X(LX_ENUM_VALUE) LX_ENUM_VALUE,
  LX_E_ENUM_VARIANTS
#undef X
};

#define LX_Type_ENUM_VARIANTS                                                  \
  X(Min)                                                                       \
  X(Int)                                                                       \
  X(Plus)                                                                      \
  X(Minus)                                                                     \
  X(Div)                                                                       \
  X(Modulus)                                                                   \
  X(Mult)                                                                      \
  X(IsEq)                                                                      \
  X(Group)                                                                     \
  X(Let)                                                                       \
  X(Fn)                                                                        \
  X(Word)                                                                      \
  X(If)                                                                        \
  X(IntDef)                                                                    \
  X(PubDef)                                                                    \
  X(Not)                                                                       \
  X(Str)                                                                       \
  X(While)                                                                     \
  X(ExtDef)                                                                    \
  X(Max)

// TODO: Rename
enum class Type
{
#define X(LX_ENUM_VALUE) LX_ENUM_VALUE,
  LX_Type_ENUM_VARIANTS
#undef X
};

struct Token;
using Tokens = UT::Vec<Token>;

struct ErrorE : public ER::E
{
  ErrorE(AR::Arena  &arena,
         const char *fn_name,
         int         line,
         const char *data,
         LX::E       error);
};

struct If
// if expr => expr else expr
// [TODO] if expr is pattern => is ... else =>
{
  Tokens condition;
  Tokens true_branch;
  Tokens else_branch;
};

struct Binding
{
  UT::String var;
  Tokens     equals;
  Tokens     in;
};

struct Fn
{
  UT::String param_name;
  Tokens     body;
};

struct SymDef
{
  Tokens     def;
  UT::String name;
};

struct ExtSym
{
  UT::String name;
  Tokens     def;
};

struct While
{
  Tokens condition;
  Tokens body;
};

// TODO: Candidate for refactor
// TODO: Needs to have begin and end
struct Token
{
  Type   type;
  size_t cursor;
  union
  {
    Tokens     tokens;
    Binding    binding;
    If         if_else;
    Fn         fn;
    SymDef     sym;
    While      whyle;
    ExtSym     ext_sym;
    UT::String string;
    ssize_t    integer = 0;
  } as;

  Token()  = default;
  ~Token() = default;
  // TODO: the line and cursor should be set
  // TODO: Candidate for removal
  Token(Type t);
  // TODO: Candidate for removal
  Token(Type type, size_t cursor);
  // TODO: Candidate for removal
  Token(Tokens tokens);
};

/*-------------------------------------------------------------------------------
 *\CLASSES
 *------------------------------------------------------------------------------*/

class Lexer
{
public:
  AR::Arena       &m_arena;
  ER::Events       m_events;
  const UT::String m_input;
  Tokens           m_tokens;
  size_t           m_cursor;
  size_t           m_begin;
  size_t           m_end;

  Lexer(const UT::String input, AR::Arena &arena, size_t begin, size_t end);

  Lexer(Lexer const &l, size_t begin, size_t end);

  ~Lexer() {}

  void generate_event_report();

  char next_char();

  char peek_char();

  E next_valid_char(char & /*out*/ c);

  E next_word(UT::String &sb);

  E next_global_sym(Token &t);

  E matches_quotm(UT::Vu<UT::String> &words);

  E matches_operator(UT::Vu<UT::String> &words);

  E matches_ifelse(UT::Vu<UT::String> &words);

  E matches_letin(UT::Vu<UT::String> &words);

  E matches_open_paren(UT::Vu<UT::String> &words);

  E matches_integer(UT::Vu<UT::String> &words);

  E matches_string(UT::Vu<UT::String> &words);

  E matches_control_operator(UT::Vu<UT::String> &words);

  E matches_lambda(UT::Vu<UT::String> &words);

  E next_non_extern_sym(Token &t);

  UT::String get_word(size_t idx);

  void strip_white_space(size_t idx);

  void strip_line(size_t idx);

  LX::E tokenize(UT::Vu<UT::String> &words);
};

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string pprint(E e, int level = 0);
std::string pprint(Type t, int level = 0);
std::string pprint(Token t, int level = 0);
std::string pprint(Tokens ts, int level = 0);

} // namespace LX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // LX_HEADER
