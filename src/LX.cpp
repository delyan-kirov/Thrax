/*-------------------------------------------------------------------------------
 *\file LX.cpp
 *\info Lexer impl
 * *----------------------------------------------------------------------------*/

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "LX.hpp"
#include "UT.hpp"

namespace LX
{

namespace
/*-------------------------------------------------------------------------------
 *\UTILS
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

LangType
c_type_to_langtype(
  UT::String s)
{
  if ("C_int" == s)
    return LangType::Int;
  else if ("C_str" == s)
    return LangType::Ptr;
  else if ("C_void" == s)
    return LangType::Void;
  else
    return LangType::Max;
}

// TODO: We may have functions as input params, but for now we
// ignore this case
// Functions can only have integer params in their signature
std::pair<LX::E, Sig>
parse_signature_helper(
  UT::Vec<UT::String> &types, AR::Arena &arena, const size_t idx)
{
  Sig sig{};

  // TODO: Don't hardcode types like that
  LangType type = c_type_to_langtype(types[idx]);
  if (LangType::Max == type)
  {
    for (size_t i = (size_t)LX::LangType::Min; i < (size_t)LX::LangType::Max;
         ++i)
    {
      UT::String s{ (char *)UT_TCS((LangType)i) };
      if (types[idx] == s)
      {
        type = (LangType)i;
      }
    }
  }

  if (LangType::Max == type)
  {
    return std::pair{ LX::E::CONTROL_STRUCTURE_ERROR, sig }; // TODO: new error
  }

  if (idx == types.m_len - 1)
  {
    // TODO: The pointer should indicate what it points to
    sig.type = type;
  }
  else
  {
    sig.type            = LangType::Fn;
    UT::Pair<Sig> *pair = &sig.as.pair;
    *pair               = { arena };
    // TODO: this should also include the type behind the pointer
    pair->begin()->type = type;
    *pair->last()       = parse_signature_helper(types, arena, idx + 1).second;
    sig.as.pair         = *pair;
  }

  return std::pair{ LX::E::OK, sig };
}

bool
is_hex_char(
  char c)
{
  switch (c)
  {
  case '0':
  case '1':
  case '2':
  case '3':
  case '4':
  case '5':
  case '6':
  case '7':
  case '8':
  case '9':
  case 'a':
  case 'b':
  case 'c':
  case 'd':
  case 'e':
  case 'f':
  case 'A':
  case 'B':
  case 'C':
  case 'D':
  case 'E':
  case 'F': return true;
  default : return false;
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
  case ';': return true;
  default : return false;
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
  sb.concatf("[%s] %s ln(%d) %s", UT_TCS(error), fn_name, line, data);
  UT::Vu<char> msg = UT::memcopy(*this->m_arena, sb.vu().m_mem);
  this->m_data     = (void *)msg.m_mem;
}

E
Lexer::operator()()
{
  return this->run();
};

// FIXME: Marked for removal, there is no point in this function
bool
Lexer::match_keyword(
  UT::String keyword, UT::String word)
{
  bool result = UT::strcompare(keyword, word);
  return result;
}

// TODO: should be comment aware
// FIXME: 'word=' does not work but it should
// FIXME: bug when ignoring comments, see find_next_global_symbol
UT::String
Lexer::get_word(
  size_t idx)
{
  UT::SB sb{};
  this->strip_white_space(idx);
  idx = this->m_cursor;

  for (char c = m_input[idx++]; c; c = m_input[idx++])
  {
    if (delimits_word(c))
    {
      if (!is_white_space(c))
      {
        idx -= 1;
      }
      break;
    }
    sb.add(c);
  }

  UT::String string = sb.to_String(m_arena);
  m_cursor          = idx;

  return string;
}

LX::E
Lexer::find_matching_paren(
  size_t &paren_match_idx)
{
  size_t stack = 1;

  for (size_t idx = this->m_cursor; idx < this->m_end; ++idx)
  {
    char c = this->m_input[idx];
    if (')' == c)
    {
      stack -= 1;
    }
    else if ('(' == c)
    {
      stack += 1;
    }
    if (0 == stack)
    {
      paren_match_idx = idx;

      return LX::E::OK;
    }
  }

  LX_ERROR_REPORT(LX::E::PARENTHESIS_UNBALANCED, "");
}

char
Lexer::next_char()
{
  if (this->m_cursor >= this->m_end) return '\0';
  char c = this->m_input[this->m_cursor];
  UT_FAIL_IF('\0' == c);
  if ('\n' == c) this->m_lines += 1;
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

// TODO: What if symbol is valid ascii and reserved but not used?
E
Lexer::next_word(
  UT::String &sb)
{
  strip_white_space(m_cursor);
  sb.m_mem = m_input.m_mem + m_cursor;

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

// TODO: Think how to tokenize with this approach
// NOTE: When we have '-' we can do the same trink we do now by checking if the
// next char is an integer, this makes parsing later way easier
bool
Lexer::matches_operator(
  UT::String s)
{
  Token t{}; // FIXME: should have start and end
  bool  does_match = false;

  if ("+" == s)
  {
    does_match = true;
    t.type     = Type::Plus;
  }
  else if ("-" == s)
  {
    // FIXME: See note above
    does_match = true;
    t.type     = Type::Minus;
  }
  else if ("?=" == s)
  {
    does_match = true;
    t.type     = Type::IsEq;
  }
  else if ("*" == s)
  {
    does_match = true;
    t.type     = Type::Mult;
  }
  else if ("%" == s)
  {
    does_match = true;
    t.type     = Type::Modulus;
  }

  if (does_match)
  {
    m_tokens.push(t);
  }

  return does_match;
}

bool
Lexer::matches_quotm(
  UT::String s)
{
  return '"' == *s.first() && '"' == *s.last();
}

// TODO: UNFINISHED
LX::E
Lexer::init()
{
  std::vector<UT::String> words;
  UT::String              sb{ 0 };
  LX::E                   e;

  for (e = next_word(sb); LX::E::OK == e; e = next_word(sb))
  {
    words.push_back(sb);
    sb = { 0 };
  }

  LX_ASSERT(E::END_OF_FILE == e, E::UNRECOGNIZED_STRING);

  AR::Arena arena{};
  Tokens    tokens{ arena };
  tokenize(words, tokens);

  return E::OK;
}

// TODO: Need to figure out if the lexer should be aware of ext symbols and
// stuff like that
// FIXME: This function should have inout params words and tokens
LX::E
Lexer::tokenize(
  std::vector<UT::String> words, Tokens tokens)
{
  UT::String sb{ 0 };
  UT_UNUSED(tokens);

  for (auto &word : words)
  {
    std::printf("INFO: " UTSTRf "\n", UTSTFa(word));
  }

  size_t idx = 0;
  for (;;)
  {
    if (idx >= words.size()) break;
    UT::String word = words[idx];
    if (matches_operator(word))
    {
      idx += 1;
      continue;
    }
    // matches quotm
    // matches if
    // matches let
    // matches `(`
    // matches ext/int
    // matches string (this is non trivial because we might match another
    //    keyword like `in` or `else` which is an error)
    else
    {
      break;
    }
  }

  UT_TODO();

  return E::OK;
}

LX::E
Lexer::push_int()
{
  int    result       = 0;
  size_t cursor       = this->m_cursor;
  size_t lines        = this->m_lines;
  bool   parse_as_hex = false;

  std::string s{
    this->m_input[this->m_cursor
                  - 1 /* since we entered this function, the point
                         where we need to start parsing is offset by 1 */
  ]
  };
  for (char c = this->next_char(); c; c = this->next_char())
  {
    if (!is_hex_char(c))
    {
      switch (c)
      {
      case '+':
      case '-':
      case '*':
      case '/':
      case '%':
      case '?':
      case ':':
      case '=': this->m_cursor -= 1; break;
      case '|':
      case '^':
      case '~':
      case '&':
      case '@':
      case '$':
      case '#':
      case '!':
        LX_ERROR_REPORT(LX::E::NUMBER_PARSING_FAILURE,
                        "Symbol reserved but currently not parse-able");
        break;
      case ')':
      case ' ':
      case '\t':
      case '\n': break;
      case 'x' : parse_as_hex = true; goto LX_ACCUMILATE_STRING;
      default:
        LX_ERROR_REPORT(LX::E::NUMBER_PARSING_FAILURE,
                        "Unparse-able symbol found");
        break;
      }
      break;
    }
  LX_ACCUMILATE_STRING:
    s += c;
  }

  try
  {
    result = parse_as_hex ? std::stoi(s.c_str(), nullptr, 16)
                          : std::stoi(s.c_str(), nullptr, 10);

    Token t{ Type::Int };
    t.as.integer = result;
    this->m_tokens.push(t);
  }
  catch (std::exception &e)
  {
    this->m_cursor = cursor;
    this->m_lines  = lines;
    LX_ERROR_REPORT(E::NUMBER_PARSING_FAILURE, "std::stoi exception occured");
  }

  return LX::E::OK;
}

