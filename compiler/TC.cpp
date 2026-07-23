/*-------------------------------------------------------------------------------
 *\file TC.cpp
 *\info Hindley-Milner type checker.
 *
 * Runs Algorithm W over the EX::Expr tree (after PatLower removes
 * pattern sugar) with a union-find of unification variables. Operators are just
 * overloaded names (see overload_db): a use is typed as a fresh variable and
 * the overload is chosen by resolve_sites once its operands are known, writing
 * the impl key to the EX Var's `resolved` slot.
 * *----------------------------------------------------------------------------*/

#include "TC.hpp"
#include "ER.hpp"
#include "OP.hpp"
#include "TCxDATA.hpp"
#include "TG.hpp"
#include "UT.hpp"

namespace TC
{

namespace
{

/*------------------------------------------------------------------------------
 *\PATTERN LOWERING
 *
 * Turns the pattern surface (`when` arms, struct/variant patterns, pattern
 * `let`/lambda, string-prefix patterns) into the primitive core (`Case` + `if`
 * + `let` + field access) the checker and both engines already understand. Runs
 * inside TC::check once the struct/union tables are built, so it reads their
 * field names / variant tags straight from the checker's own declaration tables
 * (projected into `structs`/`unions` below) instead of re-deriving them.
 *-----------------------------------------------------------------------------*/

struct PatLower
{
  using Expr    = EX::Expr;
  using ExprTag = EX::ExprTag;
  using PatTag  = EX::PatTag;
  using Pattern = EX::Pattern;

  AR::Arena  &arena;
  Diagnostics diags;
  const char *int_ty; // the target's Int

  // struct type name -> its field names, in declaration order
  std::unordered_map<std::string, std::vector<std::string>> structs;
  // One variant of a union: its tag and its payload field names (empty for an
  // anonymous positional field). Its index in the union's vector is its tag.
  struct UVariant
  {
    std::string              tag;
    std::vector<std::string> fields;
  };
  // union type name -> its variants, in declaration order
  std::unordered_map<std::string, std::vector<UVariant>> unions;
  size_t                                                 fresh_n = 0;

  PatLower(
    AR::Arena &a, const StructTable &st, const UnionTable &ut, const char *ity)
      : arena{ a },
        int_ty{ ity }
  {
    for (const auto &kv : st)
    {
      std::vector<std::string> fns;
      for (const auto &f : kv.second.fields) fns.push_back(f.first);
      structs[kv.first] = std::move(fns);
    }
    for (const auto &kv : ut)
    {
      std::vector<UVariant> vs;
      for (const VariantShape &v : kv.second.variants)
      {
        std::vector<std::string> fns;
        for (const auto &f : v.fields) fns.push_back(f.first);
        vs.push_back({ v.tag, std::move(fns) });
      }
      unions[kv.first] = std::move(vs);
    }
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
    *t        = EX::Ty{ EX::TyTag::Con, EX::TyCon{ name, {} } };
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
  mk_str(
    UT::Vu value)
  {
    Expr e{ ExprTag::Str };
    e.as = EX::ExStr{ value };
    return alloc(e);
  }

  // A Str-equality test `subject ?= "lit"` -> Int (1/0). TC resolves the `?=`
  // overload to `?=@Str` from the operand types. Backs exact string patterns.
  Expr *
  str_eq(
    Expr *subject, UT::Vu lit)
  {
    return mk_iseq(OP::TY_STR, subject, mk_str(lit));
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

  Expr *
  mk_iseq(
    const char *ty, Expr *lhs, Expr *rhs)
  {
    return mk_cmp(OP::ISEQ, ty, lhs, rhs);
  }

  Expr *
  mk_cmp(
    const char *op, const char *ty, Expr *lhs, Expr *rhs)
  {
    Expr v{ ExprTag::Var };
    v.as     = EX::ExVar{ ustr(OP::mono(op, ty)) };
    Expr *fn = alloc(v);
    return mk_app(mk_app(fn, lhs), rhs);
  }

  Pattern *
  alloc_pat(
    Pattern p)
  {
    Pattern *q = (Pattern *)arena.alloc<Pattern>(1);
    *q         = p;
    return q;
  }

  Pattern *
  mk_cons_pat(
    Pattern *h, Pattern *t, UT::Vu a, size_t ln)
  {
    UT::Vec<EX::FieldPat> fields{ arena };
    fields.push(EX::FieldPat{ UT::Vu{}, h });
    fields.push(EX::FieldPat{ UT::Vu{}, t });
    return alloc_pat(Pattern{ PatTag::Variant,
                              EX::PatVariant{ UT::Vu{ "List", 4 },
                                              UT::Vu{ "Cons", 4 },
                                              fields,
                                              a,
                                              ln,
                                              UT::Vu{ "List", 4 } } });
  }
  Pattern *
  mk_nil_pat(
    UT::Vu a, size_t ln)
  {
    UT::Vec<EX::FieldPat> fields{ arena };
    return alloc_pat(Pattern{ PatTag::Variant,
                              EX::PatVariant{ UT::Vu{ "List", 4 },
                                              UT::Vu{ "Nil", 3 },
                                              fields,
                                              a,
                                              ln,
                                              UT::Vu{ "List", 4 } } });
  }

  Pattern *
  seq_as_list(
    EX::PatSeq &pq)
  {
    Pattern *acc = pq.rest ? pq.rest : mk_nil_pat(pq.anchor, pq.line);
    for (size_t i = pq.elems.size(); i-- > 0;)
      acc = mk_cons_pat(pq.elems[i], acc, pq.anchor, pq.line);
    return acc;
  }

  // A call to a named (possibly overloaded) builtin: `name arg0 arg1 ..`. TC
  // resolves the overload from the argument types, just like for a source call.
  Expr *
  mk_call(
    const char *name, std::initializer_list<Expr *> args)
  {
    Expr v{ ExprTag::Var };
    v.as     = EX::ExVar{ UT::Vu{ name, std::strlen(name) } };
    Expr *fn = alloc(v);
    for (Expr *a : args) fn = mk_app(fn, a);
    return fn;
  }

  Expr *
  str_head(
    Expr *subject, size_t plen)
  {
    return mk_call(OP::ARR_SLICE,
                   { subject, mk_int(0), mk_int((ssize_t)plen) });
  }

  Expr *
  str_tail(
    Expr *subject, size_t plen)
  {
    return mk_call(
      OP::ARR_SLICE,
      { subject, mk_int((ssize_t)plen), mk_call(OP::ARR_LEN, { subject }) });
  }

  Expr *
  mk_unit()
  {
    Expr e{ ExprTag::Unit };
    e.as = EX::ExUnit{};
    return alloc(e);
  }

  // A thunk `\_ = body`: an ExFnDef ignoring its argument. Suspends a match's
  // fallthrough continuation so it runs only when an arm actually falls through
  // (and is shared, not duplicated, across guarded arms).
  Expr *
  mk_thunk(
    Expr *body)
  {
    EX::ExFnDef fn;
    fn.param = UT::Vu{ "_", 1 };
    fn.body  = body;
    Expr e{ ExprTag::FnDef };
    e.as = fn;
    return alloc(e);
  }

  // A fresh, collision-proof binder name. User identifiers never contain '$'.
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
  // TC (type_pattern / match_fieldpats) has already validated the type name and
  // every field, so any mismatch here is a compiler bug.
  void
  resolve_fields(
    EX::PatStruct &ps, std::vector<Pattern *> &subs)
  {
    std::string tname = std::string(ps.type_name);
    auto        it    = structs.find(tname);
    UT_FAIL_IF(it == structs.end());
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
        UT_FAIL_IF(idx == decl.size() || subs[idx]);
        subs[idx] = ps.fields[i].pat;
      }
      return; // omitted fields stay null: ignored, fine in named mode
    }

    UT_FAIL_IF(ps.fields.size() != decl.size());
    for (size_t i = 0; i < decl.size(); ++i) subs[i] = ps.fields[i].pat;
  }

