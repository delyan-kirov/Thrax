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

/*------------------------------------------------------------------------------
 *\MACROS
 *-----------------------------------------------------------------------------*/

#define LX_ERROR_REPORT(LX_ERROR_E, LX_ERROR_MSG)                              \
  do                                                                           \
  {                                                                            \
    this->m_events.push(LX::ErrorE{ this->m_arena,                             \
                                    __PRETTY_FUNCTION__,                       \
                                    __LINE__,                                  \
                                    (LX_ERROR_MSG),                            \
                                    (LX_ERROR_E) });                           \
    return (LX_ERROR_E);                                                       \
  } while (false)

#define LX_FN_TRY(LX_FN)                                                       \
  do                                                                           \
  {                                                                            \
    LX::E result = (LX_FN);                                                    \
    if (LX::E::OK != result)                                                   \
    {                                                                          \
      this->m_events.push(LX::ErrorE{ this->m_arena,                           \
                                      __PRETTY_FUNCTION__,                     \
                                      __LINE__,                                \
                                      ("The function: " #LX_FN " failed!"),    \
                                      result });                               \
      return result;                                                           \
    }                                                                          \
  } while (false)

#define LX_ASSERT(LX_BOOL_EXPR, LX_ERROR_E)                                    \
  do                                                                           \
  {                                                                            \
    if (!(LX_BOOL_EXPR))                                                       \
    {                                                                          \
      this->m_events.push(LX::ErrorE{ this->m_arena,                           \
                                      __PRETTY_FUNCTION__,                     \
                                      __LINE__,                                \
                                      (#LX_BOOL_EXPR),                         \
                                      (LX_ERROR_E) });                         \
      return (LX_ERROR_E);                                                     \
    }                                                                          \
  } while (false)

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

#define LX_LangType_ENUM_VARIANTS                                              \
  X(Fn)                                                                        \
  X(Nat)                                                                       \
  X(Nat8)                                                                      \
  X(Nat16)                                                                     \
  X(Nat32)                                                                     \
  X(Nat64)                                                                     \
  X(Int)                                                                       \
  X(Int8)                                                                      \
  X(Int16)                                                                     \
  X(Int32)                                                                     \
  X(Int64)                                                                     \
  X(Ptr)                                                                       \
  X(Void)

enum class LangType
{
#define X(LX_ENUM_VALUE) LX_ENUM_VALUE,
  LX_LangType_ENUM_VARIANTS
#undef X
};

#define LX_E_ENUM_VARIANTS                                                     \
  X(OK)                                                                        \
  X(PARENTHESIS_UNBALANCED)                                                    \
  X(NUMBER_PARSING_FAILURE)                                                    \
  X(UNRECOGNIZED_STRING)                                                       \
  X(OPERATOR_MATCH_FAILURE)                                                    \
  X(UNREACHABLE_CASE_REACHED)                                                  \
  X(FAT_ARROW)                                                                 \
  X(ELSE_KEYWORD)                                                              \
  X(IN_KEYWORD)                                                                \
  X(CONTROL_STRUCTURE_ERROR)                                                   \
  X(WORD_NOT_FOUND)

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
  UT::String name;
  Tokens     let;
  Tokens     in;
};

struct Fn
{
  UT::String param_name;
  Tokens     body;
};

struct SymDef
{
  UT::String name;
  Tokens     def;
};

// NOTE: T -> (T -> (T -> T))
//   is: T -> T -> T -> T
struct Sig
{
  LangType type;
  union
  {
    UT::Pair<Sig> pair;
  } as;
};

struct ExtSym
{
  UT::String name;
  Sig        sig;
  Tokens     def;
};

struct While
{
  Tokens condition;
  Tokens body;
};

struct Token
{
  Type   type;
  size_t line;
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
    Sig        sig;
    UT::String string;
    ssize_t    integer = 0;
  } as;

  Token()  = default;
  ~Token() = default;
  // TODO: the line and cursor should be set
  Token(Type t)
      : type{ t },
        line{ 0 },
        cursor{ 0 },
        as{} {};
  Token(
    Tokens tokens)
      : type{ Type::Group },
        line{ 0 },
        cursor{ 0 }
  {
    new (&as.tokens) Tokens{ tokens }; // NOTE: placement new
  };
};

/*-------------------------------------------------------------------------------
 *\CLASSES
 *------------------------------------------------------------------------------*/

class Lexer
{
  // TODO: use UT::String, not const char*
public:
  AR::Arena  &m_arena;
  ER::Events  m_events;
  const char *m_input;
  Tokens      m_tokens;
  size_t      m_lines;
  size_t      m_cursor;
  size_t      m_begin;
  size_t      m_end;

  Lexer(const char *const input, AR::Arena &arena, size_t begin, size_t end);

  Lexer(Lexer const &l);

  Lexer(Lexer const &l, size_t begin, size_t end);

  Lexer(Lexer const &l, size_t begin);

  ~Lexer() {}

  void skip_to(Lexer const &l);

  void generate_event_report();

  void subsume_sub_lexer(Lexer &l);

