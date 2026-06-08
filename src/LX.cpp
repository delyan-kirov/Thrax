/*-------------------------------------------------------------------------------
 *\file LX.cpp
 *\info Lexer impl
 * *----------------------------------------------------------------------------*/

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "LX.hpp"
#include "UT.hpp"
#include <cstdio>
#include <map>
#include <string>

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

#define TOKEN_HANDLE(LX_MATCH_EXPR, LX_EVENT_CODE)                             \
  {                                                                            \
    E LX_EVENT_VAR = LX_MATCH_EXPR;                                            \
    if (LX_EVENT_CODE == LX_EVENT_VAR)                                         \
    {                                                                          \
      continue;                                                                \
    }                                                                          \
    else if (E::OK != LX_EVENT_VAR)                                            \
    {                                                                          \
      return LX_EVENT_VAR;                                                     \
    }                                                                          \
  }

namespace LX
{

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string
pprint(
  E e, int level)
{
  std::string pad(level * 2, ' ');
  switch (e)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case E::LX_ENUM_VALUE: return pad + #LX_ENUM_VALUE;
    LX_E_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_IF("UNREACHABLE");
  return "";
}

std::string
pprint(
  Type t, int level)
{
  std::string pad(level * 2, ' ');
  switch (t)
  {
#define X(LX_ENUM_VALUE)                                                       \
  case Type::LX_ENUM_VALUE: return pad + #LX_ENUM_VALUE;
    LX_Type_ENUM_VARIANTS
#undef X
  }

  UT_FAIL_MSG("Got unexpected type %d", (int)t);
  return "";
}

std::string
pprint(
  Token t, int level)
{
  std::string pad(level * 2, ' ');
  switch (t.type)
  {
  case Type::Int:
    return pad + "(int " + std::to_string(t.as.integer) + ")";
  case Type::Plus   : return pad + "(op +)";
  case Type::Minus  : return pad + "(op -)";
  case Type::Mult   : return pad + "(op *)";
  case Type::Div    : return pad + "(op /)";
  case Type::IsEq   : return pad + "(op ?=)";
  case Type::Modulus: return pad + "(op %)";
  case Type::Not    : return pad + "(not)";
  case Type::Word:
    return pad + "(word " + std::to_string(t.as.string) + ")";
  case Type::Str:
    return pad + "(str \"" + std::to_string(t.as.string) + "\")";
  case Type::Min: return pad + "(min)";
  case Type::Max: return pad + "(max)";
  case Type::Let:
    return pad + "(let " + std::to_string(t.as.binding.var) + "\n"
         + pad + "  (=\n" + pprint(t.as.binding.equals, level + 2) + ")\n"
         + pad + "  (in\n" + pprint(t.as.binding.in, level + 2) + "))";
  case Type::Fn:
    return pad + "(fn \\" + std::to_string(t.as.fn.param_name) + "\n"
         + pprint(t.as.fn.body, level + 1) + ")";
  case Type::If:
    return pad + "(if\n"
         + pad + "  (cond\n" + pprint(t.as.if_else.condition, level + 2) + ")\n"
         + pad + "  (then\n" + pprint(t.as.if_else.true_branch, level + 2) + ")\n"
         + pad + "  (else\n" + pprint(t.as.if_else.else_branch, level + 2) + "))";
  case Type::Group:
    return pad + "(group\n" + pprint(t.as.tokens, level + 1) + ")";
  case Type::PubDef:
  case Type::IntDef:
    return pad + "(" + (Type::PubDef == t.type ? "pub" : "int") + " "
         + std::to_string(t.as.sym.name) + "\n"
         + pprint(t.as.sym.def, level + 1) + ")";
  case Type::While:
    return pad + "(while\n"
         + pad + "  (cond\n" + pprint(t.as.whyle.condition, level + 2) + ")\n"
         + pad + "  (body\n" + pprint(t.as.whyle.body, level + 2) + "))";
  case Type::ExtDef:
    return pad + "(ext " + std::to_string(t.as.ext_sym.name) + "\n"
         + pprint(t.as.ext_sym.def, level + 1) + ")";
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

std::string
pprint(
  Tokens ts, int level)
{
  std::string s;
  for (size_t i = 0; i < ts.m_len; ++i)
  {
    if (i > 0) s += "\n";
    s += pprint(ts[i], level);
  }
  return s;
}

namespace
/*-------------------------------------------------------------------------------
 *\INTERNAL UTILS
 *------------------------------------------------------------------------------*/
{

bool
is_white_space(
  char c)
{
  switch (c)
  {
  case ' ':
  case '\t':
  case '\n': return true;
  default  : return false;
  }
}

bool
delimits_word(
  char c)
{
  switch (c)
  {
  case ' ':
  case '.':
  case '\t':
  case '\n':
  case '(':
  case ')':
  case '+':
  case '-':
  case '*':
  case '/':
  case '\\':
  case '%':
  case '^':
  case '!':
  case '?':
  case '~':
  case '$':
  case ';':
  case '=':
  case ':':
  case ',':
  case '"':
  case '@' : return true;
  default  : return false;
  }
}

bool
delimiter_operator(
  char c)
{
  switch (c)
  {
  case '(':
  case ')':
  case ',':
  case '{':
  case '}':
  case '[':
  case ']':
  case '\\':
  case ';' : return true;
  default  : return false;
  }
}

bool
reserved_not_used(
  char c)
{
  switch (c)
  {
  case ',':
  case '.':
  case '[':
  case ']':
  case '$':
  case '@':
  case '\'':
  case '~':
  case '`':
  case '&':
  case '|':
  case '^' : return true;
  default  : return false;
  }
}

} // namespace

/*-------------------------------------------------------------------------------
 *\IMPL (LX)
 *------------------------------------------------------------------------------*/

ErrorE::ErrorE(
  AR::Arena  &arena,
  const char *fn_name,
  int         line,
  const char *data,
  LX::E       error)
    : E{
        ER::Level::ERROR,
        0,
        arena,
        (void *)data,
      }
{
  UT::SB sb{};
  sb.concatf("[%s] %s ln(%d) %s", pprint(error).c_str(), fn_name, line, data);
  UT::Vu<char> msg = UT::memcopy(*this->m_arena, sb.vu().m_mem);
  this->m_data     = (void *)msg.m_mem;
}

char
Lexer::next_char()
{
  if (this->m_cursor >= this->m_end) return '\0';
  char c = this->m_input[this->m_cursor];
  UT_FAIL_IF('\0' == c);
  this->m_cursor += 1;
  return c;
}

bool
is_operator(
  char c)
{
  switch (c)
  {
  case '+':
  case ':':
  case '-':
  case '*':
  case '/':
  case '%':
  case '!':
  case '?':
  case '=':
  {
    return true;
  }
  break;
  default:
  {
    return false;
  }
  break;
  }
}

E
get_char_validity(
  const char c)
{
  if (is_white_space(c))
  {
    return E::OK;
  }
  if ('\0' == c)
  {
    return E::END_OF_FILE;
  }
  if (std::iscntrl(c))
  {
    return E::ASCII_CTR_CHAR;
  }
  if (not isascii(c))
  {
    return E::NON_ASCII_CHAR;
  }

  return E::OK;
}

E
Lexer::next_valid_char(
  char &c)
{
  char next_char = this->next_char();
  E    e         = get_char_validity(next_char);
  if (E::OK == e)
  {
    c = next_char;
  }
  else
  {
    return e;
  }

  return E::OK;
}

bool
word_matches_global_sym_keyword(
  UT::String s)
{
  return Keyword::PUB == s || Keyword::INT == s;
}

E
Lexer::next_non_extern_sym(
  Token &t)
{
  std::vector<UT::String> words;
  Lexer                   l{ *this, m_cursor, m_end };
  UT::String              sb{ 0 };
  LX::E                   e;

  for (;;)
  {
    e = l.next_word(sb);
    if (E::END_OF_FILE == e)
    {
      break;
    }
    if (Keyword::INT == sb || Keyword::PUB == sb || Keyword::EXT == sb)
    {
      l.m_cursor -= sb.m_len + 1;
      break;
    }
    words.push_back(sb);
  }

  UT::Vu<UT::String> ws{ words };
  LX_ASSERT(ws.m_len >= 3, E::CONTROL_STRUCTURE_ERROR);

  UT::String varname = *ws.pop_front();
  LX_ASSERT("=" == *ws.pop_front(), E::CONTROL_STRUCTURE_ERROR);

  LX_FN_TRY(tokenize(ws));
  m_cursor = l.m_cursor;

  t.as.sym.name = varname;
  t.as.sym.def  = m_tokens;

  return E::OK;
}

E
Lexer::next_global_sym(
  Token &t)
{
  Lexer      l{ *this, m_cursor, m_end };
  UT::String sb{ 0 };
  LX::E      e;

  e = l.next_word(sb);
  if (E::OK != e) return e;

  if (Keyword::INT == sb || Keyword::PUB == sb)
  {
    LX_FN_TRY(l.next_non_extern_sym(t));
    m_cursor = l.m_cursor;
    t.type   = Keyword::INT == sb ? Type::IntDef : Type::PubDef;
    return E::OK;
  }
  else if (Keyword::EXT == sb)
  {
    UT_TODO(Keyword::EXT == sb);
  }
  else
  {
    return E::CONTROL_STRUCTURE_ERROR;
  }

  return E::OK;
}

// TODO: What if symbol is valid ascii and reserved but not used?
// FIXME: freezes sometimes
E
Lexer::next_word(
  UT::String &sb)
{
  strip_white_space(m_cursor);
  sb.m_mem = m_input.m_mem + m_cursor;
  sb.m_len = 0;

  // FIXME: Combine to single operation
  char current_char = m_input[m_cursor];
  LX_FN_TRY(get_char_validity(current_char));
  LX_ASSERT(not reserved_not_used(current_char), E::UNRECOGNIZED_STRING);

  if ('"' == current_char)
  {
    sb += 1;
    m_cursor += 1;
    for (;;)
    {
      LX_FN_TRY(next_valid_char(current_char));
      if ('\0' == current_char)
      {
        return E::QUOTM_UNCLOSED;
      }
      else if ('"' == current_char)
      {
        // NOTE: the cursor now points after '"'
        sb += 1;
        return E::OK;
      }
      else
      {
        sb += 1;
      }
    }
  }

  if ('#' == current_char)
  {
    strip_line(m_cursor);
    strip_white_space(m_cursor);
    sb.m_mem = m_input.m_mem + m_cursor;
  }

  if (delimiter_operator(current_char))
  {
    m_cursor += 1;
    sb += 1;
    return E::OK;
  }

  if (is_operator(current_char))
  {
    for (;;)
    {
      LX_FN_TRY(next_valid_char(current_char));
      if (is_white_space(current_char))
      {
        return E::OK;
      }
      if (std::isalnum(current_char))
      {
        m_cursor -= 1;
        return E::OK;
      }
      sb += 1;
    }
  }

  for (;;)
  {
    LX_FN_TRY(next_valid_char(current_char));

    // FIXME: deal with comments
    if ('#' == current_char)
    {
      strip_line(m_cursor);
      strip_white_space(m_cursor);
      continue;
    }
    if (delimits_word(current_char))
    {
      if (is_white_space(current_char))
      {
        return E::OK;
      }
      else
      {
        m_cursor -= 1;
        return E::OK;
      }
    }
    else
    {
      sb += 1;
    }
  }

  return E::OK;
}

const std::map<std::string, Type> opertator_info_db{
  { "+", Type::Plus }, { "-", Type::Minus },   { "?=", Type::IsEq },
  { "*", Type::Mult }, { "%", Type::Modulus },
};

const std::map<std::string, E> control_delimiter_info_db{
  { "in", E::IN_KEYWORD },
  { "=>", E::FAT_ARROW },
  { ")", E::PAREN_LEFT },
  { "else", E::ELSE_KEYWORD },
};

// NOTE: later in the parser, we can check if - is an operator or part of the
// integer
E
Lexer::matches_operator(
  UT::Vu<UT::String> &words)
{
  if (words.is_empty()) return E::OK;

  UT::String s      = *words.first();
  auto       lookup = opertator_info_db.find(std::to_string(s));
  if (opertator_info_db.end() != lookup)
  {
    Token t{ lookup->second }; // FIXME: should have start and end
    m_tokens.push(t);
    words.pop_front();
    return E::MATCHED_OPERATOR;
  }

  return E::OK;
}

E
Lexer::matches_quotm(
  UT::Vu<UT::String> &words)
{
  UT::String word = *words.first();
  if (not('"' == *word.first() && '"' == *word.last()))
  {
    return E::OK;
  }

  words.pop_front();
  Token t{ Type::Str };
  t.as.string = word;
  m_tokens.push(t);
  return E::MATCHED_QUOTM;
}

E
Lexer::matches_ifelse(
  UT::Vu<UT::String> &words)
{
  UT::String word = *words.first();
  if (Keyword::IF != word) return E::OK;

  words.pop_front();
  Lexer lcond{ *this, m_cursor, m_end };
  LX_ASSERT(E::FAT_ARROW == lcond.tokenize(words), E::UNREACHABLE_CASE_REACHED);

  Lexer ltrue{ lcond, m_cursor, m_end };
  LX_ASSERT(E::ELSE_KEYWORD == ltrue.tokenize(words),
            E::UNREACHABLE_CASE_REACHED);

  Lexer lelse{ ltrue, m_cursor, m_end };
  E     e = lelse.tokenize(words);

  if (not(E::OK == e || E::IN_KEYWORD == e))
  {
    return E::OPERATOR_MATCH_FAILURE;
  }

  if (E::IN_KEYWORD == e) words.retreat();

  Token t{ Type::If, m_cursor };
  t.as.if_else.condition   = lcond.m_tokens;
  t.as.if_else.true_branch = ltrue.m_tokens;
  t.as.if_else.else_branch = lelse.m_tokens;
  m_tokens.push(t);

  return E::MATCHES_IFELSE;
}

// let x: <type> = <expr> in <expr>
E
Lexer::matches_letin(
  UT::Vu<UT::String> &words)
{
  UT::String word = *words.first();
  if (Keyword::LET != word) return E::OK;
  words.pop_front();
  LX_ASSERT(not words.is_empty(), E::WORD_NOT_FOUND);

  UT::String varname = *words.pop_front();
  LX_ASSERT("" != varname, E::CONTROL_STRUCTURE_ERROR);
  LX_ASSERT(not words.is_empty(), E::WORD_NOT_FOUND);

  Token t{ Type::Let, m_cursor };

  LX_ASSERT("=" == *words.pop_front(), E::UNRECOGNIZED_STRING);
  LX_ASSERT(not words.is_empty(), E::UNRECOGNIZED_STRING);

  Lexer llet{ *this, m_cursor, m_end };
  LX_ASSERT(E::IN_KEYWORD == llet.tokenize(words), E::UNREACHABLE_CASE_REACHED);

  Lexer lin{ *this, m_cursor, m_end };
  E     e = lin.tokenize(words);
  LX_ASSERT(E::IN_KEYWORD == e || E::OK == e, E::CONTROL_STRUCTURE_ERROR);

  if (E::IN_KEYWORD == e) words.retreat();

  t.type              = Type::Let;
  t.as.binding.var    = varname;
  t.as.binding.equals = llet.m_tokens;
  t.as.binding.in     = lin.m_tokens;

  m_tokens.push(t);

  return E::MATCHES_LETIN;
}

E
Lexer::matches_open_paren(
  UT::Vu<UT::String> &words)
{
  UT::String word = *words.first();
  if ("(" != word) return E::OK;
  words.pop_front();

  Lexer new_l{ *this, m_cursor, m_end };
  LX_ASSERT(E::PAREN_LEFT == new_l.tokenize(words), E::PARENTHESIS_UNBALANCED);

  Token t{ new_l.m_tokens };
  m_tokens.push(t);

  return E::MATCHES_OPEN_PAREN;
}

E
Lexer::matches_integer(
  UT::Vu<UT::String> &words)
{
  UT::String s = *words.first();
  if (not std::isdigit(s[0]))
  {
    return E::OK;
  }

  words.pop_front();
  Token t{ Type::Int };

  try
  {
    t.as.integer = std::stoi(s.m_mem);
  }
  catch (...)
  {
    return E::NUMBER_PARSING_FAILURE;
  }
  m_tokens.push(t);

  return E::MATCHES_INTEGER;
}

E
Lexer::matches_string(
  UT::Vu<UT::String> &words)
{
  UT::String s = *words.first();
  if (not std::isalpha(s[0]))
  {
    return E::OK;
  }

  words.pop_front();
  Token t{ Type::Word };
  t.as.string = s;
  m_tokens.push(t);

  return E::MATCHES_STRING;
}

E
Lexer::matches_control_operator(
  UT::Vu<UT::String> &words)
{
  UT::String s      = *words.first();
  auto       lookup = control_delimiter_info_db.find(std::to_string(s));
  if (control_delimiter_info_db.end() != lookup)
  {
    words.pop_front();
    return lookup->second;
  }

  return E::OK;
}

E
Lexer::matches_lambda(
  UT::Vu<UT::String> &words)
{
  if (words.is_empty()) return E::OK;
  UT::String s = *words.first();
  if ("\\" != s) return E::OK;
  words.pop_front();
  LX_ASSERT(not words.is_empty(), E::CONTROL_STRUCTURE_ERROR);
  UT::String varname = *words.pop_front();

  LX_ASSERT(not words.is_empty(), E::CONTROL_STRUCTURE_ERROR);
  LX_ASSERT("=" == *words.pop_front(), E::CONTROL_STRUCTURE_ERROR);
  LX_ASSERT(not words.is_empty(), E::CONTROL_STRUCTURE_ERROR);

  Lexer lambda = Lexer{ *this, m_cursor, m_end };
  E     e      = lambda.tokenize(words);

  switch (e)
  {
  case E::IN_KEYWORD:
  case E::PAREN_LEFT:
  case E::ELSE_KEYWORD:
  {
    words.retreat();
    break;
  }
  case E::OK: break;
  default   : return e;
  }

  Token t{ m_arena };
  t.type             = Type::Fn;
  t.as.fn.param_name = varname;
  t.as.fn.body       = lambda.m_tokens;

  m_tokens.push(t);

  return E::MATCHES_LAMBDA;
}

LX::E
Lexer::tokenize(
  UT::Vu<UT::String> &words)
{
  while (not words.is_empty())
  {
    E e = matches_control_operator(words);
    if (E::OK != e) return e;
    TOKEN_HANDLE(matches_operator(words), E::MATCHED_OPERATOR);
    TOKEN_HANDLE(matches_quotm(words), E::MATCHED_QUOTM);
    TOKEN_HANDLE(matches_letin(words), E::MATCHES_LETIN);
    TOKEN_HANDLE(matches_ifelse(words), E::MATCHES_IFELSE);
    TOKEN_HANDLE(matches_integer(words), E::MATCHES_INTEGER);
    TOKEN_HANDLE(matches_string(words), E::MATCHES_STRING);
    TOKEN_HANDLE(matches_open_paren(words), E::MATCHES_OPEN_PAREN);
    TOKEN_HANDLE(matches_lambda(words), E::MATCHES_LAMBDA);
    LX_ASSERT(false, E::CONTROL_STRUCTURE_ERROR);
  }

  return E::OK;
}

// TODO: should be rewritten
void
Lexer::generate_event_report()
{
  ER::Events events = this->m_events;
  for (size_t i = 0; i < events.m_len; ++i)
  {
    ER::E e = events.m_mem[i];
    if (ER::Level::ERROR == e.m_level)
    {
      std::printf("[%s] %s\n", UT::SERROR, (char *)e.m_data);

      // Find the line with the error
      size_t line       = 1;
      size_t line_begin = this->m_begin;
      size_t line_end   = this->m_end;

      // Locate the start of the line
      for (size_t i = this->m_begin; i < this->m_end; ++i)
      {
        if (this->m_input[i] == '\n')
        {
          line_begin = i + 1;
          line += 1;
        }
        if (i == this->m_cursor - 1)
        {
          break;
        }
      }

      // Locate the end of the line
      for (size_t i = line_begin + 1; i < this->m_end; ++i)
      {
        if (this->m_input[i] == '\n')
        {
          line_end = i;
          break;
        }
      }

      // Extract the line
      std::string msg;
      for (size_t i = line_begin; i < line_end; ++i)
      {
        msg += this->m_input[i];
      }
      size_t offset = (this->m_cursor - line_begin) + 1;

      // Print the error context
      std::printf("   %ld |   \033[1;37m%s\033[0m\n", line, msg.c_str());
      std::printf("%*c\033[31m^\033[0m\n", (int)offset + 7, ' ');

      return;
    }
    else
    {
      std::printf("%s\n", (char *)e.m_data);
    }
  }
}

void
Lexer::strip_white_space(
  size_t idx)
{
  char c = this->m_input[idx];

  while (is_white_space(c))
  {
    idx += 1;
    c = this->m_input[idx];
  }

  this->m_cursor = idx;
};

void
Lexer::strip_line(
  size_t idx)
{
  char c = this->m_input[idx];

  while (c && '\n' != c)
  {
    idx += 1;
    c = this->m_input[idx];
  }

  this->m_cursor = idx;
}

char
Lexer::peek_char()
{
  return this->m_cursor < this->m_end ? this->m_input[this->m_cursor] : '\0';
}

Lexer::Lexer(
  const UT::String input, AR::Arena &arena, size_t begin, size_t end)
    : m_arena{ arena },
      m_events{ arena },
      m_input{ input },
      m_tokens{ Tokens(arena) },
      m_cursor{ begin },
      m_begin{ begin },
      m_end{ end }
{
}

Lexer::Lexer(
  Lexer const &l, size_t begin, size_t end)
    : m_arena{ l.m_arena },
      m_events{ l.m_arena },
      m_input{ l.m_input },
      m_cursor(l.m_cursor),
      m_begin{ begin },
      m_end{ end }
{
  new (&this->m_tokens) Tokens{ l.m_arena };
}

// FIXME: Candidate for removal
Token::Token(Type t)
    : type{ t },
      cursor{ 0 },
      as{} {};

// FIXME: Candidate for removal
Token::Token(Type type, size_t line)
    : type{ type },
      cursor{ line },
      as{} {};

// FIXME: Candidate for removal
Token::Token(
  Tokens tokens)
    : type{ Type::Group },
      cursor{ 0 }
{
  new (&as.tokens) Tokens{ tokens }; // NOTE: placement new
};

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

} // namespace LX
