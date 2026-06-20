/*-------------------------------------------------------------------------------
 *\file LL.cpp
 *\info The pattern-lowering pass. See LL.hpp for the rationale.
 *-----------------------------------------------------------------------------*/

#include "LL.hpp"

#include "OP.hpp"

namespace LL
{

namespace
{

using EX::Expr;
using EX::ExprTag;
using EX::PatTag;
using EX::Pattern;

// Carries the arena, the collected struct shapes, and the running diagnostics
// through the recursive walk.
struct Lowerer
{
  AR::Arena                  &arena;
  std::vector<ER::Diagnostic> diags;
  // struct type name -> its field names, in declaration order
  std::unordered_map<std::string, std::vector<std::string>> structs;
  size_t                                                    fresh_n = 0;

  explicit Lowerer(
    AR::Arena &a)
      : arena{ a }
  {
  }

  /*----------------------------------------------------------------------------
   *\NODE BUILDERS
   *--------------------------------------------------------------------------*/

  Expr *
  alloc(
    Expr e)
  {
    Expr *p = (Expr *)arena.alloc<Expr>(1);
    *p      = e;
    return p;
  }

  // Copy a std::string into the arena as a UT::Vu so it outlives this pass.
  UT::Vu
  ustr(
    const std::string &s)
  {
    return UT::strdup(arena, s.c_str());
  }

  Expr *
  mk_var(
    UT::Vu name)
  {
    Expr e{ ExprTag::Var };
    e.as = EX::ExVar{ name };
    return alloc(e);
  }

  Expr *
  mk_field(
    Expr *record, UT::Vu field)
  {
    Expr e{ ExprTag::Field };
    e.as = EX::ExField{ record, field };
    return alloc(e);
  }

  EX::Ty *
  mk_con(
    UT::Vu name)
  {
    EX::Ty *t = (EX::Ty *)arena.alloc<EX::Ty>(1);
    *t        = EX::Ty{ EX::TyTag::Con, EX::TyCon{ name } };
    return t;
  }

  Expr *
  mk_let(
    UT::Vu var, Expr *val, Expr *body, EX::Ty *sig)
  {
    EX::ExLet lt;
    lt.var  = var;
    lt.val  = val;
    lt.body = body;
    lt.pat  = nullptr;
    lt.sig  = sig;
    Expr e{ ExprTag::Let };
    e.as = lt;
    return alloc(e);
  }

  Expr *
  mk_int(
    ssize_t v)
  {
    Expr e{ ExprTag::Int };
    e.as = EX::ExInt{ v };
    return alloc(e);
  }

  Expr *
  mk_real(
    double v)
  {
    Expr e{ ExprTag::Real };
    e.as = EX::ExReal{ v };
    return alloc(e);
  }

  Expr *
  mk_app(
    Expr *fn, Expr *arg)
  {
    Expr e{ ExprTag::App };
    e.as = EX::ExApp{ fn, arg };
    return alloc(e);
  }

  Expr *
  mk_if(
    Expr *cond, Expr *then, Expr *alt)
  {
    Expr e{ ExprTag::If };
    e.as = EX::ExIf{ cond, then, alt };
    return alloc(e);
  }

  // `lhs op rhs` as the application of an (overloaded) operator var, exactly as
  // the parser builds operators; TC resolves the overload.
  Expr *
  mk_binop(
    const char *op, Expr *lhs, Expr *rhs)
  {
    Expr v{ ExprTag::Var };
    v.as     = EX::ExVar{ UT::Vu{ op, std::strlen(op) } };
    Expr *fn = alloc(v);
    return mk_app(mk_app(fn, lhs), rhs);
  }

  // A fresh, collision-proof binder name. User identifiers never contain '$',
  // so a '$'-prefixed name cannot shadow or be shadowed by one.
  UT::Vu
  fresh(
    const char *hint)
  {
    return ustr("$" + std::string(hint) + std::to_string(fresh_n++));
  }

  void
  err(
    ER::Code code, UT::Vu anchor, size_t line, const std::string &msg)
  {
    diags.push_back(ER::mk_root(arena, code, anchor, line, ustr(msg)));
  }

  /*----------------------------------------------------------------------------
   *\PATTERN BINDING
   *--------------------------------------------------------------------------*/

