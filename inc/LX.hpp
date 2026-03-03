#ifndef LX_HEADER
#define LX_HEADER

#include "UT.hpp"
#include <cstring>
#include <stdio.h>

namespace LX
{
enum class E;
}

namespace std
{
string to_string(LX::E);
}

namespace LX
{

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

enum class E
{
  MIN = (ssize_t)-1,
  OK,
  PARENTHESIS_UNBALANCED,
  NUMBER_PARSING_FAILURE,
  UNRECOGNIZED_STRING,
  OPERATOR_MATCH_FAILURE,
  UNREACHABLE_CASE_REACHED,
  FAT_ARROW,
  ELSE_KEYWORD,
  IN_KEYWORD,
  CONTROL_STRUCTURE_ERROR,
  WORD_NOT_FOUND,
  MAX,
};

struct ErrorE : public ER::E
{
  ErrorE(
    AR::Arena  &arena,
    const char *fn_name,
    int         line,
    const char *data,
    LX::E       error)
      : E{
          ER::Level::ERROR, //
          0,                //
          arena,            //
          (void *)data,     //
        }
  {
    UT::SB sb{};
    sb.concatf("[%s] %s ln(%d) %s", UT_TCS(error), fn_name, line, data);
    UT::Vu<char> msg = UT::memcopy(*this->m_arena, sb.vu().m_mem);
    this->m_data     = (void *)msg.m_mem;
  }
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

struct If
// if expr => expr else expr
// [TODO] if expr is pattern => is ... else =>
{
  Tokens m_condition;
  Tokens m_true_branch;
  Tokens m_else_branch;
};

struct Let
{
  UT::String m_var_name;
  Tokens     m_let_tokens;
  Tokens     m_in_tokens;
};

struct Fn
{
  UT::String m_var_name;
  Tokens     m_body;
};

struct SymDef
{
  UT::String m_sym_name;
  Tokens     m_def;
};

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

// NOTE: T -> (T -> (T -> T))
//   is: T -> T -> T -> T
struct Sig
{
  LangType m_type;
  union
  {
    UT::Pair<Sig> m_pair;
  } as;
};

struct ExtSym
{
  UT::String m_name;
  Sig        m_sig;
  Tokens     m_def;
};

struct While
{
  Tokens m_condition;
  Tokens m_body;
};

struct Token
{
  Type   m_type;
  size_t m_line;
  size_t m_cursor;
  union
  {
    Tokens     m_tokens;
    Let        m_let_tokens;
    If         m_if_tokens;
    Fn         m_fn;
    SymDef     m_sym;
    While      m_while;
    ExtSym     m_ext_sym;
    Sig        m_sig;
    UT::String m_string;
    ssize_t    m_int = 0;
  } as;

  Token()  = default;
  ~Token() = default;
  // TODO: the line and cursor should be set
  Token(Type t)
      : m_type{ t },
        m_line{ 0 },
        m_cursor{ 0 },
        as{} {};
  Token(
    Tokens tokens)
      : m_type{ Type::Group },
        m_line{ 0 },
        m_cursor{ 0 }
  {
    new (&as.m_tokens) Tokens{ tokens }; // NOTE: placement new
  };
};

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

  Lexer(
    const char *const input, AR::Arena &arena, size_t begin, size_t end)
      : m_arena{ arena },
        m_events{ arena },
        m_input{ input },
        m_tokens{ Tokens(arena) },
        m_lines{ 0 },
        m_cursor{ begin },
        m_begin{ begin },
        m_end{ end }
  {
  }

  Lexer(
    Lexer const &l)
      : m_arena(l.m_arena),              //
        m_events(std::move(l.m_events)), //
        m_input{ l.m_input },            //
        m_tokens(l.m_tokens),            //
        m_lines(l.m_lines),              //
        m_cursor(l.m_cursor),            //
        m_begin(l.m_begin),              //
        m_end(l.m_end)                   //
  {
    for (size_t i = 0; i < l.m_events.m_len; ++i)
    {
      ER::E e = l.m_events[i];
      this->m_events.push(e);
    }
  };

  Lexer(
    Lexer const &l, size_t begin, size_t end)
      : m_arena{ l.m_arena },
        m_events(l.m_arena)
  {
    this->m_begin  = l.m_begin;
    this->m_end    = l.m_end;
    this->m_cursor = l.m_cursor;
    this->m_input  = l.m_input;
    this->m_begin  = begin;
    this->m_end    = end;
    new (&this->m_tokens) Tokens{ l.m_arena };
  }

  Lexer(
    Lexer const &l, size_t begin)
      : m_arena{ l.m_arena },
        m_events(l.m_arena)
  {
    this->m_begin  = l.m_begin;
    this->m_end    = l.m_end;
    this->m_cursor = l.m_cursor;
    this->m_input  = l.m_input;
    this->m_begin  = begin;
    this->m_end    = l.m_end;
    new (&this->m_tokens) Tokens{ l.m_arena };
  }

  void
  skip_to(
    Lexer const &l)
  {
    this->m_cursor = l.m_cursor;
    this->m_lines += l.m_lines;

    for (auto e : l.m_events)
    {
      this->m_events.push(e);
    }
  }