void
Lexer::push_operator(
  char c)
{
  Type t_type = Type::Min;
  switch (c)
  {
  case '-': t_type = Type::Minus; break;
  case '+': t_type = Type::Plus; break;
  case '*': t_type = Type::Mult; break;
  case '/': t_type = Type::Div; break;
  case '%': t_type = Type::Modulus; break;
  default : /* UNREACHABLE */ UT_FAIL_IF("UNERACHABLE");
  }
  m_tokens.push(Token{ t_type });
}

// TODO: candidate for refactor
LX::E
Lexer::run()
{
  for (char c = this->next_char(); //
       c;                          //
       c = this->next_char()       //
  )
  {
    switch (c)
    {
    case 1:
    case 2:
    case 3:
    case 4:
    case 5:
    case 6:
    case 7:
    case 8:
    case 11:
    case 12:
    case 14:
    case 15:
    case 16:
    case 17:
    case 18:
    case 19:
    case 20:
    case 21:
    case 22:
    case 23:
    case 24:
    case 25:
    case 26:
    case 27:
    case 28:
    case 29:
    case 30:
    case 31:
    case 127:
    {
      UT_FAIL_MSG(
        "ASCII control char `%d` should not be present in the source file", c);
    }
    break;

    case '$':
    case '&':
    case '\'':
    case ',':
    case '.':
    case ';':
    case '<':
    case '>':
    case '@':
    case '[':
    case ']':
    case '^':
    case '_':
    case '`':
    case '|':
    case '{':
    case '}':
    case '~':
    {
      UT_FAIL_MSG("Symbol `%c` reserved but not used", c);
    }
    break;
    case '!':
    {
      Token not_token{ Type::Not };
      m_tokens.push(not_token);
    }
    break;
    case '"':
    {
      UT::SB sb{};
      for (char c = this->next_char(); c && c != '"'; c = this->next_char())
      {
        sb.add(c);
      }

      UT::String string = sb.to_String(m_arena);
      Token      string_token{ Type::Str };
      string_token.as.string = string;

      m_tokens.push(string_token);
    }
    break;
    case '-':
    {
      char next_c = this->peek_char();
      if (std::isdigit(next_c))
      {
        this->push_int();
      }
      else
      {
        push_operator('-');
      }
    }
    break;
    case '+':
    case '*':
    case '/':
    case '%':
    {
      this->push_operator(c);
    }
    break;
    case '#':
    {
      this->strip_line(this->m_cursor);
    }
    break;
    case '(':
    {
      size_t group_begin = this->m_cursor + 1;
      size_t group_end   = group_begin;

      LX_FN_TRY(this->find_matching_paren(group_end));

      Lexer new_l = Lexer(*this, group_begin, group_end);
      LX_FN_TRY(new_l());

      this->push_group(new_l);
    }
    break;
    case '?':
    {
      LX_ASSERT('=' == this->next_char(), LX::E::OPERATOR_MATCH_FAILURE);
      Token token{ Type::IsEq };
      this->m_tokens.push(token);
    }
    break;
    case ')':
    {
      LX_ERROR_REPORT(LX::E::UNREACHABLE_CASE_REACHED,
                      "')' should never match in this branch");
    }
    break;
    case '\r': // For windows compatibility
    {
      UT_TODO("Add windows support");
    }
    break;
    case '\t': // Tabs are white-space
    case ' ':  // Spaces are white-space
    case '\n': // New lines are white-space
    {
      ; // Do nothing
    }
    break;
    case '\0':
    {
      UT_FAIL_IF("UNREACHABLE");
    }
    break;
    case '=':
    {
      LX_ASSERT('>' == this->next_char(), LX::E::OPERATOR_MATCH_FAILURE);
      return E::FAT_ARROW;
    }
    break;
    case '\\': // \<var> = <expr>
    {
      this->strip_white_space(this->m_cursor);
      UT::String var_name = this->get_word(this->m_cursor);

      LX_FN_TRY(this->match_operator('='));

      Lexer body_lexer{ *this, m_cursor, m_end };
      LX::E e = body_lexer();
      LX_ASSERT(LX::E::OK == e || LX::E::IN_KEYWORD == e,
                LX::E::CONTROL_STRUCTURE_ERROR);

      Token fn{ Type::Fn, m_lines, m_cursor };
      fn.as.fn.param_name = var_name;
      fn.as.fn.body       = body_lexer.m_tokens;

      this->m_tokens.push(fn);
      this->skip_to(body_lexer);

      return e;
    }
    break;
    case ':':
    {
      Sig sig{ LangType::Max };

      Lexer sig_lexer{ *this, m_cursor, m_end };
      sig_lexer.parse_signature(sig);

      LX_ASSERT(LangType::Max > sig.type && sig.type > LangType::Min,
                LX::E::CONTROL_STRUCTURE_ERROR);

      skip_to(sig_lexer);
      m_tokens.push(Token{ Type::Sig, m_lines, m_cursor });
    }
    break;
    case '0':
    case '1':
    case '2':
    case '3':
    case '4':
    case '5':
    case '6':
    case '7':
    case '8':
    case '9':
    {
      LX_FN_TRY(this->push_int());
    }
    break;
    case 'a':
    case 'b':
    case 'c':
    case 'd':
    case 'e':
    case 'f':
    case 'g':
    case 'h':
    case 'i':
    case 'j':
    case 'k':
    case 'l':
    case 'm':
    case 'n':
    case 'o':
    case 'p':
    case 'q':
    case 'r':
    case 's':
    case 't':
    case 'u':
    case 'v':
    case 'w':
    case 'x':
    case 'y':
    case 'z':
    {
      // TODO: instead of match_keyword if-else chains, make an enum and match
      // over the variants
      UT::String word = this->get_word(
        this->m_cursor - 1); // we already got the first char so go back 1

      if (this->match_keyword(Keyword::IN, word))
      {
        return LX::E::IN_KEYWORD;
      }
      else if (this->match_keyword(Keyword::ELSE, word))
      {
        return LX::E::ELSE_KEYWORD;
      }
      else if (this->match_keyword(Keyword::INT, word)
               || this->match_keyword(Keyword::PUB, word))
      {
        size_t next_symbol_idx;
        E      e = this->find_next_global_symbol(next_symbol_idx);
        if (E::WORD_NOT_FOUND == e) next_symbol_idx = this->m_end;

        UT::String sym_name = this->get_word(this->m_cursor);

        // TODO: use a different error
        LX_ASSERT("" != sym_name, E::WORD_NOT_FOUND);

        // TODO: Simplify type annotations checks
        e = match_operator(':');
        Sig sig{ LangType::Max };

        if (E::OK == e)
        {
          Lexer sig_lexer{ *this, m_cursor, next_symbol_idx };
          sig_lexer.parse_signature(sig);
          skip_to(sig_lexer);
        }

        LX_FN_TRY(this->match_operator('='));

        Lexer new_lexer{ *this, m_cursor, next_symbol_idx };
        LX_FN_TRY(new_lexer());

        // TODO: candidate for refactor
        // Tokens are too annoying to construct, perhaps dedicated constructors
        // and other helpers could make this a lot easier to use and simpler to
        // understand
        Token symbol{ "int" == word ? Type::IntDef : Type::PubDef,
                      new_lexer.m_lines,
                      new_lexer.m_cursor };
        symbol.as.sym.def  = new_lexer.m_tokens;
        symbol.as.sym.name = sym_name;
        symbol.as.sym.sig  = sig;

        this->m_tokens.push(symbol);
        this->skip_to(new_lexer);
        this->m_cursor = next_symbol_idx;
      }
      else if (this->match_keyword(Keyword::LET, word))
      {
        UT::String var_name = this->get_word(this->m_cursor);

        // TODO: we should check for type annotation here
        Sig   sig{ LangType::Max };
        LX::E e = match_operator(':');

        if (E::OK == e)
        {
          // UT_TODO("Cannot parse annotations in let bindigns yet");
          Lexer sig_lexer{ *this, m_cursor, m_end };
          sig_lexer.parse_signature(sig);
          skip_to(sig_lexer);
        }

        LX_FN_TRY(this->match_operator('='));

        Lexer let_lexer{ *this, m_cursor, m_end };
        LX_ASSERT(E::IN_KEYWORD == let_lexer(), E::CONTROL_STRUCTURE_ERROR);

        Lexer in_lexer{ let_lexer, let_lexer.m_cursor, m_end };
        LX_FN_TRY(in_lexer());

        // TODO: Token should have an end
        Token token{ Type::Let, m_lines, m_cursor };
        token.as.binding.var    = var_name;
        token.as.binding.equals = let_lexer.m_tokens;
        token.as.binding.in     = in_lexer.m_tokens;
        token.as.binding.sig    = sig;

        this->m_tokens.push(token);
        this->skip_to(in_lexer);
      }
      else if (this->match_keyword(Keyword::IF, word))
      {
        Lexer if_condition_lexer{ *this, m_cursor, m_end };
        LX_ASSERT(E::FAT_ARROW == if_condition_lexer(),
                  E::OPERATOR_MATCH_FAILURE);

        Lexer true_branch_lexer{ if_condition_lexer,
                                 if_condition_lexer.m_cursor,
                                 m_end };
        LX_ASSERT(E::ELSE_KEYWORD == true_branch_lexer(),
                  E::CONTROL_STRUCTURE_ERROR);

        Lexer else_branch_lexer{ true_branch_lexer,
                                 true_branch_lexer.m_cursor,
                                 m_end };
        LX::E e = else_branch_lexer();
        LX_ASSERT(LX::E::OK == e || LX::E::IN_KEYWORD == e,
                  LX::E::CONTROL_STRUCTURE_ERROR);

        // TODO: candidate for refactor
        Token token{ Type::If, m_lines, m_cursor };
        token.as.if_else.condition   = if_condition_lexer.m_tokens;
        token.as.if_else.true_branch = true_branch_lexer.m_tokens;
        token.as.if_else.else_branch = else_branch_lexer.m_tokens;

        this->m_tokens.push(token);
        this->skip_to(else_branch_lexer);

        if (E::IN_KEYWORD == e) return e;
      }
      else if (this->match_keyword(Keyword::WHILE, word))
      {
        Lexer condition_lexer{ *this, m_cursor, m_end };
        LX_ASSERT(E::FAT_ARROW == condition_lexer(), E::OPERATOR_MATCH_FAILURE);

        Lexer body_lexer{ condition_lexer, condition_lexer.m_cursor, m_end };
        LX::E e = body_lexer();
        LX_ASSERT(e == E::ELSE_KEYWORD || e == E::IN_KEYWORD || e == E::OK,
                  E::CONTROL_STRUCTURE_ERROR);

        // TODO: candidate for refactor
        Token token{ Type::While };
        token.as.whyle.condition = condition_lexer.m_tokens;
        token.as.whyle.body      = body_lexer.m_tokens;

        this->m_tokens.push(token);
        this->skip_to(body_lexer);

        if (E::IN_KEYWORD == e || e == E::ELSE_KEYWORD) return e;
      }
      else if (this->match_keyword(Keyword::EXT, word))
      {
        size_t next_symbol_idx;
        E      e = this->find_next_global_symbol(next_symbol_idx);
        if (E::WORD_NOT_FOUND == e) next_symbol_idx = this->m_end;

        UT::String sym_name = this->get_word(this->m_cursor);

        LX_ASSERT("" != sym_name,
                  E::WORD_NOT_FOUND); // TODO: use a different error

        LX_FN_TRY(this->match_operator(':'));

        Lexer sig_lexer{ *this, m_cursor, next_symbol_idx };
        Sig   sig;
        sig_lexer.parse_signature(sig);

        LX_FN_TRY(sig_lexer.match_operator('='));

        sig_lexer();
        Tokens sym_defs{ m_arena };
        sym_defs.push(sig_lexer.m_tokens.last()->as.tokens[0]);
        sym_defs.push(sig_lexer.m_tokens.last()->as.tokens[1]);

        Token symbol{ Type::ExtDef };
        symbol.as.ext_sym.name = sym_name;
        symbol.as.ext_sym.sig  = sig;
        symbol.as.ext_sym.def  = sym_defs;

        skip_to(sig_lexer);
        m_cursor = next_symbol_idx;

        m_tokens.push(symbol);
      }
      else
      {
        LX_ASSERT(word.m_len > 0, LX::E::UNRECOGNIZED_STRING);
        Token t{ Type::Word };
        t.as.string = word;
        this->m_tokens.push(t);
      }
    }
    break;
    case 'A':
    case 'B':
    case 'C':
    case 'D':
    case 'E':
    case 'F':
    case 'G':
    case 'H':
    case 'I':
    case 'J':
    case 'K':
    case 'L':
    case 'M':
    case 'N':
    case 'O':
    case 'P':
    case 'Q':
    case 'R':
    case 'S':
    case 'T':
    case 'U':
    case 'V':
    case 'W':
    case 'X':
    case 'Y':
    case 'Z':
    {
      UT_TODO("Capital letters reserved for types");
    }
    break;
    default:
    {
      UT_TODO("Non ascii chars not supported yet");
    }
    }
  }

  return LX::E::OK;
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
Lexer::subsume_sub_lexer(
  Lexer &l)
{
  for (auto t : l.m_tokens)
  {
    Token token{ t };
    this->m_tokens.push(token);
  }
  this->m_cursor = l.m_cursor;

  for (size_t i = 0; i < l.m_events.m_len; ++i)
  {
    ER::E e = l.m_events[i];
    this->m_events.push(e);
  }
}

// TODO: candidate for refactor
E
Lexer::match_operator(
  char c)
{
  Lexer match_lexer{ *this, m_cursor, m_end };
  match_lexer.strip_white_space(m_cursor);
  char next_char = match_lexer.next_char();
  if (c == next_char)
  {
    skip_to(match_lexer);
    return E::OK;
  }

  return E::UNRECOGNIZED_STRING;
};

// FIXME: Should not mutate state if it fails
E
Lexer::match_operator(
  UT::String s)
{
  Lexer match_lexer{ *this, m_cursor, m_end };
  match_lexer.strip_white_space(m_cursor);

  UT::SB sb{};
  for (size_t idx = 0; idx < s.m_len; ++idx)
  {
    sb.add(match_lexer.next_char());
  }

  if (sb.vu() == s)
  {
    skip_to(match_lexer);
    return E::OK;
  }
  return E::UNRECOGNIZED_STRING;
}

// TODO: candidate for refactor
void
Lexer::strip_white_space(
  size_t idx)
{
  char   c         = this->m_input[idx];
  size_t new_lines = 0;

  while (is_white_space(c))
  {
    if ('\n' == c) new_lines += 1;
    idx += 1;
    c = this->m_input[idx];
  }

  this->m_lines += new_lines;
  this->m_cursor = idx;
};

// TODO: candidate for refactor
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

  this->m_lines += 1;
  this->m_cursor = idx;
}

