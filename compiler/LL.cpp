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
  // One variant of a union: its tag and its payload field names (a name is
  // empty for an anonymous positional field). Its index in the union's vector
  // is its constructor tag.
  struct UVariant
  {
    std::string              tag;
    std::vector<std::string> fields;
  };
  // union type name -> its variants, in declaration order
  std::unordered_map<std::string, std::vector<UVariant>> unions;
  size_t                                                 fresh_n = 0;

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
  mk_unit()
  {
    Expr e{ ExprTag::Unit };
    e.as = EX::ExUnit{};
    return alloc(e);
  }

  // A thunk `\_ = body`: an ExFnDef ignoring its argument. Used to suspend a
  // match's fallthrough continuation so it runs only when an arm actually falls
  // through (and is shared, not duplicated, across guarded arms).
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
            "'let' binding; use 'when' to match it");
      }
      return body;
    case PatTag::Variant:
      // Refutable (it tests the constructor tag); only `if .. is` may use it,
      // where lower_match_variant handles it before bind_pattern is reached.
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
    case PatTag::Variant:
      // Variant matches lower to a `case` over the constructor tag, not to a
      // boolean test, so lower_match routes them to lower_match_variant before
      // test_of is ever called. Defensive only.
      return nullptr;
    }
    return nullptr;
  }

  // `if scrut is p1 then e1 .. is pn then en else d` lowers to a binding of the
  // scrutinee and a `case` over it. A struct-free match (literal guards, plus
  // an optional irrefutable variable arm) becomes one multi-way `case`; a match
  // with struct patterns keeps the chain-of-`if`s form, each guard testing a
  // field, since structs dispatch on field values rather than a constructor.
  Expr *
  lower_match(
    Expr *e)
  {
    auto &m     = std::get<EX::ExMatch>(e->as);
    Expr *scrut = lower(m.scrut);
    Expr *alt   = lower(m.alt);

    bool has_struct = false, has_variant = false, has_guard = false;
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      if (m.arms[i].pat->tag == PatTag::Struct) has_struct = true;
      if (m.arms[i].pat->tag == PatTag::Variant) has_variant = true;
      if (m.arms[i].guard) has_guard = true;
    }

    // Guards make an arm refutable even when its pattern is not, and let two
    // arms share one constructor -- neither fits the single-dispatch `case`
    // forms below. Route the whole match through the backtracking lowering.
    if (has_guard) return lower_match_guarded(m, scrut, alt);

    // A union match dispatches on the constructor tag (a `case` of Con alts);
    // it cannot share the struct if-chain or literal flat forms.
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
  // `else`. The fallthrough is threaded as a chain of thunks so it is shared,
  // not duplicated: for arm i we emit `let $f = \_ = <rest> in <arm i, falling
  // to $f {}>`, built from the last arm inward. A guard is placed inside the
  // pattern's bindings so it can read them; on failure it invokes the same
  // thunk. Works uniformly for literal, struct and variant patterns.
  Expr *
  lower_match_guarded(
    EX::ExMatch &m, Expr *scrut, Expr *alt)
  {
    UT::Vu      mv     = fresh("m");
    Expr       *mvar   = mk_var(mv);
    std::string munion = match_union(m); // "" unless there are variant arms
    EX::Ty     *sig    = nullptr;        // scrutinee type pin (struct / union)

    Expr *acc = alt; // fallthrough base: the `else`
    for (size_t i = m.arms.size(); i-- > 0;)
    {
      UT::Vu fv    = fresh("f");
      Expr  *thunk = mk_thunk(acc);
      Expr  *arm   = build_guarded_arm(m.arms[i], mvar, fv, munion, &sig);
      acc          = mk_let(fv, thunk, arm, nullptr);
    }
    return mk_let(mv, scrut, acc, sig);
  }

  // One arm of a guarded match. `fv` names the fallthrough thunk to invoke (as
  // `$fv {}`) when this arm's pattern or guard does not match. Wraps the arm
  // body with the guard *inside* the pattern's bindings, so the guard sees
  // them.
  Expr *
  build_guarded_arm(
    EX::MatchArm      &arm,
    Expr              *mvar,
    UT::Vu             fv,
    const std::string &munion,
    EX::Ty           **sig)
  {
    Pattern *pat  = arm.pat;
    Expr    *body = lower(arm.body);
    auto     fall = [&]() { return mk_app(mk_var(fv), mk_unit()); };
    // Guard-wrap: run `body` only if the guard holds, else fall through.
    auto guarded = [&](Expr *inner) {
      return arm.guard ? mk_if(lower(arm.guard), inner, fall()) : inner;
    };

    UT::Vu anc;
    size_t line = 0;
    pat_anchor(pat, anc, line);

    switch (pat->tag)
    {
    case PatTag::Wild: return guarded(body); // always matches, binds nothing
    case PatTag::Var:
      return mk_let(
        std::get<EX::PatVar>(pat->as).name, mvar, guarded(body), nullptr);
    case PatTag::Int:
      return mk_if(
        mk_binop(OP::ISEQ, mvar, mk_int(std::get<EX::PatInt>(pat->as).value)),
        guarded(body),
        fall());
    case PatTag::Real:
      return mk_if(
        mk_binop(OP::ISEQ, mvar, mk_real(std::get<EX::PatReal>(pat->as).value)),
        guarded(body),
        fall());
    case PatTag::Str:
    {
      auto &ps = std::get<EX::PatStr>(pat->as);
      err(ER::Code::UNSUPPORTED,
          ps.anchor,
          ps.line,
          "matching a string literal is not supported yet (no string "
          "equality)");
      return fall();
    }
    case PatTag::Struct:
    {
      // Test the literal fields, bind the variable fields, then the guard runs
      // in their scope; any failure invokes the fallthrough thunk.
      Expr *test  = test_of(pat, mvar);
      Expr *bound = bind_pattern(
        pat, mvar, guarded(body), anc, line, /*refutable_ok=*/true);
      if (!*sig) *sig = mk_con(std::get<EX::PatStruct>(pat->as).type_name);
      return test ? mk_if(test, bound, fall()) : bound;
    }
    case PatTag::Variant:
    {
      auto       &pv = std::get<EX::PatVariant>(pat->as);
      EX::CaseAlt a;
      if (!make_variant_alt(pv, munion, guarded(body), sig, a)) return fall();
      EX::ExCase c;
      c.scrut = mvar;
      c.alts  = UT::Vec<EX::CaseAlt>{ arena };
      c.alts.push(a);
      c.deflt = fall();
      Expr ce{ ExprTag::Case };
      ce.as = c;
      return alloc(ce);
    }
    }
    return fall();
  }

  // Lower a struct-free match into `let $m = scrut in case $m of lit -> e ..
  // else d`. Arms are scanned in order; literal arms become alternatives, and
  // the first irrefutable (variable / wildcard) arm becomes the default,
  // binding the scrutinee and ending the scan (any later arm is dead).
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
      else if (pat->tag == PatTag::Real)
      {
        EX::CaseAlt a;
        a.kind = EX::AltKind::Real;
        a.rval = std::get<EX::PatReal>(pat->as).value;
        a.body = body;
        alts.push(a);
      }
      else if (pat->tag == PatTag::Str)
      {
        auto &ps = std::get<EX::PatStr>(pat->as);
        err(ER::Code::UNSUPPORTED,
            ps.anchor,
            ps.line,
            "matching a string literal is not supported yet (no string "
            "equality)");
      }
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
  // resolve_fields. Returns false on a fatal error (unknown field / arity).
  bool
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

    std::string vname = std::string(pv.type_name) + "." + std::string(pv.tag);
    if (named)
    {
      for (size_t i = 0; i < pv.fields.size(); ++i)
      {
        std::string fn  = std::string(pv.fields[i].name);
        size_t      idx = decl.size();
        for (size_t k = 0; k < decl.size(); ++k)
          if (decl[k] == fn) idx = k;
        if (idx == decl.size())
        {
          err(ER::Code::TYPE_UNBOUND,
              pv.anchor,
              pv.line,
              "variant '" + vname + "' has no field '" + fn + "'");
          continue;
        }
        if (subs[idx])
        {
          err(ER::Code::TYPE_MISMATCH,
              pv.anchor,
              pv.line,
              "field '" + fn + "' is matched more than once");
          continue;
        }
        subs[idx] = pv.fields[i].pat;
      }
      return true;
    }

    if (pv.fields.size() != decl.size())
    {
      err(ER::Code::TYPE_MISMATCH,
          pv.anchor,
          pv.line,
          "variant '" + vname + "' has " + std::to_string(decl.size())
            + " payload field(s) but the pattern lists "
            + std::to_string(pv.fields.size())
            + "; list every field positionally (use '_' to ignore one) or "
              "match by name");
      return false;
    }
    for (size_t i = 0; i < decl.size(); ++i) subs[i] = pv.fields[i].pat;
    return true;
  }

  // Determine which union a match's variant arms belong to. A qualified arm
  // (`Type.Tag`) names it; if every arm is bare (`.Tag`), find the unique
  // declared union whose variants include all the arms' tags. "" if undecided.
  std::string
  match_union(
    EX::ExMatch &m)
  {
    std::vector<std::string> tags;
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      if (m.arms[i].pat->tag != PatTag::Variant) continue;
      auto &pv = std::get<EX::PatVariant>(m.arms[i].pat->as);
      if (pv.type_name.size()) return std::string(pv.type_name);
      tags.push_back(std::string(pv.tag));
    }
    std::string found;
    for (auto &kv : unions)
    {
      bool all = true;
      for (const std::string &t : tags)
      {
        bool has = false;
        for (const UVariant &v : kv.second)
          if (v.tag == t) has = true;
        if (!has)
        {
          all = false;
          break;
        }
      }
      if (all)
      {
        if (!found.empty()) return ""; // ambiguous: more than one union fits
        found = kv.first;
      }
    }
    return found;
  }

  // Build the Con `CaseAlt` for one variant pattern arm. `munion` is the union
  // resolved for the whole match (used for a bare `.Tag` arm); `body` is the
  // alt body. Pins `*sig` to the union's type constructor on first success.
  // Returns false (a diagnostic recorded, or the arm skipped) if `pv` is not a
  // usable variant arm. Shared by the plain-variant and guarded lowerings.
  bool
  make_variant_alt(
    EX::PatVariant    &pv,
    const std::string &munion,
    Expr              *body,
    EX::Ty           **sig,
    EX::CaseAlt       &out)
  {
    bool        bare  = pv.type_name.size() == 0;
    std::string uname = bare ? munion : std::string(pv.type_name);
    if (bare && uname.empty())
    {
      err(ER::Code::TYPE_UNBOUND,
          pv.anchor,
          pv.line,
          "cannot infer the union for bare variant '." + std::string(pv.tag)
            + "'; qualify it (Type." + std::string(pv.tag)
            + ") or match a constructor that names the type");
      return false;
    }
    auto uit = unions.find(uname);
    if (uit == unions.end())
    {
      err(ER::Code::TYPE_UNBOUND,
          pv.anchor,
          pv.line,
          "unknown union type '" + uname + "' in pattern");
      return false;
    }
    UT::Vu uname_vu = bare ? ustr(uname) : pv.type_name;
    size_t tagidx   = uit->second.size();
    for (size_t k = 0; k < uit->second.size(); ++k)
      if (uit->second[k].tag == std::string(pv.tag)) tagidx = k;
    if (tagidx == uit->second.size())
    {
      err(ER::Code::TYPE_MISMATCH,
          pv.anchor,
          pv.line,
          "union '" + uname + "' has no variant '" + std::string(pv.tag) + "'");
      return false;
    }

    const std::vector<std::string> &decl = uit->second[tagidx].fields;
    if (!*sig) *sig = mk_con(uname_vu);

    std::vector<Pattern *> subs;
    if (!resolve_payload(pv, decl, subs)) return false;

    // One binder per payload slot; empty ignores it. Nested refutable
    // sub-patterns are not expressible in a flat Con alt yet.
    UT::Vec<UT::Vu> binders{ arena };
    for (size_t k = 0; k < decl.size(); ++k)
    {
      Pattern *sp = subs[k];
      if (!sp || sp->tag == PatTag::Wild)
        binders.push(UT::Vu{});
      else if (sp->tag == PatTag::Var)
        binders.push(std::get<EX::PatVar>(sp->as).name);
      else
      {
        err(ER::Code::UNSUPPORTED,
            pv.anchor,
            pv.line,
            "nested patterns in a variant payload are not supported yet; use "
            "a variable or '_'");
        return false;
      }
    }

    out.kind      = EX::AltKind::Con;
    out.type_name = uname_vu;
    out.ctor      = pv.tag;
    out.tag       = tagidx;
    out.binders   = binders;
    out.body      = body;
    return true;
  }

  // `if scrut is Type.Tag.{..} then e .. else d` lowers to `let $m = scrut in
  // case $m of <Con alt>.. else d`. Each variant arm becomes a Con alternative
  // carrying the constructor's tag index and a positional binder per payload
  // field (empty for an ignored slot); the first irrefutable var/wildcard arm
  // becomes the default. Payload sub-patterns must be variables or `_` for now.
  // A bare arm (`.Tag`, no type name) takes the union resolved for the match.
  Expr *
  lower_match_variant(
    EX::ExMatch &m, Expr *scrut, Expr *deflt)
  {
    UT::Vu mv   = fresh("m");
    Expr  *mvar = mk_var(mv);

    UT::Vec<EX::CaseAlt> alts{ arena };
    Expr                *dflt = deflt;
    EX::Ty              *sig  = nullptr; // pin the scrutinee to the union type
    std::string          munion = match_union(m);

    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      Pattern *pat  = m.arms[i].pat;
      Expr    *body = lower(m.arms[i].body);

      if (pat->tag != PatTag::Variant)
      {
        if (pat->tag == PatTag::Var || pat->tag == PatTag::Wild)
        {
          UT::Vu anc;
          size_t line = 0;
          pat_anchor(pat, anc, line);
          dflt
            = bind_pattern(pat, mvar, body, anc, line, /*refutable_ok=*/true);
        }
        else
        {
          UT::Vu anc;
          size_t line = 0;
          pat_anchor(pat, anc, line);
          err(
            ER::Code::UNSUPPORTED,
            anc,
            line,
            "a union match takes variant arms (and an optional variable arm), "
            "not literal or struct patterns");
        }
        break; // an irrefutable / invalid arm ends the scan
      }

      auto       &pv = std::get<EX::PatVariant>(pat->as);
      EX::CaseAlt a;
      if (make_variant_alt(pv, munion, body, &sig, a)) alts.push(a);
    }

    EX::ExCase c;
    c.scrut = mvar;
    c.alts  = alts;
    c.deflt = dflt;
    Expr ce{ ExprTag::Case };
    ce.as = c;
    return mk_let(mv, scrut, alloc(ce), sig);
  }

  // Reorder a named variant literal's payload into the variant's declared field
  // order and drop the names, so IT (which builds payloads positionally) and a
  // Con `case` see fields in the same order. A positional literal is already in
  // order. Anything malformed (unknown union/variant, wrong arity, an unknown
  // or missing field name) is left untouched for TC to diagnose.
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
    case ExprTag::Unit:
    case ExprTag::Var:
    case ExprTag::Extern:
    case ExprTag::StructDecl:
    case ExprTag::UnionDecl:
    case ExprTag::AliasDecl:
    case ExprTag::EffectDecl:
    case ExprTag::Overload: // leaf produced by MR; TC resolves it
    case ExprTag::ModDecl:  // module directives are stripped by MR before LL,
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
    case PatTag::Variant:
    {
      auto &pv = std::get<EX::PatVariant>(pat->as);
      anchor   = pv.anchor;
      line     = pv.line;
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

    // ... and union declarations, so variant patterns may reference any union
    // regardless of source order.
    for (size_t i = 0; i < exprs.size(); ++i)
    {
      if (exprs[i].tag != ExprTag::UnionDecl) continue;
      auto                 &ud = std::get<EX::ExUnionDecl>(exprs[i].as);
      std::vector<UVariant> vs;
      for (size_t v = 0; v < ud.variants.size(); ++v)
      {
        std::vector<std::string> fns;
        for (size_t f = 0; f < ud.variants[v].fields.size(); ++f)
          fns.push_back(std::string(ud.variants[v].fields[f].name));
        vs.push_back({ std::string(ud.variants[v].tag), std::move(fns) });
      }
      unions[std::string(ud.name)] = std::move(vs);
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
