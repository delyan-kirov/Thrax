#include "IT.hpp"

namespace IT
{

pVal
Machine::deref(
  pVal v)
{
  // A let binding is held weakly while its value is built (VRec); lock it to
  // recover the value. Strict data never nests VRecs, so this is shallow.
  while (v && VKind::Rec == kind(v))
  {
    pVal t = std::get<VRec>(v->as).target.lock();
    UT_FAIL_IF(!t);
    v = t;
  }
  return v;
}

pVal
Machine::eval_atom(
  const IR::Atom *a, const FrameP &frame)
{
  switch (IR::akind(a))
  {
  case IR::AKind::Local:
  {
    size_t i = std::get<IR::Local>(a->as).i;
    UT_FAIL_IF(i >= frame->locals.size());
    return frame->locals[i];
  }
  case IR::AKind::Env:
  {
    size_t i = std::get<IR::Env>(a->as).i;
    UT_FAIL_IF(i >= frame->env.size());
    return frame->env[i];
  }
  case IR::AKind::Glob: return glob(std::get<IR::Glob>(a->as).name);
  case IR::AKind::LitI: return mk_int(std::get<IR::LitI>(a->as).v);
  case IR::AKind::LitR: return mk_real(std::get<IR::LitR>(a->as).v);
  case IR::AKind::LitS:
    return mk(Value{ VStr{ std::string(std::get<IR::LitS>(a->as).v) } });
  case IR::AKind::Clos:
  {
    // Capture each free atom RAW (no deref): a recursive self-reference must be
    // captured as its weak cell, or it would form a reference cycle.
    auto  &c = std::get<IR::MkClosure>(a->as);
    ValEnv env;
    env.reserve(c.captures.size());
    for (size_t i = 0; i < c.captures.size(); ++i)
      env.push_back(eval_atom(c.captures[i], frame));
    return mk(Value{ VCode{ c.code, std::move(env) } });
  }
  }
  UT_FAIL_MSG("%s", "unreachable: bad IR::Atom in eval_atom");
  return mk(Value{ VUnk{} });
}

pVal
Machine::glob(
  UT::Vu name)
{
  std::string n(name);

  // A resolved built-in (e.g. "+@Int", "%array") is a first-class curried
  // value.
  auto bit = impls.find(n);
  if (bit != impls.end())
    return mk(Value{ VBuiltin{ name, bit->second.arity, {} } });

  // Lazy-memoized CAF: evaluate the global once on first demand.
  auto mit = memo.find(n);
  if (mit != memo.end()) return mit->second;

  auto git = prog.globals.find(n);
  UT_FAIL_IF(git == prog.globals.end());
  UT_FAIL_IF(in_progress.count(n)); // a cyclic value global (forbidden)

  in_progress.insert(n);
  pVal v = run(git->second, {}, {});
  in_progress.erase(n);
  memo[n] = v;
  return v;
}

pVal
Machine::apply(
  pVal callee, pVal arg)
{
  callee = deref(callee);
  if (VKind::Code == kind(callee))
  {
    auto  &clo = std::get<VCode>(callee->as);
    ValEnv locals;
    locals.push_back(arg);
    return run(clo.code, std::move(locals), clo.env);
  }
  if (VKind::Builtin == kind(callee))
  {
    VBuiltin b = std::get<VBuiltin>(callee->as);
    b.args.push_back(arg);
    if (b.args.size() >= b.arity)
      return impls.at(std::string(b.impl)).fn(b.args);
    return mk(Value{ b });
  }
  UT_FAIL_MSG("%s", "IR machine: applying a non-function value");
  return mk(Value{ VUnk{} });
}

pVal
Machine::run(
  size_t code_id, ValEnv locals0, ValEnv env0)
{
  FrameP frame  = std::make_shared<Frame>();
  frame->locals = std::move(locals0);
  frame->locals.resize(prog.codes[code_id].nlocals);
  frame->env           = std::move(env0);
  const IR::Expr *ctrl = prog.codes[code_id].body;

  std::vector<KRet> kont;
  // Recursive-let closures are reachable from their own captured env only
  // weakly (VRec). The C stack used to pin them across a call; with calls
  // trampolined, keep each one live here for the run's duration. Dedup against
  // the back keeps self-recursion O(1); non-recursive code pins nothing.
  std::vector<pVal> live;

  pVal result;
  bool done = false;

  // Hand a finished value to the top continuation (or finish the run).
  auto ret = [&](pVal v) {
    if (kont.empty())
    {
      result = v;
      done   = true;
      return;
    }
    KRet kr = std::move(kont.back());
    kont.pop_back();
    *kr.box                   = *v; // back-patch the let placeholder
    kr.frame->locals[kr.slot] = kr.box;
    frame                     = kr.frame;
    ctrl                      = kr.cont;
  };

  for (;;)
  {
    switch (IR::ekind(ctrl))
    {
    case IR::EKind::Ret:
      ret(deref(eval_atom(std::get<IR::Ret>(ctrl->as).a, frame)));
      if (done) return result;
      break;

    case IR::EKind::Let:
    {
      // Bind the slot to a weak self-reference, build the value (which may
      // capture that cell), then back-patch and bind it strongly (see KRet).
      auto &l               = std::get<IR::Let>(ctrl->as);
      pVal  box             = mk(Value{ VUnk{} });
      pVal  self            = mk(Value{ VRec{ std::weak_ptr<Value>(box) } });
      frame->locals[l.slot] = self;
      kont.push_back(KRet{ box, l.slot, l.body, frame });
      ctrl = l.rhs;
    }
    break;

    case IR::EKind::App:
    {
      auto &a      = std::get<IR::App>(ctrl->as);
      pVal  callee = deref(eval_atom(a.fn, frame));
      pVal  argv   = deref(eval_atom(a.arg, frame));

      if (VKind::Code == kind(callee))
      {
        auto &clo = std::get<VCode>(callee->as);
        // Pin any recursive closures reachable from this one's captures, so a
        // tail call doesn't free them out from under their own weak self-ref.
        for (const pVal &cap : clo.env)
          if (cap && VKind::Rec == kind(cap))
          {
            pVal tgt = std::get<VRec>(cap->as).target.lock();
            if (tgt && (live.empty() || live.back() != tgt))
              live.push_back(tgt);
          }
        const IR::Code &c  = prog.codes[clo.code];
        FrameP          nf = std::make_shared<Frame>();
        nf->locals.resize(c.nlocals);
        nf->locals[0] = argv;
        nf->env       = clo.env;
        frame         = nf;
        ctrl          = c.body;
        break;
      }
      if (VKind::Builtin == kind(callee))
      {
        VBuiltin b = std::get<VBuiltin>(callee->as);
        b.args.push_back(argv);
        if (b.args.size() >= b.arity)
          ret(impls.at(std::string(b.impl)).fn(b.args));
        else
          ret(mk(Value{ b }));
        if (done) return result;
        break;
      }
      UT_FAIL_MSG("%s", "IR machine: applying a non-function value");
    }
    break;

    case IR::EKind::Case:
    {
      auto          &c   = std::get<IR::Case>(ctrl->as);
      pVal           sv  = deref(eval_atom(c.scrut, frame));
      const IR::Alt *hit = nullptr;
      for (const IR::Alt &alt : c.alts)
      {
        bool m = false;
        switch (alt.kind)
        {
        case IR::AltKind::Int:
          UT_FAIL_IF(VKind::Int != kind(sv));
          m = std::get<VInt>(sv->as).val == alt.ival;
          break;
        case IR::AltKind::Real:
          UT_FAIL_IF(VKind::Real != kind(sv));
          m = std::get<VReal>(sv->as).val == alt.rval;
          break;
        case IR::AltKind::Con:
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
      if (hit && hit->kind == IR::AltKind::Con)
      {
        auto &var = std::get<VVariant>(sv->as);
        UT_FAIL_IF(hit->binders.size() != var.fields.size());
        for (size_t i = 0; i < var.fields.size(); ++i)
          frame->locals[hit->binder_base + i] = var.fields[i];
      }
      ctrl = hit ? hit->body : c.deflt;
    }
    break;

    case IR::EKind::MkStruct:
    {
      auto   &m = std::get<IR::MkStruct>(ctrl->as);
      VStruct out;
      out.name = m.name;
      for (const IR::FieldA &f : m.fields)
        out.fields.push_back({ f.name, deref(eval_atom(f.val, frame)) });
      ret(mk(Value{ out }));
      if (done) return result;
    }
    break;

    case IR::EKind::Field:
    {
      auto &f   = std::get<IR::Field>(ctrl->as);
      pVal  rec = deref(eval_atom(f.rec, frame));
      UT_FAIL_IF(VKind::Struct != kind(rec));
      pVal v;
      for (const auto &fld : std::get<VStruct>(rec->as).fields)
        if (fld.first == f.name)
        {
          v = fld.second;
          break;
        }
      UT_FAIL_IF(!v);
      ret(v);
      if (done) return result;
    }
    break;

    case IR::EKind::MkVariant:
    {
      auto    &mv = std::get<IR::MkVariant>(ctrl->as);
      VVariant out;
      out.type_name = mv.type_name;
      out.tag       = mv.tag;
      for (size_t i = 0; i < mv.fields.size(); ++i)
        out.fields.push_back(deref(eval_atom(mv.fields[i], frame)));
      ret(mk(Value{ out }));
      if (done) return result;
    }
    break;

    case IR::EKind::Extern:
      UT_FAIL_MSG("%s", "IR machine: @extern not yet supported");
      break;

    case IR::EKind::Unk:
      ret(mk(Value{ VUnk{} }));
      if (done) return result;
      break;
    }
  }
}

int
machine_main(
  const IR::Program &prog, UT::Vu entry, bool takes_arg)
{
  Machine m{ prog };
  pVal    v = m.glob(entry);
  if (takes_arg) v = m.apply(v, mk(Value{ VStr{ "" } }));
  v = m.deref(v);
  if (v && VKind::Int == kind(v)) return (int)std::get<VInt>(v->as).val;
  return 0;
}

} // namespace IT
