/*-------------------------------------------------------------------------------
 *\file EX.cpp
 *\info Pratt parser implementation.
 * *----------------------------------------------------------------------------*/

#include "EX.hpp"
#include "ER.hpp"
#include "EXxDATA.hpp"
#include "LX.hpp"
#include "UT.hpp"

namespace EX
{
/*-------------------------------------------------------------------------------
 *\NODE BUILDERS
 *------------------------------------------------------------------------------*/

Expr *
Parser::alloc(
  Expr e)
{
  Expr *p = (Expr *)m_arena.alloc<Expr>(1);
  *p      = e;
  return p;
}

Pattern *
Parser::alloc_pat(
  Pattern p)
{
  Pattern *q = (Pattern *)m_arena.alloc<Pattern>(1);
  *q         = p;
  return q;
}

Expr *
Parser::mk_int(
  const LX::Token &t)
{
  Expr e{ ExprTag::Int };
  e.as = ExInt{ std::get<LX::TkInt>(t.as).value };
  return alloc(e);
}

Expr *
Parser::mk_real(
  const LX::Token &t)
{
  Expr e{ ExprTag::Real };
  e.as = ExReal{ std::get<LX::TkReal>(t.as).value };
  return alloc(e);
}

Expr *
Parser::mk_str(
  const LX::Token &t)
{
  Expr e{ ExprTag::Str };
  e.as = ExStr{ std::get<LX::TkStr>(t.as).value };
  return alloc(e);
}

Expr *
Parser::mk_var(
  const LX::Token &t)
{
  Expr e{ ExprTag::Var };
  e.as = ExVar{ t.str };
  return alloc(e);
}

Expr *
Parser::mk_app(
  Expr *fn, Expr *arg)
{
  Expr e{ ExprTag::App };
  e.as = ExApp{ fn, arg };
  return alloc(e);
}

// An operator reference, as a plain variable of its canonical name. Operators
// are just functions with infix/prefix syntax: once parsed they are ordinary
// Var/App nodes, and TC/IT never need to know they came from an operator.
Expr *
Parser::mk_op_var(
  UT::Vu name)
{
  Expr e{ ExprTag::Var };
  e.as = ExVar{ name };
  return alloc(e);
}

// Prefix `op operand`  ==>  (op operand).
Expr *
Parser::mk_unop(
  UT::Vu op, Expr *operand)
{
  return mk_app(mk_op_var(op), operand);
}

// Infix `lhs op rhs`  ==>  ((op lhs) rhs).
Expr *
Parser::mk_binop(
  UT::Vu op, Expr *lhs, Expr *rhs)
{
  return mk_app(mk_app(mk_op_var(op), lhs), rhs);
}

Expr *
Parser::mk_if(
  Expr *cond, Expr *then, Expr *alt)
{
  Expr e{ ExprTag::If };
  e.as = ExIf{ cond, then, alt };
  return alloc(e);
}

Expr *
Parser::mk_let(
  UT::Vu var, Expr *val, Expr *body)
{
  Expr e{ ExprTag::Let };
  e.as = ExLet{ var, val, body };
  return alloc(e);
}

Expr *
Parser::mk_fndef(
  UT::Vu param, Expr *body)
{
  ExFnDef fn;
  fn.param = param;
  fn.body  = body;
  Expr e{ ExprTag::FnDef };
  e.as = fn;
  return alloc(e);
}

Expr *
Parser::mk_def(
  UT::Vu name, Ty *sig, Expr *def)
{
  Expr e{ ExprTag::Def };
  e.as = ExDef{ name, sig, def };
  return alloc(e);
}

Ty *
Parser::mk_ty(
  Ty t)
{
  Ty *p = (Ty *)m_arena.alloc<Ty>(1);
  *p    = t;
  return p;
}

Expr *
Parser::mk_extern(
  UT::Vu symbol, UT::Vu lib)
{
  Expr e{ ExprTag::Extern };
  e.as = ExExtern{ symbol, lib };
  return alloc(e);
}

Expr *
Parser::mk_field(
  Expr *record, UT::Vu field)
{
  Expr e{ ExprTag::Field };
  e.as = ExField{ record, field };
  return alloc(e);
}

/*-------------------------------------------------------------------------------
 *\PARSE
 *------------------------------------------------------------------------------*/

LX::RToken
Parser::expect(
  LX::TokenTag tag, const char *what)
{
  LX::Token t = EX_TRY(m_lex.peek());
  if (t.tag != tag)
  {
    EX_ERR(ER::Code::UNEXPECTED_TOKEN, t, "%s", what);
  }
  LX::Token consumed = EX_TRY(m_lex.next());
  return { true, consumed, {} };
}

RExpr
Parser::parse_primary()
{
  LX::Token t    = EX_TRY(m_lex.peek());
  Expr     *base = nullptr;
  switch (t.tag)
  {
  case LX::TokenTag::Int:
    m_lex.next();
    base = mk_int(t);
    break;
  case LX::TokenTag::Real:
    m_lex.next();
    base = mk_real(t);
    break;
  case LX::TokenTag::Str:
    m_lex.next();
    base = mk_str(t);
    break;
  case LX::TokenTag::Word:
    m_lex.next();
    base = mk_var(t);
    break;
  case LX::TokenTag::LParen: base = EX_TRY(parse_group()); break;
  case LX::TokenTag::KwLet : base = EX_TRY(parse_let()); break;
  case LX::TokenTag::KwIf  : base = EX_TRY(parse_if()); break;
  case LX::TokenTag::Lambda: base = EX_TRY(parse_closure()); break;
  default:
  {
    const char *desc = tok_desc(t);
    if (desc)
      EX_ERR(ER::Code::EXPECTED_OPERAND,
             t,
             "expected an expression, found %s",
             desc);
    EX_ERR(ER::Code::EXPECTED_OPERAND,
           t,
           "expected an expression, found '%s'",
           std::string(t.str).c_str());
  }
  }

  // Postfix `.`: a struct literal `Type.{...}` or field access `record.field`.
  // Both bind tighter than application, left-associative, so `a.b.c` chains and
  // `f x.y` is `f (x.y)`.
  for (;;)
  {
    if (LX::TokenTag::Dot != EX_TRY(m_lex.peek()).tag) break;
    LX::Token dot   = EX_TRY(m_lex.next()); // '.'
    LX::Token ahead = EX_TRY(m_lex.peek());

    if (LX::TokenTag::LBrace == ahead.tag)
    {
      // `Type.{ ... }` -- the base must be a bare, uppercase-initial type name.
      if (base->tag != ExprTag::Var)
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               dot,
               "a struct literal must be qualified with a type name, e.g. "
               "Type.{ ... }");
      UT::Vu tn = std::get<ExVar>(base->as).name;
      char   c  = tn.size() ? tn.data()[0] : '\0';
      if (c < 'A' || c > 'Z')
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               dot,
               "a struct literal's type name must start uppercase, found '%s'",
               std::string(tn).c_str());
      base = EX_CTX(parse_struct_lit(tn),
                    dot,
                    "in this '%s' struct literal",
                    std::string(tn).c_str());
    }
    else
    {
      LX::Token fld = EX_TRY(expect(
        LX::TokenTag::Word, "expected a field name or variant tag after '.'"));
      char      c   = fld.str.size() ? fld.str.data()[0] : '\0';
      if (c >= 'A' && c <= 'Z')
      {
        // `Type.Tag` -- a variant constructor; the base must be a bare type
        // name. An optional `.{ ... }` payload follows (see parse_variant_lit).
        if (base->tag != ExprTag::Var)
          EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                 dot,
                 "a variant constructor must be qualified with a type name, "
                 "e.g. Type.Tag");
        UT::Vu tn = std::get<ExVar>(base->as).name;
        char   bc = tn.size() ? tn.data()[0] : '\0';
        if (bc < 'A' || bc > 'Z')
          EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                 dot,
                 "a variant constructor's type name must start uppercase, "
                 "found '%s'",
                 std::string(tn).c_str());
        base = EX_CTX(parse_variant_lit(tn, fld.str, fld),
                      dot,
                      "in this '%s.%s' variant",
                      std::string(tn).c_str(),
                      std::string(fld.str).c_str());
      }
      else
      {
        base = mk_field(base, fld.str); // `record.field`
      }
    }
  }

  return { true, base, {} };
}

