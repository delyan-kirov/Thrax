/*-------------------------------------------------------------------------------
 *\file EX.hpp
 *\info Header file for the Parser.
 *
 * EX is a Pratt (precedence-climbing) parser that pulls flat tokens from a
 * streaming LX::Lexer and builds the Expr tree below. Operator precedence and
 * structure (groups, let, if, lambda) interleave through one recursion, so the
 * C++ call stack mirrors the grammar -- which is what lets errors carry a
 * meaningful context chain (see ER::Diagnostic). The Expr types are unchanged
 * from before; only the parsing engine is new.
 *-----------------------------------------------------------------------------*/

#ifndef EX_HEADER_
#define EX_HEADER_

#include "ER.hpp"
#include "EXxDATA.hpp"
#include "LX.hpp"
#include "UT.hpp"

namespace EX
{

/*-------------------------------------------------------------------------------
 *\PARSER
 *------------------------------------------------------------------------------*/

class Parser
{
public:
  AR::Arena  &m_arena;
  LX::Lexer  &m_lex;
  Exprs       m_exprs;
  Diagnostics m_diags;

  Parser(LX::Lexer &lex);

  void operator()();

private:
  UT_NODISCARD RExpr    parse_global();
  UT_NODISCARD RExpr    parse_mod_decl(); // `@mod NAME` at the start of a file
  UT_NODISCARD RExpr    parse_import();   // `$ with ...`
  UT_NODISCARD RExpr    parse_vis();      // `$ @private` / `$ @public`
  UT_NODISCARD RExpr    parse_operator_def(); // `$ @operator.{<op>} : ty = e`
  UT_NODISCARD RExpr    parse_array();        // `@array.{ size }` expression
  UT_NODISCARD RExpr    parse_extern();
  UT_NODISCARD RExpr    parse_struct_decl(const LX::Token &name);
  UT_NODISCARD RExpr    parse_struct_lit(UT::Vu type_name);
  UT_NODISCARD RExpr    parse_union_decl(const LX::Token &name);
  UT_NODISCARD RExpr    parse_alias_decl(const LX::Token &name);
  UT_NODISCARD RExpr    parse_effect_decl(const LX::Token &name);
  UT_NODISCARD RExpr    parse_variant_lit(UT::Vu           type_name,
                                          UT::Vu           tag,
                                          const LX::Token &tok);
  UT_NODISCARD RTy      parse_type();
  UT_NODISCARD RTy      parse_type_app();
  UT_NODISCARD RTy      parse_type_atom();
  UT_NODISCARD RExpr    parse_expr(int min_bp);
  UT_NODISCARD RExpr    parse_prefix();
  UT_NODISCARD RExpr    parse_primary();
  UT_NODISCARD RExpr    parse_group();
  UT_NODISCARD RExpr    parse_let();
  UT_NODISCARD RExpr    parse_if();
  UT_NODISCARD RExpr    parse_handle();
  UT_NODISCARD RExpr    parse_closure();
  UT_NODISCARD RPattern parse_pattern();
  // Parses a qualified pattern after an uppercase type name: dispatches to a
  // `Type.{ ... }` struct pattern or a `Type.Tag[.{ ... }]` variant pattern.
  UT_NODISCARD RPattern parse_struct_pattern(UT::Vu           type_name,
                                             const LX::Token &tn);
  // Parses the `{ field-patterns }` body shared by struct patterns and variant
  // payloads. Assumes the opening `{` is the next token.
  UT_NODISCARD ER::Result<UT::Vec<FieldPat>>
               parse_field_pats(UT::Vu type_name, const LX::Token &tn);
  UT_NODISCARD LX::RToken expect(LX::TokenTag tag, const char *what);
  void                    recover();

  UT_NODISCARD Expr    *alloc(Expr e);
  UT_NODISCARD Expr    *mk_int(const LX::Token &t);
  UT_NODISCARD Expr    *mk_real(const LX::Token &t);
  UT_NODISCARD Expr    *mk_str(const LX::Token &t);
  UT_NODISCARD Expr    *mk_var(const LX::Token &t);
  UT_NODISCARD Expr    *mk_app(Expr *fn, Expr *arg);
  UT_NODISCARD Expr    *mk_op_var(UT::Vu name);
  UT_NODISCARD Expr    *mk_unop(UT::Vu op, Expr *operand);
  UT_NODISCARD Expr    *mk_binop(UT::Vu op, Expr *lhs, Expr *rhs);
  UT_NODISCARD Expr    *mk_seq(Expr *lhs, Expr *rhs);
  UT_NODISCARD Expr    *mk_if(Expr *cond, Expr *then, Expr *alt);
  UT_NODISCARD Expr    *mk_let(UT::Vu var, Expr *val, Expr *body);
  UT_NODISCARD Expr    *mk_fndef(UT::Vu param, Expr *body);
  UT_NODISCARD Expr    *mk_def(UT::Vu name, Ty *sig, Expr *def);
  UT_NODISCARD Expr    *mk_extern(UT::Vu symbol, UT::Vu lib);
  UT_NODISCARD Expr    *mk_field(Expr *record, UT::Vu field);
  UT_NODISCARD Ty      *mk_ty(Ty t);
  UT_NODISCARD Pattern *alloc_pat(Pattern p);
};

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string pprint(ExprTag t, int level = 0);
std::string pprint(Expr *e, int level = 0);
std::string pprint(Pattern *p);

} // namespace EX

#endif // EX_HEADER_