  // Wrap `body` in the bindings that destructure `pat` applied to the
  // (already-lowered) expression `subject`. When `refutable_ok` is false
  // (lambda / let), a refutable sub-pattern is an error.
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
    case PatTag::Bool:
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
            "'let' binding; use 'when' to match it");
      }
      return body;
    case PatTag::StrPrefix:
    {
      auto &pp = std::get<EX::PatStrPrefix>(pat->as);
      if (!refutable_ok)
      {
        err(ER::Code::UNSUPPORTED,
            pp.anchor,
            pp.line,
            "a string-prefix pattern is refutable and cannot appear in a "
            "lambda or 'let' binding; use 'when' to match it");
        return body;
      }
      return bind_pattern(pp.rest,
                          str_tail(subject, pp.prefix.size()),
                          body,
                          anchor,
                          line,
                          refutable_ok);
    }
    case PatTag::Variant:
      // Refutable (it tests the constructor tag); only `when` may use it, where
      // lower_match_variant handles it before bind_pattern is reached.
      if (!refutable_ok)
      {
        auto &pv = std::get<EX::PatVariant>(pat->as);
        err(ER::Code::UNSUPPORTED,
            pv.anchor,
            pv.line,
            "a variant pattern is refutable and cannot appear in a lambda or "
            "'let' binding; use 'when' to match it");
      }
      return body;
    case PatTag::Struct:
      return bind_struct(pat, subject, body, anchor, line, refutable_ok);
    case PatTag::Seq:
      // Refutable (length / nil test); only `when` may use it.
      if (!refutable_ok)
      {
        auto &pq = std::get<EX::PatSeq>(pat->as);
        err(ER::Code::UNSUPPORTED,
            pq.anchor,
            pq.line,
            "a sequence pattern is refutable and cannot appear in a lambda or "
            "'let' binding; use 'when' to match it");
      }
      return body;
    }
    UT_FAIL_MSG("%s", "bind_pattern: unhandled PatTag");
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
    resolve_fields(ps, subs);

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

  // An Int-valued test that is nonzero iff `pat` matches `subject`. Returns
  // null when `pat` is irrefutable. Only the literal/struct-literal parts
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
      return mk_iseq(
        int_ty, subject, mk_int(std::get<EX::PatInt>(pat->as).value));
    case PatTag::Bool:
      return mk_iseq(
        int_ty, subject, mk_int(std::get<EX::PatBool>(pat->as).value ? 1 : 0));
    case PatTag::Real:
      return mk_iseq(
        OP::TY_REAL, subject, mk_real(std::get<EX::PatReal>(pat->as).value));
    case PatTag::Str:
      return str_eq(subject, std::get<EX::PatStr>(pat->as).value);
    case PatTag::StrPrefix:
    {
      auto  &pp        = std::get<EX::PatStrPrefix>(pat->as);
      size_t plen      = pp.prefix.size();
      Expr  *head_test = str_eq(str_head(subject, plen), pp.prefix);
      Expr  *rest_test = test_of(pp.rest, str_tail(subject, plen));
      return and_test(head_test, rest_test);
    }
    case PatTag::Struct:
    {
      auto                  &ps = std::get<EX::PatStruct>(pat->as);
      std::vector<Pattern *> subs;
      resolve_fields(ps, subs);

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
    case PatTag::Variant: return nullptr;
    case PatTag::Seq:
      // Seq patterns always route through match_seq (the guarded path). Never a
      // plain boolean test. Defensive only.
      return nullptr;
    }
    UT_FAIL_MSG("%s", "test_of: unhandled PatTag");
    return nullptr;
  }

  // Whether matching `p` needs a constructor tag test that `test_of`'s flat
  // boolean form cannot express (a variant, a seq, a string-prefix) -- possibly
  // buried in a struct field. Such a pattern must take the guarded path, whose
  // match_pat recurses; otherwise the tag test would be silently dropped.
  bool
  needs_case_test(
    Pattern *p)
  {
    switch (p->tag)
    {
    case PatTag::Variant:
    case PatTag::Seq:
    case PatTag::StrPrefix: return true;
    case PatTag::Struct:
    {
      auto                  &ps = std::get<EX::PatStruct>(p->as);
      std::vector<Pattern *> subs;
      resolve_fields(ps, subs);
      for (Pattern *s : subs)
        if (s && needs_case_test(s)) return true;
      return false;
    }
    default: return false;
    }
  }

  // `when scrut is p1 then e1 .. is pn then en else d` lowers to a binding of
  // the scrutinee and a `case` over it. A struct-free match becomes one
  // multi-way `case`; a match with struct patterns keeps the chain-of-`if`s
  // form, each guard testing a field.
  Expr *
  lower_match(
    Expr *e)
  {
    auto &m     = std::get<EX::ExMatch>(e->as);
    Expr *scrut = lower(m.scrut);
    // No `else` -> TC already proved the arms exhaustive, so this default is
    // dead; a unit literal keeps the lowered `case` well-formed (uniform Value
    // representation) without inventing a trap that would never run.
    Expr *alt = m.alt ? lower(m.alt) : mk_unit();

    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      Pattern *pat = m.arms[i].pat;
      if (pat->tag == PatTag::Seq && !std::get<EX::PatSeq>(pat->as).is_array)
        m.arms[i].pat = seq_as_list(std::get<EX::PatSeq>(pat->as));
    }

    bool has_struct = false, has_variant = false, has_guard = false,
         has_nested = false, has_str = false, has_seq = false;
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      Pattern *pat = m.arms[i].pat;
      if (pat->tag == PatTag::Struct)
      {
        has_struct = true;
        // A struct field that needs a constructor tag test (a nested variant /
        // seq) can't be expressed by the flat struct path's boolean `test_of`;
        // route the whole match through the guarded path, which recurses.
        auto                  &ps = std::get<EX::PatStruct>(pat->as);
        std::vector<Pattern *> subs;
        resolve_fields(ps, subs);
        for (Pattern *s : subs)
          if (s && needs_case_test(s)) has_nested = true;
      }
      if (pat->tag == PatTag::Str || pat->tag == PatTag::StrPrefix)
        has_str = true;
      if (pat->tag == PatTag::Seq) has_seq = true;
      if (m.arms[i].guard) has_guard = true;
      if (pat->tag == PatTag::Variant)
      {
        has_variant = true;
        auto &pv    = std::get<EX::PatVariant>(pat->as);
        // A payload slot that is not a bare variable / wildcard is a nested
        // sub-pattern -- e.g. the `List.Nil` a `[a]` list pattern desugars to.
        for (size_t k = 0; k < pv.fields.size(); ++k)
          if (pv.fields[k].pat->tag != PatTag::Var
              && pv.fields[k].pat->tag != PatTag::Wild)
            has_nested = true;
      }
    }

    if (has_guard || has_nested || has_str || has_seq)
      return lower_match_guarded(m, scrut, alt);

    if (has_variant) return lower_match_variant(m, scrut, alt);
    if (!has_struct) return lower_match_flat(m, scrut, alt);

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

  // Guarded (or otherwise fallthrough-needing) match. Every arm is tried in
  // order; an arm succeeds only if its pattern matches AND its optional guard
  // holds, otherwise control falls through to the next arm and finally to the
  // `else`. The fallthrough is threaded as a chain of thunks so it is shared.
  Expr *
  lower_match_guarded(
    EX::ExMatch &m, Expr *scrut, Expr *alt)
  {
    UT::Vu  mv   = fresh("m");
    Expr   *mvar = mk_var(mv);
    EX::Ty *sig  = nullptr; // scrutinee type pin (struct / union)

    Expr *acc = alt; // fallthrough base: the `else`
    for (size_t i = m.arms.size(); i-- > 0;)
    {
      UT::Vu fv    = fresh("f");
      Expr  *thunk = mk_thunk(acc);
      Expr  *arm   = build_guarded_arm(m.arms[i], mvar, fv, &sig);
      acc          = mk_let(fv, thunk, arm, nullptr);
    }
    return mk_let(mv, scrut, acc, sig);
  }

  // One arm of a guarded match. `fv` names the fallthrough thunk to invoke (as
  // `$fv {}`) when this arm's pattern or guard does not match.
  Expr *
  build_guarded_arm(
    EX::MatchArm &arm, Expr *mvar, UT::Vu fv, EX::Ty **sig)
  {
    Pattern *pat  = arm.pat;
    Expr    *body = lower(arm.body);
    Fall     fall = [&]() { return mk_app(mk_var(fv), mk_unit()); };
    // Run `body` only when the guard holds (it sees the pattern's bindings, so
    // it sits at the innermost success point), else fall through.
    Expr *success = arm.guard ? mk_if(lower(arm.guard), body, fall()) : body;
    return match_pat(pat, mvar, success, fall, sig, /*top=*/true);
  }

  // Lower a struct-free match into `let $m = scrut in case $m of lit -> e ..
  // else d`. The first irrefutable (variable / wildcard) arm becomes the
  // default, binding the scrutinee and ending the scan (any later arm is dead).
  Expr *
  lower_match_flat(
    EX::ExMatch &m, Expr *scrut, Expr *deflt)
  {
    UT::Vu mv   = fresh("m");
    Expr  *mvar = mk_var(mv);

    UT::Vec<EX::CaseAlt> alts{ arena };
    Expr                *dflt = deflt;
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      Pattern *pat  = m.arms[i].pat;
      Expr    *body = lower(m.arms[i].body);
      if (pat->tag == PatTag::Int)
      {
        EX::CaseAlt a;
        a.kind = EX::AltKind::Int;
        a.ival = std::get<EX::PatInt>(pat->as).value;
        a.body = body;
        alts.push(a);
      }
      else if (pat->tag == PatTag::Bool)
      {
        EX::CaseAlt a;
        a.kind = EX::AltKind::Int;
        a.ival = std::get<EX::PatBool>(pat->as).value ? 1 : 0;
        a.body = body;
        alts.push(a);
      }
      else if (pat->tag == PatTag::Real)
      {
        EX::CaseAlt a;
        a.kind = EX::AltKind::Real;
        a.rval = std::get<EX::PatReal>(pat->as).value;
        a.body = body;
        alts.push(a);
      }
      else if (pat->tag == PatTag::Str)
        // A string arm sets has_str, routing the whole match to the guarded
        // lowering; the flat path never sees one.
        UT_FAIL_MSG("%s",
                    "lower_match_flat: string pattern should route to "
                    "the guarded lowering");
      else // Var or wildcard: irrefutable, always matches -> the default
      {
        UT::Vu anc;
        size_t line = 0;
        pat_anchor(pat, anc, line);
        dflt = bind_pattern(pat, mvar, body, anc, line, /*refutable_ok=*/true);
        break;
      }
    }

    EX::ExCase c;
    c.scrut = mvar;
    c.alts  = alts;
    c.deflt = dflt;
    Expr ce{ ExprTag::Case };
    ce.as = c;
    return mk_let(mv, scrut, alloc(ce), nullptr);
  }

  // Resolve a variant pattern's payload sub-patterns against the variant's
  // declared field names, filling `subs[i]` (null => unmatched). Mirrors
  // resolve_fields; TC has already validated every field, so a mismatch here is
  // a compiler bug.
  void
  resolve_payload(
    EX::PatVariant                 &pv,
    const std::vector<std::string> &decl,
    std::vector<Pattern *>         &subs)
  {
    subs.assign(decl.size(), nullptr);

    bool named = false;
    for (size_t i = 0; i < pv.fields.size(); ++i)
      if (pv.fields[i].name.size())
      {
        named = true;
        break;
      }

    if (named)
    {
      for (size_t i = 0; i < pv.fields.size(); ++i)
      {
        std::string fn  = std::string(pv.fields[i].name);
        size_t      idx = decl.size();
        for (size_t k = 0; k < decl.size(); ++k)
          if (decl[k] == fn) idx = k;
        UT_FAIL_IF(idx == decl.size() || subs[idx]);
        subs[idx] = pv.fields[i].pat;
      }
      return;
    }

    UT_FAIL_IF(pv.fields.size() != decl.size());
    for (size_t i = 0; i < decl.size(); ++i) subs[i] = pv.fields[i].pat;
  }

  void
  resolve_variant(
    EX::PatVariant                  &pv,
    UT::Vu                          &uname_vu,
    size_t                          &tagidx,
    const std::vector<std::string> *&decl)
  {
    UT_FAIL_IF(pv.resolved_union.size() == 0);
    auto uit = unions.find(std::string(pv.resolved_union));
    UT_FAIL_IF(uit == unions.end());
    uname_vu = pv.resolved_union;
    tagidx   = uit->second.size();
    for (size_t k = 0; k < uit->second.size(); ++k)
      if (uit->second[k].tag == std::string(pv.tag)) tagidx = k;
    UT_FAIL_IF(tagidx == uit->second.size());
    decl = &uit->second[tagidx].fields;
  }

  void
  make_variant_alt(
    EX::PatVariant &pv, Expr *body, EX::Ty **sig, EX::CaseAlt &out)
  {
    UT::Vu                          uname_vu;
    size_t                          tagidx;
    const std::vector<std::string> *declp;
    resolve_variant(pv, uname_vu, tagidx, declp);
    const std::vector<std::string> &decl = *declp;
    if (!*sig) *sig = mk_con(uname_vu);

    std::vector<Pattern *> subs;
    resolve_payload(pv, decl, subs);

    UT::Vec<UT::Vu> binders{ arena };
    for (size_t k = 0; k < decl.size(); ++k)
    {
      Pattern *sp = subs[k];
      if (!sp || sp->tag == PatTag::Wild)
        binders.push(UT::Vu{});
      else if (sp->tag == PatTag::Var)
        binders.push(std::get<EX::PatVar>(sp->as).name);
      else
        UT_FAIL_MSG("%s",
                    "make_variant_alt: nested payload should route to "
                    "the guarded lowering");
    }

    out.kind      = EX::AltKind::Con;
    out.type_name = uname_vu;
    out.ctor      = pv.tag;
    out.tag       = tagidx;
    out.binders   = binders;
    out.body      = body;
  }

  // A fallthrough continuation: makes a *fresh* "fall to the next arm" node on
  // each call, since one arm's matching may fail at several points and each
  // needs its own use site of the shared thunk.
  using Fall = std::function<Expr *()>;

  // Match `pat` against `subject`, evaluating `success` (with `pat`'s bindings
  // in scope) on a match and `fall()` otherwise -- recursing into nested
  // variant payloads so that a failure at any depth lands on the same
  // fallthrough.
  Expr *
  match_pat(
    Pattern    *pat,
    Expr       *subject,
    Expr       *success,
    const Fall &fall,
    EX::Ty    **sig,
    bool        top)
  {
    switch (pat->tag)
    {
    case PatTag::Wild: return success;
    case PatTag::Var:
      return mk_let(
        std::get<EX::PatVar>(pat->as).name, subject, success, nullptr);
    case PatTag::Int:
      return mk_if(
        mk_iseq(int_ty, subject, mk_int(std::get<EX::PatInt>(pat->as).value)),
        success,
        fall());
    case PatTag::Bool:
      return mk_if(
        mk_iseq(int_ty,
                subject,
                mk_int(std::get<EX::PatBool>(pat->as).value ? 1 : 0)),
        success,
        fall());
    case PatTag::Real:
      return mk_if(mk_iseq(OP::TY_REAL,
                           subject,
                           mk_real(std::get<EX::PatReal>(pat->as).value)),
                   success,
                   fall());
    case PatTag::Str:
      return mk_if(
        str_eq(subject, std::get<EX::PatStr>(pat->as).value), success, fall());
    case PatTag::StrPrefix:
    {
      Expr  *test = test_of(pat, subject);
      UT::Vu anc;
      size_t line = 0;
      pat_anchor(pat, anc, line);
      Expr *bound
        = bind_pattern(pat, subject, success, anc, line, /*refutable_ok=*/true);
      return test ? mk_if(test, bound, fall()) : bound;
    }
    case PatTag::Struct:
    {
      // A struct has no constructor tag; match each field against its
      // sub-pattern, recursing (and threading the same `fall`) so a refutable
      // field -- a nested variant, another struct -- is actually tested rather
      // than silently ignored. Fields fold inside-out around `success`, so all
      // their bindings are in scope for it.
      auto &ps = std::get<EX::PatStruct>(pat->as);
      if (top && !*sig) *sig = mk_con(ps.type_name);
      std::vector<Pattern *> subs;
      resolve_fields(ps, subs);
      auto dit = structs.find(std::string(ps.type_name));
      if (dit == structs.end()) return success; // TC already errored; defensive
      const std::vector<std::string> &decl  = dit->second;
      Expr                           *inner = success;
      for (size_t i = decl.size(); i-- > 0;)
      {
        if (!subs[i] || subs[i]->tag == PatTag::Wild) continue;
        inner = match_pat(subs[i],
                          mk_field(subject, ustr(decl[i])),
                          inner,
                          fall,
                          sig,
                          /*top=*/false);
      }
      return inner;
    }
    case PatTag::Variant:
      return match_variant(
        std::get<EX::PatVariant>(pat->as), subject, success, fall, sig, top);
    case PatTag::Seq: return match_seq(pat, subject, success, fall, sig, top);
    }
    UT_FAIL_MSG("%s", "match_pat: unhandled PatTag");
    return fall();
  }

  Expr *
  match_seq(
    Pattern    *pat,
    Expr       *subject,
    Expr       *success,
    const Fall &fall,
    EX::Ty    **sig,
    bool        top)
  {
    auto &pq = std::get<EX::PatSeq>(pat->as);
    if (!pq.is_array)
      return match_pat(seq_as_list(pq), subject, success, fall, sig, top);

    if (top && !*sig) *sig = mk_con(ustr(OP::TY_ARRAY));
    size_t k       = pq.elems.size();
    Expr  *lenval  = mk_call(OP::ARR_LEN, { subject });
    Expr  *lentest = pq.rest
                       ? mk_cmp(OP::GEQ, int_ty, lenval, mk_int((ssize_t)k))
                       : mk_iseq(int_ty, lenval, mk_int((ssize_t)k));

    Expr *inner = success;
    if (pq.rest)
    {
      Expr *slice = mk_call(
        OP::ARR_SLICE,
        { subject, mk_int((ssize_t)k), mk_call(OP::ARR_LEN, { subject }) });
      inner = match_pat(pq.rest, slice, inner, fall, sig, /*top=*/false);
    }
    for (size_t i = k; i-- > 0;)
    {
      Expr *geti = mk_call(OP::ARR_GET, { subject, mk_int((ssize_t)i) });
      inner = match_pat(pq.elems[i], geti, inner, fall, sig, /*top=*/false);
    }
    return mk_if(lentest, inner, fall());
  }

  // Match variant pattern `pv` against `subject`: a `case` over the constructor
  // tag with a single Con alt (its payload bound to fresh slots, then matched
  // recursively) whose default is `fall()`.
  Expr *
  match_variant(
    EX::PatVariant &pv,
    Expr           *subject,
    Expr           *success,
    const Fall     &fall,
    EX::Ty        **sig,
    bool            top)
  {
    UT::Vu                          uname_vu;
    size_t                          tagidx;
    const std::vector<std::string> *declp;
    resolve_variant(pv, uname_vu, tagidx, declp);
    const std::vector<std::string> &decl = *declp;

    std::vector<Pattern *> subs;
    resolve_payload(pv, decl, subs);
    if (top && !*sig) *sig = mk_con(uname_vu);

    // One binder per payload slot: empty ignores it, a plain variable binds it
    // directly, and anything else binds a fresh slot to be matched recursively.
    UT::Vec<UT::Vu>                           binders{ arena };
    std::vector<std::pair<UT::Vu, Pattern *>> nested;
    for (size_t k = 0; k < decl.size(); ++k)
    {
      Pattern *sp = subs[k];
      if (!sp || sp->tag == PatTag::Wild)
        binders.push(UT::Vu{});
      else if (sp->tag == PatTag::Var)
        binders.push(std::get<EX::PatVar>(sp->as).name);
      else
      {
        UT::Vu v = fresh("v");
        binders.push(v);
        nested.push_back({ v, sp });
      }
    }

    // Fold the nested matches inside-out around `success` (earlier slots
    // outermost), each threading the same `fall`.
    Expr *inner = success;
    for (size_t i = nested.size(); i-- > 0;)
      inner = match_pat(nested[i].second,
                        mk_var(nested[i].first),
                        inner,
                        fall,
                        sig,
                        /*top=*/false);

    EX::CaseAlt a;
    a.kind      = EX::AltKind::Con;
    a.type_name = uname_vu;
    a.ctor      = pv.tag;
    a.tag       = tagidx;
    a.binders   = binders;
    a.body      = inner;

    EX::ExCase c;
    c.scrut = subject;
    c.alts  = UT::Vec<EX::CaseAlt>{ arena };
    c.alts.push(a);
    c.deflt = fall();
    Expr ce{ ExprTag::Case };
    ce.as = c;
    return alloc(ce);
  }

  // `when scrut is Type.Tag.{..} then e .. else d` lowers to `let $m = scrut in
  // case $m of <Con alt>.. else d`. This flat form handles only variable / `_`
  // payload slots; a nested payload sub-pattern routes to lower_match_guarded.
  Expr *
  lower_match_variant(
    EX::ExMatch &m, Expr *scrut, Expr *deflt)
  {
    UT::Vu mv   = fresh("m");
    Expr  *mvar = mk_var(mv);

    UT::Vec<EX::CaseAlt> alts{ arena };
    Expr                *dflt = deflt;
    EX::Ty              *sig  = nullptr; // pin the scrutinee to the union type

    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      Pattern *pat  = m.arms[i].pat;
      Expr    *body = lower(m.arms[i].body);

      if (pat->tag != PatTag::Variant)
      {
        UT_FAIL_IF(pat->tag != PatTag::Var && pat->tag != PatTag::Wild);
        UT::Vu anc;
        size_t line = 0;
        pat_anchor(pat, anc, line);
        dflt = bind_pattern(pat, mvar, body, anc, line, /*refutable_ok=*/true);
        break; // an irrefutable arm ends the scan
      }

      auto       &pv = std::get<EX::PatVariant>(pat->as);
      EX::CaseAlt a;
      make_variant_alt(pv, body, &sig, a);
      alts.push(a);
    }

    EX::ExCase c;
    c.scrut = mvar;
    c.alts  = alts;
    c.deflt = dflt;
    Expr ce{ ExprTag::Case };
    ce.as = c;
    return mk_let(mv, scrut, alloc(ce), sig);
  }

  // Reorder a named variant literal's payload into declared field order and
  // drop the names, so CR (which builds payloads positionally) and a Con `case`
  // see fields in the same order. Anything malformed is left untouched for TC.
  void
  normalize_variant_lit(
    EX::ExVariantLit &vl)
  {
    bool named = false;
    for (size_t i = 0; i < vl.fields.size(); ++i)
      if (vl.fields[i].name.size())
      {
        named = true;
        break;
      }
    if (!named) return;

    auto uit = unions.find(std::string(vl.type_name));
    if (uit == unions.end()) return;
    const std::vector<std::string> *decl = nullptr;
    for (size_t k = 0; k < uit->second.size(); ++k)
      if (uit->second[k].tag == std::string(vl.tag))
        decl = &uit->second[k].fields;
    if (!decl || vl.fields.size() != decl->size()) return;

    UT::Vec<EX::FieldInit> out{ arena };
    for (size_t k = 0; k < decl->size(); ++k)
    {
      Expr *val = nullptr;
      for (size_t i = 0; i < vl.fields.size(); ++i)
        if (std::string(vl.fields[i].name) == (*decl)[k])
          val = vl.fields[i].val;
      if (!val) return; // unknown/missing name: leave original for TC
      out.push(EX::FieldInit{ UT::Vu{}, val });
    }
    vl.fields = out;
  }

  /*----------------------------------------------------------------------------
   *\TREE WALK
   *--------------------------------------------------------------------------*/

  // Lower `e` and return the (possibly replaced) node. Children are rewritten
  // in place; pattern lambdas/lets/matches are replaced by their desugarings.
  Expr *
  lower(
    Expr *e)
  {
    switch (e->tag)
    {
    case ExprTag::Int:
    case ExprTag::Bool:
    case ExprTag::Real:
    case ExprTag::Str:
    case ExprTag::Unit:
    case ExprTag::Var:
    case ExprTag::Extern:
    case ExprTag::StructDecl:
    case ExprTag::UnionDecl:
    case ExprTag::AliasDecl:
    case ExprTag::EffectDecl:
    case ExprTag::Overload: // leaf produced by MR; TC resolves it
    case ExprTag::ModDecl:  // module directives are stripped by MR before TC,
    case ExprTag::Import:   // so these are unreachable here -- pass through
    case ExprTag::Vis:
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

    case ExprTag::Handle:
    {
      auto &h = std::get<EX::ExHandle>(e->as);
      h.body  = lower(h.body);
      for (auto &c : h.clauses) c.body = lower(c.body);
      if (h.else_body) h.else_body = lower(h.else_body);
      return e;
    }
    case ExprTag::Match: return lower_match(e);
    case ExprTag::Case:
    {
      // Already-lowered form; recurse defensively in case one is nested.
      auto &cs = std::get<EX::ExCase>(e->as);
      cs.scrut = lower(cs.scrut);
      cs.deflt = lower(cs.deflt);
      for (size_t i = 0; i < cs.alts.size(); ++i)
        cs.alts[i].body = lower(cs.alts[i].body);
      return e;
    }
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
    case ExprTag::VariantLit:
    {
      auto &vl = std::get<EX::ExVariantLit>(e->as);
      for (size_t i = 0; i < vl.fields.size(); ++i)
        vl.fields[i].val = lower(vl.fields[i].val);
      normalize_variant_lit(vl);
      return e;
    }
    case ExprTag::SeqLit:
    {
      auto &sl = std::get<EX::ExSeqLit>(e->as);
      for (size_t i = 0; i < sl.elems.size(); ++i)
        sl.elems[i] = lower(sl.elems[i]);
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
    UT_FAIL_MSG("%s", "PatLower::lower: unhandled ExprTag");
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
      return;
    }
    case PatTag::Variant:
    {
      auto &pv = std::get<EX::PatVariant>(pat->as);
      anchor   = pv.anchor;
      line     = pv.line;
      return;
    }
    case PatTag::Int:
    {
      auto &p = std::get<EX::PatInt>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      return;
    }
    case PatTag::Bool:
    {
      auto &p = std::get<EX::PatBool>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      return;
    }
    case PatTag::Real:
    {
      auto &p = std::get<EX::PatReal>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      return;
    }
    case PatTag::Str:
    {
      auto &p = std::get<EX::PatStr>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      return;
    }
    case PatTag::StrPrefix:
    {
      auto &p = std::get<EX::PatStrPrefix>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      return;
    }
    case PatTag::Seq:
    {
      auto &p = std::get<EX::PatSeq>(pat->as);
      anchor  = p.anchor;
      line    = p.line;
      return;
    }
    case PatTag::Wild:
    case PatTag::Var : return;
    }
    UT_FAIL_MSG("%s", "pat_anchor: unhandled PatTag");
  }

  // Lower every pattern in `exprs` in place. Returns the accumulated
  // diagnostics; on any error the caller must not proceed (an un-lowered
  // pattern would survive into desugar).
  Diagnostics
  run(
    EX::Exprs &exprs)
  {
    for (size_t i = 0; i < exprs.size(); ++i)
      if (exprs[i].tag == ExprTag::Def)
      {
        auto &d = std::get<EX::ExDef>(exprs[i].as);
        d.def   = lower(d.def);
      }
    return std::move(diags);
  }
};

/*------------------------------------------------------------------------------
 *\CHECKER
 *-----------------------------------------------------------------------------*/

class Checker
{
public:
  Checker(
    AR::Arena &arena, UT::Vu src, const TG::Target &tg)
      : m_arena{ arena },
        m_src{ src },
        m_tg{ tg },
        m_ovdb{ make_overload_db(tg.int_ty()) }
  {
    seed_primitives();
  }

  void run(EX::Exprs &exprs);

  Diagnostics m_diags;

private:
  AR::Arena    &m_arena;
  UT::Vu        m_src;
  TG::Target    m_tg;
  OverloadTable m_ovdb;
  TypeStore     m_types;
  int           m_next_id = 0;
  Env           m_prim;
  Globals       m_globals;
  StructTable   m_structs;
  UnionTable    m_unions;

  // Declared effects and their operations (M3 effect rows). `m_effects` is the
  // set of declared `@effect` names (so a `<E>` row annotation can validate its
  // labels); `m_op_effect` maps each operation to its owning effect, so a
  // handler knows which effect its clauses discharge from the ambient row.
  std::unordered_set<std::string>              m_effects;
  std::unordered_map<std::string, std::string> m_op_effect;
  // Declared parameter count (arity) of every nominal type, keyed by name.
  // Populated before any field type is built so applied cons can be
  // arity-checked even across forward references. Built-ins (Int/Str) are
  // absent => arity 0.
  std::unordered_map<std::string, size_t> m_arity;