  // Resolve a struct pattern's fields against the declaration: fill `subs[i]`
  // with the sub-pattern matched against declared field i (null => unmatched).
  // Records diagnostics for an unknown struct type, unknown/duplicate named
  // fields, or a positional arity mismatch. Returns false only on a fatal error
  // (unknown type / positional arity), where `subs` is unusable.
  bool
  resolve_fields(
    EX::PatStruct &ps, std::vector<Pattern *> &subs)
  {
    std::string tname = std::string(ps.type_name);
    auto        it    = structs.find(tname);
    if (it == structs.end())
    {
      err(ER::Code::TYPE_UNBOUND,
          ps.anchor,
          ps.line,
          "unknown struct type '" + tname + "' in pattern");
      return false;
    }
    const std::vector<std::string> &decl = it->second;
    subs.assign(decl.size(), nullptr);

    bool named = false;
    for (size_t i = 0; i < ps.fields.size(); ++i)
      if (ps.fields[i].name.size())
      {
        named = true;
        break;
      }

    if (named)
    {
      for (size_t i = 0; i < ps.fields.size(); ++i)
      {
        std::string fn  = std::string(ps.fields[i].name);
        size_t      idx = decl.size();
        for (size_t k = 0; k < decl.size(); ++k)
          if (decl[k] == fn) idx = k;
        if (idx == decl.size())
        {
          err(ER::Code::TYPE_UNBOUND,
              ps.anchor,
              ps.line,
              "struct '" + tname + "' has no field '" + fn + "'");
          continue;
        }
        if (subs[idx])
        {
          err(ER::Code::TYPE_MISMATCH,
              ps.anchor,
              ps.line,
              "field '" + fn + "' is matched more than once");
          continue;
        }
        subs[idx] = ps.fields[i].pat;
      }
      return true; // omitted fields stay null: ignored, fine in named mode
    }

    if (ps.fields.size() != decl.size())
    {
      err(ER::Code::TYPE_MISMATCH,
          ps.anchor,
          ps.line,
          "struct '" + tname + "' has " + std::to_string(decl.size())
            + " field(s) but the pattern lists "
            + std::to_string(ps.fields.size())
            + "; list every field positionally (use '_' to ignore one) or "
              "match by name");
      return false;
    }
    for (size_t i = 0; i < decl.size(); ++i) subs[i] = ps.fields[i].pat;
    return true;
  }

  // Wrap `body` in the bindings that destructure `pat` applied to the
  // (already-lowered) expression `subject`. `anchor`/`line` locate the
  // enclosing pattern for diagnostics. When `refutable_ok` is false (lambda /
  // let), a literal sub-pattern is an error; when true (match arm), its
  // equality test is emitted separately by `test_of`, so here it only
  // contributes no binding.
  Expr *
  bind_pattern(
    Pattern *pat,
    Expr    *subject,
    Expr    *body,
    UT::Vu   anchor,
    size_t   line,
    bool     refutable_ok)
  {
    switch (pat->tag)
    {
    case PatTag::Wild: return body; // matches anything, binds nothing
    case PatTag::Var:
      return mk_let(std::get<EX::PatVar>(pat->as).name, subject, body, nullptr);
    case PatTag::Int:
    case PatTag::Real:
    case PatTag::Str:
      if (!refutable_ok)
      {
        UT::Vu a = anchor;
        size_t l = line;
        pat_anchor(pat, a, l); // prefer the literal's own caret
        err(ER::Code::UNSUPPORTED,
            a,
            l,
            "a literal pattern is refutable and cannot appear in a lambda or "
            "'let' binding; use 'if .. is' to match it");
      }
      return body;
    case PatTag::Struct:
      return bind_struct(pat, subject, body, anchor, line, refutable_ok);
    }
    return body;
  }

  Expr *
  bind_struct(
    Pattern *pat,
    Expr    *subject,
    Expr    *body,
    UT::Vu   anchor,
    size_t   line,
    bool     refutable_ok)
  {
    auto                  &ps = std::get<EX::PatStruct>(pat->as);
    std::vector<Pattern *> subs;
    if (!resolve_fields(ps, subs)) return body;

    auto it = structs.find(std::string(ps.type_name));
    const std::vector<std::string> &decl = it->second;

    // Bind the subject to a fresh, struct-typed variable so each field access
    // has a statically known receiver type; then destructure the matched fields
    // last-to-first, so earlier fields nest outermost.
    UT::Vu sv    = fresh("s");
    Expr  *svar  = mk_var(sv);
    Expr  *inner = body;
    for (size_t i = decl.size(); i-- > 0;)
    {
      if (!subs[i]) continue; // unmatched field: no binding
      Expr *access = mk_field(svar, ustr(decl[i]));
      inner = bind_pattern(subs[i], access, inner, anchor, line, refutable_ok);
    }
    return mk_let(sv, subject, inner, mk_con(ps.type_name));
  }

