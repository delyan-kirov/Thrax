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

// Sequencing `lhs ; rhs`  ==>  `let _ = lhs in rhs`: run lhs for effect,
// discard its value, then evaluate rhs (whose value is the result). A plain
// desugar -- no runtime `;` operator.
Expr *
Parser::mk_seq(
  Expr *lhs, Expr *rhs)
{
  ExLet lt;
  lt.var  = UT::Vu{ "_", 1 };
  lt.val  = lhs;
  lt.body = rhs;
  Expr e{ ExprTag::Let };
  e.as = lt;
  return alloc(e);
}

// The blessed list type and its constructors (see the prelude in DR.cpp). List
// literals and `::` desugar to ordinary variant construction against these.
static const UT::Vu LIST_TY{ "List", 4 };
static const UT::Vu CONS_TAG{ "Cons", 4 };
static const UT::Vu NIL_TAG{ "Nil", 3 };

// `List.Cons.{ head, tail }` -- a positional two-field variant literal.
Expr *
Parser::mk_cons(
  Expr *head, Expr *tail, UT::Vu anchor, size_t line)
{
  UT::Vec<FieldInit> fields{ m_arena };
  fields.push(FieldInit{ UT::Vu{}, head });
  fields.push(FieldInit{ UT::Vu{}, tail });
  Expr e{ ExprTag::VariantLit };
  e.as = ExVariantLit{ LIST_TY, CONS_TAG, fields, anchor, line };
  return alloc(e);
}

// `List.Nil` -- the empty-payload variant literal.
Expr *
Parser::mk_nil(
  UT::Vu anchor, size_t line)
{
  UT::Vec<FieldInit> fields{ m_arena };
  Expr               e{ ExprTag::VariantLit };
  e.as = ExVariantLit{ LIST_TY, NIL_TAG, fields, anchor, line };
  return alloc(e);
}

// `List.Cons.{ h, t }` as a (positional) variant pattern.
Pattern *
Parser::mk_cons_pat(
  Pattern *h, Pattern *t, UT::Vu a, size_t ln)
{
  UT::Vec<FieldPat> fields{ m_arena };
  fields.push(FieldPat{ UT::Vu{}, h });
  fields.push(FieldPat{ UT::Vu{}, t });
  return alloc_pat(
    Pattern{ PatTag::Variant, PatVariant{ LIST_TY, CONS_TAG, fields, a, ln } });
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
  UT::Vu abi, UT::Vu symbol, UT::Vu lib)
{
  Expr e{ ExprTag::Extern };
  e.as = ExExtern{ abi, symbol, lib };
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
  case LX::TokenTag::LParen : base = EX_TRY(parse_group()); break;
  case LX::TokenTag::KwLet  : base = EX_TRY(parse_let()); break;
  case LX::TokenTag::KwWith : base = EX_TRY(parse_with()); break;
  case LX::TokenTag::KwIf   : base = EX_TRY(parse_if()); break;
  case LX::TokenTag::KwWhen : base = EX_TRY(parse_when()); break;
  case LX::TokenTag::KwDo   : base = EX_TRY(parse_handle()); break;
  case LX::TokenTag::KwDefer: base = EX_TRY(parse_defer()); break;
  case LX::TokenTag::Lambda : base = EX_TRY(parse_closure()); break;
  case LX::TokenTag::LBrack : base = EX_TRY(parse_list()); break;
  case LX::TokenTag::At     : base = EX_TRY(parse_at_term()); break;
  case LX::TokenTag::LBrace:
  {
    // The unit value `{}` (empty record), or a tuple literal `{a, b, ...}`
    // with positional fields "0".."n-1" (see OP.hpp).
    LX::Token lb = EX_TRY(m_lex.next()); // '{'
    if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // '}'
      Expr e{ ExprTag::Unit };
      e.as = ExUnit{};
      base = alloc(e);
      break;
    }
    UT::Vec<FieldInit> fields{ m_arena };
    for (;;)
    {
      Expr  *val = EX_CTX(parse_expr(0), lb, "in this tuple literal");
      UT::Vu nm  = UT::strdup(m_arena, std::to_string(fields.size()).c_str());
      fields.push(FieldInit{ nm, val });
      if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
      m_lex.next();                                                // ','
      if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break; // trailing
    }
    EX_TRY(
      expect(LX::TokenTag::RBrace, "expected '}' to close the tuple literal"));
    Expr e{ ExprTag::StructLit };
    e.as
      = ExStructLit{ UT::strdup(m_arena, OP::tuple_name(fields.size()).c_str()),
                     fields };
    base = alloc(e);
    break;
  }
  case LX::TokenTag::Dot:
  {
    // An unqualified literal whose type is inferred from context: a bare struct
    // literal `.{ ... }` or a bare variant constructor `.Tag [.{ ... }]`. The
    // type name is left empty for TC to resolve.
    LX::Token dot   = EX_TRY(m_lex.next()); // '.'
    LX::Token ahead = EX_TRY(m_lex.peek());
    if (LX::TokenTag::LBrace == ahead.tag)
    {
      base = EX_CTX(parse_struct_lit(UT::Vu{}), dot, "in this struct literal");
      break;
    }
    LX::Token tag = EX_TRY(expect(
      LX::TokenTag::Word, "expected '{' or a variant tag after a leading '.'"));
    char      c   = tag.str.size() ? tag.str.data()[0] : '\0';
    if (c < 'A' || c > 'Z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             tag,
             "a variant tag must start uppercase, found '%s'",
             std::string(tag.str).c_str());
    base = EX_CTX(parse_variant_lit(UT::Vu{}, tag.str, tag),
                  dot,
                  "in this '.%s' variant",
                  std::string(tag.str).c_str());
    break;
  }
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
    else if (LX::TokenTag::Int == ahead.tag || LX::TokenTag::Real == ahead.tag)
    {
      m_lex.next();
      UT::Vu lex   = ahead.str;
      size_t start = 0;
      for (size_t i = 0; i <= lex.size(); ++i)
      {
        if (i < lex.size() && lex.data()[i] != '.') continue;
        UT::Vu part = lex.substr(start, i - start);
        bool   ok   = part.size() > 0;
        for (size_t k = 0; k < part.size(); ++k)
          ok = ok && part.data()[k] >= '0' && part.data()[k] <= '9';
        if (!ok)
          EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                 ahead,
                 "expected a tuple index after '.', found '%s'",
                 std::string(lex).c_str());
        base  = mk_field(base, part);
        start = i + 1;
      }
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

        LX::Token p0 = EX_TRY(m_lex.peek(0));
        LX::Token p1 = EX_TRY(m_lex.peek(1));
        if (p0.tag == LX::TokenTag::Dot && p1.tag == LX::TokenTag::Word
            && p1.str.size() && p1.str.data()[0] >= 'A'
            && p1.str.data()[0] <= 'Z')
        {
          m_lex.next(); // '.'
          m_lex.next(); // the tag
          base = EX_CTX(parse_variant_lit(fld.str, p1.str, p1, tn),
                        dot,
                        "in this '%s.%s.%s' variant",
                        std::string(tn).c_str(),
                        std::string(fld.str).c_str(),
                        std::string(p1.str).c_str());
        }
        else
          base = EX_CTX(parse_variant_lit(tn, fld.str, fld),
                        dot,
                        "in this '%s.%s' variant",
                        std::string(tn).c_str(),
                        std::string(fld.str).c_str());
      }
      else if (base->tag == ExprTag::Var
               && std::get<ExVar>(base->as).qualifier.empty()
               && std::get<ExVar>(base->as).name.size()
               && std::get<ExVar>(base->as).name.data()[0] >= 'A'
               && std::get<ExVar>(base->as).name.data()[0] <= 'Z')
      {
        // `MOD.foo` -- an uppercase base with a lowercase member is a
        // module-qualified variable reference (MR resolves the module). Field
        // access reads from a value, whose name is lowercase, so the two never
        // collide.
        UT::Vu mod = std::get<ExVar>(base->as).name;
        Expr   e{ ExprTag::Var };
        e.as = ExVar{ fld.str, mod };
        base = alloc(e);
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
      // Sequencing and pipes desugar away here; everything else is an ordinary
      // (overloadable) binary operator call.
      if (t.str == ";")
        lhs = mk_seq(lhs, rhs);
      else if (t.str == "|>")
        lhs = mk_app(rhs, lhs); // `x |> f` == `f x`
      else if (t.str == "<|")
        lhs = mk_app(lhs, rhs); // `f <| x` == `f x`
      else if (t.str == "::")
        lhs = mk_cons(lhs, rhs, t.str, t.line); // `h :: t` == List.Cons.{h,t}
      else
        lhs = mk_binop(t.str, lhs, rhs);
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
  EX_TRY(m_lex.next()); // 'let'
  return parse_let_binding();
}

