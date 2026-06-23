#include "IT.hpp"
#include "ITxANF.hpp"

namespace IT
{

namespace
{

// Force a value to weak head normal form. Thrax is call-by-value, so most
// values arrive already forced; the exception is a constructor field, which is
// suspended as a Thunk (see Variant). Every strict eliminator -- a `case`
// scrutinee, a field access, a builtin / extern operand, the callee of an
// application -- forces its input through here, evaluating the thunk once,
// memoizing the result, and collapsing any chain of thunks. Constructor fields
// that are merely passed along stay suspended, so recursion stays lazy.
pLm
force(
  pLm v, StatEnv &senv)
{
  if (!v || v->tag != LTag::THUNK) return v;
  auto &t = std::get<Thunk>(v->as);
  if (!t.memo) t.memo = eval(t.expr, t.env, senv);
  t.memo = force(t.memo, senv);
  return t.memo;
}

// Normalize to ANF, then assign De Bruijn indices -- the finishing steps shared
// by every definition before it is stored or evaluated.
pLm
finalize(
  pLm node)
{
  Anf anf;
  node = anf.term(node);
  NameStack ns;
  assign_id(node, ns);
  return node;
}

pLm
exprs2pLm_helper(
  EX::Expr *expr, StatEnv &env)
{
  Lm lm;
  switch (expr->tag)
  {
  case EX::ExprTag::If:
  {
    EX::ExIf &ifexpr = std::get<EX::ExIf>(expr->as);
    pLm       cond   = exprs2pLm_helper(ifexpr.cond, env);
    pLm       then   = exprs2pLm_helper(ifexpr.then, env);
    pLm       othw   = exprs2pLm_helper(ifexpr.alt, env);
    // `if c then t else e`  ==>  case c of { 0 -> e } else t
    CaseAlt zero;
    zero.kind = AltKind::Int;
    zero.ival = 0;
    zero.body = othw;
    Case cs;
    cs.scrut = cond;
    cs.alts  = { zero };
    cs.deflt = then;
    lm       = { .tag = LTag::CASE, .as = cs };
  }
  break;

  case EX::ExprTag::Case:
  {
    auto &ec = std::get<EX::ExCase>(expr->as);
    Case  cs;
    cs.scrut = exprs2pLm_helper(ec.scrut, env);
    cs.deflt = exprs2pLm_helper(ec.deflt, env);
    for (size_t i = 0; i < ec.alts.size(); ++i)
    {
      EX::CaseAlt &a = ec.alts[i];
      CaseAlt      b;
      b.kind = a.kind;
      b.tag  = a.tag;
      b.ival = a.ival;
      b.rval = a.rval;
      if (a.kind == EX::AltKind::Con) // ctor / binders are Con-only
      {
        b.ctor = std::string(a.ctor);
        for (size_t j = 0; j < a.binders.size(); ++j)
          b.binders.push_back(std::string(a.binders[j]));
      }
      b.body = exprs2pLm_helper(a.body, env);
      cs.alts.push_back(b);
    }
    lm = { .tag = LTag::CASE, .as = cs };
  }
  break;

  case EX::ExprTag::Let:
  {
    auto &lt   = std::get<EX::ExLet>(expr->as);
    pLm   val  = exprs2pLm_helper(lt.val, env);
    pLm   body = exprs2pLm_helper(lt.body, env);
    lm         = { .tag = LTag::LET, .as = Let{ mkVar(lt.var), val, body } };
  }
  break;

  case EX::ExprTag::App:
  {
    auto &app = std::get<EX::ExApp>(expr->as);
    pLm   fn  = exprs2pLm_helper(app.fn, env);
    pLm   arg = exprs2pLm_helper(app.arg, env);
    lm        = { .tag = LTag::APP, .as = App{ fn, arg } };
  }
  break;

  case EX::ExprTag::FnDef:
  {
    auto &fn = std::get<EX::ExFnDef>(expr->as);
    Fun   f  = mkFun(fn.param, exprs2pLm_helper(fn.body, env));
    lm       = { .tag = LTag::FUN, .as = f };
  }
  break;

  case EX::ExprTag::Str:
  {
    lm = { .tag = LTag::STR, .as = mkStr(std::get<EX::ExStr>(expr->as).value) };
  }
  break;

  case EX::ExprTag::Var:
  {
    lm = { .tag = LTag::VAR, .as = mkVar(std::get<EX::ExVar>(expr->as).name) };
  }
  break;

  case EX::ExprTag::Int:
  {
    lm = { .tag = LTag::INT, .as = Int{ std::get<EX::ExInt>(expr->as).value } };
  }
  break;

  case EX::ExprTag::Real:
  {
    lm = { .tag = LTag::REAL,
           .as  = Real{ std::get<EX::ExReal>(expr->as).value } };
  }
  break;

  case EX::ExprTag::Def:
  {
    auto       &gd   = std::get<EX::ExDef>(expr->as);
    std::string name = std::string{ gd.name };

    pLm def;
    if (EX::ExprTag::Extern == gd.def->tag)
    {
      // Foreign binding: the call types come from the signature, not the body.
      auto  &ex = std::get<EX::ExExtern>(gd.def->as);
      Extern e;
      e.symbol = std::string{ ex.symbol };
      e.lib    = std::string{ ex.lib };
      if (gd.sig)
        flatten_sig(gd.sig, e.arg_types, e.ret_type);
      else
        e.ret_type = "Int";
      def = std::make_shared<Lm>(Lm{ .tag = LTag::EXTERN, .as = e });
    }
    else
    {
      def = exprs2pLm_helper(gd.def, env);
    }

    env[name] = finalize(def);
    lm        = { .tag = LTag::VAR, .as = Var{ name, (size_t)-1, {} } };
  }
  break;

  case EX::ExprTag::StructLit:
  {
    auto  &sl = std::get<EX::ExStructLit>(expr->as);
    Struct s;
    s.name = std::string{ sl.type_name };
    for (size_t i = 0; i < sl.fields.size(); ++i)
      s.fields.push_back({ std::string{ sl.fields[i].name },
                           exprs2pLm_helper(sl.fields[i].val, env) });
    lm = { .tag = LTag::STRUCT, .as = s };
  }
  break;

  case EX::ExprTag::Field:
  {
    auto &fa = std::get<EX::ExField>(expr->as);
    Field f;
    f.record = exprs2pLm_helper(fa.record, env);
    f.name   = std::string{ fa.field };
    lm       = { .tag = LTag::FIELD, .as = f };
  }
  break;

  case EX::ExprTag::VariantLit:
  {
    auto   &vl = std::get<EX::ExVariantLit>(expr->as);
    Variant v;
    v.type_name = std::string{ vl.type_name };
    v.tag       = std::string{ vl.tag };
    // Fields are positional in declared order (LL normalized any named form).
    for (size_t i = 0; i < vl.fields.size(); ++i)
      v.fields.push_back(exprs2pLm_helper(vl.fields[i].val, env));
    lm = { .tag = LTag::VARIANT, .as = v };
  }
  break;

  // A struct / union *declaration* is a type-level form with no runtime value.
  case EX::ExprTag::StructDecl:
  case EX::ExprTag::UnionDecl:
  {
    lm = { .tag = LTag::UNK, .as = std::monostate{} };
  }
  break;

  case EX::ExprTag::Extern:
    UT_FAIL_MSG("%s", "@extern is only valid as a global definition");
    break;

  case EX::ExprTag::Match:
    UT_FAIL_MSG("%s", "match should have been lowered by LL");
    break;

  case EX::ExprTag::Unknown:
  {
    lm = { .tag = LTag::UNK, .as = std::monostate{} };
  }
  break;
  }

  return std::make_shared<Lm>(lm);
};

} // namespace

pLm
exprs2pLm(
  EX::Expr *expr, StatEnv &env)
{
  return finalize(exprs2pLm_helper(expr, env));
}

pLm
eval(
  pLm node, DynEnv denv, StatEnv &senv)
{
  // Trampoline: a call in tail position rebinds `node`/`denv` and loops instead
  // of recursing, so tail recursion runs in constant C++ stack. Non-tail
  // sub-evaluations (an `if` condition, a `let` value, an application's atoms)
  // still recurse -- their depth is the program's, not the call chain's.
  //
  // `live` keeps recursive `let` closures alive across the loop: such a closure
  // is held only weakly by its own captured environment (the Rec cell, see
  // LET), and the C++ stack used to keep it live across a call. Globals (empty
  // env) and ordinary closures hold no Rec, so they are never added and plain
  // tail recursion stays in constant space.
  std::vector<pLm> live;

  for (;;)
  {
    UT_FAIL_IF(!node);

    switch (node->tag)
    {
    case LTag::INT:
    case LTag::REAL:
    case LTag::STR : return node;

    case LTag::VAR:
    {
      auto &v = std::get<Var>(node->as);

      // A resolved built-in (e.g. "+@Int") is a first-class, curried value.
      auto bit = impls.find(v.unwrap);
      if (bit != impls.end())
        return std::make_shared<Lm>(
          Lm{ .tag = LTag::BUILTIN,
              .as  = Builtin{ v.unwrap, bit->second.arity, {} } });

      // Global definition placeholder
      if (v.idx == (size_t)-1) return node;

      // Local variable (De Bruijn indexed)
      if (v.idx > 0)
      {
        UT_FAIL_IF(v.idx > denv.size());
        pLm entry = denv[denv.size() - v.idx];
        // A `let` binding is held weakly while its value is built (see LET), to
        // keep the closure graph acyclic; lock it to recover the value.
        if (entry && LTag::REC == entry->tag)
        {
          pLm target = std::get<Rec>(entry->as).target.lock();
          UT_FAIL_IF(!target);
          return target;
        }
        return entry;
      }

      // Global variable (idx == 0): jump to its definition in tail position.
      auto it = senv.find(v.unwrap);
      UT_FAIL_IF(it == senv.end());
      node = it->second;
      denv.clear();
      continue;
    }

    case LTag::FUN:
    {
      auto &f = std::get<Fun>(node->as);
      Fun   closure;
      closure.var  = Var{ f.var.unwrap, f.var.idx, denv };
      closure.body = f.body;
      return std::make_shared<Lm>(Lm{ .tag = LTag::FUN, .as = closure });
    }

    case LTag::LET:
    {
      auto &l = std::get<Let>(node->as);
      // Bind the name to a placeholder, then evaluate the value with that
      // binding *weakly* in scope and back-patch the slot, so recursive
      // references resolve without the value's captured environment forming a
      // shared_ptr cycle with it. (For a non-recursive let the placeholder is
      // simply never read before patching.) The body then evaluates in tail
      // position with the binding held strongly.
      pLm value
        = std::make_shared<Lm>(Lm{ .tag = LTag::UNK, .as = std::monostate{} });
      pLm self = std::make_shared<Lm>(
        Lm{ .tag = LTag::REC, .as = Rec{ std::weak_ptr<Lm>(value) } });

      DynEnv val_env = denv;
      val_env.push_back(self);
      *value = *eval(l.val, val_env, senv);

      denv.push_back(value);
      node = l.body;
      continue;
    }

    case LTag::APP:
    {
      auto &a = std::get<App>(node->as);

      pLm closure = force(eval(a.fn, denv, senv), senv);
      pLm val     = eval(a.arg, denv, senv);

      // Foreign function: accumulate the (forced) argument, call once
      // saturated.
      if (LTag::EXTERN == closure->tag)
      {
        Extern e = std::get<Extern>(closure->as); // copy and extend
        e.args.push_back(force(val, senv));
        if (e.args.size() >= e.arg_types.size()) return call_extern(e);
        return std::make_shared<Lm>(Lm{ .tag = LTag::EXTERN, .as = e });
      }

      // Built-in: accumulate the (forced) argument, run the impl once
      // saturated.
      if (LTag::BUILTIN == closure->tag)
      {
        Builtin b = std::get<Builtin>(closure->as); // copy and extend
        b.args.push_back(force(val, senv));
        if (b.args.size() >= b.arity) return impls.at(b.impl).fn(b.args);
        return std::make_shared<Lm>(Lm{ .tag = LTag::BUILTIN, .as = b });
      }

      UT_FAIL_IF(closure->tag != LTag::FUN);
      auto  &fun     = std::get<Fun>(closure->as);
      DynEnv new_env = fun.var.env;
      new_env.push_back(val);
      // A recursive `let` value is reachable from a closure's captured
      // environment only weakly, through a Rec cell (see LET). The C++ stack
      // used to keep it alive across a call; with the call trampolined, pin
      // each such value for the loop's lifetime. Globals (empty env) and
      // ordinary closures carry no Rec, so nothing is pinned and plain tail
      // recursion stays in constant space; dedup keeps self-recursion at O(1).
      for (const pLm &slot : fun.var.env)
        if (slot && LTag::REC == slot->tag)
        {
          pLm tgt = std::get<Rec>(slot->as).target.lock();
          if (tgt && (live.empty() || live.back() != tgt)) live.push_back(tgt);
        }
      node = fun.body;
      denv = std::move(new_env);
      continue;
    }

    case LTag::EXTERN:
    {
      auto &e = std::get<Extern>(node->as);
      // A nullary extern is already saturated; otherwise it is a function
      // value.
      if (e.args.size() >= e.arg_types.size()) return call_extern(e);
      return node;
    }

    case LTag::CASE:
    {
      auto          &c   = std::get<Case>(node->as);
      pLm            sv  = force(eval(c.scrut, denv, senv), senv); // to WHNF
      const CaseAlt *hit = nullptr;
      for (auto &alt : c.alts)
      {
        bool m = false;
        switch (alt.kind)
        {
        case AltKind::Int:
          UT_FAIL_IF(sv->tag != LTag::INT);
          m = std::get<Int>(sv->as).unwrap == alt.ival;
          break;
        case AltKind::Real:
          UT_FAIL_IF(sv->tag != LTag::REAL);
          m = std::get<Real>(sv->as).unwrap == alt.rval;
          break;
        case AltKind::Con:
          UT_FAIL_IF(sv->tag != LTag::VARIANT);
          m = std::get<Variant>(sv->as).tag == alt.ctor;
          break;
        }
        if (m)
        {
          hit = &alt;
          break;
        }
      }
      // A Con alt binds its payload positionally onto `denv`, in the same order
      // assign_id pushed the binders, so De Bruijn indices line up. The fields
      // are the constructor's thunks -- binders stay lazy until forced.
      if (hit && hit->kind == AltKind::Con)
      {
        auto &var = std::get<Variant>(sv->as);
        UT_FAIL_IF(hit->binders.size() != var.fields.size());
        for (size_t i = 0; i < var.fields.size(); ++i)
          denv.push_back(var.fields[i]);
      }
      node = hit ? hit->body : c.deflt;
      continue;
    }

    // An unapplied built-in is already a value.
    case LTag::BUILTIN: return node;

    case LTag::STRUCT:
    {
      // Force every field, building a fully evaluated struct value.
      auto  &s = std::get<Struct>(node->as);
      Struct out;
      out.name = s.name;
      for (auto &f : s.fields)
        out.fields.push_back({ f.first, eval(f.second, denv, senv) });
      return std::make_shared<Lm>(Lm{ .tag = LTag::STRUCT, .as = out });
    }

    case LTag::FIELD:
    {
      auto &fa  = std::get<Field>(node->as);
      pLm   rec = force(eval(fa.record, denv, senv), senv);
      UT_FAIL_IF(rec->tag != LTag::STRUCT);
      for (auto &f : std::get<Struct>(rec->as).fields)
        if (f.first == fa.name) return f.second;
      UT_FAIL_MSG("no field '%s' on struct '%s'",
                  fa.name.c_str(),
                  std::get<Struct>(rec->as).name.c_str());
      return node;
    }

    case LTag::VARIANT:
    {
      // Construct eagerly but suspend each payload field into a Thunk capturing
      // the current environment: data constructors are non-strict, so the
      // fields are evaluated only when forced (see force / Variant).
      auto   &v = std::get<Variant>(node->as);
      Variant out;
      out.type_name = v.type_name;
      out.tag       = v.tag;
      for (auto &f : v.fields)
        out.fields.push_back(std::make_shared<Lm>(
          Lm{ .tag = LTag::THUNK, .as = Thunk{ f, denv, nullptr } }));
      return std::make_shared<Lm>(Lm{ .tag = LTag::VARIANT, .as = out });
    }

    // Reached only if a thunk is evaluated directly (a VAR bound to one returns
    // it unforced); force it to its value.
    case LTag::THUNK: return force(node, senv);

    // A let binding's weak self-reference (see LET); lock it to the value. eval
    // is never called on one directly -- VAR resolves it -- but handle it for
    // safety.
    case LTag::REC:
    {
      pLm target = std::get<Rec>(node->as).target.lock();
      UT_FAIL_IF(!target);
      return target;
    }

    case LTag::UNK: return node;
    }

    return node;
  }
}

} // namespace IT