RExpr
Parser::parse_prefix()
{
  LX::Token t = EX_TRY(m_lex.peek());
  if (LX::TokenTag::Op == t.tag)
  {
    if (const char *const *name = UT::try_lookup(unop_db, std::string(t.str)))
    {
      m_lex.next();
      Expr *operand = EX_TRY(parse_expr(PREFIX_BP));
      return { true,
               mk_unop(UT::Vu{ *name, std::strlen(*name) }, operand),
               {} };
    }
  }
  return parse_primary();
}

RExpr
Parser::parse_expr(
  int min_bp)
{
  Expr *lhs = EX_TRY(parse_prefix());

  for (;;)
  {
    LX::Token t = EX_TRY(m_lex.peek());

    if (LX::TokenTag::Op == t.tag)
    {
      const Bp *bp = UT::try_lookup(infix_db, std::string(t.str));
      if (!bp)
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               t,
               "operator '%s' cannot be used here",
               std::string(t.str).c_str());
      if (bp->l < min_bp) break;
      EX_TRY(m_lex.next()); // consume the operator
      Expr *rhs = EX_TRY(parse_expr(bp->r));
      lhs       = mk_binop(t.str, lhs, rhs);
    }
    else if (operand_starters.count(t.tag))
    {
      if (APP_BP.l < min_bp) break;
      Expr *rhs = EX_TRY(parse_expr(APP_BP.r));
      lhs       = mk_app(lhs, rhs);
    }
    else if (expr_terminators.count(t.tag))
    {
      break;
    }
    else
    {
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             t,
             "Unexpected symbol '%s'",
             std::string(t.str).c_str());
    }
  }

  return { true, lhs, {} };
}