// One binding of a (possibly comma-chained) `let`, parsed from its binder
// through its body. Bindings chain with ',': `let x = a, y = b in e` desugars
// to `let x = a in let y = b in e` -- a ',' makes the *next* binding this let's
// body, while 'in' introduces the final body. Each binding stays its own ExLet
// (its own scope and error context), so chaining is a flat surface over the
// nested lets we already had, not a new multi-binding node.
RExpr
Parser::parse_let_binding()
{
  // A `let` binder is either a plain variable (`let x = ...`) or a structural
  // pattern (`let Person.{x,y} = ...`, `let {a, b} = ...`). An
  // uppercase-initial word or a '{' starts a pattern; the LL pass lowers it
  // into nested plain lets.
  LX::Token la     = EX_TRY(m_lex.peek());
  bool      is_pat = LX::TokenTag::LBrace == la.tag;
  if (LX::TokenTag::Word == la.tag)
  {
    char c = la.str.size() ? la.str.data()[0] : '\0';
    is_pat = (c >= 'A' && c <= 'Z');
  }

  ExLet lt;
  // Anchors the "in the body of this 'let'" context below at the binder.
  LX::Token anchor = la;
  if (is_pat)
  {
    lt.pat = EX_TRY(parse_pattern());
    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the 'let' pattern"));
    lt.val = EX_CTX(parse_expr(0), la, "in this 'let' binding");
  }
  else
  {
    LX::Token name = EX_TRY(
      expect(LX::TokenTag::Word, "expected a variable name after 'let'"));
    anchor = name;
    lt.var = name.str;

    // An optional type annotation: `let x : T = ...`. It pins the bound value's
    // type (and gives a recursive binding its declared type).
    if (LX::TokenTag::Colon == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // ':'
      lt.sig = EX_CTX(parse_type(),
                      name,
                      "in the type of 'let %s'",
                      std::string(name.str).c_str());
    }

    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the 'let' variable"));
    lt.val = EX_CTX(parse_expr(0),
                    name,
                    "in the binding of 'let %s'",
                    std::string(name.str).c_str());
  }

  Expr *body = nullptr;
  if (LX::TokenTag::Comma == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // ','
    if (LX::TokenTag::KwIn != EX_TRY(m_lex.peek()).tag)
      body = EX_TRY(parse_let_binding());
  }
  if (!body)
  {
    EX_TRY(expect(LX::TokenTag::KwIn,
                  "expected 'in' or ',' after the 'let' binding"));
    body = EX_CTX(parse_expr(0), anchor, "in the body of this 'let'");
  }
  lt.body = body;

  Expr e{ ExprTag::Let };
  e.as = lt;
  return { true, alloc(e), {} };
}

// `with p in body` brings every field of `p`'s struct into scope unqualified for
// `body`. It desugars to a pattern-let whose pattern binds all of `p`'s fields
// punned; the struct (and thus its field names) is only known once TC types `p`,
// so this is a `bind_all` struct pattern that TC resolves. `p` may be any
// expression.
// `with <subject> in body` as a pattern-let whose `bind_all` struct pattern TC
// resolves against `subject`'s type. Shared by `parse_with` and the
// `{with p: T}` parameter sugar.
Expr *
Parser::mk_with_scope(
  Expr *subject, Expr *body, const LX::Token &anchor)
{
  UT::Vec<FieldPat> none{ m_arena };
  Pattern pat{ PatTag::Struct,
               PatStruct{ UT::Vu{}, none, anchor.str, anchor.line } };
  std::get<PatStruct>(pat.as).bind_all = true;
  ExLet lt;
  lt.var  = UT::Vu{};
  lt.pat  = alloc_pat(pat);
  lt.val  = subject;
  lt.body = body;
  Expr e{ ExprTag::Let };
  e.as = lt;
  return alloc(e);
}

RExpr
Parser::parse_with()
{
  LX::Token kw = EX_TRY(m_lex.next()); // 'with'
  Expr     *subject
    = EX_CTX(parse_expr(0), kw, "in the subject of this 'with'");
  EX_TRY(expect(LX::TokenTag::KwIn, "expected 'in' after the 'with' subject"));
  Expr *body = EX_CTX(parse_expr(0), kw, "in the body of this 'with'");
  return { true, mk_with_scope(subject, body, kw), {} };
}

RExpr
Parser::parse_if()
{
  LX::Token kw   = EX_TRY(m_lex.next()); // 'if'
  Expr     *cond = EX_CTX(parse_expr(0), kw, "in the condition of this 'if'");

  EX_TRY(
    expect(LX::TokenTag::KwThen, "expected 'then' after the 'if' condition"));

  Expr *then = EX_CTX(parse_expr(0), kw, "in the then-branch of this 'if'");

  LX::Token els = EX_TRY(
    expect(LX::TokenTag::KwElse, "expected 'else' after the then-branch"));

  Expr *alt = EX_CTX(parse_expr(0), els, "in the else branch of this 'if'");
  return { true, mk_if(cond, then, alt), {} };
}

// True when `p` binds at least one variable (a `name` pattern, a `"lit" ++ rest`
// tail, a struct/variant field pun or sub-binder, or a list/array element). Used
// to reject binders in or-pattern alternatives, which share one body.
static bool
pat_binds_any(
  const Pattern *p)
{
  switch (p->tag)
  {
  case PatTag::Wild:
  case PatTag::Int:
  case PatTag::Bool:
  case PatTag::Real:
  case PatTag::Str: return false;
  case PatTag::Var: return true;
  case PatTag::StrPrefix:
    return pat_binds_any(std::get<PatStrPrefix>(p->as).rest);
  case PatTag::Struct:
    for (const FieldPat &f : std::get<PatStruct>(p->as).fields)
      if (pat_binds_any(f.pat)) return true;
    return false;
  case PatTag::Variant:
    for (const FieldPat &f : std::get<PatVariant>(p->as).fields)
      if (pat_binds_any(f.pat)) return true;
    return false;
  case PatTag::Seq:
  {
    const PatSeq &s = std::get<PatSeq>(p->as);
    for (Pattern *e : s.elems)
      if (pat_binds_any(e)) return true;
    return s.rest && pat_binds_any(s.rest);
  }
  }
  return false;
}