void
Lexer::push_group(
  Lexer l)
{
  Token t{ l.m_tokens };
  this->m_tokens.push(t);
  this->m_cursor = l.m_cursor + 1;
}

char
Lexer::peek_char()
{
  return this->m_cursor < this->m_end ? this->m_input[this->m_cursor] : '\0';
}

// TODO: candidate for refactor
LX::E
Lexer::find_next_global_symbol(
  size_t &idx)
{
  Lexer search_lexer{
    this->m_input, this->m_arena, this->m_cursor, this->m_end
  };

  for (UT::String next_word = search_lexer.get_word(this->m_cursor);
       search_lexer.m_cursor < search_lexer.m_end;
       next_word = search_lexer.get_word(search_lexer.m_cursor))
  {
    // FIXME: lexer .get_word method should be aware of comments '#'
    if ("#" == next_word)
    {
      search_lexer.strip_line(search_lexer.m_cursor);
    }
    if (delimits_word(search_lexer.m_input[search_lexer.m_cursor]))
    {
      search_lexer.m_cursor += 1;
    }
    if ("int" == next_word || "pub" == next_word || "ext" == next_word)
    {
      /*
         Need to return the cursor just before
         ..int...
          ^    ^
      */
      idx = search_lexer.m_cursor - 3 - 1;
      return E::OK;
    }
  }

  return E::WORD_NOT_FOUND;
}

