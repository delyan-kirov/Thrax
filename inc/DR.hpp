/*-------------------------------------------------------------------------------
 *\file DR.hpp
 *\info Header file for the Driver (pipeline orchestration)
 * *----------------------------------------------------------------------------*/

#ifndef DR_HEADER
#define DR_HEADER

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/

#include "EX.hpp"
#include "IT.hpp"
#include "LX.hpp"

namespace DR
{

// Lex one top-level definition from an existing lexer
LX::E lex_sym(LX::Lexer &lexer, LX::Token &out);

// Lex an entire file into tokens
// out_content receives file contents (needed by parse functions)
LX::E lex_file(UT::String file, AR::Arena &arena, LX::Tokens &out_tokens,
               UT::String &out_content);

// Parse one top-level definition token into an expression
EX::E parse_sym(LX::Token &token, AR::Arena &arena, UT::String input,
                EX::Expr &out);

// Parse all tokens into expressions
EX::E parse_tokens(LX::Tokens &tokens, AR::Arena &arena, UT::String input,
                   UT::Vec<EX::Expr> &out);

// Full pipeline: lex -> parse -> interpret
IT::StatEnv interpret_file(UT::String file);

} // namespace DR

#endif // DR_HEADER