// A pattern match: `when scrut is pat [is pat ...] [if guard] then e .. [else d]`.
// Each arm tests one or more `is pat` alternatives against the scrutinee and,
// when it carries an `if guard`, an extra boolean over the pattern's bindings; a
// failed pattern or guard falls through to the next arm. Or-pattern alternatives
// share the arm's body, so each is emitted as its own arm (a matrix row for the
// exhaustiveness checker) and may bind no variables. The `else` may be omitted
// when the arms are exhaustive (TC checks this and rejects a non-exhaustive
// `when` that has no `else`). TC's PatLower lowers the whole form into a `case` /
// if-chain, so no layer below sees an ExMatch.
RExpr
Parser::parse_when()
{
  LX::Token kw = EX_TRY(m_lex.next()); // 'when'
  Expr *scrut  = EX_CTX(parse_expr(0), kw, "in the scrutinee of this 'when'");

  if (LX::TokenTag::KwIs != EX_TRY(m_lex.peek()).tag)
    EX_ERR(ER::Code::UNEXPECTED_TOKEN,
           EX_TRY(m_lex.peek()),
           "expected 'is' to open the first arm of this 'when'");

  UT::Vec<MatchArm> arms{ m_arena };
  while (LX::TokenTag::KwIs == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // 'is'
    UT::Vec<Pattern *> alts{ m_arena };
    alts.push(EX_TRY(parse_pattern()));

    // Further `is pat` before the `then`/guard are or-pattern alternatives of
    // this same arm, not new arms.
    while (LX::TokenTag::KwIs == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // 'is'
      alts.push(EX_TRY(parse_pattern()));
    }

    // Optional `if guard`: a boolean tested in the pattern's scope. The guard
    // expression ends at `then` (an expression terminator).
    Expr *guard = nullptr;
    if (LX::TokenTag::KwIf == EX_TRY(m_lex.peek()).tag)
    {
      LX::Token gk = EX_TRY(m_lex.next()); // 'if'
      guard = EX_CTX(parse_expr(0), gk, "in the guard of this match arm");
    }

    EX_TRY(
      expect(LX::TokenTag::KwThen,
             "expected 'then' after the match pattern (or its 'if' guard)"));
    Expr *body = EX_CTX(parse_expr(0), kw, "in this match arm");

    // An or-pattern's alternatives share this one body, so none may bind a
    // variable (there would be no consistent binding to hand the body).
    if (alts.size() > 1)
      for (size_t i = 0; i < alts.size(); ++i)
        if (pat_binds_any(alts[i]))
          EX_ERR(ER::Code::UNSUPPORTED,
                 kw,
                 "an or-pattern alternative cannot bind a variable; the "
                 "alternatives of one 'is ... is ...' arm share its body");

    // Each alternative becomes its own arm sharing the body and guard.
    for (size_t i = 0; i < alts.size(); ++i)
      arms.push(MatchArm{ alts[i], body, guard });
  }
  // The `else` is optional: when present it supplies the fallthrough value;
  // when absent, TC requires the arms to be exhaustive (and PatLower fills in
  // an unreachable default). A non-'is', non-'else' token here is still an
  // error.
  Expr *alt = nullptr;
  if (LX::TokenTag::KwElse == EX_TRY(m_lex.peek()).tag)
  {
    LX::Token els = EX_TRY(m_lex.next()); // 'else'
    alt = EX_CTX(parse_expr(0), els, "in the else branch of this 'when'");
  }

  Expr e{ ExprTag::Match };
  e.as = ExMatch{ scrut, arms, alt, kw.str };
  return { true, alloc(e), {} };
}

// A handler: `do <body> ctl k  is op a = e ...  [else x = e]`. The `do` body is
// a single expression (parenthesize a nested handler); `ctl k` binds the one
// continuation shared by every clause; each `is op a = e` clause handles an
// operation, binding its argument to `a`; the optional `else x = e` is the
// value clause for normal completion. There is no `perform`/`resume` syntax: an
// operation is performed by calling it, and `k` is resumed by applying it.
RExpr
Parser::parse_handle()
{
  LX::Token kw   = EX_TRY(m_lex.next()); // 'do'
  Expr     *body = EX_CTX(parse_expr(0), kw, "in the body of this 'do' block");

  // A `do` with no `ctl` is just a block -- it groups its body (handy after
  // `defer`, or to parenthesize-free a sequence). It has no runtime form of its
  // own, so return the body directly.
  if (LX::TokenTag::KwCtl != EX_TRY(m_lex.peek()).tag)
    return { true, body, {} };

  EX_TRY(m_lex.next()); // 'ctl'
  LX::Token kt = EX_TRY(
    expect(LX::TokenTag::Word, "expected a continuation name after 'ctl'"));

  UT::Vec<HandlerClause> clauses{ m_arena };
  while (LX::TokenTag::KwIs == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // 'is'
    LX::Token op = EX_TRY(
      expect(LX::TokenTag::Word, "expected an operation name after 'is'"));
    char oc = op.str.size() ? op.str.data()[0] : '\0';

    // An uppercase head followed by '.' is an effect qualifier: `is Effect.op
    // a`.
    UT::Vu qual{};
    if (oc >= 'A' && oc <= 'Z' && LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // '.'
      qual = op.str;
      op   = EX_TRY(expect(LX::TokenTag::Word,
                         "expected an operation name after the effect prefix"));
      oc   = op.str.size() ? op.str.data()[0] : '\0';
    }
    if (oc < 'a' || oc > 'z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             op,
             "a handler operation must start lowercase, found '%s'",
             std::string(op.str).c_str());
    LX::Token arg = EX_TRY(
      expect(LX::TokenTag::Word, "expected the operation's argument binder"));
    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the clause head"));
    Expr *cbody = EX_CTX(parse_expr(0), op, "in this handler clause");
    clauses.push(HandlerClause{ op.str, arg.str, cbody, qual });
  }
  if (clauses.empty())
    EX_ERR(ER::Code::UNEXPECTED_TOKEN,
           kt,
           "a handler needs at least one 'is op a = ...' clause");

  UT::Vu els_var{};
  Expr  *els_body = nullptr;
  if (LX::TokenTag::KwElse == EX_TRY(m_lex.peek()).tag)
  {
    LX::Token els = EX_TRY(m_lex.next()); // 'else'
    LX::Token v   = EX_TRY(
      expect(LX::TokenTag::Word, "expected the result binder after 'else'"));
    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the 'else' binder"));
    els_var  = v.str;
    els_body = EX_CTX(parse_expr(0), els, "in the 'else' value clause");
  }

  Expr e{ ExprTag::Handle };
  e.as = ExHandle{ body, kt.str, clauses, els_var, els_body };
  return { true, alloc(e), {} };
}

// `defer <cleanup> do <body>` -- run `cleanup` when `body`'s scope exits
// (normal completion OR an abort that discards the continuation). `do` delimits
// the cleanup from the body; the body is an ordinary `do` block (optionally a
// `ctl` handler). Desugars to `%defer (\_ = body) (\_ = cleanup)` -- the
// cleanup intrinsic; both sides become thunks so neither runs until the machine
// wants it.
RExpr
Parser::parse_defer()
{
  LX::Token kw      = EX_TRY(m_lex.next()); // 'defer'
  Expr     *cleanup = EX_CTX(
    parse_expr(0), kw, "in the cleanup of this 'defer' (it ends at 'do')");

  LX::Token d = EX_TRY(m_lex.peek());
  if (LX::TokenTag::KwDo != d.tag)
    EX_ERR(ER::Code::UNEXPECTED_TOKEN,
           d,
           "expected 'do' to open the 'defer' body, found '%s'",
           std::string(d.str).c_str());
  Expr *body = EX_TRY(parse_handle()); // consumes `do <body> [ctl ...]`

  Expr *body_thunk    = mk_fndef(UT::Vu{ "_", 1 }, body);
  Expr *cleanup_thunk = mk_fndef(UT::Vu{ "_", 1 }, cleanup);
  Expr *call
    = mk_app(mk_app(mk_op_var(UT::Vu{ OP::DEFER }), body_thunk), cleanup_thunk);
  return { true, call, {} };
}

// A pattern, folding a trailing `::` cons: `h :: t` desugars to a
// `List.Cons.{ h, t }` variant pattern (right-associative). Cons/Nil patterns
// are refutable, so a `::` or list pattern is rejected by LL where only
// irrefutable patterns are allowed (lambda / let).
RPattern
Parser::parse_pattern()
{
  Pattern  *head = EX_TRY(parse_pattern_atom());
  LX::Token t    = EX_TRY(m_lex.peek());
  if (LX::TokenTag::Op == t.tag && t.str == "::")
  {
    m_lex.next();                            // '::'
    Pattern *tail = EX_TRY(parse_pattern()); // right-associative
    return { true, mk_cons_pat(head, tail, t.str, t.line), {} };
  }
  // `"lit" ++ rest`
  if (LX::TokenTag::Op == t.tag && t.str == "++")
  {
    if (head->tag != PatTag::Str)
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             t,
             "the left of '++' in a pattern must be a string literal");
    m_lex.next();                            // '++'
    Pattern *rest = EX_TRY(parse_pattern()); // right-associative remainder
    auto    &ps   = std::get<PatStr>(head->as);
    Pattern  pat{ PatTag::StrPrefix,
                 PatStrPrefix{ ps.value, rest, ps.anchor, ps.line } };
    return { true, alloc_pat(pat), {} };
  }
  return { true, head, {} };
}