RExpr
Parser::parse_group()
{
  LX::Token lp = EX_TRY(m_lex.next()); // '('
  Expr     *e  = EX_CTX(parse_expr(0), lp, "in this parenthesized group");

  LX::Token t = EX_TRY(m_lex.peek());
  if (LX::TokenTag::RParen != t.tag)
  {
    EX_ERR(ER::Code::PARENTHESIS_UNBALANCED,
           lp,
           "expected a closing ')' to match this '('");
  }
  m_lex.next();
  return { true, e, {} };
}

RExpr
Parser::parse_let()
{
  LX::Token kw = EX_TRY(m_lex.next()); // 'let'

  // A `let` binder is either a plain variable (`let x = ...`) or a structural
  // pattern (`let Person.{x,y} = ...`). An uppercase-initial word starts a
  // pattern; the LL pass lowers it into nested plain lets.
  LX::Token la     = EX_TRY(m_lex.peek());
  bool      is_pat = false;
  if (LX::TokenTag::Word == la.tag)
  {
    char c = la.str.size() ? la.str.data()[0] : '\0';
    is_pat = (c >= 'A' && c <= 'Z');
  }

  if (is_pat)
  {
    Pattern *pat = EX_TRY(parse_pattern());
    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the 'let' pattern"));
    Expr *val = EX_CTX(parse_expr(0), la, "in this 'let' binding");
    EX_TRY(expect(LX::TokenTag::KwIn, "expected 'in' after the 'let' binding"));
    Expr *body = EX_CTX(parse_expr(0), kw, "in the body of this 'let'");

    ExLet lt;
    lt.var  = UT::Vu{};
    lt.val  = val;
    lt.body = body;
    lt.pat  = pat;
    Expr e{ ExprTag::Let };
    e.as = lt;
    return { true, alloc(e), {} };
  }

  LX::Token name = EX_TRY(
    expect(LX::TokenTag::Word, "expected a variable name after 'let'"));
  EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the 'let' variable"));

  Expr *val = EX_CTX(parse_expr(0),
                     name,
                     "in the binding of 'let %s'",
                     std::string(name.str).c_str());

  EX_TRY(expect(LX::TokenTag::KwIn, "expected 'in' after the 'let' binding"));

  Expr *body = EX_CTX(parse_expr(0), kw, "in the body of this 'let'");
  return { true, mk_let(name.str, val, body), {} };
}

RExpr
Parser::parse_if()
{
  LX::Token kw   = EX_TRY(m_lex.next()); // 'if'
  Expr     *cond = EX_CTX(parse_expr(0), kw, "in the condition of this 'if'");

  // `if scrut is pat then e ... else d` is a match; `if c then t else e` is the
  // boolean conditional. The `is` after the scrutinee chooses between them.
  if (LX::TokenTag::KwIs == EX_TRY(m_lex.peek()).tag)
  {
    UT::Vec<MatchArm> arms{ m_arena };
    while (LX::TokenTag::KwIs == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // 'is'
      Pattern *pat = EX_TRY(parse_pattern());
      EX_TRY(expect(LX::TokenTag::KwThen,
                    "expected 'then' after the match pattern"));
      Expr *body = EX_CTX(parse_expr(0), kw, "in this match arm");
      arms.push(MatchArm{ pat, body });
    }
    LX::Token els
      = EX_TRY(expect(LX::TokenTag::KwElse,
                      "expected another 'is' arm or 'else' to close "
                      "the match"));
    Expr *alt = EX_CTX(parse_expr(0), els, "in the else branch of this match");

    Expr e{ ExprTag::Match };
    e.as = ExMatch{ cond, arms, alt };
    return { true, alloc(e), {} };
  }

  EX_TRY(
    expect(LX::TokenTag::KwThen, "expected 'then' after the 'if' condition"));

  Expr *then = EX_CTX(parse_expr(0), kw, "in the then-branch of this 'if'");

  LX::Token els = EX_TRY(
    expect(LX::TokenTag::KwElse, "expected 'else' after the then-branch"));

  Expr *alt = EX_CTX(parse_expr(0), els, "in the else branch of this 'if'");
  return { true, mk_if(cond, then, alt), {} };
}