  // Transparent type aliases: `$ Name : @alias = target` registers Name -> the
  // target's EX type. sig_to_type resolves an alias name to its target wherever
  // it is written, so the two are interchangeable. `m_alias_depth` guards a
  // cyclic alias (A = B, B = A) from recursing forever.
  std::unordered_map<std::string, EX::Ty *> m_aliases;
  size_t                                    m_alias_depth = 0;

  // current diagnostic anchor (the global under check)
  UT::Vu m_anchor;
  size_t m_line = 0;

  // overload resolution: deferred sites collected during an inference scope and
  // resolved at its end (see infer's Var case / resolve_sites).
  ResolveSites m_sites;

  // A deferred user-overload resolution: a use MR left as an overload set. Like
  // an operator site it is typed as a fresh `use` var; resolve_user_sites later
  // picks the one candidate whose (instantiated) type fits and writes its
  // mangled name to `slot` (the EX::ExOverload's `chosen` field).
  struct UserSite
  {
    UT::Vu                *slot;
    Type                  *use;
    const UT::Vec<UT::Vu> *cands;
    std::string            name;
    UT::Vu                 anchor;
    bool done = false; // settled inside resolve_sites' fixpoint
  };
  std::vector<UserSite> m_usites;

  // When non-null, unify records every variable it binds here so a speculative
  // unification (a candidate trial in resolve_user_sites) can be rolled back.
  std::vector<Type *> *m_trail = nullptr;

  // An unqualified struct/union literal whose type is inferred from context.
  // Like an overload site, it is typed as a fresh `use` var and settled once
  // unification binds it to a concrete nominal type (see resolve_lit_sites).
  struct LitSite
  {
    bool   is_struct;
    bool   is_seq = false; // `[..]` literal
    Type  *use;
    UT::Vu anchor;
    std::vector<std::pair<std::string, Type *>> fields; // (name, value type)
    std::string                                 vtag;   // variant only
    Type             *elem     = nullptr;               // seq: elem type
    EX::ExStructLit  *exs      = nullptr;               // write-back
    EX::ExVariantLit *exv      = nullptr;               // write-back
    EX::ExSeqLit     *exseq    = nullptr;               // write-back (literal)
    EX::PatSeq       *exseqpat = nullptr;               // write-back (pattern)
  };
  std::vector<LitSite> m_lit_sites;

  // A deferred field access `record.field`. Field access needs the receiver's
  // struct type known, but that type may settle only later -- after an overload
  // is resolved, or a parameter's signature type is applied. So a use is typed
  // as a fresh `result` var here and checked once its `record` type is concrete
  // (see resolve_field_sites). Sites are resolved in recording order, which is
  // innermost-first, so a chain `a.b.c` settles `a.b` before `(a.b).c`.
  struct FieldSite
  {
    Type       *record; // receiver type (a var until its context settles it)
    std::string field;
    UT::Vu      anchor;
    Type       *result;       // unified with the field's type once resolved
    bool        done = false; // settled early, inside resolve_sites' fixpoint
  };
  std::vector<FieldSite> m_field_sites;

  // type constructors
  Type *fresh(bool rigid = false, std::string name = "");
  Type *con(std::string name, std::vector<Type *> args = {});
  Type *arrow(Type *from, Type *to, Type *eff);
  Type *arrow(Type *from, Type *to); // eff defaults to a fresh row variable
  Type *row_empty();
  Type *row_extend(std::string label, Type *rest);
  bool  unify_row(Type *a, Type *b);
  bool  subrow(Type *sub, Type *super);
  bool  rewrite_row(Type *row, const std::string &label, Type *&found_rest);

  // substitution / nominal instantiation
  Type *subst(Type *t, const Subst &s);
  Subst fresh_subst(const VarIds &params, std::vector<Type *> &args_out);

  // union-find
  Type *prune(Type *t);
  bool  occurs(Type *var, Type *t);
  bool  unify(Type *a, Type *b);
  bool  try_unify(Type *a, Type *b); // speculative: rolled back, never reported

  // schemes
  void   free_vars(Type *t, VarIds &out);
  void   env_free_vars(const Env &env, VarIds &out);
  Scheme generalize(const Env &locals, Type *t);
  Type  *instantiate(const Scheme &s);

  // signatures
  Type *
  sig_to_type(EX::Ty *sig, TyVarEnv &tv, bool rigid, VarIds *order = nullptr);
  Scheme scheme_of_sig(EX::Ty *sig);

  // inference
  bool        occurs_free(const std::string &name, EX::Expr *e);
  void        type_pattern(EX::Pattern       *p,
                           Type              *scrut,
                           Env               &env,
                           const std::string &munion);
  void        match_fieldpats(const UT::Vec<EX::FieldPat> &pats,
                              const StructFields          &decl,
                              const char                  *what,
                              const std::string           &tname,
                              UT::Vu                       anchor,
                              std::vector<EX::Pattern *>  &subs);
  std::string resolve_match_union(Type *scrut, EX::ExMatch &m);
  void        check_exhaustive(EX::ExMatch &m);

  // Exhaustiveness (Maranget's usefulness algorithm). A pattern's constructor
  // "head": its id and payload sub-patterns in declared order (nullptr slot =
  // wildcard). `finite` is false for a head drawn from an infinite/opaque
  // domain (Int/Real/Str literals, string-prefix, array `[..]`) -- such a
  // column can be exhausted only by a wildcard, never by enumerating
  // constructors.
  struct PHead
  {
    std::string                      id;
    std::vector<const EX::Pattern *> subs;
    bool                             finite = true;
  };
  bool pat_head(const EX::Pattern *p, PHead &out);
  std::vector<const EX::Pattern *>
                     ordered_payload(const UT::Vec<EX::FieldPat> &fields,
                                     const StructFields          &decl);
  const EX::Pattern *seq_tail(const EX::PatSeq &sq);
  bool               column_sig(const EX::Pattern                           *rep,
                                std::vector<std::pair<std::string, size_t>> &sig,
                                std::string                                 &tyname);
  std::string        list_union();
  std::string        render_ctor(const std::string              &tyname,
                                 const std::string              &ctor,
                                 const std::vector<std::string> &subs,
                                 size_t                          arity);
  std::vector<std::vector<const EX::Pattern *>>
  specialize(const std::vector<std::vector<const EX::Pattern *>> &rows,
             const std::string                                   &ctor,
             size_t                                               arity);
  std::vector<std::vector<std::string>>
  match_witnesses(const std::vector<std::vector<const EX::Pattern *>> &rows,
                  size_t                                               width);

  Type *infer(EX::Expr *e, Env &locals, Type *amb);
  Type *infer_apply(Type *fn, EX::Expr *arg, Env &locals, Type *amb);
  Type *infer_against(EX::Expr *e, Type *expected, Env &locals, Type *amb);

  // resolution
  Scheme resolve(const std::string &name);
  enum class SiteState
  {
    Resolved,
    Pending,
    Failed
  };
  SiteState resolve_one_site(const ResolveSite &s, bool allow_default);
  void      resolve_sites(size_t from, size_t ffrom, size_t ufrom);
  SiteState resolve_one_user_site(UserSite &s, bool conservative);
  void      resolve_user_sites(size_t from);
  void      resolve_lit_sites(size_t from);
  bool      settle_field_site(FieldSite &s);
  bool      settle_ready_field_sites(size_t ffrom);
  void      resolve_field_sites(size_t from);
  void      ensure_tuple(size_t n);
  UT::Vu    intern(const std::string &s);

  // diagnostics
  size_t      line_of(UT::Vu anchor);
  std::string show(Type *t);
  void        fail(ER::Code code, UT::Vu anchor, const char *fmt, ...)
    UT_PRINTF_LIKE(4, 5);

  void seed_primitives();
};

/*------------------------------------------------------------------------------
 *\CONSTRUCTORS
 *-----------------------------------------------------------------------------*/

Type *
Checker::fresh(
  bool rigid, std::string name)
{
  m_types.push_back(
    Type{ TVar{ m_next_id++, nullptr, rigid, std::move(name) } });
  return &m_types.back();
}

Type *
Checker::con(
  std::string name, std::vector<Type *> args)
{
  m_types.push_back(Type{ TCon{ std::move(name), std::move(args) } });
  return &m_types.back();
}

Type *
Checker::arrow(
  Type *from, Type *to, Type *eff)
{
  m_types.push_back(Type{ TArrow{ from, to, eff } });
  return &m_types.back();
}

// A function type whose latent effect is left open: a fresh row variable. Used
// for shaped/probed arrows and effect-polymorphic primitives -- the variable
// unifies with whatever ambient effect the use site demands.
Type *
Checker::arrow(
  Type *from, Type *to)
{
  return arrow(from, to, fresh());
}

Type *
Checker::row_empty()
{
  m_types.push_back(Type{ TRowEmpty{} });
  return &m_types.back();
}

Type *
Checker::row_extend(
  std::string label, Type *rest)
{
  m_types.push_back(Type{ TRowExtend{ std::move(label), rest } });
  return &m_types.back();
}

/*------------------------------------------------------------------------------
 *\UNION-FIND
 *-----------------------------------------------------------------------------*/

Type *
Checker::prune(
  Type *t)
{
  if (t->kind() == Kind::Var)
  {
    TVar &v = std::get<TVar>(t->as);
    if (v.ref)
    {
      v.ref = prune(v.ref);
      return v.ref;
    }
  }
  return t;
}

bool
Checker::occurs(
  Type *var, Type *t)
{
  t = prune(t);
  if (t == var) return true;
  if (t->kind() == Kind::Arrow)
  {
    TArrow &a = std::get<TArrow>(t->as);
    return occurs(var, a.from) || occurs(var, a.to) || occurs(var, a.eff);
  }
  if (t->kind() == Kind::Con)
  {
    for (Type *arg : std::get<TCon>(t->as).args)
      if (occurs(var, arg)) return true;
  }
  if (t->kind() == Kind::RowExtend)
    return occurs(var, std::get<TRowExtend>(t->as).rest);
  return false;
}

bool
Checker::unify(
  Type *a, Type *b)
{
  a = prune(a);
  b = prune(b);
  if (a == b) return true;

  if (a->kind() == Kind::Var && !std::get<TVar>(a->as).rigid)
  {
    if (occurs(a, b))
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "infinite type: '%s' occurs in '%s'",
           show(a).c_str(),
           show(b).c_str());
      return false;
    }
    if (m_trail) m_trail->push_back(a);
    std::get<TVar>(a->as).ref = b;
    return true;
  }
  if (b->kind() == Kind::Var && !std::get<TVar>(b->as).rigid)
  {
    if (occurs(b, a))
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "infinite type: '%s' occurs in '%s'",
           show(b).c_str(),
           show(a).c_str());
      return false;
    }
    if (m_trail) m_trail->push_back(b);
    std::get<TVar>(b->as).ref = a;
    return true;
  }

  if (a->kind() == Kind::Con && b->kind() == Kind::Con)
  {
    TCon &ca = std::get<TCon>(a->as);
    TCon &cb = std::get<TCon>(b->as);
    if (ca.name == cb.name && ca.args.size() == cb.args.size())
    {
      bool ok = true;
      for (size_t i = 0; i < ca.args.size(); ++i)
        ok &= unify(ca.args[i], cb.args[i]);
      return ok;
    }
  }

  if (a->kind() == Kind::Arrow && b->kind() == Kind::Arrow)
  {
    TArrow &fa = std::get<TArrow>(a->as);
    TArrow &fb = std::get<TArrow>(b->as);
    bool    l  = unify(fa.from, fb.from);
    bool    r  = unify(fa.to, fb.to);
    bool    e  = unify(fa.eff, fb.eff);
    return l && r && e;
  }

  if (a->kind() == Kind::RowEmpty && b->kind() == Kind::RowEmpty) return true;

  if (a->kind() == Kind::RowExtend || b->kind() == Kind::RowExtend)
    return unify_row(a, b);

  fail(ER::Code::TYPE_MISMATCH,
       m_anchor,
       "type mismatch: expected '%s', got '%s'",
       show(a).c_str(),
       show(b).c_str());
  return false;
}

// Unify two effect rows, at least one a `<label | rest>` extension (Var-vs-row
// and empty-vs-empty are handled by unify's earlier arms). Following Leijen's
// scoped-label discipline: pull the head label of `a` out of `b` (rewrite_row),
// then unify the two remaining tails. A label absent from a closed `b` is the
// unhandled-effect error.
bool
Checker::unify_row(
  Type *a, Type *b)
{
  // Normalize so `a` is the extension we decompose.
  if (a->kind() != Kind::RowExtend) std::swap(a, b);
  TRowExtend &ea = std::get<TRowExtend>(a->as);

  Type *b_rest = nullptr;
  if (!rewrite_row(b, ea.label, b_rest))
  {
    fail(ER::Code::TYPE_MISMATCH,
         m_anchor,
         "effect '%s' is performed but not handled (expected '%s', got '%s')",
         ea.label.c_str(),
         show(b).c_str(),
         show(a).c_str());
    return false;
  }
  return unify(ea.rest, b_rest);
}

// Effect subsumption: require `sub` to be a SUBROW of `super` -- every effect
// the callee performs (`sub`) must be permitted by the ambient (`super`). Used
// at a call site so a pure (or smaller-effect) function fits a larger ambient.
//   - empty `sub` fits any `super` (a pure call performs nothing);
//   - a labelled `sub` peels each label out of `super` (which may grow an open
//     tail) and recurses, so a missing label in a closed `super` is the
//     unhandled-effect error;
//   - a variable `sub` (an unknown callee effect) is tied to `super` by
//     equality -- sound, since `super` is a subrow of itself.
bool
Checker::subrow(
  Type *sub, Type *super)
{
  sub = prune(sub);
  if (sub->kind() == Kind::RowEmpty) return true;
  if (sub->kind() == Kind::Var) return unify(sub, super);
  if (sub->kind() == Kind::RowExtend)
  {
    TRowExtend &r          = std::get<TRowExtend>(sub->as);
    Type       *super_rest = nullptr;
    if (!rewrite_row(super, r.label, super_rest))
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "effect '%s' is performed but not handled (it is not in '%s')",
           r.label.c_str(),
           show(super).c_str());
      return false;
    }
    return subrow(r.rest, super_rest);
  }
  return unify(sub, super); // non-row (shouldn't occur); fall back to equality
}

// Bring an occurrence of effect `label` to the head of `row`, yielding the row
// that remains once it is removed (`found_rest`). If `row`'s tail is a variable
// and `label` is absent, extend that variable with `<label | fresh>` and report
// the fresh tail -- this is how an open row grows to accept a new effect.
// Returns false iff `row` is closed and lacks `label`.
bool
Checker::rewrite_row(
  Type *row, const std::string &label, Type *&found_rest)
{
  row = prune(row);
  if (row->kind() == Kind::RowExtend)
  {
    TRowExtend &r = std::get<TRowExtend>(row->as);
    if (r.label == label)
    {
      found_rest = r.rest;
      return true;
    }
    Type *deeper = nullptr;
    if (!rewrite_row(r.rest, label, deeper)) return false;
    // Keep this head, splice the rest found below it.
    found_rest = row_extend(r.label, deeper);
    return true;
  }
  if (row->kind() == Kind::Var && !std::get<TVar>(row->as).rigid)
  {
    Type *tail = fresh();
    if (m_trail) m_trail->push_back(row);
    std::get<TVar>(row->as).ref = row_extend(label, tail);
    found_rest                  = tail;
    return true;
  }
  // Closed row (empty or rigid var) without the label.
  return false;
}

// Speculatively unify `a` and `b`, then undo every binding it made and discard
// any diagnostics it produced -- the union-find is left exactly as it was. Used
// to probe whether an overload candidate fits without committing to it. (Path
// compression by prune may shorten already-bound chains; that is harmless,
// since nothing pre-existing points into the freshly-bound vars we roll back.)
bool
Checker::try_unify(
  Type *a, Type *b)
{
  std::vector<Type *>  trail;
  std::vector<Type *> *save  = m_trail;
  size_t               diag0 = m_diags.size();
  m_trail                    = &trail;
  bool ok                    = unify(a, b);
  m_trail                    = save;
  for (Type *v : trail) std::get<TVar>(v->as).ref = nullptr;
  m_diags.resize(diag0);
  return ok;
}

/*------------------------------------------------------------------------------
 *\SCHEMES
 *-----------------------------------------------------------------------------*/

void
Checker::free_vars(
  Type *t, VarIds &out)
{
  t = prune(t);
  switch (t->kind())
  {
  case Kind::Var:
  {
    TVar &v = std::get<TVar>(t->as);
    if (v.rigid) break;
    for (int id : out)
      if (id == v.id) return;
    out.push_back(v.id);
    break;
  }
  case Kind::Con:
  {
    for (Type *arg : std::get<TCon>(t->as).args) free_vars(arg, out);
    break;
  }
  case Kind::Arrow:
  {
    TArrow &a = std::get<TArrow>(t->as);
    free_vars(a.from, out);
    free_vars(a.to, out);
    free_vars(a.eff, out);
    break;
  }
  case Kind::RowExtend: free_vars(std::get<TRowExtend>(t->as).rest, out); break;
  case Kind::RowEmpty : break;
  }
}

void
Checker::env_free_vars(
  const Env &env, VarIds &out)
{
  for (const auto &[name, s] : env)
  {
    VarIds tv;
    free_vars(s.type, tv);
    for (int id : tv)
    {
      bool quantified = false;
      for (int q : s.vars)
        if (q == id) quantified = true;
      if (!quantified) out.push_back(id);
    }
  }
}