// A single pattern: `_`, a variable, a literal, a `Type.{ ... }` struct
// pattern, or a `[..]` list pattern. Literals and literal-bearing struct
// patterns are refutable; the LL pass rejects them where only irrefutable
// patterns are allowed (lambda / let).
RPattern
Parser::parse_pattern_atom()
{
  LX::Token t = EX_TRY(m_lex.peek());
  switch (t.tag)
  {
  case LX::TokenTag::LBrack: return parse_list_pattern();
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
  case LX::TokenTag::At:
  {
    const OP::AtIntrinsic *in = OP::at_lookup(t.str, OP::AtNs::Term);
    if (!in || !in->pat_ok)
      EX_ERR(ER::Code::EXPECTED_OPERAND,
             t,
             "expected a pattern, found directive '%s' (only '@true'/'@false' "
             "are patterns)",
             std::string(t.str).c_str());
    m_lex.next();
    return { true,
             alloc_pat(
               Pattern{ PatTag::Bool,
                        PatBool{ in->id == OP::AtId::True, t.str, t.line } }),
             {} };
  }
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
  case LX::TokenTag::LBrace:
  {
    LX::Token         lb = EX_TRY(m_lex.next()); // '{'
    UT::Vec<FieldPat> fields{ m_arena };
    for (;;)
    {
      if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break;
      fields.push(FieldPat{ UT::Vu{}, EX_TRY(parse_pattern()) });
      if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
      m_lex.next(); // ','
    }
    EX_TRY(
      expect(LX::TokenTag::RBrace, "expected '}' to close the tuple pattern"));
    if (fields.size() == 0)
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             lb,
             "'{}' is not a pattern; match the unit value with '_'");
    UT::Vu  nm = UT::strdup(m_arena, OP::tuple_name(fields.size()).c_str());
    Pattern pat{ PatTag::Struct, PatStruct{ nm, fields, lb.str, lb.line } };
    return { true, alloc_pat(pat), {} };
  }
  case LX::TokenTag::Dot:
  {
    // A bare variant pattern `.Tag` / `.Tag.{ ... }`: the union type is left
    // empty and inferred from the match (see LL). Only an uppercase tag is a
    // bare constructor here.
    m_lex.next(); // '.'
    LX::Token tag = EX_TRY(expect(
      LX::TokenTag::Word, "expected a variant tag after '.' in a pattern"));
    char      tc  = tag.str.size() ? tag.str.data()[0] : '\0';
    if (tc < 'A' || tc > 'Z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             tag,
             "a variant tag must start uppercase, found '%s'",
             std::string(tag.str).c_str());

    UT::Vec<FieldPat> fields{ m_arena };
    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
        && LX::TokenTag::LBrace == EX_TRY(m_lex.peek(1)).tag)
    {
      m_lex.next(); // '.'
      fields = EX_TRY(parse_field_pats(UT::Vu{}, tag));
    }
    Pattern pat{ PatTag::Variant,
                 PatVariant{ UT::Vu{}, tag.str, fields, tag.str, tag.line } };
    return { true, alloc_pat(pat), {} };
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

RPattern
Parser::parse_list_pattern()
{
  LX::Token          lb = EX_TRY(m_lex.next()); // '['
  UT::Vec<Pattern *> elems{ m_arena };
  Pattern           *tail = nullptr; // a `..rest` open tail, else null (closed)
  if (LX::TokenTag::RBrack != EX_TRY(m_lex.peek()).tag)
    for (;;)
    {
      if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
          && LX::TokenTag::Dot == EX_TRY(m_lex.peek(1)).tag)
      {
        m_lex.next(); // first '.'
        m_lex.next(); // second '.'
        tail = EX_TRY(parse_pattern());
        break;
      }
      elems.push(EX_TRY(parse_pattern()));
      if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
      m_lex.next();                                                // ','
      if (LX::TokenTag::RBrack == EX_TRY(m_lex.peek()).tag) break; // trailing
    }
  EX_TRY(
    expect(LX::TokenTag::RBrack, "expected ']' to close the sequence pattern"));

  Pattern pat{ PatTag::Seq, PatSeq{ elems, tail, lb.str, lb.line, false } };
  return { true, alloc_pat(pat), {} };
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

    // A third uppercase segment makes this a module-qualified variant pattern
    // `Qual.Type.Tag`: `ahead` is the type, the segment after is the tag.
    UT::Vu qualifier{};
    UT::Vu vtype = type_name;
    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
        && LX::TokenTag::Word == EX_TRY(m_lex.peek(1)).tag
        && EX_TRY(m_lex.peek(1)).str.size()
        && EX_TRY(m_lex.peek(1)).str.data()[0] >= 'A'
        && EX_TRY(m_lex.peek(1)).str.data()[0] <= 'Z')
    {
      m_lex.next();                        // '.'
      LX::Token s3 = EX_TRY(m_lex.next()); // the tag
      qualifier    = type_name;            // S1
      vtype        = tag;                  // S2 (the type)
      tag          = s3.str;               // S3 (the tag)
    }

    // An optional `.{ ... }` payload; its absence means a unit payload.
    UT::Vec<FieldPat> fields{ m_arena };
    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
        && LX::TokenTag::LBrace == EX_TRY(m_lex.peek(1)).tag)
    {
      m_lex.next(); // '.'
      fields = EX_TRY(parse_field_pats(vtype, tn));
    }
    Pattern pat{ PatTag::Variant,
                 PatVariant{
                   vtype, tag, fields, tn.str, tn.line, {}, qualifier } };
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