  LX::E find_matching_paren(size_t &paren_match_idx);

  LX::E find_next_global_symbol(size_t &idx);

  char next_char();

  char peek_char();

  E push_int();

  void push_operator(char c);

  void push_group(Lexer l);

  E match_operator(char c);

  E match_operator(UT::String s);

  UT::String get_word(size_t idx);

  bool match_keyword(UT::String keyword, UT::String word);

  void strip_white_space(size_t idx);

  void strip_line(size_t idx);

  E run();

  E operator()();
};

} // namespace LX

/*-------------------------------------------------------------------------------
 *\UTILS
 *------------------------------------------------------------------------------*/

namespace std
{

inline string
to_string(
  LX::E e)
{
  switch (e)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case LX::E::LX_ENUM_VALUE: return #LX_ENUM_VALUE;
    LX_E_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_IF("UNREACHABLE");
  return "";
};

inline string
to_string(
  LX::LangType lang_type)
{
  switch (lang_type)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case LX::LangType::LX_ENUM_VALUE: return #LX_ENUM_VALUE;
    LX_LangType_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_MSG("Got unexpected type %d", lang_type);
  return "";
}

// TODO: Knowing what type I need from the union is not obvious.
// There should be a better way to do it
inline string
to_string(
  LX::Sig sig)
{
  switch (sig.type)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case LX::LangType::LX_ENUM_VALUE:                                            \
    if constexpr (LX::LangType::LX_ENUM_VALUE == LX::LangType::Fn)             \
    {                                                                          \
      UT::Pair<LX::Sig> pair = sig.as.pair;                                    \
      return to_string(pair.first()) + " -> " + to_string(pair.second());      \
    }                                                                          \
    return to_string(sig.type);
    LX_LangType_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_MSG("Unreachable variant %d\n", sig.type);

  return "";
}

inline string
to_string(
  LX::Type t)
{
  switch (t)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case LX::Type::LX_ENUM_VALUE: return #LX_ENUM_VALUE;
    LX_Type_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_MSG("Got unexpected type %d", t);
  return "";
}

inline string to_string(LX::Tokens ts);

inline string
to_string(
  LX::Token t)
{
  switch (t.type)
  {
  case LX::Type::Int:
    return string("Int") + "(" + to_string(t.as.integer) + ")";
  case LX::Type::Plus:
    return "Op("
           "+"
           ")";
  case LX::Type::Minus:
    return "Op("
           "-"
           ")";
  case LX::Type::Mult:
    return "Op("
           "*"
           ")";
  case LX::Type::Div:
    return "Op("
           "/"
           ")";
  case LX::Type::IsEq:
    return "Op("
           "?="
           ")";
  case LX::Type::Modulus:
    return "Op("
           "%"
           ")";
  case LX::Type::Let:
  {
    std::string let_string = to_string(t.as.binding.let);
    std::string in_string  = to_string(t.as.binding.in);
    std::string var_name   = to_string(t.as.binding.name);
    return "let " + var_name + " = " + let_string + " in " + in_string;
  }
  break;
  case LX::Type::Fn:
  {
    std::string var_name    = to_string(t.as.fn.param_name);
    std::string body_string = to_string(t.as.fn.body);
    return "(\\" + var_name + " = " + body_string + ")";
  }
  break;
  case LX::Type::Word:
  {
    return "Word " + to_string(t.as.string);
  }
  case LX::Type::If:
  {
    return "if " + to_string(t.as.if_else.condition) +    //
           " => " + to_string(t.as.if_else.true_branch) + //
           " else " + to_string(t.as.if_else.else_branch);
  }
  case LX::Type::Group:
  {
    return to_string(t.as.tokens);
  }
  case LX::Type::PubDef:
  {
    return "pub " + to_string(t.as.sym.name) + " = " + to_string(t.as.sym.def);
  }
  case LX::Type::IntDef:
  {
    return "int " + to_string(t.as.sym.name) + " = " + to_string(t.as.sym.def);
  }
  case LX::Type::Not:
  {
    return "(not)";
  }
  case LX::Type::Str:
  {
    return "\"" + to_string(t.as.string) + "\"";
  }
  case LX::Type::Min:
  {
    return "Min";
  }
  case LX::Type::Max:
  {
    return "Max";
  }
  case LX::Type::While:
  {
    return "while " + to_string(t.as.whyle.condition) + " "
           + to_string(t.as.whyle.body);
  }
  case LX::Type::ExtDef:
  {
    auto ext_sym = t.as.ext_sym;

    return "ext " + to_string(ext_sym.name) + ": " + to_string(ext_sym.sig)
           + " = " + to_string(ext_sym.def);
  }
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

inline string
to_string(
  LX::Tokens ts)
{
  string s{ "[ " };
  for (size_t i = 0; i < ts.m_len; ++i)
  {
    LX::Token t = ts[i];
    s += to_string(t);
    s += (i != ts.m_len - 1) ? " , " : "";
  }
  s += " ]";
  return s;
}
} // namespace std

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

#endif // LX_HEADER