Scheme
Checker::generalize(
  const Env &locals, Type *t)
{
  VarIds tv;
  free_vars(t, tv);
  VarIds ev;
  env_free_vars(locals, ev);

  // Also keep monomorphic any variable an unresolved overload site still
  // constrains: it will be pinned (or defaulted) by resolution, so generalizing
  // it would let one binding be used at two types while a single overload was
  // chosen -- SML's "monomorphism restriction" for overloaded numerics.
  // (Without this, `let square = \x = x*x` would generalize its result variable
  // and the call's result would never be tied back to the resolved operand
  // type.)
  for (const ResolveSite &s : m_sites) free_vars(s.use, ev);
  for (const UserSite &s : m_usites) free_vars(s.use, ev);
  // A pending field access is likewise monomorphic until its receiver settles.
  for (const FieldSite &s : m_field_sites) free_vars(s.result, ev);

  VarIds q;
  for (int id : tv)
  {
    bool in_env = false;
    for (int e : ev)
      if (e == id) in_env = true;
    if (!in_env) q.push_back(id);
  }
  return Scheme{ q, t };
}

// Copy `t`, replacing any type-variable whose id is in `s` with its mapping and
// rebuilding constructors/arrows around the result. Variables not in `s`, and
// constructors with no substituted descendant, are shared structurally.
Type *
Checker::subst(
  Type *t, const Subst &s)
{
  t = prune(t);
  switch (t->kind())
  {
  case Kind::Var:
  {
    auto it = s.find(std::get<TVar>(t->as).id);
    return it == s.end() ? t : it->second;
  }
  case Kind::Con:
  {
    TCon &c = std::get<TCon>(t->as);
    if (c.args.empty()) return t;
    std::vector<Type *> args;
    args.reserve(c.args.size());
    for (Type *a : c.args) args.push_back(subst(a, s));
    return con(c.name, std::move(args));
  }
  case Kind::Arrow:
  {
    TArrow &a = std::get<TArrow>(t->as);
    return arrow(subst(a.from, s), subst(a.to, s), subst(a.eff, s));
  }
  case Kind::RowExtend:
  {
    TRowExtend &r = std::get<TRowExtend>(t->as);
    return row_extend(r.label, subst(r.rest, s));
  }
  case Kind::RowEmpty: return t;
  }
  return t;
}

// Make a fresh unification variable for each parameter id, returning the
// id->var substitution and (in `args_out`) the fresh vars in parameter order --
// exactly the type arguments of the instantiated nominal type.
Subst
Checker::fresh_subst(
  const VarIds &params, std::vector<Type *> &args_out)
{
  Subst sub;
  args_out.clear();
  args_out.reserve(params.size());
  for (int id : params)
  {
    Type *v = fresh();
    sub[id] = v;
    args_out.push_back(v);
  }
  return sub;
}

Type *
Checker::instantiate(
  const Scheme &s)
{
  if (s.vars.empty()) return s.type;

  Subst sub;
  for (int id : s.vars) sub[id] = fresh();
  return subst(s.type, sub);
}

/*------------------------------------------------------------------------------
 *\SIGNATURES
 *-----------------------------------------------------------------------------*/

Type *
Checker::sig_to_type(
  EX::Ty *sig, TyVarEnv &tv, bool rigid, VarIds *order)
{
  switch (sig->tag)
  {
  case EX::TyTag::Con:
  {
    auto       &c    = std::get<EX::TyCon>(sig->as);
    std::string name = std::string(c.name);

    if (OP::is_tuple_name(c.name)) ensure_tuple(c.args.size());

    // A transparent alias used without arguments resolves to its target.
    // (Aliases are nullary for now; an alias applied to type arguments is left
    // as an ordinary con, so the arity check below reports it.)
    if (c.args.empty() && m_aliases.count(name) && m_alias_depth < 64)
    {
      m_alias_depth++;
      Type *r = sig_to_type(m_aliases.at(name), tv, rigid, order);
      m_alias_depth--;
      return r;
    }

    size_t arity = m_arity.count(name) ? m_arity.at(name) : 0;
    size_t given = c.args.size();

    // Either spell out every argument, or none (then infer them with fresh
    // holes); any other count is a kind/arity error. Built-ins have arity 0.
    // Reported only on the non-rigid pass so a signature checked twice (its
    // scheme in phase B, then rigidly against the body in phase C) faults once.
    if (!rigid && given != 0 && given != arity)
    {
      fail(ER::Code::TYPE_MISMATCH,
           m_anchor,
           "type '%s' expects %zu argument(s), but %zu given",
           name.c_str(),
           arity,
           given);
      return fresh();
    }

    std::vector<Type *> args;
    args.reserve(arity);
    if (given == arity)
      for (EX::Ty *a : c.args) args.push_back(sig_to_type(a, tv, rigid, order));
    else // given == 0, arity > 0: infer each argument
      for (size_t i = 0; i < arity; ++i) args.push_back(fresh());
    return con(std::move(name), std::move(args));
  }
  case EX::TyTag::Var:
  {
    std::string nm = std::string(std::get<EX::TyVar>(sig->as).name);
    auto        it = tv.find(nm);
    if (it != tv.end()) return it->second;
    Type *v = fresh(rigid, nm);
    tv[nm]  = v;
    if (order) order->push_back(std::get<TVar>(v->as).id);
    return v;
  }
  case EX::TyTag::Arrow:
  {
    auto &ar = std::get<EX::TyArrow>(sig->as);
    Type *f  = sig_to_type(ar.from, tv, rigid, order);
    Type *t  = sig_to_type(ar.to, tv, rigid, order);

    // The effect row: the tail (a row variable, shared with type vars via `tv`
    // so it skolemizes/generalizes consistently) or the empty closed row, then
    // each written label extended onto it. A label must name a declared effect.
    Type *eff;
    if (ar.eff_tail.data() && ar.eff_tail.size())
    {
      std::string nm(ar.eff_tail);
      auto        it = tv.find(nm);
      if (it != tv.end())
        eff = it->second;
      else
      {
        eff    = fresh(rigid, nm);
        tv[nm] = eff;
        if (order) order->push_back(std::get<TVar>(eff->as).id);
      }
    }
    else
      eff = row_empty();
    for (UT::Vu lab : ar.eff_labels)
    {
      std::string name(lab);
      if (!rigid && !m_effects.count(name))
        fail(ER::Code::TYPE_UNBOUND,
             m_anchor,
             "unknown effect '%s' in effect row",
             name.c_str());
      eff = row_extend(name, eff);
    }
    return arrow(f, t, eff);
  }
  }
  return fresh();
}

Scheme
Checker::scheme_of_sig(
  EX::Ty *sig)
{
  TyVarEnv tv;
  Type    *t = sig_to_type(sig, tv, false);
  VarIds   q;
  for (auto &[name, v] : tv) q.push_back(std::get<TVar>(v->as).id);
  return Scheme{ q, t };
}

/*------------------------------------------------------------------------------
 *\PATTERN HELPERS
 *-----------------------------------------------------------------------------*/

namespace
{
bool
pat_shadows(
  EX::Pattern *p, const std::string &name)
{
  switch (p->tag)
  {
  case EX::PatTag::Wild:
  case EX::PatTag::Int:
  case EX::PatTag::Bool:
  case EX::PatTag::Real:
  case EX::PatTag::Str : return false;
  case EX::PatTag::Var:
    return std::string(std::get<EX::PatVar>(p->as).name) == name;
  case EX::PatTag::StrPrefix:
    return pat_shadows(std::get<EX::PatStrPrefix>(p->as).rest, name);
  case EX::PatTag::Struct:
    for (EX::FieldPat &f : std::get<EX::PatStruct>(p->as).fields)
      if (pat_shadows(f.pat, name)) return true;
    return false;
  case EX::PatTag::Variant:
    for (EX::FieldPat &f : std::get<EX::PatVariant>(p->as).fields)
      if (pat_shadows(f.pat, name)) return true;
    return false;
  case EX::PatTag::Seq:
  {
    auto &pq = std::get<EX::PatSeq>(p->as);
    for (size_t i = 0; i < pq.elems.size(); ++i)
      if (pat_shadows(pq.elems[i], name)) return true;
    return pq.rest && pat_shadows(pq.rest, name);
  }
  }
  UT_FAIL_MSG("%s", "pat_shadows: unhandled PatTag");
  return false;
}
} // namespace

/*------------------------------------------------------------------------------
 *\FREE-VARIABLE CHECK
 *-----------------------------------------------------------------------------*/

bool
Checker::occurs_free(
  const std::string &name, EX::Expr *e)
{
  switch (e->tag)
  {
  case EX::ExprTag::Var:
    return std::string(std::get<EX::ExVar>(e->as).name) == name;
  case EX::ExprTag::Overload:
    return std::string(std::get<EX::ExOverload>(e->as).name) == name;
  case EX::ExprTag::Int:
  case EX::ExprTag::Bool:
  case EX::ExprTag::Real:
  case EX::ExprTag::Str:
  case EX::ExprTag::Unit:
  case EX::ExprTag::Extern: return false;
  case EX::ExprTag::FnDef:
  {
    EX::ExFnDef &l = std::get<EX::ExFnDef>(e->as);
    if (l.param_pat ? pat_shadows(l.param_pat, name)
                    : std::string(l.param) == name)
      return false; // shadowed
    return occurs_free(name, l.body);
  }
  case EX::ExprTag::App:
  {
    EX::ExApp &a = std::get<EX::ExApp>(e->as);
    return occurs_free(name, a.fn) || occurs_free(name, a.arg);
  }
  case EX::ExprTag::If:
  {
    EX::ExIf &i = std::get<EX::ExIf>(e->as);
    return occurs_free(name, i.cond) || occurs_free(name, i.then)
           || occurs_free(name, i.alt);
  }
  case EX::ExprTag::Let:
  {
    EX::ExLet &l = std::get<EX::ExLet>(e->as);
    if (l.pat) // pattern binders are in scope only in the body
      return occurs_free(name, l.val)
             || (!pat_shadows(l.pat, name) && occurs_free(name, l.body));
    // name shadowed in the body; still visible in the (recursive) value
    if (std::string(l.var) == name) return occurs_free(name, l.val);
    return occurs_free(name, l.val) || occurs_free(name, l.body);
  }
  case EX::ExprTag::StructLit:
    for (EX::FieldInit &f : std::get<EX::ExStructLit>(e->as).fields)
      if (occurs_free(name, f.val)) return true;
    return false;
  case EX::ExprTag::VariantLit:
    for (EX::FieldInit &f : std::get<EX::ExVariantLit>(e->as).fields)
      if (occurs_free(name, f.val)) return true;
    return false;
  case EX::ExprTag::SeqLit:
    for (EX::Expr *el : std::get<EX::ExSeqLit>(e->as).elems)
      if (occurs_free(name, el)) return true;
    return false;
  case EX::ExprTag::Field:
    return occurs_free(name, std::get<EX::ExField>(e->as).record);
  case EX::ExprTag::Handle:
  {
    EX::ExHandle &h = std::get<EX::ExHandle>(e->as);
    if (occurs_free(name, h.body)) return true;
    if (h.else_body && std::string(h.else_var) != name
        && occurs_free(name, h.else_body))
      return true;
    for (EX::HandlerClause &c : h.clauses)
      if (std::string(c.arg) != name && std::string(h.k) != name
          && occurs_free(name, c.body))
        return true;
    return false;
  }
  case EX::ExprTag::Case:
  {
    EX::ExCase &c = std::get<EX::ExCase>(e->as);
    if (occurs_free(name, c.scrut) || occurs_free(name, c.deflt)) return true;
    for (EX::CaseAlt &alt : c.alts)
    {
      bool shadowed = false; // bound by this alt's payload (Con only)
      if (alt.kind == EX::AltKind::Con)
        for (UT::Vu b : alt.binders)
          if (std::string(b) == name) shadowed = true;
      if (!shadowed && occurs_free(name, alt.body)) return true;
    }
    return false;
  }
  case EX::ExprTag::Match:
  {
    EX::ExMatch &m = std::get<EX::ExMatch>(e->as);
    if (occurs_free(name, m.scrut)) return true;
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      if (pat_shadows(m.arms[i].pat, name)) continue; // bound by this arm
      if (m.arms[i].guard && occurs_free(name, m.arms[i].guard)) return true;
      if (occurs_free(name, m.arms[i].body)) return true;
    }
    return m.alt && occurs_free(name, m.alt);
  }
  case EX::ExprTag::Def:
  case EX::ExprTag::StructDecl:
  case EX::ExprTag::UnionDecl:
  case EX::ExprTag::AliasDecl:
  case EX::ExprTag::EffectDecl:
  case EX::ExprTag::ModDecl:
  case EX::ExprTag::Import:
  case EX::ExprTag::Vis:
  case EX::ExprTag::Unknown:
    UT_FAIL_MSG("occurs_free: unexpected node %s", EX::pprint(e->tag).c_str());
  }
  UT_FAIL_MSG("%s", "occurs_free: unhandled ExprTag");
  return false;
}

/*------------------------------------------------------------------------------
 *\PATTERN TYPING
 *-----------------------------------------------------------------------------*/

std::string
Checker::resolve_match_union(
  Type *scrut, EX::ExMatch &m)
{
  Type *s = prune(scrut);
  if (s->kind() == Kind::Con) return std::get<TCon>(s->as).name;

  std::vector<std::string> tags;
  for (size_t i = 0; i < m.arms.size(); ++i)
  {
    if (m.arms[i].pat->tag != EX::PatTag::Variant) continue;
    auto &pv = std::get<EX::PatVariant>(m.arms[i].pat->as);
    if (pv.type_name.size()) return std::string(pv.type_name);
    tags.push_back(std::string(pv.tag));
  }
  std::string found;
  for (auto &kv : m_unions)
  {
    bool all = true;
    for (const std::string &t : tags)
    {
      bool has = false;
      for (const VariantShape &v : kv.second.variants)
        if (v.tag == t) has = true;
      if (!has)
      {
        all = false;
        break;
      }
    }
    if (all)
    {
      if (!found.empty()) return ""; // ambiguous
      found = kv.first;
    }
  }
  return found;
}

// The union whose variants are exactly Nil/Cons (the blessed List), or "" when
// there is none -- lets exhaustiveness treat `[..]` seq patterns as List.
std::string
Checker::list_union()
{
  for (auto &kv : m_unions)
  {
    bool nil = false, cons = false;
    for (const VariantShape &v : kv.second.variants)
    {
      nil  = nil || v.tag == "Nil";
      cons = cons || v.tag == "Cons";
    }
    if (nil && cons) return kv.first;
  }
  return "";
}

// A variant/struct payload's sub-patterns in DECLARED field order (nullptr
// where a named field is omitted -- an ignored slot, i.e. a wildcard).
std::vector<const EX::Pattern *>
Checker::ordered_payload(
  const UT::Vec<EX::FieldPat> &fields, const StructFields &decl)
{
  std::vector<const EX::Pattern *> subs(decl.size(), nullptr);
  bool                             named = false;
  for (size_t i = 0; i < fields.size(); ++i)
    if (fields[i].name.size()) named = true;
  if (named)
  {
    for (size_t i = 0; i < fields.size(); ++i)
    {
      std::string fn(fields[i].name);
      for (size_t k = 0; k < decl.size(); ++k)
        if (decl[k].first == fn) subs[k] = fields[i].pat;
    }
  }
  else
    for (size_t i = 0; i < fields.size() && i < decl.size(); ++i)
      subs[i] = fields[i].pat;
  return subs;
}

// The tail of a List seq: `[e0, e1, .., en (..r)]` has tail `[e1, .., en
// (..r)]`.
const EX::Pattern *
Checker::seq_tail(
  const EX::PatSeq &sq)
{
  EX::PatSeq t;
  t.elems = UT::Vec<EX::Pattern *>{ m_arena };
  for (size_t i = 1; i < sq.elems.size(); ++i) t.elems.push(sq.elems[i]);
  t.rest     = sq.rest;
  t.anchor   = sq.anchor;
  t.line     = sq.line;
  t.is_array = false;
  auto *p    = (EX::Pattern *)m_arena.alloc<EX::Pattern>(1);
  *p         = EX::Pattern{ EX::PatTag::Seq, t };
  return p;
}

// A pattern's constructor head (id + ordered payload). Returns false for a
// wildcard (Wild / Var / null slot). List seqs are read as Nil / Cons so `[..]`
// and `List.Cons` patterns unify; array seqs and literals are opaque (finite =
// false), so their column needs a wildcard rather than a constructor set.
bool
Checker::pat_head(
  const EX::Pattern *p, PHead &out)
{
  if (!p) return false;
  switch (p->tag)
  {
  case EX::PatTag::Wild:
  case EX::PatTag::Var : return false;
  case EX::PatTag::Int:
    out.finite = false;
    out.id     = "#i" + std::to_string(std::get<EX::PatInt>(p->as).value);
    return true;
  case EX::PatTag::Bool:
    out.finite = true;
    out.id
      = std::get<EX::PatBool>(p->as).value ? OP::BOOL_TRUE : OP::BOOL_FALSE;
    return true;
  case EX::PatTag::Real:
    out.finite = false;
    out.id     = "#r" + std::to_string(std::get<EX::PatReal>(p->as).value);
    return true;
  case EX::PatTag::Str:
    out.finite = false;
    out.id     = "#s" + std::string(std::get<EX::PatStr>(p->as).value);
    return true;
  case EX::PatTag::StrPrefix:
    out.finite = false; // refutable, cannot complete a signature
    out.id     = "#sp";
    return true;
  case EX::PatTag::Struct:
  {
    auto &ps = std::get<EX::PatStruct>(p->as);
    auto  it = m_structs.find(std::string(ps.type_name));
    if (it == m_structs.end())
    {
      out.finite = false;
      out.id     = "#opaque";
      return true;
    }
    out.id   = std::string(ps.type_name);
    out.subs = ordered_payload(ps.fields, it->second.fields);
    return true;
  }
  case EX::PatTag::Variant:
  {
    auto       &pv = std::get<EX::PatVariant>(p->as);
    std::string u  = pv.resolved_union.size() ? std::string(pv.resolved_union)
                                              : std::string(pv.type_name);
    auto        it = m_unions.find(u);
    const VariantShape *vs = nullptr;
    if (it != m_unions.end())
      for (const VariantShape &v : it->second.variants)
        if (v.tag == std::string(pv.tag)) vs = &v;
    if (!vs)
    {
      out.finite = false;
      out.id     = "#opaque";
      return true;
    }
    out.id   = std::string(pv.tag);
    out.subs = ordered_payload(pv.fields, vs->fields);
    return true;
  }
  case EX::PatTag::Seq:
  {
    auto &sq = std::get<EX::PatSeq>(p->as);
    if (sq.is_array)
    {
      out.finite = false; // dynamic length: not structurally exhaustible
      out.id     = "#array";
      return true;
    }
    if (sq.elems.empty())
    {
      if (sq.rest) return pat_head(sq.rest, out); // `[..r]` is just `r`
      out.id = "Nil";
      return true;
    }
    out.id = "Cons";
    out.subs.push_back(sq.elems[0]);
    out.subs.push_back(seq_tail(sq));
    return true;
  }
  }
  return false;
}