// `[e1, e2, .., en]` -- a sequence literal. Its container type is inferred: a
// blessed `List` (the default) or an `Array` in an Array-typed context. The
// choice is deferred to the type checker (ExSeqLit); CR desugars the resolved
// form. `[]` is the empty sequence.
RExpr
Parser::parse_list()
{
  LX::Token       lb = EX_TRY(m_lex.next()); // '['
  UT::Vec<Expr *> elems{ m_arena };
  if (LX::TokenTag::RBrack != EX_TRY(m_lex.peek()).tag)
    for (;;)
    {
      elems.push(EX_CTX(parse_expr(0), lb, "in this sequence literal"));
      if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
      m_lex.next(); // ','
      // A trailing comma before ']' is allowed.
      if (LX::TokenTag::RBrack == EX_TRY(m_lex.peek()).tag) break;
    }
  EX_TRY(
    expect(LX::TokenTag::RBrack, "expected ']' to close the sequence literal"));

  Expr e{ ExprTag::SeqLit };
  e.as = ExSeqLit{ elems, lb.str, lb.line, false };
  return { true, alloc(e), {} };
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

  // `$ with ...` is an import directive; `$ @private` / `$ @public` toggle
  // export visibility. Both are stripped by MR before later passes.
  LX::Token after = EX_TRY(m_lex.peek());
  if (LX::TokenTag::KwWith == after.tag) return parse_import();
  if (LX::TokenTag::At == after.tag)
  {
    if (const OP::AtIntrinsic *in = OP::at_lookup(after.str, OP::AtNs::Decl))
      switch (in->id)
      {
      case OP::AtId::Operator: return parse_operator_def();
      case OP::AtId::Assert  : return parse_ctime_assert();
      case OP::AtId::Run     : return parse_ctime_run();
      case OP::AtId::Private :
      case OP::AtId::Public  : return parse_vis();
      default: break; // @mod/@struct/@union/@alias/@effect/@extern: wrong slot
      }
    OP::AtNs ns;
    if (OP::at_classify(after.str, ns))
      EX_ERR(ER::Code::UNSUPPORTED,
             after,
             "'%s' is not valid directly after '$' (it is a %s form)",
             std::string(after.str).c_str(),
             OP::at_ns_name(ns));
    EX_ERR(ER::Code::UNSUPPORTED,
           after,
           "unknown intrinsic '%s' after '$'",
           std::string(after.str).c_str());
  }

  LX::Token name
    = EX_TRY(expect(LX::TokenTag::Word, "expected a name after '$'"));

  // Optional type annotation: `$ name : type = ...`. The special annotations
  // `@struct` / `@union` mark a type declaration rather than a value, so the
  // body is a field/variant list, not an expression.
  Ty *sig = nullptr;
  if (LX::TokenTag::Colon == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // ':'
    LX::Token ann = EX_TRY(m_lex.peek());
    if (LX::TokenTag::At == ann.tag)
      if (const OP::AtIntrinsic *in = OP::at_lookup(ann.str, OP::AtNs::Decl))
      {
        // The declaration-body forms: `$ name : @X = <body>`. Any other `@` in
        // annotation position (a base type, or a wrong-slot decl) falls through
        // to parse_type, which validates it.
        auto eat = [&](const char *what) -> LX::RToken {
          m_lex.next(); // the `@X`
          return expect(LX::TokenTag::Eq, what);
        };
        switch (in->id)
        {
        case OP::AtId::Struct:
          EX_TRY(eat("expected '=' after '@struct'"));
          return parse_struct_decl(name);
        case OP::AtId::Union:
          EX_TRY(eat("expected '=' after '@union'"));
          return parse_union_decl(name);
        case OP::AtId::Alias:
          EX_TRY(eat("expected '=' after '@alias'"));
          return parse_alias_decl(name);
        case OP::AtId::Effect:
          EX_TRY(eat("expected '=' after '@effect'"));
          return parse_effect_decl(name);
        default: break; // other decls: let parse_type produce the error
        }
      }
    sig = EX_CTX(parse_type(),
                 name,
                 "in the type signature of global '%s'",
                 std::string(name.str).c_str());
  }

  EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the global name"));

  // An `@extern` body is only valid here, and needs a signature to know its
  // foreign call types. Other `@` forms (e.g. `@array`) are ordinary
  // expressions, so they fall through to parse_expr below.
  LX::Token eqbody = EX_TRY(m_lex.peek());
  if (LX::TokenTag::At == eqbody.tag
      && OP::at_lookup(eqbody.str, OP::AtNs::Decl)
      && OP::at_lookup(eqbody.str, OP::AtNs::Decl)->id == OP::AtId::Extern)
  {
    if (!sig)
      EX_ERR(ER::Code::EXPECTED_GLOBAL,
             name,
             "extern global '%s' requires a type signature",
             std::string(name.str).c_str());
    if (has_rec_fields(sig))
      EX_ERR(ER::Code::UNSUPPORTED,
             name,
             "an extern signature cannot use a named-record parameter type");
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

  if (sig) body = EX_TRY(desugar_record_params(sig, body, name));
  return { true, mk_def(name.str, sig, body), {} };
}

// Whether any `TyCon` reachable from `t` still carries named-record field names
// (`{a: T, ..}`). True only for a named-record type that desugar_record_params
// did not consume, i.e. one outside a leading parameter position.
bool
Parser::has_rec_fields(
  Ty *t)
{
  switch (t->tag)
  {
  case TyTag::Con:
  {
    TyCon &c = std::get<TyCon>(t->as);
    if (c.rec_fields.size() != 0) return true;
    for (size_t i = 0; i < c.args.size(); ++i)
      if (has_rec_fields(c.args[i])) return true;
    return false;
  }
  case TyTag::Arrow:
  {
    TyArrow &a = std::get<TyArrow>(t->as);
    return has_rec_fields(a.from) || has_rec_fields(a.to);
  }
  case TyTag::Var: return false;
  }
  return false;
}

// `$ f : {x: T, y: U} -> R = body` is sugar for a positional-tuple parameter
// destructured into its named fields. Peel the leading named-record parameters
// off the signature's arrow spine, erasing each in place (a one-field record
// `{x: T}` collapses to `T`, a plain named parameter), and wrap `body` so the
// field names are bound: `\$p = let {x, y} = $p in body`. A named-record type
// anywhere but a leading parameter position is an error.
RExpr
Parser::desugar_record_params(
  Ty *sig, Expr *body, const LX::Token &name)
{
  std::vector<UT::Vec<RecField>> binders;
  Ty                            *a = sig;
  while (TyTag::Arrow == a->tag)
  {
    TyArrow &ar = std::get<TyArrow>(a->as);
    if (TyTag::Con != ar.from->tag) break;
    TyCon &c = std::get<TyCon>(ar.from->as);
    if (c.rec_fields.size() == 0) break;
    UT::Vec<RecField> fields = c.rec_fields;
    if (fields.size() == 1)
      ar.from = c.args[0]; // `{x: T}` collapses to `T`
    else
      c.rec_fields = UT::Vec<RecField>{ m_arena }; // erase names -> plain tuple
    binders.push_back(fields);
    a = ar.to;
  }

  if (has_rec_fields(sig))
    EX_ERR(ER::Code::UNSUPPORTED,
           name,
           "a named-record type '{field: T, ..}' is only allowed as a function "
           "parameter, not in a return type or nested position");

  for (size_t i = binders.size(); i-- > 0;)
  {
    UT::Vec<RecField> &fields = binders[i];

    // A `with`-marked field scopes its own struct's fields into the body,
    // innermost, once the field name is bound.
    for (size_t k = fields.size(); k-- > 0;)
      if (fields[k].with_scope)
        body = mk_with_scope(mk_op_var(fields[k].name), body, name);

    if (fields.size() == 1)
    {
      body = mk_fndef(fields[0].name, body);
      continue;
    }
    UT::Vu pv
      = UT::strdup(m_arena, ("%rec" + std::to_string(m_rec_n++)).c_str());
    UT::Vec<FieldPat> fps{ m_arena };
    for (size_t k = 0; k < fields.size(); ++k)
      fps.push(FieldPat{
        UT::Vu{}, alloc_pat(Pattern{ PatTag::Var, PatVar{ fields[k].name } }) });
    UT::Vu tn = UT::strdup(m_arena, OP::tuple_name(fields.size()).c_str());
    Pattern *pat
      = alloc_pat(Pattern{ PatTag::Struct,
                           PatStruct{ tn, fps, name.str, name.line } });
    ExLet lt;
    lt.var  = UT::Vu{};
    lt.pat  = pat;
    lt.val  = mk_op_var(pv);
    lt.body = body;
    Expr le{ ExprTag::Let };
    le.as = lt;
    body  = mk_fndef(pv, alloc(le));
  }
  return { true, body, {} };
}

// `$ @operator.{<op>} : type = expr` -- defines an overload of a built-in
// operator. The body is an ordinary expression (typically a lambda); the
// definition is just a global named after the operator's lexeme (e.g. "+"), so
// the rest of the pipeline treats it like any other global -- MR mangles it,
// and an operator use resolves to it (or a built-in) by type. The leading `$`
// is already consumed; `@operator` is the next token.
RExpr
Parser::parse_operator_def()
{
  m_lex.next(); // '@operator'
  EX_TRY(expect(LX::TokenTag::Dot, "expected '.{' after '@operator'"));
  EX_TRY(expect(LX::TokenTag::LBrace, "expected '{' after '@operator.'"));

  LX::Token op = EX_TRY(m_lex.peek());
  if (LX::TokenTag::Op != op.tag || !OP::is_operator(op.str))
    EX_ERR(ER::Code::UNSUPPORTED,
           op,
           "'%s' is not an overloadable operator",
           std::string(op.str).c_str());
  m_lex.next(); // the operator
  EX_TRY(expect(LX::TokenTag::RBrace, "expected '}' after the operator name"));

  // A signature is required: operand types are what select the overload.
  EX_TRY(expect(LX::TokenTag::Colon,
                "an operator overload needs a type signature ': ...'"));
  Ty *sig = EX_CTX(parse_type(),
                   op,
                   "in the type signature of operator '%s'",
                   std::string(op.str).c_str());

  EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after the operator signature"));
  Expr *body = EX_CTX(parse_expr(0),
                      op,
                      "in the definition of operator '%s'",
                      std::string(op.str).c_str());
  return { true, mk_def(op.str, sig, body), {} };
}

RExpr
Parser::parse_ctime_assert()
{
  LX::Token at = EX_TRY(m_lex.peek()); // '@assert' (the diagnostic anchor)
  m_lex.next();                        // '@assert'
  Expr *cond
    = EX_CTX(parse_expr(0), at, "in this compile-time '@assert' condition");

  UT::Vu name
    = UT::strdup(m_arena, ("%assert$" + std::to_string(m_assert_n++)).c_str());

  Ty   *sig         = mk_ty(Ty{ TyTag::Con, TyCon{ UT::Vu{ "Bool" }, {} } });
  Expr *d           = mk_def(name, sig, cond);
  auto &def         = std::get<ExDef>(d->as);
  def.ctime_assert  = true;
  def.assert_anchor = at.str; // anchor for the build-time diagnostic
  return { true, d, {} };
}

// `$ @run <expr>` -- compile-time execution (Jai's #run): the expression
// becomes a hidden global the build FORCES through the interpreter after
// linking. The value is discarded, except that BUILD directives
// (`BUILD.lib`, `BUILD.lib_path` -- BUILD.Directive values) steer the
// compilation's link set and library search paths, so a project declares
// its libraries in Thrax itself. No signature: the type is whatever the
// expression has.
RExpr
Parser::parse_ctime_run()
{
  LX::Token at = EX_TRY(m_lex.peek()); // '@run' (the diagnostic anchor)
  m_lex.next();                        // '@run'
  Expr *body
    = EX_CTX(parse_expr(0), at, "in this compile-time '@run' expression");

  UT::Vu name
    = UT::strdup(m_arena, ("%run$" + std::to_string(m_run_n++)).c_str());

  Expr *d           = mk_def(name, nullptr, body);
  auto &def         = std::get<ExDef>(d->as);
  def.ctime_run     = true;
  def.assert_anchor = at.str; // anchor for build-time diagnostics
  return { true, d, {} };
}

// Decode the single Unicode scalar in `bytes` to its code point. Fails (returns
// false) unless the bytes are exactly one well-formed UTF-8 sequence -- empty,
// truncated, a stray continuation byte, or trailing bytes after the first
// scalar all reject. Used by `@char`.
static bool
decode_one_scalar(
  UT::Vu bytes, uint32_t &out)
{
  if (bytes.empty()) return false;
  const unsigned char *p = (const unsigned char *)bytes.data();
  unsigned char        b0 = p[0];
  size_t               len;
  uint32_t             cp;
  if (b0 < 0x80) { len = 1; cp = b0; }
  else if ((b0 & 0xE0) == 0xC0) { len = 2; cp = b0 & 0x1F; }
  else if ((b0 & 0xF0) == 0xE0) { len = 3; cp = b0 & 0x0F; }
  else if ((b0 & 0xF8) == 0xF0) { len = 4; cp = b0 & 0x07; }
  else return false;
  if (bytes.size() != len) return false;
  for (size_t i = 1; i < len; i += 1)
  {
    if ((p[i] & 0xC0) != 0x80) return false;
    cp = (cp << 6) | (p[i] & 0x3F);
  }
  out = cp;
  return true;
}

// Dispatch an `@`-intrinsic appearing in expression position. Routes by the
// registry `form`; unknown or wrong-namespace names get a targeted error.
RExpr
Parser::parse_at_term()
{
  LX::Token          at  = EX_TRY(m_lex.peek());
  const OP::AtIntrinsic *in = OP::at_lookup(at.str, OP::AtNs::Term);
  if (!in)
  {
    OP::AtNs ns;
    if (OP::at_classify(at.str, ns))
      EX_ERR(ER::Code::UNSUPPORTED,
             at,
             "'%s' is a %s intrinsic, not valid in an expression",
             std::string(at.str).c_str(),
             OP::at_ns_name(ns));
    EX_ERR(ER::Code::UNSUPPORTED,
           at,
           "unknown intrinsic '%s' in expression",
           std::string(at.str).c_str());
  }

  switch (in->form)
  {
  case OP::AtForm::Nullary:
  {
    m_lex.next();
    Expr e{ ExprTag::Bool };
    e.as = ExBool{ in->id == OP::AtId::True };
    return { true, alloc(e), {} };
  }
  case OP::AtForm::DotBlock: return parse_array();
  case OP::AtForm::StrArg:
  {
    m_lex.next(); // '@char'
    LX::Token s = EX_TRY(
      expect(LX::TokenTag::Str, "expected a string literal after '@char'"));
    UT::Vu   bytes = std::get<LX::TkStr>(s.as).value;
    uint32_t cp;
    if (!decode_one_scalar(bytes, cp))
      EX_ERR(ER::Code::UNSUPPORTED,
             s,
             "'@char' takes exactly one character, found %zu byte(s)",
             bytes.size());
    Expr e{ ExprTag::Char };
    e.as = ExChar{ cp };
    return { true, alloc(e), {} };
  }
  case OP::AtForm::None: break;
  }
  EX_ERR(ER::Code::UNSUPPORTED,
         at,
         "intrinsic '%s' is not usable in an expression",
         std::string(at.str).c_str());
}

// `@array.{ size }` (or `@array.{ .size = expr }`) -- allocates a contiguous,
// zeroed block of `size` bytes, of type Array. The brace block mirrors a struct
// literal (positional or one named `.size` field). It desugars to an
// application of the internal allocation builtin, so the rest of the pipeline
// sees an ordinary `Int -> Array` call.
RExpr
Parser::parse_array()
{
  LX::Token at = EX_TRY(m_lex.peek());
  m_lex.next(); // '@array'

  EX_TRY(expect(LX::TokenTag::Dot, "expected '.{' after '@array'"));
  EX_TRY(expect(LX::TokenTag::LBrace, "expected '{' after '@array.'"));

  Expr *size = nullptr;
  if (LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
  {
    // Named form: `.size = expr`.
    m_lex.next(); // '.'
    LX::Token f
      = EX_TRY(expect(LX::TokenTag::Word, "expected '.size' in @array"));
    if (f.str != "size")
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             f,
             "unknown @array field '%s' (expected 'size')",
             std::string(f.str).c_str());
    EX_TRY(expect(LX::TokenTag::Eq, "expected '=' after '.size'"));
    size = EX_CTX(parse_expr(0), at, "in the size of this @array");
  }
  else
  {
    // Positional form: a single size expression.
    size = EX_CTX(parse_expr(0), at, "in the size of this @array");
  }
  if (LX::TokenTag::Comma == EX_TRY(m_lex.peek()).tag) m_lex.next(); // trailing
  EX_TRY(expect(LX::TokenTag::RBrace, "expected '}' to close @array"));

  return { true, mk_app(mk_op_var(UT::Vu{ OP::ARR_ALLOC }), size), {} };
}

// `@mod NAME` -- the module header that must open every file. The name's
// grammar (uppercase, no lowercase but `x`, etc.) is validated later by the MR
// pass.
RExpr
Parser::parse_mod_decl()
{
  LX::Token at = EX_TRY(m_lex.peek());
  if (LX::TokenTag::At != at.tag || at.str != "@mod")
    EX_ERR(ER::Code::EXPECTED_GLOBAL,
           at,
           "a source file must begin with a module header, '@mod NAME'");
  m_lex.next(); // '@mod'
  LX::Token name
    = EX_TRY(expect(LX::TokenTag::Word, "expected a module name after '@mod'"));
  Expr e{ ExprTag::ModDecl };
  e.as = ExModDecl{ name.str };
  return { true, alloc(e), {} };
}

// `$ with <lhs> [= <rhs>]` -- captured literally; the MR pass classifies it
// into a whole-module or single-symbol import. The leading `$` is already
// consumed.
RExpr
Parser::parse_import()
{
  m_lex.next(); // 'with'

  ExImport  im;
  LX::Token l1
    = EX_TRY(expect(LX::TokenTag::Word, "expected a name after 'with'"));
  im.lhs_name = l1.str;
  if (LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // '.'
    LX::Token l2 = EX_TRY(
      expect(LX::TokenTag::Word, "expected a name after '.' in an import"));
    im.lhs_prefix = l1.str;
    im.lhs_name   = l2.str;
  }

  if (LX::TokenTag::Eq == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // '='
    im.has_eq    = true;
    LX::Token r1 = EX_TRY(
      expect(LX::TokenTag::Word, "expected a module or 'MOD.sym' after '='"));
    im.rhs_name = r1.str;
    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // '.'
      LX::Token r2 = EX_TRY(
        expect(LX::TokenTag::Word, "expected a name after '.' in an import"));
      im.rhs_prefix = r1.str;
      im.rhs_name   = r2.str;
    }
  }

  Expr e{ ExprTag::Import };
  e.as = im;
  return { true, alloc(e), {} };
}