  /*----------------------------------------------------------------------------
   *\MATCH TESTS
   *--------------------------------------------------------------------------*/

  // AND of two Int-valued (0/1) tests, short-circuiting; null means "always
  // true", so it is the identity.
  Expr *
  and_test(
    Expr *a, Expr *b)
  {
    if (!a) return b;
    if (!b) return a;
    return mk_if(a, b, mk_int(0));
  }

  // An Int-valued test that is nonzero iff `pat` matches `subject` (an
  // expression of statically known type). Returns null when `pat` is
  // irrefutable (always matches). Only the literal/struct-literal parts
  // contribute a test; variable/wildcard parts are bound separately by
  // `bind_pattern`.
  Expr *
  test_of(
    Pattern *pat, Expr *subject)
  {
    switch (pat->tag)
    {
    case PatTag::Wild:
    case PatTag::Var : return nullptr;
    case PatTag::Int:
      return mk_binop(
        OP::ISEQ, subject, mk_int(std::get<EX::PatInt>(pat->as).value));
    case PatTag::Real:
      return mk_binop(
        OP::ISEQ, subject, mk_real(std::get<EX::PatReal>(pat->as).value));
    case PatTag::Str:
    {
      auto &ps = std::get<EX::PatStr>(pat->as);
      err(
        ER::Code::UNSUPPORTED,
        ps.anchor,
        ps.line,
        "matching a string literal is not supported yet (no string equality)");
      return nullptr;
    }
    case PatTag::Struct:
    {
      auto                  &ps = std::get<EX::PatStruct>(pat->as);
      std::vector<Pattern *> subs;
      if (!resolve_fields(ps, subs)) return nullptr;

      const std::vector<std::string> &decl
        = structs.find(std::string(ps.type_name))->second;
      Expr *acc = nullptr;
      for (size_t i = 0; i < decl.size(); ++i)
      {
        if (!subs[i]) continue;
        Expr *t = test_of(subs[i], mk_field(subject, ustr(decl[i])));
        acc     = and_test(acc, t);
      }
      return acc;
    }
    }
    return nullptr;
  }

  // `if scrut is p1 then e1 .. is pn then en else d` lowers to a single binding
  // of the scrutinee followed by a chain of boolean ifs whose conditions are
  // the arms' equality tests and whose then-branches bind the arm's variables.
  // An irrefutable arm (no test) always matches, ending the chain.
  Expr *
  lower_match(
    Expr *e)
  {
    auto &m     = std::get<EX::ExMatch>(e->as);
    Expr *scrut = lower(m.scrut);
    Expr *alt   = lower(m.alt);

    // Pin the scrutinee's type from the first struct pattern, so the generated
    // field accesses have a statically known receiver type.
    EX::Ty *sig = nullptr;
    for (size_t i = 0; i < m.arms.size(); ++i)
      if (m.arms[i].pat->tag == PatTag::Struct)
      {
        sig = mk_con(std::get<EX::PatStruct>(m.arms[i].pat->as).type_name);
        break;
      }

    UT::Vu mv    = fresh("m");
    Expr  *mvar  = mk_var(mv);
    Expr  *chain = alt;
    for (size_t i = m.arms.size(); i-- > 0;)
    {
      Pattern *pat = m.arms[i].pat;
      UT::Vu   anc;
      size_t   line = 0;
      pat_anchor(pat, anc, line);
      Expr *test  = test_of(pat, mvar);
      Expr *bound = bind_pattern(
        pat, mvar, lower(m.arms[i].body), anc, line, /*refutable_ok=*/true);
      chain = test ? mk_if(test, bound, chain) : bound;
    }
    return mk_let(mv, scrut, chain, sig);
  }

  /*----------------------------------------------------------------------------
   *\TREE WALK
   *--------------------------------------------------------------------------*/