// A single pattern: `_`, a variable, a literal, or a `Type.{ ... }` struct
// pattern. Literals and literal-bearing struct patterns are refutable; the LL
// pass rejects them where only irrefutable patterns are allowed (lambda / let).
RPattern
Parser::parse_pattern()
{
  LX::Token t = EX_TRY(m_lex.peek());
  switch (t.tag)
  {
  case LX::TokenTag::Int:
    m_lex.next();
    return { true,
             alloc_pat(Pattern{
               PatTag::Int,
               PatInt{ std::get<LX::TkInt>(t.as).value, t.str, t.line } }),
             {} };
  case LX::TokenTag::Real:
    m_lex.next();
    return { true,
             alloc_pat(Pattern{
               PatTag::Real,
               PatReal{ std::get<LX::TkReal>(t.as).value, t.str, t.line } }),
             {} };
  case LX::TokenTag::Str:
    m_lex.next();
    return { true,
             alloc_pat(Pattern{
               PatTag::Str,
               PatStr{ std::get<LX::TkStr>(t.as).value, t.str, t.line } }),
             {} };
  case LX::TokenTag::Word:
  {
    m_lex.next();
    UT::Vu nm = t.str;
    if (nm.size() == 1 && nm.data()[0] == '_')
      return { true, alloc_pat(Pattern{ PatTag::Wild, PatWild{} }), {} };
    char c = nm.size() ? nm.data()[0] : '\0';
    if (c >= 'A' && c <= 'Z') return parse_struct_pattern(nm, t);
    return { true, alloc_pat(Pattern{ PatTag::Var, PatVar{ nm } }), {} };
  }
  default:
  {
    const char *desc = tok_desc(t);
    if (desc)
      EX_ERR(
        ER::Code::EXPECTED_OPERAND, t, "expected a pattern, found %s", desc);
    EX_ERR(ER::Code::EXPECTED_OPERAND,
           t,
           "expected a pattern, found '%s'",
           std::string(t.str).c_str());
  }
  }
}

// `{ field-patterns }`. A field-pattern is either `.name = subpat` / `.name`
// (named, the bare form punning `name` == `name = name`) or a bare positional
// sub-pattern. All-named or all-positional, not a mix. Shared by struct
// patterns and variant payloads; assumes the opening `{` is next. Mode/arity
// validation against the declaration happens in LL.
ER::Result<UT::Vec<FieldPat>>
Parser::parse_field_pats(
  UT::Vu type_name, const LX::Token &tn)
{
  EX_TRY(expect(LX::TokenTag::LBrace, "expected '{' to open the pattern"));

  UT::Vec<FieldPat> fields{ m_arena };
  bool              any_named = false;

  for (;;)
  {
    if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break;

    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
    {
      // A named field: `.field = subpat`, or `.field` punning to `.field =
      // field`.
      m_lex.next(); // '.'
      LX::Token fname
        = EX_TRY(expect(LX::TokenTag::Word, "expected a field name after '.'"));
      char c = fname.str.size() ? fname.str.data()[0] : '\0';
      if (c < 'a' || c > 'z')
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               fname,
               "a field name must start lowercase, found '%s'",
               std::string(fname.str).c_str());

      Pattern *sub;
      if (LX::TokenTag::Eq == EX_TRY(m_lex.peek()).tag)
      {
        m_lex.next(); // '='
        sub = EX_TRY(parse_pattern());
      }
      else
        sub = alloc_pat(Pattern{ PatTag::Var, PatVar{ fname.str } }); // pun
      fields.push(FieldPat{ fname.str, sub });
      any_named = true;
    }
    else
    {
      Pattern *sub = EX_TRY(parse_pattern());
      fields.push(FieldPat{ UT::Vu{}, sub });
    }

    if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
    m_lex.next(); // ','
  }
  EX_TRY(expect(LX::TokenTag::RBrace, "expected '}' to close the pattern"));

  // All-named (`.field`) or all-positional, not a mix.
  if (any_named)
    for (size_t i = 0; i < fields.size(); ++i)
      if (!fields[i].name.size())
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               tn,
               "pattern '%s' mixes named ('.field') and positional "
               "entries; use one or the other",
               std::string(type_name).c_str());

  return { true, fields, {} };
}