// `$ @private` / `$ @public` -- toggles export visibility of following symbols.
// The leading `$` is already consumed.
RExpr
Parser::parse_vis()
{
  LX::Token              at = EX_TRY(m_lex.next()); // '@private' / '@public'
  const OP::AtIntrinsic *in = OP::at_lookup(at.str, OP::AtNs::Decl);
  if (!in || (in->id != OP::AtId::Private && in->id != OP::AtId::Public))
    EX_ERR(ER::Code::UNSUPPORTED,
           at,
           "unknown directive '%s' (expected @private or @public)",
           std::string(at.str).c_str());
  bool is_private = in->id == OP::AtId::Private;
  Expr e{ ExprTag::Vis };
  e.as = ExVis{ is_private };
  return { true, alloc(e), {} };
}

// `@extern "C" "symbol" "lib"` -- a foreign binding, in plain application
// shape: the ABI first ("C" is the only one today; "js" is reserved for wasm
// imports), then the symbol, then the SYMBOLIC library name ("libc", "libm",
// "raylib"). No path or soname appears in source: the interpreter resolves
// the symbolic name to a host soname at dlopen time, the native backend to a
// link-line flag (doc/platform-abstraction.md section 3.3).
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

  LX::Token abi = EX_TRY(expect(
    LX::TokenTag::Str, "expected an \"ABI\" string after '@extern' (\"C\")"));
  LX::Token sym = EX_TRY(expect(
    LX::TokenTag::Str, "expected a \"symbol-name\" string after the ABI"));
  LX::Token lib = EX_TRY(expect(
    LX::TokenTag::Str, "expected a \"library\" string after the symbol"));

  UT::Vu abi_v = std::get<LX::TkStr>(abi.as).value;
  if (abi_v != "C")
    EX_ERR(ER::Code::UNSUPPORTED,
           abi,
           "unsupported @extern ABI '%s' (only \"C\" for now)",
           std::string(abi_v).c_str());

  return { true,
           mk_extern(abi_v,
                     std::get<LX::TkStr>(sym.as).value,
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
  e.as = ExStructDecl{ name.str, fields, name.str };
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
  Expr              *base = nullptr; // `..base` record-update spread, if any
  for (;;)
  {
    if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break;

    // A record-update spread `..base`: copy the unlisted fields from an
    // existing value of the same struct type. Must be the last entry. The type
    // may be written (`Person.{ ..base }`) or inferred from context, in which
    // case a bare `.{ ..base }` is settled at its lit site like any bare literal.
    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
        && LX::TokenTag::Dot == EX_TRY(m_lex.peek(1)).tag)
    {
      LX::Token dots = EX_TRY(m_lex.peek(0));
      m_lex.next(); // first '.'
      m_lex.next(); // second '.'
      base = EX_CTX(parse_expr(0), dots, "in the record-update source");
      break;
    }

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

  Expr        e{ ExprTag::StructLit };
  ExStructLit sl{ type_name, fields };
  sl.base = base;
  e.as    = sl;
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
  e.as = ExUnionDecl{ name.str, variants, name.str };
  return { true, alloc(e), {} };
}

// `$ Name : @alias = target` -- a transparent type alias. The body is a single
// type (the `=` is already consumed; `name` is the alias's type name).
RExpr
Parser::parse_alias_decl(
  const LX::Token &name)
{
  Ty  *target = EX_CTX(parse_type(),
                      name,
                      "in the target of type alias '%s'",
                      std::string(name.str).c_str());
  Expr e{ ExprTag::AliasDecl };
  e.as = ExAliasDecl{ name.str, target, name.str };
  return { true, alloc(e), {} };
}

// An effect declaration body: a comma-separated `op : type` list (trailing
// comma allowed), ending at the next global ('$') or end of input. Each `op` is
// a lowercase name; its type is a full signature `A -> B`. The name token was
// consumed by parse_global.
RExpr
Parser::parse_effect_decl(
  const LX::Token &name)
{
  char nc = name.str.size() ? name.str.data()[0] : '\0';
  if (nc < 'A' || nc > 'Z')
    EX_ERR(ER::Code::EXPECTED_GLOBAL,
           name,
           "an effect name must start uppercase, found '%s'",
           std::string(name.str).c_str());

  UT::Vec<FieldDecl> ops{ m_arena };
  for (;;)
  {
    LX::TokenTag tag = EX_TRY(m_lex.peek()).tag;
    if (LX::TokenTag::Dollar == tag || LX::TokenTag::Eof == tag) break;

    LX::Token oname
      = EX_TRY(expect(LX::TokenTag::Word, "expected an operation name"));
    char oc = oname.str.size() ? oname.str.data()[0] : '\0';
    if (oc < 'a' || oc > 'z')
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             oname,
             "an effect operation must start lowercase, found '%s'",
             std::string(oname.str).c_str());

    EX_TRY(
      expect(LX::TokenTag::Colon, "expected ':' after the operation name"));
    Ty *ty = EX_CTX(parse_type(),
                    oname,
                    "in the signature of operation '%s'",
                    std::string(oname.str).c_str());
    ops.push(FieldDecl{ oname.str, ty });

    if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
    m_lex.next(); // ','
  }

  Expr e{ ExprTag::EffectDecl };
  e.as = ExEffectDecl{ name.str, ops, name.str };
  return { true, alloc(e), {} };
}