// The full constructor signature (id + arity) of the column whose
// representative finite pattern is `rep`, plus the display type name. False
// when `rep`'s type is not a finite union / struct (so the column is not
// exhaustible).
bool
Checker::column_sig(
  const EX::Pattern                           *rep,
  std::vector<std::pair<std::string, size_t>> &sig,
  std::string                                 &tyname)
{
  if (!rep) return false;
  switch (rep->tag)
  {
  case EX::PatTag::Bool:
    tyname = OP::TY_BOOL;
    sig.push_back({ OP::BOOL_TRUE, 0 });
    sig.push_back({ OP::BOOL_FALSE, 0 });
    return true;
  case EX::PatTag::Variant:
  {
    auto       &pv = std::get<EX::PatVariant>(rep->as);
    std::string u  = pv.resolved_union.size() ? std::string(pv.resolved_union)
                                              : std::string(pv.type_name);
    auto        it = m_unions.find(u);
    if (it == m_unions.end()) return false;
    tyname = u;
    for (const VariantShape &v : it->second.variants)
      sig.push_back({ v.tag, v.fields.size() });
    return true;
  }
  case EX::PatTag::Struct:
  {
    auto &ps = std::get<EX::PatStruct>(rep->as);
    auto  it = m_structs.find(std::string(ps.type_name));
    if (it == m_structs.end()) return false;
    tyname = std::string(ps.type_name);
    sig.push_back({ std::string(ps.type_name), it->second.fields.size() });
    return true;
  }
  case EX::PatTag::Seq:
  {
    auto &sq = std::get<EX::PatSeq>(rep->as);
    if (sq.is_array) return false;
    if (sq.elems.empty() && sq.rest) return column_sig(sq.rest, sig, tyname);
    std::string lu = list_union();
    auto        it = m_unions.find(lu);
    if (lu.empty() || it == m_unions.end()) return false;
    tyname = lu;
    for (const VariantShape &v : it->second.variants)
      sig.push_back({ v.tag, v.fields.size() });
    return true;
  }
  default: return false;
  }
}

// Specialize the pattern matrix by constructor `c` (arity `a`): keep rows whose
// first pattern is `c` (replacing it with its payload) or a wildcard (replacing
// it with `a` wildcards); drop rows headed by a different constructor.
std::vector<std::vector<const EX::Pattern *>>
Checker::specialize(
  const std::vector<std::vector<const EX::Pattern *>> &rows,
  const std::string                                   &c,
  size_t                                               a)
{
  std::vector<std::vector<const EX::Pattern *>> out;
  for (const auto &r : rows)
  {
    PHead                            h;
    std::vector<const EX::Pattern *> nr;
    if (!pat_head(r[0], h))
      nr.assign(a, nullptr); // wildcard row -> `a` wildcard slots
    else if (h.finite && h.id == c)
    {
      nr = h.subs;
      nr.resize(a, nullptr);
    }
    else
      continue; // different constructor
    nr.insert(nr.end(), r.begin() + 1, r.end());
    out.push_back(std::move(nr));
  }
  return out;
}

// Render a constructor witness `Type.Ctor` (payload `.{..}` when it has one),
// shown with the surface `.` spelling rather than the mangled `MOD/Name`.
std::string
Checker::render_ctor(
  const std::string              &tyname,
  const std::string              &ctor,
  const std::vector<std::string> &subs,
  size_t                          arity)
{
  if (tyname == OP::TY_BOOL) return ctor;
  std::string ty = tyname;
  if (size_t sl = ty.find('/'); sl != std::string::npos) ty[sl] = '.';
  std::string head = ty.empty() ? ctor : ty + "." + ctor;
  if (arity == 0) return head;
  std::string s = head + ".{";
  for (size_t i = 0; i < arity; ++i) s += (i ? ", " : "") + subs[i];
  return s + "}";
}

// Maranget's usefulness, producing witnesses: the value vectors (rendered per
// column) that no row of `rows` matches. Empty result == the matrix is
// exhaustive. Recurses column by column; the first column drives
// specialization.
std::vector<std::vector<std::string>>
Checker::match_witnesses(
  const std::vector<std::vector<const EX::Pattern *>> &rows, size_t width)
{
  if (width == 0)
    return rows.empty() ? std::vector<std::vector<std::string>>{ {} }
                        : std::vector<std::vector<std::string>>{};

  // Survey column 0: the finite constructors present, and any finite rep.
  const EX::Pattern       *finite_rep = nullptr;
  std::vector<std::string> present;
  for (const auto &r : rows)
  {
    PHead h;
    if (!pat_head(r[0], h) || !h.finite) continue;
    if (!finite_rep) finite_rep = r[0];
    if (std::find(present.begin(), present.end(), h.id) == present.end())
      present.push_back(h.id);
  }

  auto default_matrix = [&]() {
    std::vector<std::vector<const EX::Pattern *>> d;
    for (const auto &r : rows)
    {
      PHead h;
      if (pat_head(r[0], h)) continue; // headed by a constructor -> dropped
      d.emplace_back(r.begin() + 1, r.end());
    }
    return d;
  };

  std::vector<std::pair<std::string, size_t>> sig;
  std::string                                 tyname;
  // No finite signature to complete (all-wildcard, literal, or opaque column):
  // exhaustible only by a wildcard -> recurse on the default, prefixing `_`.
  if (!finite_rep || !column_sig(finite_rep, sig, tyname))
  {
    std::vector<std::vector<std::string>> out;
    for (auto &w : match_witnesses(default_matrix(), width - 1))
    {
      std::vector<std::string> row{ "_" };
      row.insert(row.end(), w.begin(), w.end());
      out.push_back(std::move(row));
    }
    return out;
  }

  bool complete = true;
  for (auto &c : sig)
    if (std::find(present.begin(), present.end(), c.first) == present.end())
      complete = false;

  std::vector<std::vector<std::string>> out;
  if (complete)
  {
    // Every constructor is present: a witness must live under one of them.
    for (auto &c : sig)
      for (auto &w : match_witnesses(specialize(rows, c.first, c.second),
                                     c.second + width - 1))
      {
        std::vector<std::string> row{ render_ctor(
          tyname, c.first, w, c.second) };
        row.insert(row.end(), w.begin() + c.second, w.end());
        out.push_back(std::move(row));
      }
  }
  else
  {
    // A missing constructor is itself a witness (with wildcard payload),
    // combined with any witness for the remaining columns.
    auto sub = match_witnesses(default_matrix(), width - 1);
    for (auto &c : sig)
    {
      if (std::find(present.begin(), present.end(), c.first) != present.end())
        continue;
      std::vector<std::string> wild(c.second, "_");
      std::string head = render_ctor(tyname, c.first, wild, c.second);
      for (auto &w : sub)
      {
        std::vector<std::string> row{ head };
        row.insert(row.end(), w.begin(), w.end());
        out.push_back(std::move(row));
      }
    }
  }
  return out;
}

// An `else`-less `when` must be exhaustive. Builds the pattern matrix from the
// UNGUARDED arms (a guard may fail, so a guarded arm never guarantees coverage)
// and reports the unmatched value witnesses when any exist. Reasons recursively
// through nested variant / struct / list payloads (Maranget usefulness).
void
Checker::check_exhaustive(
  EX::ExMatch &m)
{
  std::vector<std::vector<const EX::Pattern *>> rows;
  for (size_t i = 0; i < m.arms.size(); ++i)
    if (!m.arms[i].guard) rows.push_back({ m.arms[i].pat });

  std::vector<std::vector<std::string>> w = match_witnesses(rows, 1);
  if (w.empty()) return; // exhaustive

  // Collect the (deduplicated) unmatched patterns; each witness has one column.
  std::vector<std::string> items;
  for (auto &row : w)
    if (!row.empty()
        && std::find(items.begin(), items.end(), row[0]) == items.end())
      items.push_back(row[0]);

  if (items.size() == 1 && items[0] == "_")
  {
    fail(ER::Code::TYPE_MISMATCH,
         m.anchor,
         "this 'when' has no 'else' and cannot be exhaustive: its scrutinee is "
         "not a finite union. Add an 'else' branch or a wildcard '_' arm");
    return;
  }

  std::string  list;
  const size_t CAP = 6;
  for (size_t i = 0; i < items.size() && i < CAP; ++i)
    list += (i ? ", " : "") + items[i];
  if (items.size() > CAP) list += ", ...";
  fail(ER::Code::TYPE_MISMATCH,
       m.anchor,
       "this 'when' has no 'else' and is not exhaustive; unmatched: %s. Add "
       "those arms or an 'else' branch",
       list.c_str());
}

void
Checker::match_fieldpats(
  const UT::Vec<EX::FieldPat> &pats,
  const StructFields          &decl,
  const char                  *what,
  const std::string           &tname,
  UT::Vu                       anchor,
  std::vector<EX::Pattern *>  &subs)
{
  subs.assign(decl.size(), nullptr);
  bool named = false;
  for (size_t i = 0; i < pats.size(); ++i)
    if (pats[i].name.size()) named = true;

  if (named)
  {
    for (size_t i = 0; i < pats.size(); ++i)
    {
      std::string fn(pats[i].name);
      size_t      idx = decl.size();
      for (size_t k = 0; k < decl.size(); ++k)
        if (decl[k].first == fn) idx = k;
      if (idx == decl.size())
        fail(ER::Code::TYPE_UNBOUND,
             anchor,
             "%s '%s' has no field '%s'",
             what,
             tname.c_str(),
             fn.c_str());
      else if (subs[idx])
        fail(ER::Code::TYPE_MISMATCH,
             anchor,
             "field '%s' is matched more than once",
             fn.c_str());
      else
        subs[idx] = pats[i].pat;
    }
    return; // omitted fields stay null: ignored, fine in named mode
  }

  if (pats.size() != decl.size())
  {
    fail(ER::Code::TYPE_MISMATCH,
         anchor,
         "%s '%s' has %zu field(s) but the pattern lists %zu; list every field "
         "positionally (use '_' to ignore one) or match by name",
         what,
         tname.c_str(),
         decl.size(),
         pats.size());
    return;
  }
  for (size_t i = 0; i < decl.size(); ++i) subs[i] = pats[i].pat;
}

void
Checker::type_pattern(
  EX::Pattern *p, Type *scrut, Env &env, const std::string &munion)
{
  switch (p->tag)
  {
  case EX::PatTag::Wild: return;
  case EX::PatTag::Var:
    env[std::string(std::get<EX::PatVar>(p->as).name)] = Scheme{ {}, scrut };
    return;
  case EX::PatTag::Int : unify(scrut, con(m_tg.int_ty())); return;
  case EX::PatTag::Bool: unify(scrut, con(OP::TY_BOOL)); return;
  case EX::PatTag::Real: unify(scrut, con(OP::TY_REAL)); return;
  case EX::PatTag::Str : unify(scrut, con(OP::TY_STR)); return;
  case EX::PatTag::StrPrefix:
    unify(scrut, con(OP::TY_STR));
    type_pattern(
      std::get<EX::PatStrPrefix>(p->as).rest, con(OP::TY_STR), env, "");
    return;
  case EX::PatTag::Struct:
  {
    auto       &ps = std::get<EX::PatStruct>(p->as);
    std::string tn(ps.type_name);
    if (OP::is_tuple_name(ps.type_name)) ensure_tuple(ps.fields.size());
    auto sit = m_structs.find(tn);
    if (sit == m_structs.end())
    {
      fail(ER::Code::TYPE_UNBOUND,
           ps.anchor,
           "unknown struct type '%s' in pattern",
           tn.c_str());
      return;
    }
    std::vector<Type *> args;
    Subst               sub = fresh_subst(sit->second.params, args);
    unify(scrut, con(tn, args));
    std::vector<EX::Pattern *> subs;
    match_fieldpats(
      ps.fields, sit->second.fields, "struct", tn, ps.anchor, subs);
    for (size_t k = 0; k < subs.size(); ++k)
      if (subs[k])
        type_pattern(
          subs[k], subst(sit->second.fields[k].second, sub), env, "");
    return;
  }
  case EX::PatTag::Variant:
  {
    auto       &pv = std::get<EX::PatVariant>(p->as);
    std::string uname
      = pv.type_name.size() ? std::string(pv.type_name) : munion;
    if (uname.empty())
    {
      fail(
        ER::Code::TYPE_UNBOUND,
        pv.anchor,
        "cannot infer the union for bare variant '.%s'; qualify it (Type.%s) "
        "or match a constructor that names the type",
        std::string(pv.tag).c_str(),
        std::string(pv.tag).c_str());
      return;
    }
    auto uit = m_unions.find(uname);
    if (uit == m_unions.end())
    {
      fail(ER::Code::TYPE_UNBOUND,
           pv.anchor,
           "unknown union type '%s' in pattern",
           uname.c_str());
      return;
    }
    std::string         vtag(pv.tag);
    const VariantShape *vs = nullptr;
    for (const VariantShape &v : uit->second.variants)
      if (v.tag == vtag) vs = &v;
    if (!vs)
    {
      fail(ER::Code::TYPE_MISMATCH,
           pv.anchor,
           "union '%s' has no variant '%s'",
           uname.c_str(),
           vtag.c_str());
      return;
    }
    pv.resolved_union = intern(uname); // authoritative union for PatLower
    std::vector<Type *> args;
    Subst               sub = fresh_subst(uit->second.params, args);
    unify(scrut, con(uname, args));
    std::vector<EX::Pattern *> subs;
    std::string                vname = uname + "." + vtag;
    match_fieldpats(pv.fields, vs->fields, "variant", vname, pv.anchor, subs);
    for (size_t k = 0; k < subs.size(); ++k)
      if (subs[k])
        type_pattern(subs[k], subst(vs->fields[k].second, sub), env, "");
    return;
  }
  case EX::PatTag::Seq:
  {
    // `[..]`: defer the List-vs-Array choice to a seq site (mirrors ExSeqLit),
    // settled in resolve_lit_sites, which writes `is_array`. Elements share one
    // element type; the `..rest` tail has the whole sequence type.
    auto &pq   = std::get<EX::PatSeq>(p->as);
    Type *elem = fresh();
    for (size_t i = 0; i < pq.elems.size(); ++i)
      type_pattern(pq.elems[i], elem, env, "");
    if (pq.rest) type_pattern(pq.rest, scrut, env, "");
    LitSite s;
    s.is_seq   = true;
    s.use      = scrut;
    s.anchor   = pq.anchor;
    s.elem     = elem;
    s.exseqpat = &pq;
    m_lit_sites.push_back(std::move(s));
    return;
  }
  }
  UT_FAIL_MSG("%s", "type_pattern: unhandled PatTag");
}

/*------------------------------------------------------------------------------
 *\INFERENCE
 *-----------------------------------------------------------------------------*/

// `amb` is the ambient effect row: the effects this expression is allowed to
// perform. Performing an operation injects its effect into `amb` (an App
// unifies the callee's latent effect with `amb`); a handler discharges the
// effects it handles by typing its body under `<handled... | amb>`; a lambda
// body runs under its own fresh ambient (the arrow's latent effect). A
// top-level body is typed under the empty closed row, so an unhandled effect
// fails to unify.
// Apply a function type to an argument expression: the callee's latent effect
// must be a SUBROW of the ambient (subsumption), argument and result unify
// exactly. Shared by App and If (`if` is the application of a primitive).
Type *
Checker::infer_apply(
  Type *ft, EX::Expr *arg, Env &locals, Type *amb)
{
  Type *at = infer(arg, locals, amb);
  Type *rv = fresh();
  Type *ef = fresh(); // the callee's latent effect, extracted then subsumed
  unify(ft, arrow(at, rv, ef));
  subrow(ef, amb);
  return rv;
}