  // Lower `e` and return the (possibly replaced) node. Children are rewritten
  // in place; pattern lambdas/lets are replaced by their desugared equivalents.
  Expr *
  lower(
    Expr *e)
  {
    switch (e->tag)
    {
    case ExprTag::Int:
    case ExprTag::Real:
    case ExprTag::Str:
    case ExprTag::Var:
    case ExprTag::Extern:
    case ExprTag::StructDecl:
    case ExprTag::Unknown   : return e;

    case ExprTag::App:
    {
      auto &a = std::get<EX::ExApp>(e->as);
      a.fn    = lower(a.fn);
      a.arg   = lower(a.arg);
      return e;
    }
    case ExprTag::If:
    {
      auto &i = std::get<EX::ExIf>(e->as);
      i.cond  = lower(i.cond);
      i.then  = lower(i.then);
      i.alt   = lower(i.alt);
      return e;
    }
    case ExprTag::Match: return lower_match(e);
    case ExprTag::Field:
    {
      auto &f  = std::get<EX::ExField>(e->as);
      f.record = lower(f.record);
      return e;
    }
    case ExprTag::StructLit:
    {
      auto &sl = std::get<EX::ExStructLit>(e->as);
      for (size_t i = 0; i < sl.fields.size(); ++i)
        sl.fields[i].val = lower(sl.fields[i].val);
      return e;
    }
    case ExprTag::Def:
    {
      auto &d = std::get<EX::ExDef>(e->as);
      d.def   = lower(d.def);
      return e;
    }
    case ExprTag::Let:
    {
      auto &lt = std::get<EX::ExLet>(e->as);
      if (lt.pat)
      {
        Expr  *val  = lower(lt.val);
        Expr  *body = lower(lt.body);
        UT::Vu anc;
        size_t line = 0;
        pat_anchor(lt.pat, anc, line);
        return bind_pattern(
          lt.pat, val, body, anc, line, /*refutable_ok=*/false);
      }
      lt.val  = lower(lt.val);
      lt.body = lower(lt.body);
      return e;
    }
    case ExprTag::FnDef:
    {
      auto &fn = std::get<EX::ExFnDef>(e->as);
      if (fn.param_pat)
      {
        UT::Vu anc;
        size_t line = 0;
        pat_anchor(fn.param_pat, anc, line);
        UT::Vu pv   = fresh("p");
        Expr  *body = lower(fn.body);
        Expr  *dtor = bind_pattern(
          fn.param_pat, mk_var(pv), body, anc, line, /*refutable_ok=*/false);
        fn.param     = pv;
        fn.param_pat = nullptr;
        fn.body      = dtor;
        return e;
      }
      fn.body = lower(fn.body);
      return e;
    }
    }
    return e;
  }

  static void
  pat_anchor(
    Pattern *pat, UT::Vu &anchor, size_t &line)
  {
    switch (pat->tag)
    {
    case PatTag::Struct:
    {
      auto &ps = std::get<EX::PatStruct>(pat->as);
      anchor   = ps.anchor;
      line     = ps.line;
      break;
    }
    case PatTag::Int:
    {
      auto &p = std::get<EX::PatInt>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      break;
    }
    case PatTag::Real:
    {
      auto &p = std::get<EX::PatReal>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      break;
    }
    case PatTag::Str:
    {
      auto &p = std::get<EX::PatStr>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      break;
    }
    case PatTag::Wild:
    case PatTag::Var : break;
    }
  }

  std::vector<ER::Diagnostic>
  run(
    EX::Exprs &exprs)
  {
    // Collect struct declarations first so patterns may reference any of them
    // regardless of source order.
    for (size_t i = 0; i < exprs.size(); ++i)
    {
      if (exprs[i].tag != ExprTag::StructDecl) continue;
      auto                    &sd = std::get<EX::ExStructDecl>(exprs[i].as);
      std::vector<std::string> fns;
      for (size_t k = 0; k < sd.fields.size(); ++k)
        fns.push_back(std::string(sd.fields[k].name));
      structs[std::string(sd.name)] = std::move(fns);
    }

    for (size_t i = 0; i < exprs.size(); ++i)
      if (exprs[i].tag == ExprTag::Def)
      {
        auto &d = std::get<EX::ExDef>(exprs[i].as);
        d.def   = lower(d.def);
      }

    return std::move(diags);
  }
};

} // namespace

std::vector<ER::Diagnostic>
lower(
  EX::Exprs &exprs, AR::Arena &arena)
{
  Lowerer l{ arena };
  return l.run(exprs);
}

} // namespace LL