// A variant construction. Called with `Type` and `Tag` already consumed; an
// optional `.{ ... }` payload follows. Without it the payload is unit (e.g.
// `Bool.True`). Payload values are positional (`{a, b}`) or named (`{.f = a}`),
// not mixed -- the same rule as a struct literal.
RExpr
Parser::parse_variant_lit(
  UT::Vu type_name, UT::Vu tag, const LX::Token &tok, UT::Vu qualifier)
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

      // A named field is `.field = value` with a lowercase field name. A
      // leading
      // `.` otherwise begins a positional value that is itself a bare literal
      // (`.{...}` or `.Tag...`, whose tag is uppercase), so let parse_expr take
      // it -- only `.lowercase =` is a field name here.
      LX::Token p1          = EX_TRY(m_lex.peek(1));
      LX::Token p2          = EX_TRY(m_lex.peek(2));
      bool      named_field = LX::TokenTag::Dot == EX_TRY(m_lex.peek(0)).tag
                         && LX::TokenTag::Word == p1.tag && p1.str.size()
                         && p1.str.data()[0] >= 'a' && p1.str.data()[0] <= 'z'
                         && LX::TokenTag::Eq == p2.tag;

      if (named_field)
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
  e.as = ExVariantLit{ type_name, tag, fields, tok.str, tok.line, qualifier };
  return { true, alloc(e), {} };
}

/*-------------------------------------------------------------------------------
 *\PARSE TYPES
 *
 * type ::= app ('->' type)?      -- '->' right-associative
 * app  ::= UpperWord atom*       -- type application, juxtaposition (Maybe Int)
 *        | atom
 * atom ::= UpperWord | TyVar | '(' type ')'
 *
 * Application binds tighter than '->' and only an uppercase con may head it; an
 * argument is a bare atom, so a nested application is parenthesized: `List
 * (Maybe Int)`.
 *------------------------------------------------------------------------------*/