// FIXME: Signature parsing should be the job of the parser
LX::E
Lexer::parse_signature(
  Sig &sig)
{
  UT::Vec<UT::String> types{ m_arena };
  LX::E               e = E::OK;
  UT::String          type{};

  while (E::OK == e)
  {
    type = get_word(m_cursor);
    LX_ASSERT("" != type, E::UNRECOGNIZED_STRING);
    types.push(type);
    e = match_operator("->");
    if (E::OK == e)
    {
      continue;
    }
    else
    {
      // TODO: parsing type annotations should be more robust
      // we cannot check if we matched on something valid in this context
      // e = match_operator('=');
    }
  }

  auto parse_result = parse_signature_helper(types, m_arena, 0);

  LX_ASSERT(E::OK == parse_result.first, E::CONTROL_STRUCTURE_ERROR);

  sig = parse_result.second;

  return E::OK;
}

Lexer::Lexer(
  const UT::String input, AR::Arena &arena, size_t begin, size_t end)
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

void
Lexer::skip_to(
  Lexer const &l)
{
  this->m_cursor = l.m_cursor;
  this->m_lines += l.m_lines;

  for (auto e : l.m_events)
  {
    this->m_events.push(e);
  }
}

// TODO: Candidate for removal
Token::Token(Type t)
    : type{ t },
      line{ 0 },
      cursor{ 0 },
      as{} {};

// TODO: Candidate for removal
Token::Token(Type type, size_t line, size_t cursor)
    : type{ type },
      line{ line },
      cursor{ cursor },
      as{} {};

// TODO: Candidate for removal
Token::Token(
  Tokens tokens)
    : type{ Type::Group },
      line{ 0 },
      cursor{ 0 }
{
  new (&as.tokens) Tokens{ tokens }; // NOTE: placement new
};

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/

} // namespace LX