Type *
Checker::infer(
  EX::Expr *e, Env &locals, Type *amb)
{
  switch (e->tag)
  {
  case EX::ExprTag::Int:
  {
    long long v = (long long)std::get<EX::ExInt>(e->as).value;
    if (v > m_tg.lit_max())
      fail(ER::Code::INT_LITERAL_RANGE,
           m_anchor,
           "integer literal %lld does not fit the target's %d-bit word (max "
           "%lld for %s)",
           v,
           m_tg.ptr_bits(),
           m_tg.lit_max(),
           m_tg.name().c_str());
    return con(m_tg.int_ty());
  }
  case EX::ExprTag::Bool  : return con(OP::TY_BOOL);
  case EX::ExprTag::Real  : return con(OP::TY_REAL);
  case EX::ExprTag::Str   : return con(OP::TY_STR);
  case EX::ExprTag::Unit  : return con(OP::TY_UNIT);
  case EX::ExprTag::Extern: return fresh(); // unifies with declared signature

  case EX::ExprTag::Handle:
  {
    // The handler discharges the effects named by its clauses: the body runs
    // under `<handled... | amb>`, the handle expression itself under `amb`.
    // else x = e: x has the body's type, e has `result`; with no else the
    // body's result IS the handler's. Each clause `is op a = e` for `op : A ->
    // B` binds a : A and k : B ->^amb result (resuming is deep, so k runs under
    // amb).
    EX::ExHandle &h      = std::get<EX::ExHandle>(e->as);
    Type         *result = fresh();

    // Build the body's ambient: amb extended with each DISTINCT handled effect
    // (several clauses may handle one effect, e.g. get/put both belong to
    // State; adding the label once keeps the row in step with what the body
    // performs).
    Type *inner = amb;
    {
      std::unordered_set<std::string> seen;
      for (EX::HandlerClause &c : h.clauses)
      {
        std::string op(c.op);
        auto        oe = m_op_effect.find(op);
        if (oe != m_op_effect.end() && seen.insert(oe->second).second)
          inner = row_extend(oe->second, inner);
      }
    }
    Type *tbody = infer(h.body, locals, inner);
    if (h.else_body)
    {
      Env e2                      = locals;
      e2[std::string(h.else_var)] = Scheme{ {}, tbody };
      unify(infer(h.else_body, e2, amb), result);
    }
    else
      unify(tbody, result);
    for (EX::HandlerClause &c : h.clauses)
    {
      std::string op(c.op);
      auto        pit = m_prim.find(op);
      if (pit == m_prim.end())
      {
        fail(ER::Code::TYPE_MISMATCH,
             m_anchor,
             "unknown effect operation '%s' in handler",
             op.c_str());
        continue;
      }
      Type *opty = instantiate(pit->second);
      Type *A    = fresh();
      Type *B    = fresh();
      unify(opty, arrow(A, B));
      Env e2                 = locals;
      e2[std::string(c.arg)] = Scheme{ {}, A };
      e2[std::string(h.k)]   = Scheme{ {}, arrow(B, result, amb) };
      unify(infer(c.body, e2, amb), result);
    }
    return result;
  }

  case EX::ExprTag::Var:
  {
    EX::ExVar  &v = std::get<EX::ExVar>(e->as);
    std::string name(v.name);

    auto it = locals.find(name);
    if (it != locals.end()) return instantiate(it->second);
    if (m_globals.find(name)) return instantiate(resolve(name));
    auto pit = m_prim.find(name);
    if (pit != m_prim.end()) return instantiate(pit->second);

    // An overloaded name: type it as a fresh variable and defer the choice of
    // overload to resolve_sites, once its operands (and result) are settled.
    if (m_ovdb.count(name))
    {
      // The chosen impl key is written to `resolved` (which CR reads), NOT to
      // `name`, so a re-inference of this same node still keys off the original
      // overloaded name and resolves identically.
      Type *use = fresh();
      m_sites.push_back({ &v.resolved, use, name, v.name });
      return use;
    }

    fail(ER::Code::TYPE_UNBOUND,
         v.name.data() ? v.name : m_anchor,
         "unbound variable '%s'",
         name.c_str());
    return fresh();
  }

  case EX::ExprTag::Overload:
  {
    // A use MR left as an overload set: type as a fresh var and defer the
    // choice to resolve_user_sites, once the use's type is settled by context.
    EX::ExOverload &ov  = std::get<EX::ExOverload>(e->as);
    Type           *use = fresh();
    m_usites.push_back(
      { &ov.chosen, use, &ov.candidates, std::string(ov.name), ov.anchor });
    return use;
  }

  case EX::ExprTag::FnDef:
  {
    // Constructing a closure performs no effect (the lambda itself is pure
    // under `amb`); its body runs under a fresh ambient that becomes the
    // arrow's latent effect.
    EX::ExFnDef &l      = std::get<EX::ExFnDef>(e->as);
    Type        *pv     = fresh();
    Type        *e_body = fresh();
    Env          inner  = locals;
    if (l.param_pat)
      type_pattern(l.param_pat, pv, inner, "");
    else
      inner[std::string(l.param)] = Scheme{ {}, pv }; // params are monomorphic
    Type *bt = infer(l.body, inner, e_body);
    return arrow(pv, bt, e_body);
  }

  case EX::ExprTag::App:
  {
    // The callee may perform AT MOST what the ambient allows (subsumption); see
    // infer_apply. (Operations are Apps whose callee carries `<L|mu>`, so
    // subsuming it forces L into amb.)
    EX::ExApp &ap = std::get<EX::ExApp>(e->as);
    Type      *ft = infer(ap.fn, locals, amb);
    return infer_apply(ft, ap.arg, locals, amb);
  }

  case EX::ExprTag::If:
  {
    // `if` is the effect-polymorphic primitive `Int -> T -> T -> T` applied to
    // cond/then/alt; typing it as three applications keeps the effect handling
    // and result type identical to a source call.
    EX::ExIf &i   = std::get<EX::ExIf>(e->as);
    auto      pit = m_prim.find(OP::IF);
    UT_FAIL_IF(pit == m_prim.end());
    Type *ft = instantiate(pit->second);
    ft       = infer_apply(ft, i.cond, locals, amb);
    ft       = infer_apply(ft, i.then, locals, amb);
    ft       = infer_apply(ft, i.alt, locals, amb);
    return ft;
  }

  case EX::ExprTag::Let:
  {
    EX::ExLet &lt  = std::get<EX::ExLet>(e->as);
    Type      *sig = nullptr;
    if (lt.sig)
    {
      TyVarEnv tv;
      sig = sig_to_type(lt.sig, tv, false);
    }
    if (lt.pat)
    {
      Type *vt = infer(lt.val, locals, amb);
      if (sig) unify(sig, vt);
      Env inner = locals;
      type_pattern(lt.pat, vt, inner, "");
      return infer(lt.body, inner, amb);
    }
    std::string nm(lt.var);
    Type       *vt;
    if (occurs_free(nm, lt.val))
    {
      // recursive: bind the name monomorphically while inferring the value
      Type *nv  = fresh();
      Env   rec = locals;
      rec[nm]   = Scheme{ {}, nv };
      vt        = infer(lt.val, rec, amb);
      unify(nv, vt);
      vt = nv;
    }
    else
    {
      vt = infer(lt.val, locals, amb);
    }
    if (sig) unify(sig, vt); // pin the value's type (pattern-let subject)
    Scheme s     = generalize(locals, vt);
    Env    inner = locals;
    inner[nm]    = s;
    return infer(lt.body, inner, amb);
  }

  case EX::ExprTag::StructLit:
  {
    EX::ExStructLit &sl = std::get<EX::ExStructLit>(e->as);
    UT::Vu           at = sl.type_name.data() ? sl.type_name : m_anchor;
    std::string      tn(sl.type_name);

    // Bare `.{...}`: defer to a lit site, typed as a fresh var that later
    // unification (an annotation or an enclosing field) must bind to a struct.
    if (tn.empty())
    {
      LitSite s;
      s.is_struct = true;
      s.use       = fresh();
      s.anchor    = at;
      s.exs       = &sl;
      for (size_t i = 0; i < sl.fields.size(); ++i)
        s.fields.push_back({ std::string(sl.fields[i].name),
                             infer(sl.fields[i].val, locals, amb) });
      Type *use = s.use;
      m_lit_sites.push_back(std::move(s));
      return use;
    }

    if (OP::is_tuple_name(sl.type_name)) ensure_tuple(sl.fields.size());
    auto sit = m_structs.find(tn);
    if (sit == m_structs.end())
    {
      fail(ER::Code::TYPE_UNBOUND, at, "unknown struct type '%s'", tn.c_str());
      return fresh();
    }
    const StructFields &decl = sit->second.fields;

    // Instantiate the struct's parameters fresh for this use; field types are
    // read through this substitution and the result is `con(name, args)`.
    std::vector<Type *> args;
    Subst               sub = fresh_subst(sit->second.params, args);

    // Every declared field must be initialized exactly once; reject unknown and
    // duplicate fields. Each initializer is checked against its declared type.
    std::vector<bool> seen(decl.size(), false);
    for (size_t i = 0; i < sl.fields.size(); ++i)
    {
      std::string fn(sl.fields[i].name);
      size_t      idx = decl.size();
      for (size_t k = 0; k < decl.size(); ++k)
        if (decl[k].first == fn) idx = k;

      if (idx == decl.size())
      {
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "struct '%s' has no field '%s'",
             tn.c_str(),
             fn.c_str());
        continue;
      }
      if (seen[idx])
      {
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "field '%s' is set more than once in '%s'",
             fn.c_str(),
             tn.c_str());
        continue;
      }
      seen[idx] = true;
      Type *vt  = infer(sl.fields[i].val, locals, amb);
      unify(subst(decl[idx].second, sub), vt);
    }
    for (size_t k = 0; k < decl.size(); ++k)
      if (!seen[k])
        fail(ER::Code::TYPE_MISMATCH,
             at,
             "struct '%s' is missing field '%s'",
             tn.c_str(),
             decl[k].first.c_str());

    return con(tn, args);
  }

  case EX::ExprTag::SeqLit:
  {
    // `[e1, .., en]`: all elements share one type; the container (List vs
    // Array) is deferred to a lit site, settled once `use` is bound by context.
    EX::ExSeqLit &sq   = std::get<EX::ExSeqLit>(e->as);
    UT::Vu        at   = sq.anchor.data() ? sq.anchor : m_anchor;
    Type         *elem = fresh();
    for (size_t i = 0; i < sq.elems.size(); ++i)
      unify(infer(sq.elems[i], locals, amb), elem);
    LitSite s;
    s.is_seq  = true;
    s.use     = fresh();
    s.anchor  = at;
    s.elem    = elem;
    s.exseq   = &sq;
    Type *use = s.use;
    m_lit_sites.push_back(std::move(s));
    return use;
  }

  case EX::ExprTag::VariantLit:
  {
    EX::ExVariantLit &vl = std::get<EX::ExVariantLit>(e->as);
    UT::Vu            at = vl.anchor.data() ? vl.anchor : m_anchor;
    std::string       tn(vl.type_name);
    std::string       vtag(vl.tag);

    // Bare `.Tag...`: defer to a lit site keyed by the tag; later unification
    // binds its `use` var to the union the tag belongs to.
    if (tn.empty())
    {
      LitSite s;
      s.is_struct = false;
      s.use       = fresh();
      s.anchor    = at;
      s.vtag      = vtag;
      s.exv       = &vl;
      for (size_t i = 0; i < vl.fields.size(); ++i)
        s.fields.push_back({ std::string(vl.fields[i].name),
                             infer(vl.fields[i].val, locals, amb) });
      Type *use = s.use;
      m_lit_sites.push_back(std::move(s));
      return use;
    }

    auto uit = m_unions.find(tn);
    if (uit == m_unions.end())
    {
      fail(ER::Code::TYPE_UNBOUND, at, "unknown union type '%s'", tn.c_str());
      return fresh();
    }
    const StructFields *decl = nullptr;
    for (auto &v : uit->second.variants)
      if (v.tag == vtag) decl = &v.fields;
    if (!decl)
    {
      fail(ER::Code::TYPE_MISMATCH,
           at,
           "union '%s' has no variant '%s'",
           tn.c_str(),
           vtag.c_str());
      return fresh();
    }

    // Instantiate the union's parameters fresh; payload types are read through
    // this substitution and the result is `con(name, args)`.
    std::vector<Type *> args;
    Subst               sub = fresh_subst(uit->second.params, args);

    // Named when any value carries a field name (then all must be set exactly
    // once, any order); positional otherwise (one value per field, in order).
    bool named = false;
    for (size_t i = 0; i < vl.fields.size(); ++i)
      if (vl.fields[i].name.size()) named = true;

    if (named)
    {
      std::vector<bool> seen(decl->size(), false);
      for (size_t i = 0; i < vl.fields.size(); ++i)
      {
        std::string fn(vl.fields[i].name);
        size_t      idx = decl->size();
        for (size_t k = 0; k < decl->size(); ++k)
          if ((*decl)[k].first == fn) idx = k;
        if (idx == decl->size())
        {
          fail(ER::Code::TYPE_MISMATCH,
               at,
               "variant '%s.%s' has no field '%s'",
               tn.c_str(),
               vtag.c_str(),
               fn.c_str());
          continue;
        }
        if (seen[idx])
        {
          fail(ER::Code::TYPE_MISMATCH,
               at,
               "field '%s' is set more than once in '%s.%s'",
               fn.c_str(),
               tn.c_str(),
               vtag.c_str());
          continue;
        }
        seen[idx] = true;
        unify(subst((*decl)[idx].second, sub),
              infer(vl.fields[i].val, locals, amb));
      }
      for (size_t k = 0; k < decl->size(); ++k)
        if (!seen[k])
          fail(ER::Code::TYPE_MISMATCH,
               at,
               "variant '%s.%s' is missing field '%s'",
               tn.c_str(),
               vtag.c_str(),
               (*decl)[k].first.c_str());
    }
    else if (vl.fields.size() != decl->size())
    {
      fail(ER::Code::TYPE_MISMATCH,
           at,
           "variant '%s.%s' takes %zu payload field(s), but %zu given",
           tn.c_str(),
           vtag.c_str(),
           decl->size(),
           vl.fields.size());
    }
    else
    {
      for (size_t k = 0; k < decl->size(); ++k)
        unify(subst((*decl)[k].second, sub),
              infer(vl.fields[k].val, locals, amb));
    }

    return con(tn, args);
  }

  case EX::ExprTag::Field:
  {
    // Defer: the receiver's type may not be concrete yet (e.g. it is the result
    // of an as-yet-unresolved overload). Type the access as a fresh var and
    // settle it in resolve_field_sites once the receiver is known.
    EX::ExField &fld = std::get<EX::ExField>(e->as);
    UT::Vu       at  = fld.field.data() ? fld.field : m_anchor;
    Type        *rt  = infer(fld.record, locals, amb);
    Type        *res = fresh();
    m_field_sites.push_back({ rt, std::string(fld.field), at, res });
    return res;
  }

  case EX::ExprTag::Case:
  {
    EX::ExCase &cc = std::get<EX::ExCase>(e->as);
    Type       *st = infer(cc.scrut, locals, amb); // the scrutinee
    Type       *rt = fresh(); // every arm and the default share one type
    for (size_t ai = 0; ai < cc.alts.size(); ++ai)
    {
      EX::CaseAlt &alt   = cc.alts[ai];
      Env          inner = locals;
      switch (alt.kind)
      {
      case EX::AltKind::Int : unify(st, con(m_tg.int_ty())); break;
      case EX::AltKind::Real: unify(st, con(OP::TY_REAL)); break;
      case EX::AltKind::Con:
      {
        // The scrutinee is the constructor's union type; instantiate the union
        // fresh and bind its payload from the matched variant's field types,
        // read through that same instantiation.
        std::string tn(alt.type_name);
        auto        uit = m_unions.find(tn);
        if (uit != m_unions.end() && alt.tag < uit->second.variants.size())
        {
          std::vector<Type *> args;
          Subst               sub = fresh_subst(uit->second.params, args);
          unify(st, con(tn, args));
          const StructFields &decl = uit->second.variants[alt.tag].fields;
          for (size_t i = 0; i < alt.binders.size() && i < decl.size(); ++i)
            if (alt.binders[i].size())
              inner[std::string(alt.binders[i])]
                = Scheme{ {}, subst(decl[i].second, sub) };
        }
        else
          unify(st, con(tn));
        break;
      }
      }
      unify(rt, infer(alt.body, inner, amb));
    }
    unify(rt, infer(cc.deflt, locals, amb)); // the default arm
    return rt;
  }

  case EX::ExprTag::Match:
  {
    EX::ExMatch &m  = std::get<EX::ExMatch>(e->as);
    Type        *st = infer(m.scrut, locals, amb);
    std::string  mu = resolve_match_union(st, m);
    Type        *rt = fresh();
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      Env inner = locals;
      type_pattern(m.arms[i].pat, st, inner, mu);
      if (m.arms[i].guard)
        unify(infer(m.arms[i].guard, inner, amb), con(OP::TY_BOOL));
      unify(rt, infer(m.arms[i].body, inner, amb));
    }
    // The `else` value (if written) is a fallthrough result of the same type;
    // with no `else`, the arms must cover every case (checked here so PatLower
    // can safely supply an unreachable default).
    if (m.alt)
      unify(rt, infer(m.alt, locals, amb));
    else
      check_exhaustive(m);
    return rt;
  }

  case EX::ExprTag::Def:
  case EX::ExprTag::StructDecl:
  case EX::ExprTag::UnionDecl:
  case EX::ExprTag::AliasDecl:
  case EX::ExprTag::EffectDecl:
  case EX::ExprTag::ModDecl:
  case EX::ExprTag::Import:
  case EX::ExprTag::Vis:
  case EX::ExprTag::Unknown:
    UT_FAIL_MSG("infer: unexpected node %s (top-level-only, or removed by "
                "MR/PatLower)",
                EX::pprint(e->tag).c_str());
  }
  UT_FAIL_MSG("%s", "infer: unhandled ExprTag");
  return fresh();
}

Type *
Checker::infer_against(
  EX::Expr *e, Type *expected, Env &locals, Type *amb)
{
  Type *exp = prune(expected);
  if (e->tag == EX::ExprTag::FnDef && exp->kind() == Kind::Arrow)
  {
    EX::ExFnDef &l     = std::get<EX::ExFnDef>(e->as);
    TArrow      &a     = std::get<TArrow>(exp->as);
    Env          inner = locals;
    if (l.param_pat)
      type_pattern(l.param_pat, a.from, inner, "");
    else
      inner[std::string(l.param)]
        = Scheme{ {}, a.from }; // param pinned to decl
    Type *bt = infer_against(l.body, a.to, inner, a.eff);
    return arrow(a.from, bt, a.eff);
  }
  Type *t = infer(e, locals, amb);
  unify(t, expected);
  return t;
}

/*------------------------------------------------------------------------------
 *\RESOLUTION (phase B)
 *-----------------------------------------------------------------------------*/

// A global's source-name view, for anchoring a diagnostic at its definition.
// (`name` is the mangled `MOD/x`, which is an arena string with no source
// location; `origin` keeps the original view into the file.)
static UT::Vu
def_anchor(
  const EX::ExDef *d)
{
  return d->origin.data() ? d->origin : d->name;
}

// A global's name for a user-facing message: the mangled `MOD/x` shown `MOD.x`.
static std::string
def_label(
  UT::Vu mangled)
{
  std::string s(mangled);
  size_t      slash = s.find('/');
  if (slash != std::string::npos) s[slash] = '.';
  return s;
}

