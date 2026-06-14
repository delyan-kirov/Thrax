/*-------------------------------------------------------------------------------
 *\file LX.cpp
 *\info Lexer impl
 * *----------------------------------------------------------------------------*/

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "LX.hpp"
#include "UT.hpp"
#include <map>
#include <string>

/*------------------------------------------------------------------------------
 *\MACROS
 *-----------------------------------------------------------------------------*/
#define LX_FN_TRY(LX_FN)                                                       \
  do                                                                           \
  {                                                                            \
    LX::E result = (LX_FN);                                                    \
    if (LX::E::OK != result)                                                   \
    {                                                                          \
      this->m_events->push(LX::ErrorE{ this->m_arena,                          \
                                       __FILE__,                               \
                                       __PRETTY_FUNCTION__,                    \
                                       __LINE__,                               \
                                       ("The function: " #LX_FN " failed!"),   \
                                       result,                                 \
                                       this->m_input,                          \
                                       this->m_filename,                       \
                                       this->m_cursor });                      \
      return result;                                                           \
    }                                                                          \
  } while (false)

#define LX_FN_TRY_L(LX_LEXER, LX_FN)                                          \
  do                                                                           \
  {                                                                            \
    LX::E result = (LX_FN);                                                    \
    if (LX::E::OK != result)                                                   \
    {                                                                          \
      this->subsume(LX_LEXER);                                                 \
      this->m_events->push(LX::ErrorE{ this->m_arena,                          \
                                       __FILE__,                               \
                                       __PRETTY_FUNCTION__,                    \
                                       __LINE__,                               \
                                       ("The function: " #LX_FN " failed!"),   \
                                       result,                                 \
                                       this->m_input,                          \
                                       this->m_filename,                       \
                                       this->m_cursor });                      \
      return result;                                                           \
    }                                                                          \
  } while (false)

#define LX_RETURN_ERROR(LX_ERROR)                                              \
  do                                                                           \
  {                                                                            \
    this->m_events->push(LX::ErrorE{ this->m_arena,                            \
                                     __FILE__,                                 \
                                     __PRETTY_FUNCTION__,                      \
                                     __LINE__,                                 \
                                     ("The function failed!"),                 \
                                     LX_ERROR,                                 \
                                     this->m_input,                            \
                                     this->m_filename,                         \
                                     this->m_cursor });                        \
    return LX_ERROR;                                                           \
  } while (false)

#define LX_ASSERT(LX_BOOL_EXPR, LX_ERROR_E)                                    \
  do                                                                           \
  {                                                                            \
    if (!(LX_BOOL_EXPR))                                                       \
    {                                                                          \
      this->m_events->push(LX::ErrorE{ this->m_arena,                          \
                                       __FILE__,                               \
                                       __PRETTY_FUNCTION__,                    \
                                       __LINE__,                               \
                                       (#LX_BOOL_EXPR),                        \
                                       (LX_ERROR_E),                           \
                                       this->m_input,                          \
                                       this->m_filename,                       \
                                       this->m_cursor });                      \
      return (LX_ERROR_E);                                                     \
    }                                                                          \
  } while (false)

#define LX_ASSERT_L(LX_LEXER, LX_BOOL_EXPR, LX_ERROR_E)                       \
  do                                                                           \
  {                                                                            \
    if (!(LX_BOOL_EXPR))                                                       \
    {                                                                          \
      this->subsume(LX_LEXER);                                                 \
      this->m_events->push(LX::ErrorE{ this->m_arena,                          \
                                       __FILE__,                               \
                                       __PRETTY_FUNCTION__,                    \
                                       __LINE__,                               \
                                       (#LX_BOOL_EXPR),                        \
                                       (LX_ERROR_E),                           \
                                       this->m_input,                          \
                                       this->m_filename,                       \
                                       (LX_LEXER).m_cursor });                \
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
 *\ UTILS
 *------------------------------------------------------------------------------*/

struct ErrorData
{
  LX::E      error;
  UT::String file;
  UT::String fn_name;
  int        cpp_line;
  UT::String message;
  UT::String input;
  UT::String filename;
  size_t     cursor;
};

struct ErrorE : public ER::E
{
  ErrorE(AR::Arena &arena,
         UT::String file,
         UT::String fn_name,
         int        line,
         UT::String data,
         LX::E      error,
         UT::String input,
         UT::String filename,
         size_t     cursor);
};

std::string
pprint(
  E e, size_t level)
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
  TokenTag t, size_t level)
{
  std::string pad(level * 2, ' ');
  switch (t)
  {
#define X(tag, type)                                                           \
  case TokenTag::tag: return pad + #tag;
    LX_TOKEN_VARIANTS
#undef X
  }

  UT_FAIL_MSG("Got unexpected tag %d", (int)t);
  return "";
}

std::string
pprint(
  Token t, size_t level)
{
  std::string pad(level * 2, ' ');
  switch (t.tag)
  {
  case TokenTag::Int:
    return pad + "(int " + std::to_string(std::get<TkInt>(t.as).value) + ")";
  case TokenTag::Plus   : return pad + "(op +)";
  case TokenTag::Minus  : return pad + "(op -)";
  case TokenTag::Mult   : return pad + "(op *)";
  case TokenTag::Div    : return pad + "(op /)";
  case TokenTag::IsEq   : return pad + "(op ?=)";
  case TokenTag::Modulus: return pad + "(op %)";
  case TokenTag::Not    : return pad + "(not)";
  case TokenTag::Word:
    return pad + "(word " + std::to_string(std::get<TkWord>(t.as).value) + ")";
  case TokenTag::Str:
    return pad + "(str \"" + std::to_string(std::get<TkStr>(t.as).value)
           + "\")";
  case TokenTag::Min: return pad + "(min)";
  case TokenTag::Max: return pad + "(max)";
  case TokenTag::Let:
  {
    auto &b = std::get<TkLet>(t.as);
    return pad + "(let " + std::to_string(b.var) + "\n" + pad + "  (=\n"
           + pprint(b.equals, level + 2) + ")\n" + pad + "  (in\n"
           + pprint(b.in, level + 2) + "))";
  }
  case TokenTag::Fn:
  {
    auto &f = std::get<TkFn>(t.as);
    return pad + "(fn \\" + std::to_string(f.param_name) + "\n"
           + pprint(f.body, level + 1) + ")";
  }
  case TokenTag::If:
  {
    auto &ie = std::get<TkIf>(t.as);
    return pad + "(if\n" + pad + "  (cond\n" + pprint(ie.condition, level + 2)
           + ")\n" + pad + "  (then\n" + pprint(ie.true_branch, level + 2)
           + ")\n" + pad + "  (else\n" + pprint(ie.else_branch, level + 2)
           + "))";
  }
  case TokenTag::Group:
    return pad + "(group\n" + pprint(std::get<TkGroup>(t.as).tokens, level + 1)
           + ")";
  case TokenTag::PubDef:
  {
    auto &pd = std::get<TkPubDef>(t.as);
    return pad + "(pub " + std::to_string(pd.name) + "\n"
           + pprint(pd.def, level + 1) + ")";
  }
  case TokenTag::IntDef:
  {
    auto &id = std::get<TkIntDef>(t.as);
    return pad + "(int " + std::to_string(id.name) + "\n"
           + pprint(id.def, level + 1) + ")";
  }
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

std::string
pprint(
  Tokens ts, size_t level)
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

UT_NODISCARD E
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

Token
mk_token(
  TokenTag tag, UT::String word)
{
  Token t;
  t.tag  = tag;
  t.word = word;
  switch (tag)
  {
#define X(xtag, xtype)                                                         \
  case TokenTag::xtag: t.as = xtype{}; break;
    LX_TOKEN_VARIANTS
#undef X
  }
  return t;
}

Token
mk_token(
  TokenTag tag, TokenData as, UT::String word)
{
  Token t;
  t.tag  = tag;
  t.word = word;
  t.as   = as;
  return t;
}

} // namespace

/*-------------------------------------------------------------------------------
 *\IMPL (LX)
 *------------------------------------------------------------------------------*/

ErrorE::ErrorE(
  AR::Arena &arena,
  UT::String file,
  UT::String fn_name,
  int        line,
  UT::String data,
  LX::E      error,
  UT::String input,
  UT::String filename,
  size_t     cursor)
    : E{
        ER::Level::ERROR,
        0,
        arena,
        nullptr,
      }
{
  auto *ed     = (ErrorData *)arena.alloc(sizeof(ErrorData));
  ed->error    = error;
  ed->file     = file;
  ed->fn_name  = fn_name;
  ed->cpp_line = line;
  ed->message  = data;
  ed->input    = input;
  ed->filename = filename;
  ed->cursor   = cursor;
  this->m_data = (void *)ed;
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
    LX_RETURN_ERROR(e);
  }

  return E::OK;
}

E
Lexer::next_sym(
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
    if (E::OK != e)
    {
      subsume(l);
      LX_RETURN_ERROR(e);
    }
    words.push_back(sb);
  }

  UT::Vu<UT::String> ws{ words };
  LX_ASSERT(ws.m_len >= 3, E::GLOBAL_DEF_STRUCTURE_MALFORMED);

  UT::String varname = pop_word(ws);
  UT::String eq      = pop_word(ws);
  LX_ASSERT("=" == eq, E::EXPECT_EQUALS_AFTER_GLOBAL_SYM_DEF);
  LX_FN_TRY(tokenize(ws));
  m_cursor = l.m_cursor;

  t = mk_token(TokenTag::IntDef, TkIntDef{ m_tokens, varname }, varname);

  return E::OK;
}

E
Lexer::next(
  Token &t)
{
  Lexer      l{ *this, m_cursor, m_end };
  UT::String sb{ 0 };

  LX_FN_TRY_L(l, l.next_word(sb));

  if (Keyword::INT == sb || Keyword::PUB == sb)
  {
    LX_FN_TRY_L(l, l.next_sym(t));
    m_cursor = l.m_cursor;
    if (Keyword::PUB == sb)
    {
      auto id = std::get<TkIntDef>(t.as);
      t = mk_token(TokenTag::PubDef, TkPubDef{ id.def, id.name }, id.name);
    }
    return E::OK;
  }
  else if (Keyword::EXT == sb)
  {
    UT_TODO(Keyword::EXT == sb);
  }
  else
  {
    std::printf("%s\n", UT_TCS(sb));
    LX_RETURN_ERROR(E::UNEXPECTED_GLOBAL_DEF_SYM_MARKER);
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
  LX_ASSERT(not reserved_not_used(current_char),
            E::ILLEGAL_USE_OF_RESERVED_CHAR);

  if ('"' == current_char)
  {
    sb += 1;
    m_cursor += 1;
    for (;;)
    {
      LX_FN_TRY(next_valid_char(current_char));
      if ('\0' == current_char)
      {
        // TODO: Perhaps string literals should be by default on one line, with
        // multiline being a special case
        LX_RETURN_ERROR(E::QUOTM_UNCLOSED);
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

const std::map<std::string, TokenTag> opertator_info_db{
  { "+", TokenTag::Plus }, { "-", TokenTag::Minus },   { "?=", TokenTag::IsEq },
  { "*", TokenTag::Mult }, { "%", TokenTag::Modulus },
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
    Token t = mk_token(lookup->second, s);
    m_tokens.push(t);
    pop_word(words);
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

  pop_word(words);
  Token t = mk_token(TokenTag::Str, TkStr{ word }, word);
  m_tokens.push(t);
  return E::MATCHED_QUOTM;
}

E
Lexer::matches_ifelse(
  UT::Vu<UT::String> &words)
{
  UT::String word = *words.first();
  if (Keyword::IF != word) return E::OK;

  pop_word(words);
  Lexer lcond{ *this, m_cursor, m_end };
  LX_ASSERT_L(lcond, E::FAT_ARROW == lcond.tokenize(words),
              E::IF_CONDITION_SEPARATOR_MISSING);

  Lexer ltrue{ lcond, m_cursor, m_end };
  LX_ASSERT_L(ltrue, E::ELSE_KEYWORD == ltrue.tokenize(words),
              E::IF_EXPR_MISSING_ELSE_BRANCH);

  Lexer lelse{ ltrue, m_cursor, m_end };
  E     e = lelse.tokenize(words);

  if (not(E::OK == e || E::IN_KEYWORD == e))
  {
    subsume(lelse);
    LX_RETURN_ERROR(E::IF_EXPR_MALFORMED_ELSE_BRANCH);
  }

  LX_ASSERT_L(lelse, not lelse.m_tokens.is_empty(),
              E::IF_EXPR_ELSE_BRANCH_EMPTY); // FIXME [empty tokens]

  if (E::IN_KEYWORD == e) words.retreat();

  Token t = mk_token(
    TokenTag::If, TkIf{ lcond.m_tokens, ltrue.m_tokens, lelse.m_tokens }, word);
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
  pop_word(words);
  LX_ASSERT(not words.is_empty(), E::LET_EXPR_VAR_NAME_MISSING);

  UT::String varname = pop_word(words);
  LX_ASSERT("" != varname, E::LET_EXPR_EMPTY_VAR_NAME);
  LX_ASSERT(not words.is_empty(), E::LET_EXPR_VAR_DEF_EMPTY);

  UT::String eq = pop_word(words);
  LX_ASSERT("=" == eq, E::LET_EXPR_EQ_SYMB_AFTER_VAR_MISSING);
  LX_ASSERT(not words.is_empty(), E::LET_EXPR_EXPECTED_DEF_AFTER_EQ);

  // FIXME: what if tokens from llet or lin are empty?
  Lexer llet{ *this, m_cursor, m_end };
  LX_ASSERT_L(llet, E::IN_KEYWORD == llet.tokenize(words),
              E::LET_EXPR_MISSING_IN);

  Lexer lin{ *this, m_cursor, m_end };
  E     e = lin.tokenize(words);
  LX_ASSERT_L(lin, E::IN_KEYWORD == e || E::OK == e,
              E::LET_EXPR_ERRONEOUS_IN_EXPR);

  if (E::IN_KEYWORD == e) words.retreat();

  Token t = mk_token(
    TokenTag::Let, TkLet{ varname, llet.m_tokens, lin.m_tokens }, word);
  m_tokens.push(t);

  return E::MATCHES_LETIN;
}

E
Lexer::matches_open_paren(
  UT::Vu<UT::String> &words)
{
  UT::String word = *words.first();
  if ("(" != word) return E::OK;
  pop_word(words);

  Lexer new_l{ *this, m_cursor, m_end };
  LX_ASSERT_L(new_l, E::PAREN_LEFT == new_l.tokenize(words),
              E::PARENTHESIS_UNBALANCED);

  Token t = mk_token(
    TokenTag::Group, TkGroup{ std::move(new_l.m_tokens) }, word);
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

  pop_word(words);

  try
  {
    Token t = mk_token(
      TokenTag::Int, TkInt{ static_cast<ssize_t>(std::stoi(s.m_mem)) }, s);
    m_tokens.push(t);
  }
  catch (...)
  {
    LX_RETURN_ERROR(E::NUMBER_PARSING_FAILURE);
  }

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

  pop_word(words);
  Token t = mk_token(TokenTag::Word, TkWord{ s }, s);
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
    pop_word(words);
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
  pop_word(words);
  LX_ASSERT(not words.is_empty(), E::LAMBDA_NO_VAR_NAME);
  UT::String varname = pop_word(words);

  LX_ASSERT(not words.is_empty(), E::LAMBDA_NOTHING_AFTER_VAR);
  UT::String eq = pop_word(words);
  LX_ASSERT("=" == eq, E::LAMBDA_EQ_EXPECTED_AFTER_VARNAME);
  LX_ASSERT(not words.is_empty(), E::LAMBDA_EXPECTED_DEF_AFTER_EQ);

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
  default:
    subsume(lambda);
    LX_RETURN_ERROR(e);
  }
  m_cursor = lambda.m_cursor;

  Token t = mk_token(TokenTag::Fn, TkFn{ varname, lambda.m_tokens }, s);
  m_tokens.push(t);

  return E::MATCHES_LAMBDA;
}

LX::E
Lexer::tokenize(
  UT::Vu<UT::String> &words)
{
  while (not words.is_empty())
  {
    LX_FN_TRY(matches_control_operator(words));
    TOKEN_HANDLE(matches_operator(words), E::MATCHED_OPERATOR);
    TOKEN_HANDLE(matches_quotm(words), E::MATCHED_QUOTM);
    TOKEN_HANDLE(matches_letin(words), E::MATCHES_LETIN);
    TOKEN_HANDLE(matches_ifelse(words), E::MATCHES_IFELSE);
    TOKEN_HANDLE(matches_integer(words), E::MATCHES_INTEGER);
    TOKEN_HANDLE(matches_string(words), E::MATCHES_STRING);
    TOKEN_HANDLE(matches_open_paren(words), E::MATCHES_OPEN_PAREN);
    TOKEN_HANDLE(matches_lambda(words), E::MATCHES_LAMBDA);
    LX_ASSERT(false, E::MATCHES_NOTHING);
  }

  if (m_tokens.is_empty()) return E::NO_TOKENS;

  return E::OK;
}

namespace
{

size_t
compute_src_line(
  UT::String input, size_t cursor)
{
  size_t line = 1;
  for (size_t j = 0; j < input.m_len && j < cursor; ++j)
  {
    if (input[j] == '\n') line += 1;
  }
  return line;
}

std::string
format_src_context(
  UT::String input, size_t cursor, std::string pad)
{
  std::string s;
  size_t      line       = 1;
  size_t      line_begin = 0;
  size_t      line_end   = input.m_len;

  for (size_t j = 0; j < input.m_len; ++j)
  {
    if (input[j] == '\n')
    {
      if (j >= cursor) break;
      line_begin = j + 1;
      line += 1;
    }
  }

  for (size_t j = line_begin + 1; j < input.m_len; ++j)
  {
    if (input[j] == '\n')
    {
      line_end = j;
      break;
    }
  }

  std::string src_line;
  for (size_t j = line_begin; j < line_end; ++j)
  {
    src_line += input[j];
  }

  size_t offset = (cursor > line_begin) ? (cursor - line_begin) : 0;

  s += pad + std::to_string(line) + " | \033[1;37m" + src_line + "\033[0m\n";
  s += pad + std::string(std::to_string(line).size(), ' ') + " | "
       + std::string(offset, ' ') + "\033[31m^\033[0m\n";

  return s;
}

} // namespace

std::string
pprint(
  ER::Events events, size_t level)
{
  std::string s;
  LX::E       prev_error    = LX::E::OK;
  size_t      prev_src_line = 0;
  size_t      depth         = level;

  for (size_t i = 0; i < events.m_len; ++i)
  {
    ER::E e = events.m_mem[i];
    if (ER::Level::ERROR != e.m_level) continue;

    auto       *ed = (ErrorData *)e.m_data;
    std::string pad(depth * 2, ' ');

    if (ed->error != prev_error)
    {
      // New error group — find the furthest cursor among events with this code
      depth         = level;
      pad           = std::string(depth * 2, ' ');
      prev_error    = ed->error;
      prev_src_line = 0;

      size_t max_cursor = ed->cursor;
      for (size_t j = i + 1; j < events.m_len; ++j)
      {
        ER::E ej = events.m_mem[j];
        if (ER::Level::ERROR != ej.m_level) continue;
        auto *ej_ed = (ErrorData *)ej.m_data;
        if (ej_ed->error != ed->error) break;
        if (ej_ed->cursor > max_cursor) max_cursor = ej_ed->cursor;
      }

      size_t src_line = compute_src_line(ed->input, max_cursor);

      s += pad + "\033[31m[" + pprint(ed->error) + "]\033[0m "
           + std::to_string(ed->filename) + ":" + std::to_string(src_line)
           + "\n";
      s += format_src_context(ed->input, max_cursor, pad + "  ");
      prev_src_line = src_line;
    }

    // Show source context when the source line changes
    size_t src_line = compute_src_line(ed->input, ed->cursor);
    if (src_line != prev_src_line)
    {
      s += format_src_context(ed->input, ed->cursor, pad + "  ");
      prev_src_line = src_line;
    }

    // C++ source location and message
    s += pad + "  " + std::to_string(ed->file) + ":"
         + std::to_string(ed->cpp_line) + " " + std::to_string(ed->fn_name)
         + "\n";
    s += pad + "    " + std::to_string(ed->message) + "\n";

    depth += 1;
  }

  return s;
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

UT::String
Lexer::pop_word(
  UT::Vu<UT::String> &words)
{
  UT::String w = *words.pop_front();
  m_cursor     = w.m_mem - m_input.m_mem;
  return w;
}

void
Lexer::subsume(
  const Lexer &l)
{
  for (size_t i = 0; i < l.m_events->m_len; ++i)
  {
    m_events->push((*l.m_events)[i]);
  }
}

char
Lexer::peek_char()
{
  return this->m_cursor < this->m_end ? this->m_input[this->m_cursor] : '\0';
}

Lexer::Lexer(
  const UT::String input,
  const UT::String filename,
  AR::Arena       &arena,
  size_t           begin,
  size_t           end)
    : m_arena{ arena },
      m_events{ new (arena.alloc(sizeof(ER::Events))) ER::Events(arena) },
      m_input{ input },
      m_filename{ filename },
      m_tokens{ Tokens(arena) },
      m_cursor{ begin },
      m_begin{ begin },
      m_end{ end }
{
}

Lexer::Lexer(
  Lexer const &l, size_t begin, size_t end)
    : m_arena{ l.m_arena },
      m_events{ new (l.m_arena.alloc(sizeof(ER::Events))) ER::Events(l.m_arena) },
      m_input{ l.m_input },
      m_filename{ l.m_filename },
      m_cursor(l.m_cursor),
      m_begin{ begin },
      m_end{ end }
{
  new (&this->m_tokens) Tokens{ l.m_arena };
}

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

} // namespace LX