// True when `t` can begin a type atom -- i.e. start an application argument.
// `{` starts the unit type `{}` and `@` a canonical base type (`@int64`);
// both are complete atoms in parse_type_atom, so `Map `T {}` and
// `Map `T @int64` apply like any other argument.
static bool
ty_atom_starts(
  const LX::Token &t)
{
  switch (t.tag)
  {
  case LX::TokenTag::Word:
    return t.str.size() && t.str.data()[0] >= 'A' && t.str.data()[0] <= 'Z';
  case LX::TokenTag::TyVar:
  case LX::TokenTag::At:
  case LX::TokenTag::LBrace:
  case LX::TokenTag::LParen: return true;
  default                  : return false;
  }
}

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

    if (LX::TokenTag::Dot == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // '.'
      LX::Token nm
        = EX_TRY(expect(LX::TokenTag::Word,
                        "expected a type name after '.' in a qualified type"));
      char nc = nm.str.size() ? nm.str.data()[0] : '\0';
      if (nc < 'A' || nc > 'Z')
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               nm,
               "a qualified type name must start uppercase, found '%s'",
               std::string(nm.str).c_str());
      TyCon tc{ nm.str, {} };
      tc.qualifier = t.str;
      return { true, mk_ty(Ty{ TyTag::Con, tc }), {} };
    }
    return { true, mk_ty(Ty{ TyTag::Con, TyCon{ t.str, {} } }), {} };
  }
  case LX::TokenTag::At:
  {
    if (!OP::is_base_type(t.str))
    {
      OP::AtNs ns;
      if (OP::at_classify(t.str, ns))
        EX_ERR(ER::Code::UNEXPECTED_TOKEN,
               t,
               "'%s' is a %s intrinsic, not a type",
               std::string(t.str).c_str(),
               OP::at_ns_name(ns));
      EX_ERR(ER::Code::UNEXPECTED_TOKEN,
             t,
             "unknown base type '%s'",
             std::string(t.str).c_str());
    }
    m_lex.next();
    return { true, mk_ty(Ty{ TyTag::Con, TyCon{ t.str, {} } }), {} };
  }
  case LX::TokenTag::TyVar:
  {
    m_lex.next();
    // strip the leading backtick for the stored name
    UT::Vu nm{ t.str.data() + 1, t.str.size() - 1 };
    return { true, mk_ty(Ty{ TyTag::Var, TyVar{ nm } }), {} };
  }
  case LX::TokenTag::LBrace:
  {
    // The unit type `{}` (empty record), or a tuple type `{A, B, ...}`.
    LX::Token lb = EX_TRY(m_lex.next()); // '{'
    if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag)
    {
      m_lex.next(); // '}'
      return { true,
               mk_ty(Ty{ TyTag::Con, TyCon{ UT::Vu{ OP::TY_UNIT, 2 }, {} } }),
               {} };
    }
    // A named-record parameter type `{a: T, b: U}`: sugar over the positional
    // tuple `{T, U}` whose field names become binders (parse_global erases them).
    // A type never starts with a lowercase word, so a lowercase field name
    // followed by ':' (or a leading `with`) unambiguously distinguishes this
    // from a positional tuple. A `with`-prefixed field also scopes its own
    // struct's fields into the body, as if by `with <field> in ..`.
    LX::Token f0    = EX_TRY(m_lex.peek());
    bool      named = LX::TokenTag::KwWith == f0.tag
              || (LX::TokenTag::Word == f0.tag && f0.str.size()
                  && f0.str.data()[0] >= 'a' && f0.str.data()[0] <= 'z'
                  && LX::TokenTag::Colon == EX_TRY(m_lex.peek(1)).tag);

    UT::Vec<Ty *>     elems{ m_arena };
    UT::Vec<RecField> fields{ m_arena };
    for (;;)
    {
      if (named)
      {
        bool with_scope = LX::TokenTag::KwWith == EX_TRY(m_lex.peek()).tag;
        if (with_scope) m_lex.next(); // 'with'
        LX::Token fn = EX_TRY(expect(
          LX::TokenTag::Word, "expected a field name in this named-record type"));
        char fc = fn.str.size() ? fn.str.data()[0] : '\0';
        if (fc < 'a' || fc > 'z')
          EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                 fn,
                 "a named-record field must be lowercase, found '%s'",
                 std::string(fn.str).c_str());
        EX_TRY(expect(LX::TokenTag::Colon,
                      "expected ':' after a named-record field name"));
        fields.push(RecField{ fn.str, with_scope });
      }
      elems.push(EX_CTX(parse_type(), lb, "in this tuple type"));
      if (LX::TokenTag::Comma != EX_TRY(m_lex.peek()).tag) break;
      m_lex.next();                                                // ','
      if (LX::TokenTag::RBrace == EX_TRY(m_lex.peek()).tag) break; // trailing
    }
    EX_TRY(
      expect(LX::TokenTag::RBrace, "expected '}' to close the tuple type"));
    UT::Vu nm = UT::strdup(m_arena, OP::tuple_name(elems.size()).c_str());
    return { true, mk_ty(Ty{ TyTag::Con, TyCon{ nm, elems, {}, fields } }), {} };
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
Parser::parse_type_app()
{
  Ty *head = EX_TRY(parse_type_atom());

  // Only an uppercase con may be applied; juxtaposed atoms are its arguments.
  if (TyTag::Con != head->tag || !ty_atom_starts(EX_TRY(m_lex.peek())))
    return { true, head, {} };

  UT::Vec<Ty *> args{ m_arena };
  while (ty_atom_starts(EX_TRY(m_lex.peek())))
    args.push(EX_TRY(parse_type_atom()));

  UT::Vu name = std::get<TyCon>(head->as).name;
  return { true, mk_ty(Ty{ TyTag::Con, TyCon{ name, args } }), {} };
}

RTy
Parser::parse_type()
{
  Ty *lhs = EX_TRY(parse_type_app());

  if (LX::TokenTag::Arrow == EX_TRY(m_lex.peek()).tag)
  {
    m_lex.next(); // '->'

    // Optional effect-row annotation between the arrow and the result type
    // (M3):
    //   <>                empty / pure         <E1, E2>         closed labels
    //   <`e>              just a row variable  <E1, E2 | `e>    open row
    // Op-char runs coalesce, so the opener arrives as `<`, `<>` or `<|`. Bare
    // `->` is implicitly pure too.
    UT::Vec<UT::Vu> eff_labels{ m_arena };
    UT::Vu          eff_tail{};
    bool            has_eff = false;

    LX::Token pk           = EX_TRY(m_lex.peek());
    bool      is_op        = LX::TokenTag::Op == pk.tag;
    bool      pending_tail = false; // a `<|` opener already consumed the '|'
    if (is_op && pk.str == "<>")
    {
      m_lex.next(); // '<>'
      has_eff = true;
    }
    else if ((is_op && pk.str == "<") || (is_op && pk.str == "<|"))
    {
      has_eff      = true;
      pending_tail = pk.str == "<|";
      m_lex.next(); // '<' or '<|'
      // Labels (comma-separated), an optional `| `e` tail, then `>`. A bare row
      // variable in label position is the tail (so `<`e>` is a pure row var).
      for (;;)
      {
        LX::Token t = EX_TRY(m_lex.peek());
        if (!pending_tail && LX::TokenTag::Op == t.tag && t.str == ">")
        {
          m_lex.next();
          break;
        }
        if (pending_tail || (LX::TokenTag::Op == t.tag && t.str == "|")
            || LX::TokenTag::TyVar == t.tag)
        {
          if (!pending_tail && LX::TokenTag::Op == t.tag) m_lex.next(); // '|'
          LX::Token tv = EX_TRY(m_lex.peek());
          if (LX::TokenTag::TyVar != tv.tag)
            EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                   tv,
                   "expected a row variable (e.g. `e) in the effect row, found "
                   "'%s'",
                   std::string(tv.str).c_str());
          m_lex.next();
          eff_tail        = UT::Vu{ tv.str.data() + 1, tv.str.size() - 1 };
          LX::Token close = EX_TRY(m_lex.peek());
          if (!(LX::TokenTag::Op == close.tag && close.str == ">"))
            EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                   close,
                   "expected '>' to close the effect row, found '%s'",
                   std::string(close.str).c_str());
          m_lex.next();
          break;
        }
        // An effect name: an uppercase Word.
        char c = t.str.size() ? t.str.data()[0] : '\0';
        if (LX::TokenTag::Word != t.tag || c < 'A' || c > 'Z')
          EX_ERR(ER::Code::UNEXPECTED_TOKEN,
                 t,
                 "expected an effect name (uppercase) in the effect row, found "
                 "'%s'",
                 std::string(t.str).c_str());
        m_lex.next();
        eff_labels.push(t.str);

        LX::Token sep = EX_TRY(m_lex.peek());
        if (LX::TokenTag::Comma == sep.tag) m_lex.next();
      }
    }

    Ty     *rhs = EX_TRY(parse_type()); // right-associative
    TyArrow a{ lhs, rhs, eff_labels, eff_tail, has_eff };
    return { true, mk_ty(Ty{ TyTag::Arrow, a }), {} };
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
  // Every file must open with a `@mod NAME` header declaring its module.
  {
    LX::RToken pk = m_lex.peek();
    if (!pk.ok)
    {
      m_diags.push_back(pk.err);
      return;
    }
    RExpr md = parse_mod_decl();
    if (!md.ok)
    {
      m_diags.push_back(md.err);
      return;
    }
    m_exprs.push(*md.value);
  }

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