Scheme
Checker::resolve(
  const std::string &name)
{
  GlobalEntry &g = *m_globals.find(name);
  if (g.state == GlobalEntry::Resolved) return g.scheme;
  if (g.state == GlobalEntry::Resolving)
  {
    fail(ER::Code::TYPE_CYCLE,
         def_anchor(g.def),
         "global '%s' depends on itself but has no type annotation",
         def_label(g.def->name).c_str());
    g.scheme = Scheme{ {}, fresh() };
    g.state  = GlobalEntry::Resolved;
    return g.scheme;
  }

  g.state = GlobalEntry::Resolving;

  if (g.def->sig)
  {
    UT::Vu save_a = m_anchor;
    size_t save_l = m_line;
    m_anchor      = def_anchor(g.def);
    m_line        = line_of(m_anchor);
    g.scheme      = scheme_of_sig(g.def->sig);
    m_anchor      = save_a;
    m_line        = save_l;
  }
  else
  {
    UT::Vu save_a = m_anchor;
    size_t save_l = m_line;
    m_anchor      = def_anchor(g.def);
    m_line        = line_of(m_anchor);

    Env    empty;
    size_t base  = m_sites.size();
    size_t ubase = m_usites.size();
    size_t fbase = m_field_sites.size();
    size_t lbase = m_lit_sites.size();
    Type  *t     = infer(g.body, empty, row_empty()); // top level is pure
    resolve_sites(base, fbase, ubase); // settle this body's overloads first
    resolve_user_sites(ubase);  // ... then force the user overloads left open
    resolve_field_sites(fbase); // ... then field accesses (receivers now known)
    resolve_lit_sites(lbase);   // ... and any unqualified literals (no context)
    t = prune(t);

    if (t->kind() != Kind::Con)
    {
      fail(
        ER::Code::TYPE_ANNOTATION_REQUIRED,
        def_anchor(g.def),
        "global '%s' needs a type annotation (its type is '%s', not a ground "
        "Int/Str)",
        def_label(g.def->name).c_str(),
        show(t).c_str());
      g.scheme = Scheme{ {}, t };
    }
    else
    {
      g.scheme = Scheme{ {}, t };
    }
    m_anchor = save_a;
    m_line   = save_l;
  }

  g.state = GlobalEntry::Resolved;
  return g.scheme;
}

// Resolve the overload sites collected since index `from` (one inference
// scope's worth), then drop them. For each site we force the use type into an
// arrow chain of the operator's arity -- so an under-applied use still exposes
// operand variables -- default any still-unconstrained operand to Int
// (SML-style), then pick the overload whose operand types match and rewrite the
// EX Var name to its implementation key.
// Try to resolve one overload site. `allow_default` permits SML-style
// defaulting of an unconstrained operand to Int (`\x = x * x`). Returns:
//   Resolved -- an overload was chosen (`*s.slot` written, result unified);
//   Pending  -- some operand is still an unresolved variable and defaulting is
//               off, so a later pass (once a dependency resolves) may settle
//               it;
//   Failed   -- operands are concrete but match no overload (a diagnostic was
//               emitted).
// Splitting resolution from the driver lets nested/chained overloads settle in
// dependency order: `a ++ b ++ c` needs the inner `++` first, `let b = push a
// .. in push b ..` needs the outer first -- opposite orders. A fixpoint over
// the no-default pass handles both; defaulting is a last resort.
Checker::SiteState
Checker::resolve_one_site(
  const ResolveSite &s, bool allow_default)
{
  const std::vector<Overload> *ovs = UT::try_lookup(m_ovdb, s.base);
  UT_FAIL_IF(!ovs || ovs->empty()); // a site is only made for overloaded names
  size_t arity = ovs->front().sig.size() - 1;

  // Shape the use as (a1 -> ... -> ak -> r) so the operands are reachable
  // whether the operator was fully applied, partially applied, or passed bare.
  std::vector<Type *> args(arity);
  Type               *result = fresh();
  Type               *shape  = result;
  for (size_t k = arity; k-- > 0;)
  {
    args[k] = fresh();
    shape   = arrow(args[k], shape);
  }
  unify(s.use, shape);

  for (Type *&a : args) a = prune(a);

  // Without defaulting, an unresolved operand means "come back later" -- a
  // dependency (an inner/earlier overload) may still make it concrete.
  if (!allow_default)
    for (Type *a : args)
      if (a->kind() == Kind::Var) return SiteState::Pending;

  // Last resort: an operand still unconstrained defaults to Int.
  if (allow_default)
    for (Type *&a : args)
      if (a->kind() == Kind::Var && !std::get<TVar>(a->as).rigid)
      {
        std::get<TVar>(a->as).ref = con(m_tg.int_ty());
        a                         = prune(a);
      }

  // Match on operand types. The result type is stored in every signature and
  // unified back in, but is not yet used to discriminate (return-type
  // overloading is the next extension; see Overload).
  const Overload *hit = nullptr;
  for (const Overload &o : *ovs)
  {
    if (o.sig.size() != arity + 1) continue;
    bool ok = true;
    for (size_t k = 0; k < arity; ++k)
      if (args[k]->kind() != Kind::Con
          || std::get<TCon>(args[k]->as).name != o.sig[k])
      {
        ok = false;
        break;
      }
    if (ok)
    {
      hit = &o;
      break;
    }
  }

  if (!hit)
  {
    std::string operands;
    for (size_t k = 0; k < arity; ++k)
      operands += (k ? ", " : "") + show(args[k]);
    fail(ER::Code::TYPE_MISMATCH,
         s.anchor,
         "no overload of '%s' for operand type(s): %s",
         s.base.c_str(),
         operands.c_str());
    return SiteState::Failed;
  }

  unify(prune(result), con(hit->sig.back()));
  // Intern the key: `hit->mono` lives in the per-checker overload table, which
  // dies with the Checker, while the write-back is read later by CR.
  *s.slot = UT::strdup(m_arena, hit->mono.c_str());
  return SiteState::Resolved;
}

void
Checker::resolve_sites(
  size_t from, size_t ffrom, size_t ufrom)
{
  std::vector<size_t> pending;
  for (size_t i = from; i < m_sites.size(); ++i) pending.push_back(i);

  // Fixpoint over the no-default pass: resolve every site whose operands are
  // already concrete, repeating while any pass makes progress. This settles
  // dependencies in whatever order they actually chain (inner-first for nested
  // ops, outer-first for let-threaded results) without committing to one.
  // Field accesses whose receiver is already a known struct join the fixpoint:
  // `p.snd ?= "x"` needs the projection's type grounded BEFORE the operator is
  // judged, or the last-resort Int-defaulting below would mistype it. User
  // overload sites join for the same reason, conservatively (commit only when
  // exactly one candidate fits, never default): `(pow 2.0 10.0) ?= 1024.0`
  // needs `pow` committed to its Real overload BEFORE `?=` is judged -- and a
  // resolving operator can conversely narrow a user site's candidates. Both
  // directions are safe to interleave because unification only ever shrinks a
  // site's fit set: a single-fit commit now is the same commit any later pass
  // would make.
  bool progress = true;
  while (progress)
  {
    progress = settle_ready_field_sites(ffrom);
    for (size_t &idx : pending)
    {
      if (idx == (size_t)-1) continue; // already resolved
      if (resolve_one_site(m_sites[idx], /*allow_default=*/false)
          == SiteState::Resolved)
      {
        idx      = (size_t)-1;
        progress = true;
      }
    }
    for (size_t i = ufrom; i < m_usites.size(); ++i)
    {
      UserSite &u = m_usites[i];
      if (u.done) continue;
      if (resolve_one_user_site(u, /*conservative=*/true)
          == SiteState::Resolved)
        progress = true;
    }
  }

  // Whatever remains is genuinely unconstrained (or a real mismatch): force it
  // with Int-defaulting, which either resolves or reports the error. The
  // remaining user sites are forced by resolve_user_sites, which the caller
  // runs right after this.
  for (size_t idx : pending)
    if (idx != (size_t)-1)
      resolve_one_site(m_sites[idx], /*allow_default=*/true);

  m_sites.erase(m_sites.begin() + (long)from, m_sites.end());
}

// Resolve the user-overload sites collected since `from`. By now the use's type
// is fixed by its surrounding context, so for each site we probe every
// candidate global -- instantiating its scheme and speculatively unifying it
// against the use (try_unify rolls the trial back) -- and keep the ones that
// fit. For an operator site the built-in meanings (overload_db) are folded in
// too: the use is shaped into an arrow chain, unconstrained operands default to
// Int (as for a plain built-in operator), and a matching built-in counts as a
// candidate. Exactly one fit is the answer: we commit it and write the chosen
// name to the slot (the EX::ExOverload's `chosen`) -- a mangled global for a
// user overload, or a "+@Int"-style key for a built-in, both of which IT reads.
// None or several is an error.
// Try to resolve one user-overload site. In `conservative` mode (inside
// resolve_sites' fixpoint) nothing is ever forced: an operator shape with a
// still-open operand returns Pending (the built-in candidates cannot be judged
// yet), and several fitting candidates return Pending (a later resolution may
// narrow them). A commit happens only on exactly one fit -- which is sound to
// do early, since unification only ever removes candidates from the fit set.
// In final mode (from resolve_user_sites) unconstrained operator operands
// default to Int (so `\x = x + x` still picks the Int built-in) and anything
// other than exactly one fit is an error.
Checker::SiteState
Checker::resolve_one_user_site(
  UserSite &s, bool conservative)
{
  Type                        *use = prune(s.use);
  const std::vector<Overload> *ovs = UT::try_lookup(m_ovdb, s.name);

  // For an operator, shape the use as an arrow chain so the operands are
  // reachable by type.
  std::vector<Type *> args;
  Type               *result = nullptr;
  if (ovs)
  {
    size_t arity = ovs->front().sig.size() - 1;
    result       = fresh();
    Type *shape  = result;
    args.resize(arity);
    for (size_t k = arity; k-- > 0;)
    {
      args[k] = fresh();
      shape   = arrow(args[k], shape);
    }
    unify(use, shape);
    for (Type *&a : args)
    {
      a = prune(a);
      if (a->kind() == Kind::Var && !std::get<TVar>(a->as).rigid)
      {
        if (conservative) return SiteState::Pending;
        std::get<TVar>(a->as).ref = con(m_tg.int_ty());
        a                         = prune(a);
      }
    }
    use = prune(use);
  }

  // The matching built-in overload, if any (operators only).
  const Overload *bhit = nullptr;
  if (ovs)
    for (const Overload &o : *ovs)
    {
      if (o.sig.size() != args.size() + 1) continue;
      bool ok = true;
      for (size_t k = 0; k < args.size(); ++k)
        if (args[k]->kind() != Kind::Con
            || std::get<TCon>(args[k]->as).name != o.sig[k])
        {
          ok = false;
          break;
        }
      if (ok)
      {
        bhit = &o;
        break;
      }
    }

  // The fitting user candidates. A candidate may be an effect operation
  // (its scheme is in m_prim, keyed by `Effect.op`) rather than a global.
  std::vector<size_t> fits;
  for (size_t k = 0; k < s.cands->size(); ++k)
  {
    std::string cname(std::string((*s.cands)[k]));
    auto        pit = m_prim.find(cname);
    Type       *cand
      = instantiate(pit != m_prim.end() ? pit->second : resolve(cname));
    if (try_unify(use, cand)) fits.push_back(k);
  }

  size_t total = fits.size() + (bhit ? 1u : 0u);
  if (total == 0)
  {
    // No fit can appear later (grounding only removes candidates), so this is
    // a real error even conservatively.
    fail(ER::Code::TYPE_MISMATCH,
         s.anchor,
         "no overload of '%s' has type '%s'",
         s.name.c_str(),
         show(use).c_str());
    s.done = true;
    return SiteState::Failed;
  }
  if (total > 1)
  {
    if (conservative) return SiteState::Pending;
    std::string cands;
    if (bhit) cands += "built-in " + bhit->mono;
    for (size_t f : fits)
      cands += (cands.empty() ? "" : ", ") + def_label((*s.cands)[f]);
    fail(ER::Code::AMBIGUOUS_NAME,
         s.anchor,
         "ambiguous overload of '%s' at type '%s'; candidates: %s",
         s.name.c_str(),
         show(use).c_str(),
         cands.c_str());
    s.done = true;
    return SiteState::Failed;
  }

  s.done = true;
  if (bhit)
  {
    unify(prune(result), con(bhit->sig.back()));
    // Interned for the same reason as the operator-site write-back above.
    *s.slot = UT::strdup(m_arena, bhit->mono.c_str());
    return SiteState::Resolved;
  }

  UT::Vu      chosen = (*s.cands)[fits.front()];
  std::string cname(chosen);
  auto        pit = m_prim.find(cname); // an operation candidate lives here
  unify(use, instantiate(pit != m_prim.end() ? pit->second : resolve(cname)));
  *s.slot = chosen;
  return SiteState::Resolved;
}

void
Checker::resolve_user_sites(
  size_t from)
{
  for (size_t i = from; i < m_usites.size(); ++i)
  {
    UserSite &s = m_usites[i];
    if (s.done) continue; // settled inside resolve_sites' fixpoint
    resolve_one_user_site(s, /*conservative=*/false);
  }
  m_usites.erase(m_usites.begin() + (long)from, m_usites.end());
}

void
Checker::ensure_tuple(
  size_t n)
{
  std::string nm = OP::tuple_name(n);
  if (m_structs.count(nm)) return;
  StructFields flds;
  VarIds       params;
  for (size_t k = 0; k < n; ++k)
  {
    Type *v = fresh();
    params.push_back(std::get<TVar>(v->as).id);
    flds.push_back({ std::to_string(k), v });
  }
  m_arity[nm]   = n;
  m_structs[nm] = StructDef{ std::move(params), std::move(flds) };
}

bool
Checker::settle_field_site(
  FieldSite &s)
{
  Type                      *rt    = prune(s.record);
  const std::string         &rname = std::get<TCon>(rt->as).name;
  const StructDef           &sdef  = m_structs.at(rname);
  Subst                      sub;
  const std::vector<Type *> &actual = std::get<TCon>(rt->as).args;
  for (size_t k = 0; k < sdef.params.size() && k < actual.size(); ++k)
    sub[sdef.params[k]] = actual[k];

  s.done = true;
  for (auto &f : sdef.fields)
    if (f.first == s.field)
    {
      unify(s.result, subst(f.second, sub));
      return true;
    }
  if (OP::is_tuple_name(rname))
    fail(ER::Code::TYPE_UNBOUND,
         s.anchor,
         "tuple '%s' has no element '.%s' (indices are 0..%zu)",
         show(rt).c_str(),
         s.field.c_str(),
         sdef.fields.size() - 1);
  else
    fail(ER::Code::TYPE_UNBOUND,
         s.anchor,
         "struct '%s' has no field '%s'",
         rname.c_str(),
         s.field.c_str());
  return true;
}

// The eager half of field resolution, run inside resolve_sites' fixpoint:
// settle every not-yet-done site whose receiver has ALREADY become a known
// struct (via an annotation, a signature, an earlier resolution), so the
// projection's type can feed operator-overload resolution. Sites whose
// receiver is still open are left for resolve_field_sites, which may run
// after further resolution grounds them. Returns whether anything settled.
bool
Checker::settle_ready_field_sites(
  size_t ffrom)
{
  bool progress = false;
  for (size_t i = ffrom; i < m_field_sites.size(); ++i)
  {
    FieldSite &s = m_field_sites[i];
    if (s.done) continue;
    Type *rt = prune(s.record);
    if (rt->kind() != Kind::Con
        || !m_structs.count(std::get<TCon>(rt->as).name))
      continue; // not ready; the strict pass owns the eventual error
    progress |= settle_field_site(s);
  }
  return progress;
}

// Settle the field accesses collected since `from`. By now each receiver's type
// has been fixed by its context (an overload resolved, a signature applied,
// ...), so we can look the field up on its struct and unify its type into the
// access's result var. Resolved in recording order (innermost-first), so
// `a.b.c` settles `a.b` -- giving the receiver of `.c` its struct type --
// before `(a.b).c`. Sites already settled inside resolve_sites' fixpoint are
// skipped; a receiver still not a known struct here is an error.
void
Checker::resolve_field_sites(
  size_t from)
{
  for (size_t i = from; i < m_field_sites.size(); ++i)
  {
    FieldSite &s = m_field_sites[i];
    if (s.done) continue;
    Type *rt = prune(s.record);
    if (rt->kind() != Kind::Con
        || !m_structs.count(std::get<TCon>(rt->as).name))
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "cannot access field '%s': '%s' is not a known struct type",
           s.field.c_str(),
           show(rt).c_str());
      continue;
    }
    settle_field_site(s);
  }
  m_field_sites.erase(m_field_sites.begin() + (long)from, m_field_sites.end());
}

UT::Vu
Checker::intern(
  const std::string &s)
{
  char *mem = (char *)m_arena.alloc(s.size() ? s.size() : 1);
  std::memcpy(mem, s.data(), s.size());
  return UT::Vu{ mem, s.size() };
}