// A qualified pattern, the type name already consumed. `Type.{ ... }` is a
// struct pattern; `Type.Tag` / `Type.Tag.{ ... }` (uppercase tag) is a variant
// pattern.
RPattern
Parser::parse_struct_pattern(
  UT::Vu type_name, const LX::Token &tn)
{
  EX_TRY(expect(LX::TokenTag::Dot,
                "expected '.{' or '.Tag' after a pattern's type name"));

  LX::Token ahead = EX_TRY(m_lex.peek());
  if (LX::TokenTag::Word == ahead.tag)
  {
    char c = ahead.str.size() ? ahead.str.data()[0] : '\0';
    if (c < 'A' || c > 'Z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             ahead,
             "a variant tag must start uppercase, found '%s'",
             std::string(ahead.str).c_str());
    m_lex.next(); // tag
    UT::Vu tag = ahead.str;

    // An optional `.{ ... }` payload; its absence means a unit payload.
    UT::Vec<FieldPat> fields{ m_arena };
    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
        && LX::TokenTag::LBrace == EX_TRY(m_lex.peek(1)).tag)
    {
      m_lex.next(); // '.'
      fields = EX_TRY(parse_field_pats(type_name, tn));
    }
    Pattern pat{ PatTag::Variant,
                 PatVariant{ type_name, tag, fields, tn.str, tn.line } };
    return { true, alloc_pat(pat), {} };
  }

  UT::Vec<FieldPat> fields = EX_TRY(parse_field_pats(type_name, tn));
  Pattern           pat{ PatTag::Struct,
               PatStruct{ type_name, fields, tn.str, tn.line } };
  return { true, alloc_pat(pat), {} };
}

RExpr
Parser::parse_closure()
{
  LX::Token bs = EX_TRY(m_lex.next()); // '\'

  // `\p1 p2 .. pn = e` binds one pattern per parameter; the `=` ends the list.
  UT::Vec<Pattern *> params{ m_arena };
  params.push(EX_TRY(parse_pattern()));
  for (LX::Token nxt = EX_TRY(m_lex.peek()); LX::TokenTag::Word == nxt.tag;
       nxt           = EX_TRY(m_lex.peek()))
    params.push(EX_TRY(parse_pattern()));

  EX_TRY(
    expect(LX::TokenTag::Eq,
           "expected '=' or another parameter after the lambda parameter"));

  Expr *body = EX_CTX(parse_expr(0), bs, "while parsing this closure");

  // Curried sugar: `\p1 p2 .. pn = e` desugars to `\p1 = \p2 = .. = e`. A plain
  // variable/wildcard parameter stays an ordinary binder; a structural pattern
  // is carried on `param_pat` for the LL pass to lower.
  for (size_t i = params.size(); i-- > 0;)
  {
    Pattern *p = params[i];
    if (PatTag::Var == p->tag)
      body = mk_fndef(std::get<PatVar>(p->as).name, body);
    else if (PatTag::Wild == p->tag)
      body = mk_fndef(UT::Vu{ "_", 1 }, body);
    else
    {
      ExFnDef fn;
      fn.param     = UT::Vu{};
      fn.body      = body;
      fn.param_pat = p;
      Expr e{ ExprTag::FnDef };
      e.as = fn;
      body = alloc(e);
    }
  }
  return { true, body, {} };
}

RExpr
Parser::parse_global()
{
  LX::Token marker = EX_TRY(m_lex.peek());

  if (LX::TokenTag::KwExt == marker.tag)
    EX_ERR(
      ER::Code::UNSUPPORTED, marker, "'ext' definitions are not supported yet");

  if (LX::TokenTag::Dollar != marker.tag)
  {
    const char *desc = tok_desc(marker);
    if (desc)
      EX_ERR(ER::Code::EXPECTED_GLOBAL,
             marker,
             "expected a global definition (starting with '$'), found %s",
             desc);
    EX_ERR(ER::Code::EXPECTED_GLOBAL,
           marker,
           "expected a global definition (starting with '$'), found '%s'",
           std::string(marker.str).c_str());
  }
  m_lex.next(); // consume '$'

  LX::Token name
    = EX_TRY(expect(LX::TokenTag::Word, "expected a name after '$'"));

  // Optional type annotation: `$ name : type = ...`. The special annotation
  // `Struct` marks a struct type declaration rather than a value, so the body
  // is a field list, not an expression.
  Ty *sig = nullptr;
  if (LX::TokenTag::Colon == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // ':'
    LX::Token ann = EX_TRY(m_lex.peek());
    if (LX::TokenTag::Word == ann.tag && ann.str == "Struct")
    {
      m_lex.next(); // 'Struct'
      EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after 'Struct'"));
      return parse_struct_decl(name);
    }
    if (LX::TokenTag::Word == ann.tag && ann.str == "Union")
    {
      m_lex.next(); // 'Union'
      EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after 'Union'"));
      return parse_union_decl(name);
    }
    sig = EX_CTX(parse_type(),
                 name,
                 "in the type signature of global '%s'",
                 std::string(name.str).c_str());
  }

  EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the global name"));

  // A `@extern (...)` body is only valid here, and needs a signature to know
  // its foreign call types.
  if (LX::TokenTag::At == EX_TRY(m_lex.peek()).tag)
  {
    if (!sig)
      EX_ERR(ER::Code::EXPECTED_GLOBAL,
             name,
             "extern global '%s' requires a type signature",
             std::string(name.str).c_str());
    Expr *ext = EX_CTX(parse_extern(),
                       name,
                       "in the definition of extern '%s'",
                       std::string(name.str).c_str());
    return { true, mk_def(name.str, sig, ext), {} };
  }

  Expr *body = EX_CTX(parse_expr(0),
                      name,
                      "in the definition of global '%s'",
                      std::string(name.str).c_str());

  return { true, mk_def(name.str, sig, body), {} };
}

