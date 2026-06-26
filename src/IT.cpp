#include "IT.hpp"

namespace IT
{

namespace
{

// Force a value to weak head normal form. Thrax is call-by-value, so most
// values arrive already forced; the exception is a constructor field, which is
// suspended as a VThunk (see VVariant). Every strict eliminator -- a `case`
// scrutinee, a field access, a builtin / extern operand, the callee of an
// application -- forces its input through here, evaluating the thunk once,
// memoizing the result, and collapsing any chain of thunks. Constructor fields
// that are merely passed along stay suspended, so recursion stays lazy.
pVal
force(
  pVal v, CR::StatEnv &senv)
{
  if (!v || VKind::Thunk != kind(v)) return v;
  auto &t = std::get<VThunk>(v->as);
  if (!t.memo) t.memo = eval(t.expr, t.env, senv);
  t.memo = force(t.memo, senv);
  return t.memo;
}

} // namespace

pVal
eval(
  const CR::Term *node, ValEnv denv, CR::StatEnv &senv)
{
  // Trampoline: a call in tail position rebinds `node`/`denv` and loops instead
  // of recursing, so tail recursion runs in constant C++ stack. Non-tail
  // sub-evaluations (a `case` scrutinee, a `let` value, an application's atoms)
  // still recurse -- their depth is the program's, not the call chain's.
  //
  // `live` keeps recursive `let` closures alive across the loop: such a closure
  // is held only weakly by its own captured environment (the VRec cell, see
  // LET), and by the C++ stack used to keep it live across a call. Globals
  // (empty env) and ordinary closures hold no VRec, so they are never added and
  // plain tail recursion stays in constant space.
  std::vector<pVal> live;

  for (;;)
  {
    UT_FAIL_IF(!node);

    switch (kind(node))
    {
    case CR::Kind::Int : return mk_int(std::get<CR::Int>(node->as).val);
    case CR::Kind::Real: return mk_real(std::get<CR::Real>(node->as).val);
    case CR::Kind::Str:
      return mk(Value{ VStr{ std::string(std::get<CR::Str>(node->as).val) } });

    case CR::Kind::Var:
    {
      auto &v = std::get<CR::Var>(node->as);

      // A resolved built-in (e.g. "+@Int") is a first-class, curried value.
      auto bit = impls.find(std::string(v.name));
      if (bit != impls.end())
        return mk(Value{ VBuiltin{ v.name, bit->second.arity, {} } });

      // Global definition placeholder (the value a `$ name = ...` evaluates
      // to).
      if (v.idx == (size_t)-1) return mk(Value{ VUnk{} });

      // Local variable (De-Bruijn indexed).
      if (v.idx > 0)
      {
        UT_FAIL_IF(v.idx > denv.size());
        pVal entry = denv[denv.size() - v.idx];
        // A `let` binding is held weakly while its value is built (see LET), to
        // keep the closure graph acyclic; lock it to recover the value.
        if (entry && VKind::Rec == kind(entry))
        {
          pVal target = std::get<VRec>(entry->as).target.lock();
          UT_FAIL_IF(!target);
          return target;
        }
        return entry;
      }

      // Global variable (idx == 0): jump to its definition in tail position.
      auto it = senv.find(std::string(v.name));
      UT_FAIL_IF(it == senv.end());
      node = it->second;
      denv.clear();
      continue;
    }

    case CR::Kind::Fun:
    {
      auto &f = std::get<CR::Fun>(node->as);
      return mk(Value{ VClosure{ f.body, f.param, denv } });
    }

    case CR::Kind::Let:
    {
      auto &l = std::get<CR::Let>(node->as);
      // Bind the name to a placeholder, then evaluate the value with that
      // binding *weakly* in scope and back-patch the slot, so recursive
      // references resolve without the value's captured environment forming a
      // shared_ptr cycle with it. (For a non-recursive let the placeholder is
      // simply never read before patching.) The body then evaluates in tail
      // position with the binding held strongly.
      pVal value = mk(Value{ VUnk{} });
      pVal self  = mk(Value{ VRec{ std::weak_ptr<Value>(value) } });

      ValEnv val_env = denv;
      val_env.push_back(self);
      *value = *eval(l.val, val_env, senv);

      denv.push_back(value);
      node = l.body;
      continue;
    }

    case CR::Kind::App:
    {
      auto &a = std::get<CR::App>(node->as);

      pVal callee = force(eval(a.fn, denv, senv), senv);
      pVal val    = eval(a.arg, denv, senv);

      // Foreign function: accumulate the (forced) argument, call once
      // saturated.
      if (VKind::Extern == kind(callee))
      {
        VExtern e = std::get<VExtern>(callee->as); // copy and extend
        e.args.push_back(force(val, senv));
        if (e.args.size() >= e.decl->arg_types.size())
          return call_extern(*e.decl, e.args);
        return mk(Value{ e });
      }

      // Built-in: accumulate the (forced) argument, run the impl once
      // saturated.
      if (VKind::Builtin == kind(callee))
      {
        VBuiltin b = std::get<VBuiltin>(callee->as); // copy and extend
        b.args.push_back(force(val, senv));
        if (b.args.size() >= b.arity)
          return impls.at(std::string(b.impl)).fn(b.args);
        return mk(Value{ b });
      }

      UT_FAIL_IF(VKind::Closure != kind(callee));
      auto  &clo     = std::get<VClosure>(callee->as);
      ValEnv new_env = clo.env;
      new_env.push_back(val);
      // A recursive `let` value is reachable from a closure's captured
      // environment only weakly, through a VRec cell (see LET). The C++ stack
      // used to keep it alive across a call; with the call trampolined, pin
      // each such value for the loop's lifetime. Globals (empty env) and
      // ordinary closures carry no VRec, so nothing is pinned and plain tail
      // recursion stays in constant space; dedup keeps self-recursion at O(1).
      for (const pVal &slot : clo.env)
        if (slot && VKind::Rec == kind(slot))
        {
          pVal tgt = std::get<VRec>(slot->as).target.lock();
          if (tgt && (live.empty() || live.back() != tgt)) live.push_back(tgt);
        }
      node = clo.body;
      denv = std::move(new_env);
      continue;
    }

    case CR::Kind::Extern:
    {
      auto &e = std::get<CR::Extern>(node->as);
      // A nullary extern is already saturated; otherwise it is a function
      // value.
      if (e.arg_types.size() == 0) return call_extern(e, {});
      return mk(Value{ VExtern{ &e, {} } });
    }

    case CR::Kind::Case:
    {
      auto          &c   = std::get<CR::Case>(node->as);
      pVal           sv  = force(eval(c.scrut, denv, senv), senv); // to WHNF
      const CR::Alt *hit = nullptr;
      for (const CR::Alt &alt : c.alts)
      {
        bool m = false;
        switch (alt.kind)
        {
        case CR::AltKind::Int:
          UT_FAIL_IF(VKind::Int != kind(sv));
          m = std::get<VInt>(sv->as).val == alt.ival;
          break;
        case CR::AltKind::Real:
          UT_FAIL_IF(VKind::Real != kind(sv));
          m = std::get<VReal>(sv->as).val == alt.rval;
          break;
        case CR::AltKind::Con:
          UT_FAIL_IF(VKind::Variant != kind(sv));
          m = std::get<VVariant>(sv->as).tag == alt.ctor;
          break;
        }
        if (m)
        {
          hit = &alt;
          break;
        }
      }
      // A Con alt binds its payload positionally onto `denv`, in the same order
      // assign_id pushed the binders, so De-Bruijn indices line up. The fields
      // are the constructor's thunks -- binders stay lazy until forced.
      if (hit && hit->kind == CR::AltKind::Con)
      {
        auto &var = std::get<VVariant>(sv->as);
        UT_FAIL_IF(hit->binders.size() != var.fields.size());
        for (const pVal &field : var.fields) denv.push_back(field);
      }
      node = hit ? hit->body : c.deflt;
      continue;
    }

    case CR::Kind::Struct:
    {
      // Force every field, building a fully evaluated struct value.
      auto   &s = std::get<CR::Struct>(node->as);
      VStruct out;
      out.name = s.name;
      for (const CR::FieldInit &f : s.fields)
        out.fields.push_back({ f.name, eval(f.val, denv, senv) });
      return mk(Value{ out });
    }

    case CR::Kind::Field:
    {
      auto &fa  = std::get<CR::Field>(node->as);
      pVal  rec = force(eval(fa.record, denv, senv), senv);
      UT_FAIL_IF(VKind::Struct != kind(rec));
      for (const auto &f : std::get<VStruct>(rec->as).fields)
        if (f.first == fa.name) return f.second;
      UT_FAIL_MSG("no field '%s' on struct '%s'",
                  std::string(fa.name).c_str(),
                  std::string(std::get<VStruct>(rec->as).name).c_str());
      return mk(Value{ VUnk{} });
    }

    case CR::Kind::Variant:
    {
      // Construct eagerly but suspend each payload field into a VThunk
      // capturing the current environment: data constructors are non-strict, so
      // the fields are evaluated only when forced (see force / VVariant).
      auto    &v = std::get<CR::Variant>(node->as);
      VVariant out;
      out.type_name = v.type_name;
      out.tag       = v.tag;
      for (const CR::Term *f : v.fields)
        out.fields.push_back(mk(Value{ VThunk{ f, denv, nullptr } }));
      return mk(Value{ out });
    }

    case CR::Kind::Unk: return mk(Value{ VUnk{} });
    }

    UT_FAIL_MSG("%s", "unreachable: unhandled CR::Kind in eval");
    return mk(Value{ VUnk{} });
  }
}

} // namespace IT
