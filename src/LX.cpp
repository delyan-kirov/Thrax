#include "LX.hpp"
#include "UT.hpp"
#include <cstdio>

namespace LX
{

namespace
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

// TODO: We may have functions as input params, but for now we
// ignore this case
// Functions can only have integer params in their signature
std::pair<LX::E, LX::Sig>
parse_sig(
  UT::Vec<UT::String> &types, AR::Arena &arena, const size_t idx)
{
  Sig sig{};

  // TODO: Don't hardcode types like that
  if ("C_int" == types[idx])
  {
    if (idx == types.m_len - 1)
    {
      sig.m_type = LX::LangType::Int;
    }
    else
    {
      sig.m_type            = LangType::Fn;
      UT::Pair<Sig> *pair   = &sig.as.m_pair;
      *pair                 = { arena };
      pair->begin()->m_type = LX::LangType::Int;
      *pair->last()         = parse_sig(types, arena, idx + 1).second;
      sig.as.m_pair         = *pair;
    }
  }
  else if ("C_void" == types[idx])
  {
    if (idx == types.m_len - 1)
    {
      sig.m_type = LX::LangType::Void;
    }
    else
    {
      sig.m_type            = LangType::Fn;
      UT::Pair<Sig> *pair   = &sig.as.m_pair;
      *pair                 = { arena };
      pair->begin()->m_type = LX::LangType::Void;
      *pair->last()         = parse_sig(types, arena, idx + 1).second;
      sig.as.m_pair         = *pair;
    }
  }
  else if ("C_str" == types[idx])
  {
    if (idx == types.m_len - 1)
    {
      // TODO: The pointer should indicate what it points to
      sig.m_type = LX::LangType::Ptr;
    }
    else
    {
      sig.m_type          = LangType::Fn;
      UT::Pair<Sig> *pair = &sig.as.m_pair;
      *pair               = { arena };
      // TODO: this should also include the type behind the pointer
      pair->begin()->m_type = LX::LangType::Ptr;
      *pair->last()         = parse_sig(types, arena, idx + 1).second;
      sig.as.m_pair         = *pair;
    }
  }
  else
  {
    // TODO: Think how to handle the other cases
  }
  return std::pair{ LX::E::OK, sig };
}

bool
delimits_word(
  char c)
{
  switch (c)
  {
  case ' ':
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
  case '@' : return true;
  default  : return false;
  }
}

} // namespace

bool
Lexer::match_keyword(
  UT::String keyword, UT::String word)
{
  UT_BEGIN_TRACE(this->m_arena,
                 this->m_events,
                 "keyword = %s, word = %s",
                 UT_TCS(keyword),
                 UT_TCS(word));
  bool result = UT::strcompare(keyword, word);
  return result;
}

// TODO: should be comment aware
// FIXME: 'word=' does not work but it should
// FIXME: bug when ignoring comments, see find_next_global_symbol
UT::String
LX::Lexer::get_word(
  size_t idx)
{
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "idx = %d", idx);

  UT::SB sb{};
  this->strip_white_space(idx);
  idx = this->m_cursor;

  for (char c = m_input[idx++]; c && (!delimits_word(c)); c = m_input[idx++])
  {
    sb.add(c);
  }

  UT::String string = sb.to_String(m_arena);
  m_cursor          = idx;

  return string;
}