RExpr
Parser::parse_extern()
{
  LX::Token at = EX_TRY(m_lex.peek());
  if (at.str != "@extern")
    EX_ERR(ER::Code::UNSUPPORTED,
           at,
           "unknown intrinsic '%s' (only '@extern' is supported)",
           std::string(at.str).c_str());
  m_lex.next(); // '@extern'

  EX_TRY(expect(LX::TokenTag::LParen, "expected '(' after '@extern'"));
  LX::Token sym = EX_TRY(
    expect(LX::TokenTag::Str, "expected a \"symbol-name\" string in @extern"));
  EX_TRY(
    expect(LX::TokenTag::Comma, "expected ',' between symbol and library"));
  LX::Token lib = EX_TRY(
    expect(LX::TokenTag::Str, "expected a \"library\" string in @extern"));
  EX_TRY(expect(LX::TokenTag::RParen, "expected ')' to close @extern"));

  return { true,
           mk_extern(std::get<LX::TkStr>(sym.as).value,
                     std::get<LX::TkStr>(lib.as).value),
           {} };
}

// A struct declaration body: a comma-separated `field : type` list, trailing
// comma allowed, ending at the next global ('$') or end of input. The name
// token (already consumed by parse_global) is the struct's type name.
RExpr
Parser::parse_struct_decl(
  const LX::Token &name)
{
  char nc = name.str.size() ? name.str.data()[0] : '\0';
  if (nc < 'A' || nc > 'Z')
    EX_ERR(ER::Code::EXPECTED_GLOBAL,
           name,
           "a struct type name must start uppercase, found '%s'",
           std::string(name.str).c_str());

  UT::Vec<FieldDecl> fields{ m_arena };
  for (;;)
  {
    LX::TokenTag tag = EX_TRY(m_lex.peek()).tag;
    if (LX::TokenTag::Dollar == tag || LX::TokenTag::Eof == tag) break;

    LX::Token fname
      = EX_TRY(expect(LX::TokenTag::Word, "expected a field name"));
    char fc = fname.str.size() ? fname.str.data()[0] : '\0';
    if (fc >= 'A' && fc <= 'Z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             fname,
             "a struct field name must start lowercase, found '%s'",
             std::string(fname.str).c_str());

    EX_TRY(expect(LX::TokenTag::Colon, "expected ':' after the field name"));
    Ty *ty = EX_CTX(parse_type(),
                    fname,
                    "in the type of field '%s'",
                    std::string(fname.str).c_str());
    fields.push(FieldDecl{ fname.str, ty });

    if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
    m_lex.next(); // ','
  }

  Expr e{ ExprTag::StructDecl };
  e.as = ExStructDecl{ name.str, fields };
  return { true, alloc(e), {} };
}

// A struct literal `Type.{ field = expr, ... }`. Called with the leading `.`
// already consumed and `type_name` resolved from the qualifying type; the next
// token is the opening '{'.
RExpr
Parser::parse_struct_lit(
  UT::Vu type_name)
{
  EX_TRY(
    expect(LX::TokenTag::LBrace, "expected '{' after the struct type name"));

  UT::Vec<FieldInit> fields{ m_arena };
  for (;;)
  {
    if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break;

    EX_TRY(expect(LX::TokenTag::Dot,
                  "expected '.' before the field name (fields "
                  "are written '.field = value')"));
    LX::Token fname
      = EX_TRY(expect(LX::TokenTag::Word, "expected a field name"));
    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the field name"));
    Expr *val = EX_CTX(parse_expr(0),
                       fname,
                       "in the value of field '%s'",
                       std::string(fname.str).c_str());
    fields.push(FieldInit{ fname.str, val });

    if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
    m_lex.next(); // ','
  }

  EX_TRY(
    expect(LX::TokenTag::RBrace, "expected '}' to close the struct literal"));

  Expr e{ ExprTag::StructLit };
  e.as = ExStructLit{ type_name, fields };
  return { true, alloc(e), {} };
}