  ~Lexer() {}

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
};

} // namespace LX

namespace std
{

inline string
to_string(
  LX::E e)
{
  switch (e)
  {
  case LX::E::MIN                     : return "MIN";
  case LX::E::OK                      : return "OK";
  case LX::E::PARENTHESIS_UNBALANCED  : return "PARENTHESIS_UNBALANCED";
  case LX::E::NUMBER_PARSING_FAILURE  : return "NUMBER_PARSING_FAILURE";
  case LX::E::UNRECOGNIZED_STRING     : return "UNRECOGNIZED_STRING";
  case LX::E::OPERATOR_MATCH_FAILURE  : return "OPERATOR_MATCH_FAILURE";
  case LX::E::UNREACHABLE_CASE_REACHED: return "UNREACHABLE_CASE_REACHED";
  case LX::E::FAT_ARROW               : return "FAT_ARROW";
  case LX::E::CONTROL_STRUCTURE_ERROR : return "CONTROL_STRUCTURE_ERROR";
  case LX::E::ELSE_KEYWORD            : return "ELSE_KEYWORD";
  case LX::E::IN_KEYWORD              : return "IN_KEYWORD";
  case LX::E::WORD_NOT_FOUND          : return "WORD_NOT_FOUND";
  case LX::E::MAX                     : return "MAX";
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
  switch (sig.m_type)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case LX::LangType::LX_ENUM_VALUE:                                            \
    if constexpr (LX::LangType::LX_ENUM_VALUE == LX::LangType::Fn)             \
    {                                                                          \
      UT::Pair<LX::Sig> pair = sig.as.m_pair;                                  \
      return to_string(pair.first()) + " -> " + to_string(pair.second());      \
    }                                                                          \
    return to_string(sig.m_type);
    LX_LangType_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_MSG("Unreachable variant %d\n", sig.m_type);

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
  switch (t.m_type)
  {
  case LX::Type::Min:
  case LX::Type::Max: return "Unknown";
  case LX::Type::Int: return string("Int") + "(" + to_string(t.as.m_int) + ")";
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
    std::string let_string = to_string(t.as.m_let_tokens.m_let_tokens);
    std::string in_string  = to_string(t.as.m_let_tokens.m_in_tokens);
    std::string var_name   = to_string(t.as.m_let_tokens.m_var_name);
    return "let " + var_name + " = " + let_string + " in " + in_string;
  }
  break;
  case LX::Type::Fn:
  {
    std::string var_name    = to_string(t.as.m_fn.m_var_name);
    std::string body_string = to_string(t.as.m_fn.m_body);
    return "(\\" + var_name + " = " + body_string + ")";
  }
  break;
  case LX::Type::Word:
  {
    return to_string(t.as.m_string);
  }
  case LX::Type::If:
  {
    return "if " + to_string(t.as.m_if_tokens.m_condition) +    //
           " => " + to_string(t.as.m_if_tokens.m_true_branch) + //
           " else " + to_string(t.as.m_if_tokens.m_else_branch);
  }
  case LX::Type::Group:
  {
    return to_string(t.as.m_tokens);
  }
  case LX::Type::PubDef:
  {
    return "pub " + to_string(t.as.m_sym.m_sym_name) + " = "
           + to_string(t.as.m_sym.m_def);
  }
  case LX::Type::IntDef:
  {
    return "int " + to_string(t.as.m_sym.m_sym_name) + " = "
           + to_string(t.as.m_sym.m_def);
  }
  case LX::Type::Not:
  {
    return "(not)";
  }
  case LX::Type::Str:
  {
    return "\"" + to_string(t.as.m_string) + "\"";
  }
  case LX::Type::While:
  {
    return "while " + to_string(t.as.m_while.m_condition) + " "
           + to_string(t.as.m_while.m_body);
  }
  case LX::Type::ExtDef:
  {
    auto ext_sym = t.as.m_ext_sym;

    return "ext " + to_string(ext_sym.m_name) + ": " + to_string(ext_sym.m_sig)
           + " = " + to_string(ext_sym.m_def);
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
    switch (t.m_type)
    {
    case LX::Type::Group  : s += to_string((LX::Tokens)t.as.m_tokens); break;
    case LX::Type::Let    :
    case LX::Type::Fn     :
    case LX::Type::Div    :
    case LX::Type::Int    :
    case LX::Type::Minus  :
    case LX::Type::Modulus:
    case LX::Type::Mult   :
    case LX::Type::Plus   :
    case LX::Type::IsEq   :
    case LX::Type::IntDef :
    case LX::Type::ExtDef :
    case LX::Type::Min    :
    case LX::Type::Max    : s += to_string(t); break;
    case LX::Type::Word   : s += "Word(" + to_string(t.as.m_string) + ")"; break;
    case LX::Type::If     : s += to_string(t); break;
    case LX::Type::Str    : s += "Str(" + to_string(t.as.m_string) + ")"; break;
    default               : UT_FAIL_MSG("Got unexpected type: %s", UT_TCS(t.m_type));
    }

    s += (i != ts.m_len - 1) ? " , " : "";
  }
  s += " ]";
  return s;
}
} // namespace std

#endif // LX_HEADER