// Settle the bare struct/union literals collected since `from`. Each was typed
// as a fresh `use` var; by now unification (an annotation, an enclosing field,
// ...) should have bound it to a concrete nominal type. We check the payload
// against that type's (instantiated) fields and patch the EX node's type name
// so the interpreter sees a resolved literal. Resolved outermost-first (reverse
// of the post-order in which they were recorded) so an outer literal binds an
// inner one's `use` before the inner is judged.
void
Checker::resolve_lit_sites(
  size_t from)
{
  for (size_t i = m_lit_sites.size(); i-- > from;)
  {
    LitSite &s = m_lit_sites[i];
    Type    *u = prune(s.use);

    // A `[..]` sequence literal: settle its container. Unconstrained defaults
    // to a List (backward-compatible with the list literals); an Array context
    // makes it a byte vector (elements must be Int). CR reads `is_array`.
    if (s.is_seq)
    {
      if (u->kind() == Kind::Var)
      {
        unify(s.use, con("List", { s.elem }));
        if (s.exseq) s.exseq->is_array = false;
        if (s.exseqpat) s.exseqpat->is_array = false;
        continue;
      }
      if (u->kind() == Kind::Con)
      {
        TCon &uc = std::get<TCon>(u->as);
        if (uc.name == OP::TY_ARRAY)
        {
          unify(s.elem, con(m_tg.int_ty())); // Array holds bytes (Int 0..255)
          if (s.exseq) s.exseq->is_array = true;
          if (s.exseqpat) s.exseqpat->is_array = true;
          continue;
        }
        if (uc.name == "List")
        {
          if (!uc.args.empty()) unify(uc.args[0], s.elem);
          if (s.exseq) s.exseq->is_array = false;
          if (s.exseqpat) s.exseqpat->is_array = false;
          continue;
        }
      }
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "a sequence '[..]' must be a List or an Array, not '%s'",
           show(u).c_str());
      continue;
    }

    if (u->kind() != Kind::Con)
    {
      fail(ER::Code::TYPE_ANNOTATION_REQUIRED,
           s.anchor,
           "cannot infer the type of this %s literal; add a type annotation",
           s.is_struct ? "struct" : "variant");
      continue;
    }
    TCon              &uc = std::get<TCon>(u->as);
    const std::string &nm = uc.name;

    // Bind the nominal type's parameters to the receiver's actual arguments, so
    // declared field types are read at this literal's instantiation.
    auto bind = [&](const VarIds &params) {
      Subst sub;
      for (size_t k = 0; k < params.size() && k < uc.args.size(); ++k)
        sub[params[k]] = uc.args[k];
      return sub;
    };

    if (s.is_struct)
    {
      auto it = m_structs.find(nm);
      if (it == m_structs.end())
      {
        fail(ER::Code::TYPE_MISMATCH,
             s.anchor,
             "'%s' is not a struct type",
             nm.c_str());
        continue;
      }
      const StructFields &decl = it->second.fields;
      Subst               sub  = bind(it->second.params);

      std::vector<bool> seen(decl.size(), false);
      for (auto &fv : s.fields)
      {
        size_t idx = decl.size();
        for (size_t k = 0; k < decl.size(); ++k)
          if (decl[k].first == fv.first) idx = k;
        if (idx == decl.size())
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "struct '%s' has no field '%s'",
               nm.c_str(),
               fv.first.c_str());
          continue;
        }
        if (seen[idx])
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "field '%s' is set more than once in '%s'",
               fv.first.c_str(),
               nm.c_str());
          continue;
        }
        seen[idx] = true;
        unify(subst(decl[idx].second, sub), fv.second);
      }
      for (size_t k = 0; k < decl.size(); ++k)
        if (!seen[k])
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "struct '%s' is missing field '%s'",
               nm.c_str(),
               decl[k].first.c_str());

      if (s.exs) s.exs->type_name = intern(nm); // patch for the interpreter
      continue;
    }

    // Variant.
    auto it = m_unions.find(nm);
    if (it == m_unions.end())
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "'%s' is not a union type",
           nm.c_str());
      continue;
    }
    const StructFields *decl = nullptr;
    for (auto &v : it->second.variants)
      if (v.tag == s.vtag) decl = &v.fields;
    if (!decl)
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "union '%s' has no variant '%s'",
           nm.c_str(),
           s.vtag.c_str());
      continue;
    }
    Subst sub = bind(it->second.params);

    bool named = false;
    for (auto &fv : s.fields)
      if (!fv.first.empty()) named = true;

    if (named)
    {
      std::vector<bool> seen(decl->size(), false);
      for (auto &fv : s.fields)
      {
        size_t idx = decl->size();
        for (size_t k = 0; k < decl->size(); ++k)
          if ((*decl)[k].first == fv.first) idx = k;
        if (idx == decl->size())
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "variant '%s.%s' has no field '%s'",
               nm.c_str(),
               s.vtag.c_str(),
               fv.first.c_str());
          continue;
        }
        if (seen[idx])
        {
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "field '%s' is set more than once in '%s.%s'",
               fv.first.c_str(),
               nm.c_str(),
               s.vtag.c_str());
          continue;
        }
        seen[idx] = true;
        unify(subst((*decl)[idx].second, sub), fv.second);
      }
      for (size_t k = 0; k < decl->size(); ++k)
        if (!seen[k])
          fail(ER::Code::TYPE_MISMATCH,
               s.anchor,
               "variant '%s.%s' is missing field '%s'",
               nm.c_str(),
               s.vtag.c_str(),
               (*decl)[k].first.c_str());
    }
    else if (s.fields.size() != decl->size())
    {
      fail(ER::Code::TYPE_MISMATCH,
           s.anchor,
           "variant '%s.%s' takes %zu payload field(s), but %zu given",
           nm.c_str(),
           s.vtag.c_str(),
           decl->size(),
           s.fields.size());
    }
    else
    {
      for (size_t k = 0; k < decl->size(); ++k)
        unify(subst((*decl)[k].second, sub), s.fields[k].second);
    }

    if (s.exv)
    {
      s.exv->type_name = intern(nm); // patch for the interpreter
      // A named bare literal was left unnormalized by LL (it had no type then);
      // reorder its payload into declared field order, dropping the names, so
      // IT (which builds positionally) and a `case` agree.
      if (named && s.exv->fields.size() == decl->size())
      {
        UT::Vec<EX::FieldInit> out{ m_arena };
        bool                   ok = true;
        for (size_t k = 0; k < decl->size(); ++k)
        {
          EX::Expr *val = nullptr;
          for (size_t j = 0; j < s.exv->fields.size(); ++j)
            if (std::string(s.exv->fields[j].name) == (*decl)[k].first)
              val = s.exv->fields[j].val;
          if (!val)
          {
            ok = false;
            break;
          }
          out.push(EX::FieldInit{ UT::Vu{}, val });
        }
        if (ok) s.exv->fields = out;
      }
    }
  }
  m_lit_sites.erase(m_lit_sites.begin() + (long)from, m_lit_sites.end());
}

/*------------------------------------------------------------------------------
 *\DRIVER
 *-----------------------------------------------------------------------------*/

// Collect the distinct type-variable names free in `t`, in order of first
// appearance, appending to `out`. A nominal type's implicit parameters are
// exactly these over its whole declaration body.
static void
collect_tyvars(
  EX::Ty *t, std::vector<std::string> &out)
{
  switch (t->tag)
  {
  case EX::TyTag::Var:
  {
    std::string nm = std::string(std::get<EX::TyVar>(t->as).name);
    for (const std::string &s : out)
      if (s == nm) return;
    out.push_back(nm);
    break;
  }
  case EX::TyTag::Con:
    for (EX::Ty *a : std::get<EX::TyCon>(t->as).args) collect_tyvars(a, out);
    break;
  case EX::TyTag::Arrow:
  {
    auto &ar = std::get<EX::TyArrow>(t->as);
    collect_tyvars(ar.from, out);
    collect_tyvars(ar.to, out);
    break;
  }
  }
}

void
Checker::run(
  EX::Exprs &exprs)
{
  // The built-in base types are nullary type constructors, known up front. They
  // share the nominal-type arity table so a sized type used with arguments
  // (e.g. `Int32 Foo`) is still an arity error like any other nullary type.
  for (const char *t : OP::base_types) m_arity[t] = 0;
  m_arity[OP::TY_VEC] = 1;

  // Register transparent type aliases up front, so sig_to_type can resolve an
  // alias used by any (possibly earlier) declaration.
  for (EX::Expr &e : exprs)
    if (e.tag == EX::ExprTag::AliasDecl)
    {
      auto &ad                        = std::get<EX::ExAliasDecl>(e.as);
      m_aliases[std::string(ad.name)] = ad.target;
    }

  // Record every declared effect name up front, so an effect-row annotation
  // `<E>` in any (possibly earlier) signature can validate its labels.
  for (EX::Expr &e : exprs)
    if (e.tag == EX::ExprTag::EffectDecl)
      m_effects.insert(std::string(std::get<EX::ExEffectDecl>(e.as).name));

  // Phase A0: record every nominal type's arity (its distinct free type vars)
  // before any field type is built, so applied cons can be arity-checked even
  // when they reference a type declared later.
  for (EX::Expr &e : exprs)
  {
    std::vector<std::string> ps;
    if (e.tag == EX::ExprTag::StructDecl)
    {
      auto &sd = std::get<EX::ExStructDecl>(e.as);
      for (auto &f : sd.fields) collect_tyvars(f.ty, ps);
      m_arity[std::string(sd.name)] = ps.size();
    }
    else if (e.tag == EX::ExprTag::UnionDecl)
    {
      auto &ud = std::get<EX::ExUnionDecl>(e.as);
      for (auto &v : ud.variants)
        for (auto &f : v.fields) collect_tyvars(f.ty, ps);
      m_arity[std::string(ud.name)] = ps.size();
    }
  }

  // Phase A: register struct types and collect value globals. Structs are
  // registered up front so a global may reference a struct declared later. One
  // TyVarEnv per declaration ties every `T in the body to the same parameter;
  // `order` records the parameter ids in appearance order.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::StructDecl) continue;
    EX::ExStructDecl &sd = std::get<EX::ExStructDecl>(e->as);
    StructFields      flds;
    TyVarEnv          tv;
    VarIds            params;
    m_anchor = sd.origin.size() ? sd.origin : sd.name;
    for (size_t f = 0; f < sd.fields.size(); ++f)
    {
      Type *ft = sig_to_type(sd.fields[f].ty, tv, false, &params);
      flds.push_back({ std::string(sd.fields[f].name), ft });
    }
    m_structs[std::string(sd.name)]
      = StructDef{ std::move(params), std::move(flds) };
  }

  // ... and union types, likewise registered up front for forward references.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::UnionDecl) continue;
    EX::ExUnionDecl &ud = std::get<EX::ExUnionDecl>(e->as);
    Variants         variants;
    TyVarEnv         tv;
    VarIds           params;
    m_anchor = ud.origin.size() ? ud.origin : ud.name;
    for (size_t v = 0; v < ud.variants.size(); ++v)
    {
      StructFields flds;
      for (size_t f = 0; f < ud.variants[v].fields.size(); ++f)
      {
        Type *ft = sig_to_type(ud.variants[v].fields[f].ty, tv, false, &params);
        flds.push_back({ std::string(ud.variants[v].fields[f].name), ft });
      }
      variants.push_back({ std::string(ud.variants[v].tag), std::move(flds) });
    }
    m_unions[std::string(ud.name)]
      = UnionDef{ std::move(params), std::move(variants) };
  }

  // Effect declarations: register each operation as a typed name (a primitive
  // scheme) keyed by its canonical `Effect.op` identity (the same identity MR
  // rewrites uses and clause heads to), so a use `op a` type-checks against its
  // declared signature. Generalize over the signature's free type variables, so
  // a generic effect's operation is polymorphic per use. Same-named operations
  // from different effects coexist -- their identities differ.
  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::EffectDecl) continue;
    EX::ExEffectDecl &ed = std::get<EX::ExEffectDecl>(e->as);
    for (auto &op : ed.ops)
    {
      std::string key
        = std::string(ed.name) + "." + std::string(op.name); // Effect.op
      m_anchor         = op.name;
      m_op_effect[key] = std::string(ed.name);
      TyVarEnv tv;
      VarIds   params;
      Type    *t = sig_to_type(op.ty, tv, false, &params);

      // The performing arrow carries an open-tailed effect row `<Eff | mu>`
      // (mu fresh and quantified): performing the operation forces `Eff` into
      // the ambient and fits any ambient that already contains more effects.
      if (t->kind() == Kind::Arrow)
      {
        Type *mu = fresh();
        params.push_back(std::get<TVar>(mu->as).id);
        std::get<TArrow>(t->as).eff = row_extend(std::string(ed.name), mu);
      }
      m_prim[key] = Scheme{ std::move(params), t };
    }
  }

  for (size_t i = 0; i < exprs.size(); ++i)
  {
    EX::Expr *e = &exprs[i];
    if (e->tag != EX::ExprTag::Def) continue;
    EX::ExDef  &d = std::get<EX::ExDef>(e->as);
    GlobalEntry g;
    g.def  = &d;
    g.body = d.def;
    m_globals.add(std::move(g));
  }

  // Phase B: resolve every global's type (on-demand, DAG, cycle-checked).
  for (GlobalEntry &g : m_globals) resolve(std::string(g.def->name));

  // Phase C: check every body against its resolved/declared type, then resolve
  // the overloaded uses it contains (their operand types are now known). A
  // signatured global is only inferred here, so this is where its overloads are
  // resolved; a signature-less one was already resolved in Phase B, and is
  // re-resolved to the same impl here, which is idempotent.
  for (GlobalEntry &g : m_globals)
  {
    m_anchor = def_anchor(g.def);
    m_line   = line_of(m_anchor);

    Env    empty;
    size_t base  = m_sites.size();
    size_t ubase = m_usites.size();
    size_t fbase = m_field_sites.size();
    size_t lbase = m_lit_sites.size();

    if (g.def->sig)
    {
      // Check the body against its declared type so parameters carry their
      // signature types into the body (see infer_against).
      TyVarEnv tv;
      Type    *rigid = sig_to_type(g.def->sig, tv, true);
      infer_against(g.body, rigid, empty, row_empty()); // top level is pure
    }
    else
    {
      Type *bt = infer(g.body, empty, row_empty()); // top level is pure
      unify(bt, g.scheme.type);
    }

    resolve_sites(base, fbase, ubase);
    resolve_user_sites(ubase);
    resolve_field_sites(fbase);
    // Bare literals last: their type comes from the unification just done.
    resolve_lit_sites(lbase);
  }

  if (!m_diags.empty()) return;
  Diagnostics pd
    = PatLower{ m_arena, m_structs, m_unions, m_tg.int_ty() }.run(exprs);
  for (ER::Diagnostic &d : pd) m_diags.push_back(d);
}

/*------------------------------------------------------------------------------
 *\DIAGNOSTICS
 *-----------------------------------------------------------------------------*/

size_t
Checker::line_of(
  UT::Vu anchor)
{
  if (!anchor.data() || !m_src.data()) return 0;
  if (anchor.data() < m_src.data()) return 0;
  size_t off  = (size_t)(anchor.data() - m_src.data());
  size_t line = 1;
  for (size_t i = 0; i < off && i < m_src.size(); ++i)
    if (m_src.data()[i] == '\n') line++;
  return line;
}

std::string
Checker::show(
  Type *t)
{
  t = prune(t);
  switch (t->kind())
  {
  case Kind::Con:
  {
    TCon &c = std::get<TCon>(t->as);
    if (OP::is_tuple_name(c.name)) // display as `{A, B}`, never `%tupleN`
    {
      std::string out = "{";
      for (size_t i = 0; i < c.args.size(); ++i)
        out += (i ? ", " : "") + show(c.args[i]);
      return out + "}";
    }
    std::string out = c.name;
    for (Type *arg : c.args)
    {
      Type *pa = prune(arg);
      bool  nest
        = pa->kind() == Kind::Arrow
          || (pa->kind() == Kind::Con && !std::get<TCon>(pa->as).args.empty());
      std::string a = show(arg);
      out += " " + (nest ? "(" + a + ")" : a);
    }
    return out;
  }
  case Kind::Var:
  {
    TVar &v = std::get<TVar>(t->as);
    if (v.rigid) return "`" + v.name;
    return "?" + std::to_string(v.id);
  }
  case Kind::Arrow:
  {
    TArrow     &a = std::get<TArrow>(t->as);
    std::string l = show(a.from);
    if (prune(a.from)->kind() == Kind::Arrow) l = "(" + l + ")";
    Type       *eff = prune(a.eff);
    std::string e   = eff->kind() == Kind::RowEmpty ? "" : show(eff) + " ";
    return l + " -> " + e + show(a.to);
  }
  case Kind::RowEmpty: return "<>";
  case Kind::RowExtend:
  {
    std::string out   = "<";
    Type       *cur   = t;
    bool        first = true;
    for (; prune(cur)->kind() == Kind::RowExtend;)
    {
      TRowExtend &r = std::get<TRowExtend>(prune(cur)->as);
      if (!first) out += ", ";
      out += r.label;
      first = false;
      cur   = r.rest;
    }
    Type *tail = prune(cur);
    if (tail->kind() != Kind::RowEmpty) out += " | " + show(tail);
    return out + ">";
  }
  }
  return "?";
}

void
Checker::fail(
  ER::Code code, UT::Vu anchor, const char *fmt, ...)
{
  va_list args;
  va_start(args, fmt);
  va_list copy;
  va_copy(copy, args);
  int len = std::vsnprintf(nullptr, 0, fmt, args);
  va_end(args);

  UT::Vu msg{ "<message formatting failed>" };
  if (len >= 0)
  {
    char *mem = (char *)m_arena.alloc((size_t)len + 1);
    std::vsnprintf(mem, (size_t)len + 1, fmt, copy);
    msg = UT::Vu{ mem, (size_t)len };
  }
  va_end(copy);

  m_diags.push_back(ER::mk_root(m_arena, code, anchor, line_of(anchor), msg));
}

void
Checker::seed_primitives()
{
  Type *tv       = fresh();
  Type *ift      = arrow(con(OP::TY_BOOL), arrow(tv, arrow(tv, tv)));
  m_prim[OP::IF] = generalize(Env{}, ift);

  Type *arrt            = arrow(con(m_tg.int_ty()), con(OP::TY_ARRAY));
  m_prim[OP::ARR_ALLOC] = generalize(Env{}, arrt);

  // The generic-vector primitives. `Vec` itself is opaque
  {
    Type *t = fresh();
    m_prim[OP::VEC_NEW]
      = generalize(Env{}, arrow(con(OP::TY_UNIT), con(OP::TY_VEC, { t })));
  }
  {
    Type *t              = fresh();
    m_prim[OP::VEC_FILL] = generalize(
      Env{}, arrow(con(m_tg.int_ty()), arrow(t, con(OP::TY_VEC, { t }))));
  }
  {
    Type *t = fresh();
    m_prim[OP::VEC_LEN]
      = generalize(Env{}, arrow(con(OP::TY_VEC, { t }), con(m_tg.int_ty())));
  }
  {
    Type *t             = fresh();
    m_prim[OP::VEC_GET] = generalize(
      Env{}, arrow(con(OP::TY_VEC, { t }), arrow(con(m_tg.int_ty()), t)));
  }
  {
    Type *t             = fresh();
    m_prim[OP::VEC_SET] = generalize(
      Env{},
      arrow(con(OP::TY_VEC, { t }),
            arrow(con(m_tg.int_ty()), arrow(t, con(OP::TY_VEC, { t })))));
  }
  {
    Type *t              = fresh();
    m_prim[OP::VEC_PUSH] = generalize(
      Env{}, arrow(con(OP::TY_VEC, { t }), arrow(t, con(OP::TY_VEC, { t }))));
  }

  Type *e           = fresh(); // the shared effect row
  Type *a           = fresh();
  Type *unit        = con(OP::TY_UNIT);
  Type *act         = arrow(unit, a, e);    // {} ->e a
  Type *cln         = arrow(unit, unit, e); // {} ->e {}
  Type *fin         = arrow(act, arrow(cln, a, e), e);
  m_prim[OP::DEFER] = generalize(Env{}, fin);
}

} // namespace

std::vector<ER::Diagnostic>
check(
  EX::Exprs &exprs, AR::Arena &arena, UT::Vu src, const TG::Target &tg)
{
  Checker c{ arena, src, tg };
  c.run(exprs);
  return c.m_diags;
}

} // namespace TC