// A union declaration body: a comma-separated `Tag: payload` list, trailing
// comma allowed, ending at the next global ('$') or end of input. A payload is
// a brace list `{...}` (positional types `{T, U}`, named `{f: T, ...}`, or
// empty
// `{}`) or a bare type `T` (one anonymous field). The name token was consumed
// by parse_global.
RExpr
Parser::parse_union_decl(
  const LX::Token &name)
{
  char nc = name.str.size() ? name.str.data()[0] : '\0';
  if (nc < 'A' || nc > 'Z')
    EX_ERR(ER::Code::EXPECTED_GLOBAL,
           name,
           "a union type name must start uppercase, found '%s'",
           std::string(name.str).c_str());

  UT::Vec<VariantDecl> variants{ m_arena };
  for (;;)
  {
    LX::TokenTag tag = EX_TRY(m_lex.peek()).tag;
    if (LX::TokenTag::Dollar == tag || LX::TokenTag::Eof == tag) break;

    LX::Token tname
      = EX_TRY(expect(LX::TokenTag::Word, "expected a variant tag"));
    char tc = tname.str.size() ? tname.str.data()[0] : '\0';
    if (tc < 'A' || tc > 'Z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             tname,
             "a variant tag must start uppercase, found '%s'",
             std::string(tname.str).c_str());

    EX_TRY(expect(LX::TokenTag::Colon, "expected ':' after the variant tag"));

    UT::Vec<FieldDecl> fields{ m_arena };
    if (LX::TokenTag::LBrace == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // '{'
      bool any_named = false;
      for (;;)
      {
        if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break;

        // A named field `f: T` (lowercase word then ':') vs a positional type.
        LX::Token t0  = EX_TRY(m_lex.peek(0));
        char      t0c = t0.str.size() ? t0.str.data()[0] : '\0';
        bool named    = LX::TokenTag::Word == t0.tag && t0c >= 'a' && t0c <= 'z'
                     && LX::TokenTag::Colon == EX_TRY(m_lex.peek(1)).tag;
        UT::Vu fname{};
        if (named)
        {
          LX::Token fn = EX_TRY(m_lex.next()); // field name
          m_lex.next();                        // ':'
          fname     = fn.str;
          any_named = true;
        }
        Ty *ty = EX_CTX(parse_type(),
                        tname,
                        "in the payload of variant '%s'",
                        std::string(tname.str).c_str());
        fields.push(FieldDecl{ fname, ty });

        if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
        m_lex.next(); // ','
      }
      EX_TRY(expect(LX::TokenTag::RBrace,
                    "expected '}' to close the variant payload"));
      if (any_named)
        for (size_t i = 0; i < fields.size(); ++i)
          if (!fields[i].name.size())
            EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                   tname,
                   "variant '%s' mixes named and positional payload fields; "
                   "use one or the other",
                   std::string(tname.str).c_str());
    }
    else
    {
      // A bare type is one anonymous field, e.g. `Num: Int`.
      Ty *ty = EX_CTX(parse_type(),
                      tname,
                      "in the payload of variant '%s'",
                      std::string(tname.str).c_str());
      fields.push(FieldDecl{ UT::Vu{}, ty });
    }

    variants.push(VariantDecl{ tname.str, fields, tname.str, tname.line });

    if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
    m_lex.next(); // ','
  }

  Expr e{ ExprTag::UnionDecl };
  e.as = ExUnionDecl{ name.str, variants };
  return { true, alloc(e), {} };
}