LX::E
LX::Lexer::find_matching_paren(
  size_t &paren_match_idx)
{
  UT_BEGIN_TRACE(
    this->m_arena, this->m_events, "paren_match_idx = %d", paren_match_idx);
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

      UT_TRACE("Found matching paren at: %d", this->m_cursor);
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

LX::E
Lexer::push_int()
{
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

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
    if (!c || !std::isdigit(c))
    {
      switch (c)
      {
      case '+':
      case '-':
      case '*':
      case '/':
      case '%':
      case '?':
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

    LX::Token t{ LX::Type::Int };
    t.as.m_int = result;
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
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

  LX::Type t_type = LX::Type::Min;
  switch (c)
  {
  case '-': t_type = LX::Type::Minus; break;
  case '+': t_type = LX::Type::Plus; break;
  case '*': t_type = LX::Type::Mult; break;
  case '/': t_type = LX::Type::Div; break;
  case '%': t_type = LX::Type::Modulus; break;
  default : /* UNREACHABLE */ UT_FAIL_IF("UNERACHABLE");
  }
  m_tokens.push(LX::Token{ t_type });
}

// TODO: candidate for refactor
LX::E
Lexer::run()
{
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

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
    case ':':
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
      Token not_token{ LX::Type::Not };
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
      string_token.as.m_string = string;

      m_tokens.push(string_token);
    }
    break;
    case '-':
    {
      char next_c = this->peek_char();
      switch (next_c)
      {
      case ' ':
      case '\t':
      case '\n':
      case '(' : this->push_operator(c); break;
      case '\0':
      {
        // TODO: There should be better error message reporting
        LX_ERROR_REPORT(
          E::UNREACHABLE_CASE_REACHED,
          "Expression starting with '-' should be followed by a variable, "
          "literal or parenthesis but expression ends unexpectedly");
      }
      default:
      {
        if (std::isalpha(next_c))
        {
          if (std::islower(next_c))
          {
            this->push_operator(c);
          }
          else
          {
            LX_ASSERT(false, E::UNRECOGNIZED_STRING);
          }
        }
        else if (std::isdigit(next_c))
        {
          this->push_int();
        }
        else
        {
          LX_ASSERT(false, E::OPERATOR_MATCH_FAILURE);
        }
      }
      break;
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

      LX::Lexer new_l = LX::Lexer(*this, group_begin, group_end);
      LX_FN_TRY(new_l.run());

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

      Lexer body_lexer{
        this->m_input, this->m_arena, this->m_cursor, this->m_end
      };
      LX::E e = body_lexer.run();
      LX_ASSERT(LX::E::OK == e || LX::E::IN_KEYWORD == e,
                LX::E::CONTROL_STRUCTURE_ERROR);

      Token fn{};
      fn.m_type             = Type::Fn;
      fn.m_line             = this->m_lines;
      fn.m_cursor           = this->m_cursor;
      fn.as.m_fn.m_var_name = var_name;
      fn.as.m_fn.m_body     = body_lexer.m_tokens;

      this->m_tokens.push(fn);
      this->skip_to(body_lexer);

      return e;
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
      UT::String word = this->get_word(
        this->m_cursor - 1); // we already got the first char so go back 1

      if (this->match_keyword(LX::Keyword::IN, word))
      {
        return LX::E::IN_KEYWORD;
      }
      else if (this->match_keyword(LX::Keyword::ELSE, word))
      {
        return LX::E::ELSE_KEYWORD;
      }
      else if (this->match_keyword(LX::Keyword::INT, word)
               || this->match_keyword(LX::Keyword::PUB, word))
      {
        size_t next_symbol_idx;
        E      e = this->find_next_global_symbol(next_symbol_idx);
        if (E::WORD_NOT_FOUND == e) next_symbol_idx = this->m_end;

        UT::String sym_name = this->get_word(this->m_cursor);

        // TODO: use a different error
        LX_ASSERT("" != sym_name, E::WORD_NOT_FOUND);

        LX_FN_TRY(this->match_operator('='));

        Lexer new_lexer{ m_input, m_arena, m_cursor, next_symbol_idx };
        LX_FN_TRY(new_lexer.run());

        // TODO: candidate for refactor
        Token symbol{ "int" == word ? Type::IntDef : Type::PubDef };
        symbol.m_cursor            = new_lexer.m_cursor;
        symbol.m_line              = new_lexer.m_lines;
        symbol.as.m_sym.m_def      = new_lexer.m_tokens;
        symbol.as.m_sym.m_sym_name = sym_name;

        this->m_tokens.push(symbol);
        this->skip_to(new_lexer);
        this->m_cursor = next_symbol_idx;
      }
      else if (this->match_keyword(LX::Keyword::LET, word))
      {
        UT::String var_name = this->get_word(this->m_cursor);

        LX_FN_TRY(this->match_operator('='));

        Lexer let_lexer{
          this->m_input, this->m_arena, this->m_cursor, this->m_end
        };
        LX_ASSERT(E::IN_KEYWORD == let_lexer.run(), E::CONTROL_STRUCTURE_ERROR);

        Lexer in_lexer{
          let_lexer.m_input, let_lexer.m_arena, let_lexer.m_cursor, this->m_end
        };
        LX_FN_TRY(in_lexer.run());

        // TODO: Token should have an end
        Token token{ Type::Let };
        token.m_line                       = this->m_lines;
        token.m_cursor                     = this->m_cursor;
        token.as.m_let_tokens.m_var_name   = var_name;
        token.as.m_let_tokens.m_let_tokens = let_lexer.m_tokens;
        token.as.m_let_tokens.m_in_tokens  = in_lexer.m_tokens;

        this->m_tokens.push(token);
        this->skip_to(in_lexer);
      }
      else if (this->match_keyword(LX::Keyword::IF, word))
      {
        Lexer if_condition_lexer{
          this->m_input, this->m_arena, this->m_cursor, this->m_end
        };
        LX_ASSERT(E::FAT_ARROW == if_condition_lexer.run(),
                  E::OPERATOR_MATCH_FAILURE);

        Lexer true_branch_lexer{ if_condition_lexer.m_input,
                                 if_condition_lexer.m_arena,
                                 if_condition_lexer.m_cursor,
                                 this->m_end };
        LX_ASSERT(E::ELSE_KEYWORD == true_branch_lexer.run(),
                  E::CONTROL_STRUCTURE_ERROR);

        Lexer else_branch_lexer{ true_branch_lexer.m_input,
                                 true_branch_lexer.m_arena,
                                 true_branch_lexer.m_cursor,
                                 this->m_end };
        LX::E e = else_branch_lexer.run();
        LX_ASSERT(LX::E::OK == e || LX::E::IN_KEYWORD == e,
                  LX::E::CONTROL_STRUCTURE_ERROR);

        // TODO: candidate for refactor
        Token token{ Type::If };
        token.as.m_if_tokens.m_condition   = if_condition_lexer.m_tokens;
        token.as.m_if_tokens.m_true_branch = true_branch_lexer.m_tokens;
        token.as.m_if_tokens.m_else_branch = else_branch_lexer.m_tokens;

        this->m_tokens.push(token);
        this->skip_to(else_branch_lexer);

        UT_TRACE("If expression tokenized: %s", UT_TCS(token));
        if (E::IN_KEYWORD == e) return e;
      }
      else if (this->match_keyword(LX::Keyword::WHILE, word))
      {
        Lexer condition_lexer{
          this->m_input, this->m_arena, this->m_cursor, this->m_end
        };
        LX_ASSERT(E::FAT_ARROW == condition_lexer.run(),
                  E::OPERATOR_MATCH_FAILURE);

        Lexer body_lexer{ condition_lexer.m_input,
                          condition_lexer.m_arena,
                          condition_lexer.m_cursor,
                          this->m_end };
        LX::E e = body_lexer.run();
        LX_ASSERT(e == E::ELSE_KEYWORD || e == E::IN_KEYWORD || e == E::OK,
                  E::CONTROL_STRUCTURE_ERROR);

        // TODO: candidate for refactor
        Token token{ Type::While };
        token.as.m_while.m_condition = condition_lexer.m_tokens;
        token.as.m_while.m_body      = body_lexer.m_tokens;

        this->m_tokens.push(token);
        this->skip_to(body_lexer);

        UT_TRACE("While expression tokenized: %s", UT_TCS(token));
        if (E::IN_KEYWORD == e || e == E::ELSE_KEYWORD) return e;
      }
      else if (this->match_keyword(LX::Keyword::EXT, word))
      {
        size_t next_symbol_idx;
        E      e = this->find_next_global_symbol(next_symbol_idx);
        if (E::WORD_NOT_FOUND == e) next_symbol_idx = this->m_end;

        UT::String sym_name = this->get_word(this->m_cursor);

        LX_ASSERT("" != sym_name,
                  E::WORD_NOT_FOUND); // TODO: use a different error

        LX_FN_TRY(this->match_operator(':'));

        Lexer sig_lexer{ m_input, m_arena, m_cursor, next_symbol_idx };
        UT::Vec<UT::String> types{ m_arena };
        UT::String          type{};
        e = E::OK;

        while (E::OK == e)
        {
          type = sig_lexer.get_word(sig_lexer.m_cursor);
          LX_ASSERT("" != type, E::UNRECOGNIZED_STRING);
          types.push(type);
          e = sig_lexer.match_operator("->");
        }

        auto parse_result = parse_sig(types, m_arena, 0);

        // TODO : Use better error
        LX_ASSERT(E::OK == parse_result.first, E::CONTROL_STRUCTURE_ERROR);
        Sig sig = parse_result.second;

        // TODO: parse this
        // LX_FN_TRY(sig_lexer.match_operator('='));

        // UT_FAIL_IF("here");

        sig_lexer.run();
        Tokens sym_defs{ m_arena };
        sym_defs.push(sig_lexer.m_tokens.last()->as.m_tokens[0]);
        sym_defs.push(sig_lexer.m_tokens.last()->as.m_tokens[1]);

        Token symbol{ Type::ExtDef };
        symbol.as.m_ext_sym.m_name = sym_name;
        symbol.as.m_ext_sym.m_sig  = sig;
        symbol.as.m_ext_sym.m_def  = sym_defs;

        // UT_VAR_INSP(symbol);
        m_cursor = next_symbol_idx;
        m_lines += sig_lexer.m_lines;


        m_tokens.push(symbol);
      }
      else
      {
        LX_ASSERT(word.m_len > 0, LX::E::UNRECOGNIZED_STRING);
        LX::Token t{ LX::Type::Word };
        t.as.m_string = word;
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
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

  for (auto t : l.m_tokens)
  {
    LX::Token token{ t };
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
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

  this->strip_white_space(this->m_cursor - 1);
  LX_ASSERT(c == this->next_char(), E::UNRECOGNIZED_STRING);

  UT_TRACE("Successfully matched operator %c", c);
  return E::OK;
};

E
Lexer::match_operator(
  UT::String s)
{
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

  this->strip_white_space(this->m_cursor - 1);
  for (size_t idx = 0; idx < s.m_len; ++idx)
  {
    LX_ASSERT(s[idx] == this->next_char(), E::UNRECOGNIZED_STRING);
  }

  UT_TRACE("Successfully matched operator %c", c);
  return E::OK;
}

// TODO: candidate for refactor
void
Lexer::strip_white_space(
  size_t idx)
{
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "idx = %d", idx);

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
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "idx = %d", idx);

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
  UT_BEGIN_TRACE(this->m_arena, this->m_events, "{}", 0);

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

} // namespace LX
