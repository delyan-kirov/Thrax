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

  // The cleanup intrinsic `defer` desugars to: a value applied to (action,
  // cleanup). `%`-prefixed, so no user identifier reaches this.
  if (n == OP::DEFER) return mk(Value{ VDefer{ {} } });

  // An effect operation is a first-class value that performs when applied.
  if (prog.operations.count(n)) return mk(Value{ VOp{ n } });

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

  std::vector<KFrame> kont;
  // Recursive-let closures are reachable from their own captured env only
  // weakly (VRec). The C stack used to pin them across a call; with calls
  // trampolined, keep each one live here for the run's duration. Dedup against
  // the back keeps self-recursion O(1); non-recursive code pins nothing.
  std::vector<pVal> live;

  pVal result;
  bool done = false;

  // Jump into a 1-argument closure `clo` with `arg` in slot 0 (like a tail
  // call): set the frame and control so the main loop runs its body next.
  auto jump1 = [&](const pVal &clo, pVal arg) {
    UT_FAIL_IF(VKind::Code != kind(clo));
    auto           &c  = std::get<VCode>(clo->as);
    const IR::Code &cc = prog.codes[c.code];
    FrameP          nf = std::make_shared<Frame>();
    nf->locals.resize(cc.nlocals);
    nf->locals[0] = std::move(arg);
    nf->env       = c.env;
    frame         = nf;
    ctrl          = cc.body;
  };

  // Hand a finished value to the top continuation (or finish the run). A KRet
  // resumes its saved activation; a KPrompt means the handler's body completed
  // normally, so we run its value clause `els` on the result.
  auto ret = [&](pVal v) {
    for (;;)
    {
      if (kont.empty())
      {
        result = v;
        done   = true;
        return;
      }
      KFrame kf = std::move(kont.back());
      kont.pop_back();
      if (std::holds_alternative<KRet>(kf))
      {
        KRet &kr                  = std::get<KRet>(kf);
        *kr.box                   = *v; // back-patch the let placeholder
        kr.frame->locals[kr.slot] = kr.box;
        frame                     = kr.frame;
        ctrl                      = kr.cont;
        return;
      }
      if (std::holds_alternative<KPrompt>(kf))
      {
        pVal els = deref(std::get<KPrompt>(kf).handler.els);
        jump1(els, v); // run the value clause on the normal result
        return;
      }
      if (std::holds_alternative<KDefer>(kf))
      {
        // Normal completion through a `defer`: run the cleanup, then deliver
        // the protected value `v` (KThunkRet discards the cleanup's own
        // result).
        pVal cleanup = deref(std::get<KDefer>(kf).cleanup);
        kont.push_back(KThunkRet{ v });
        jump1(cleanup, mk(Value{ VUnk{} }));
        return;
      }
      if (std::holds_alternative<KThunkRet>(kf))
      {
        v = std::get<KThunkRet>(kf).saved; // discard incoming, deliver saved
        continue;
      }
      // KAfterClause: the handler operation clause just finished with value
      // `v`. If its resumption `k` was resumed, its defer cleanups run on the
      // resumed computation's completion (their KDefer); if `k` was stored (an
      // extra live reference to kval beyond this frame and the clause slot),
      // they run when it is later resumed and completes. Otherwise `k` was
      // discarded (the exception/abort case): run its captured defer cleanups
      // now, here, with the enclosing handlers still installed, then deliver
      // `v`.
      {
        pVal       &kval    = std::get<KAfterClause>(kf).kval;
        Resumption *res     = std::get<VResump>(deref(kval)->as).seg.get();
        bool        resumed = res->used;
        // Baseline references to kval: this frame's (kf) and the clause's
        // slot 1. Any beyond that means the clause stashed `k` somewhere (a
        // generator / scheduler) -- don't finalize; its cleanup runs when it
        // resumes.
        bool stored = kval.use_count() > 2;
        if (resumed || stored) continue; // deliver v unchanged
        // Schedule the captured cleanups (innermost last in seg => ends on top
        // => runs first: LIFO), then re-deliver the clause's value `v`.
        kont.push_back(KThunkRet{ v });
        for (KFrame &sf : res->seg)
          if (std::holds_alternative<KDefer>(sf))
            kont.push_back(KDefer{ std::get<KDefer>(sf).cleanup });
        v = mk(Value{ VUnk{} }); // start unwinding the freshly pushed KDefers
        continue;
      }
    }
  };

  for (;;)
  {
    switch (IR::ekind(ctrl))
    {
    case IR::EKind::Ret:
      ret(deref(eval_atom(std::get<IR::Ret>(ctrl->as).a, frame)));
      if (done) goto finish;
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
        if (done) goto finish;
        break;
      }
      if (VKind::Extern == kind(callee))
      {
        // A foreign function: accumulate operands until saturated, then make
        // the C call (call_extern marshals args <-> machine words via
        // FF::call).
        VExtern x = std::get<VExtern>(callee->as);
        x.args.push_back(argv);
        if (x.args.size() >= x.decl->arg_types.size())
          ret(call_extern(*x.decl, x.args));
        else
          ret(mk(Value{ x }));
        if (done) goto finish;
        break;
      }
      if (VKind::Defer == kind(callee))
      {
        // `Defer action cleanup`: once both thunks are in hand, install the
        // cleanup as a KDefer marker and run `action {}` under it. The cleanup
        // runs when the action's value returns through the marker (normal
        // completion) or when its continuation is dropped (see ~Resumption).
        VDefer defer = std::get<VDefer>(callee->as);
        defer.args.push_back(argv);
        if (defer.args.size() < 2)
        {
          ret(mk(Value{ defer }));
          if (done) goto finish;
          break;
        }
        kont.push_back(KDefer{ defer.args[1] });          // cleanup
        jump1(deref(defer.args[0]), mk(Value{ VUnk{} })); // action {}
        break;
      }
      if (VKind::Op == kind(callee))
      {
        // Perform: find the nearest installed prompt with a clause for this
        // operation, capture the continuation slice from that prompt up to here
        // (INCLUDING the prompt -> deep handler), and run the clause OUTSIDE
        // the prompt (the stack below it). The clause is the 2-argument curried
        // closure `\arg = \k = e`; we apply it to the operation argument then
        // the resumption, and hand its result to the (now truncated)
        // continuation.
        const std::string &op = std::get<VOp>(callee->as).name;
        size_t             p  = kont.size();
        pVal               clause;
        for (size_t i = kont.size(); i-- > 0;)
        {
          if (!std::holds_alternative<KPrompt>(kont[i])) continue;
          for (auto &cl : std::get<KPrompt>(kont[i]).handler.clauses)
            if (cl.first == op)
            {
              clause = cl.second;
              p      = i;
              break;
            }
          if (clause) break;
        }
        UT_FAIL_IF(!clause); // unhandled effect operation (untyped: a fault)

        auto seg = std::make_shared<Resumption>();
        seg->seg.assign(kont.begin() + (long)p, kont.end());
        kont.resize(p); // the clause runs below the prompt (outside it)

        // Mark the clause boundary with the resumption, so when the clause
        // finishes we can finalize `k`'s `Defer` cleanups if it was discarded
        // (see ret's KAfterClause case). This sits below the clause and above
        // the enclosing handlers, so a discard-time cleanup still sees them.
        pVal kval = mk(Value{ VResump{ seg } });
        kont.push_back(KAfterClause{ kval });

        // Jump into the clause inline: a 2-slot Code with the operation
        // argument in slot 0 and the resumption k in slot 1. Its body runs in
        // this same loop on the now-truncated stack -- no nested call, so
        // resume/perform chains stay constant-stack. The clause's eventual Ret
        // flows to the continuation below the (removed) prompt.
        pVal cl = deref(clause);
        UT_FAIL_IF(VKind::Code != kind(cl));
        auto           &clo = std::get<VCode>(cl->as);
        const IR::Code &cc  = prog.codes[clo.code];
        FrameP          nf  = std::make_shared<Frame>();
        nf->locals.resize(cc.nlocals);
        nf->locals[0] = argv;
        nf->locals[1] = kval;
        nf->env       = clo.env;
        frame         = nf;
        ctrl          = cc.body;
        break;
      }
      if (VKind::Resump == kind(callee))
      {
        // Resume: splice the captured continuation slice back on (re-installing
        // its prompt -> deep) and deliver the value to the suspended point.
        // Affine: a resumption may be used at most once.
        auto &r = std::get<VResump>(callee->as);
        UT_FAIL_IF(r.seg->used);
        r.seg->used = true;
        kont.insert(kont.end(), r.seg->seg.begin(), r.seg->seg.end());
        ret(argv);
        if (done) goto finish;
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
      if (done) goto finish;
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
      if (done) goto finish;
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
      if (done) goto finish;
    }
    break;

    case IR::EKind::Handle:
    {
      // Install a prompt holding the (evaluated) clause and value closures,
      // then run the body under it. The clause/els closures capture the current
      // activation here, exactly when the handler is entered.
      auto   &h = std::get<IR::Handle>(ctrl->as);
      Handler handler;
      for (const IR::HandleClause &c : h.clauses)
        handler.clauses.push_back(
          { std::string(c.op), deref(eval_atom(c.fn, frame)) });
      handler.els = deref(eval_atom(h.els, frame));
      kont.push_back(KPrompt{ std::move(handler) });
      ctrl = h.body;
    }
    break;

    case IR::EKind::Extern:
      // A foreign binding evaluates to a curried foreign callable; the C call
      // happens once it is saturated (see the App handling above). The IR node
      // lives for the whole run, so the pointer is stable.
      ret(mk(Value{ VExtern{ &std::get<IR::Extern>(ctrl->as), {} } }));
      if (done) goto finish;
      break;

    case IR::EKind::Unk:
      ret(mk(Value{ VUnk{} }));
      if (done) goto finish;
      break;
    }
  }

finish:
  return result;
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