// A variant construction. Called with `Type` and `Tag` already consumed; an
// optional `.{ ... }` payload follows. Without it the payload is unit (e.g.
// `Bool.True`). Payload values are positional (`{a, b}`) or named (`{.f = a}`),
// not mixed -- the same rule as a struct literal.
RExpr
Parser::parse_variant_lit(
  UT::Vu type_name, UT::Vu tag, const LX::Token &tok)
{
  UT::Vec<FieldInit> fields{ m_arena };

  if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
      && LX::TokenTag::LBrace == EX_TRY(m_lex.peek(1)).tag)
  {
    m_lex.next(); // '.'
    m_lex.next(); // '{'
    bool any_named = false;
    for (;;)
    {
      if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break;

      if (LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
      {
        m_lex.next(); // '.'
        LX::Token fn = EX_TRY(
          expect(LX::TokenTag::Word, "expected a field name after '.'"));
        EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the field name"));
        Expr *val = EX_CTX(parse_expr(0),
                           fn,
                           "in the value of field '%s'",
                           std::string(fn.str).c_str());
        fields.push(FieldInit{ fn.str, val });
        any_named = true;
      }
      else
      {
        Expr *val = EX_CTX(parse_expr(0),
                           tok,
                           "in a positional payload value of '%s'",
                           std::string(tag).c_str());
        fields.push(FieldInit{ UT::Vu{}, val });
      }

      if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
      m_lex.next(); // ','
    }
    EX_TRY(expect(LX::TokenTag::RBrace,
                  "expected '}' to close the variant payload"));
    if (any_named)
      for (size_t i = 0; i < fields.size(); ++i)
        if (!fields[i].name.size())
          EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                 tok,
                 "variant '%s' mixes named and positional payload values; use "
                 "one or the other",
                 std::string(tag).c_str());
  }

  Expr e{ ExprTag::VariantLit };
  e.as = ExVariantLit{ type_name, tag, fields, tok.str, tok.line };
  return { true, alloc(e), {} };
}

/*-------------------------------------------------------------------------------
 *\PARSE TYPES
 *
 * type ::= atom ('->' type)?     -- '->' right-associative
 * atom ::= UpperWord | TyVar | '(' type ')'
 *------------------------------------------------------------------------------*/

RTy
Parser::parse_type_atom()
{
  LX::Token t = EX_TRY(m_lex.peek());
  switch (t.tag)
  {
  case LX::TokenTag::Word:
  {
    char c = t.str.size() ? t.str.data()[0] : '\0';
    if (c < 'A' || c > 'Z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             t,
             "expected a type name (must start uppercase), found '%s'",
             std::string(t.str).c_str());
    m_lex.next();
    return { true, mk_ty(Ty{ TyTag::Con, TyCon{ t.str } }), {} };
  }
  case LX::TokenTag::TyVar:
  {
    m_lex.next();
    // strip the leading backtick for the stored name
    UT::Vu nm{ t.str.data() + 1, t.str.size() - 1 };
    return { true, mk_ty(Ty{ TyTag::Var, TyVar{ nm } }), {} };
  }
  case LX::TokenTag::LParen:
  {
    LX::Token lp = EX_TRY(m_lex.next()); // '('
    Ty       *ty = EX_CTX(parse_type(), lp, "in this parenthesized type");
    LX::Token rp = EX_TRY(m_lex.peek());
    if (LX::TokenTag::RParen != rp.tag)
      EX_ERR(ER::Code::PARENTHESIS_UNBALANCED,
             lp,
             "expected a closing ')' to match this '(' in the type");
    m_lex.next();
    return { true, ty, {} };
  }
  default:
    EX_ERR(ER::Code::EXPECTED_OPERAND,
           t,
           "expected a type, found '%s'",
           std::string(t.str).c_str());
  }
}

RTy
Parser::parse_type()
{
  Ty *lhs = EX_TRY(parse_type_atom());

  if (LX::TokenTag::Arrow == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next();                   // '->'
    Ty *rhs = EX_TRY(parse_type()); // right-associative
    return { true, mk_ty(Ty{ TyTag::Arrow, TyArrow{ lhs, rhs } }), {} };
  }
  return { true, lhs, {} };
}

void
Parser::recover()
{
  for (;;)
  {
    LX::RToken pk = m_lex.peek();
    if (!pk.ok) return;
    LX::TokenTag tag = pk.value.tag;
    if (LX::TokenTag::Eof == tag || LX::TokenTag::Dollar == tag
        || LX::TokenTag::KwExt == tag)
      return;
    if (!m_lex.next().ok) return;
  }
}

void
Parser::operator()()
{
  for (;;)
  {
    LX::RToken pk = m_lex.peek();
    if (!pk.ok)
    {
      m_diags.push_back(pk.err);
      return; // lexer error: cannot make progress, stop
    }
    if (LX::TokenTag::Eof == pk.value.tag) return;

    size_t before = m_lex.mark();
    RExpr  r      = parse_global();
    if (r.ok)
    {
      m_exprs.push(*r.value);
      continue;
    }

    m_diags.push_back(r.err);
    recover();
    if (m_lex.mark() == before)
    {
      // No progress (e.g. an unrecoverable lexer error): bail out.
      if (!m_lex.next().ok) return;
    }
  }
}

/*-------------------------------------------------------------------------------
 *\CTOR
 *------------------------------------------------------------------------------*/

Parser::Parser(
  LX::Lexer &lex)
    : m_arena{ lex.m_arena },
      m_lex{ lex },
      m_exprs{ lex.m_arena },
      m_diags{}
{
}

} // namespace EX

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
