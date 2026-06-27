#include "IR.hpp"

#include <set>

namespace IR
{

Atom *
alloc_atom(
  AR::Arena &arena, Atom a)
{
  Atom *p = (Atom *)arena.alloc<Atom>();
  *p      = std::move(a);
  return p;
}
Expr *
alloc_expr(
  AR::Arena &arena, Expr e)
{
  Expr *p = (Expr *)arena.alloc<Expr>();
  *p      = std::move(e);
  return p;
}

namespace
{

// Per-function conversion state: `scope` maps each in-scope CR binder
// (innermost last, parallel to CR's De-Bruijn stack) to the IR atom that reads
// it in *this* function; `next_local`/`nlocals` allocate activation slots
// (stack-disciplined, so independent scopes reuse slots and `nlocals` is the
// high-water mark).
struct Ctx
{
  std::vector<Atom *> scope;
  size_t              next_local = 0;
  size_t              nlocals    = 0;

  size_t
  alloc_local()
  {
    size_t s = next_local++;
    if (next_local > nlocals) nlocals = next_local;
    return s;
  }
  void
  free_local()
  {
    next_local--;
  }
};

// Collect the enclosing-scope positions (< m) a Fun body references, so a
// lifted function captures exactly its free variables. `depth` counts binders
// in scope *inside* the Fun (starts at 1 for the param); a CR Var at De-Bruijn
// `idx` resolves to outer position (m + depth) - idx, which is free iff < m.
// Descends through nested Funs (transitive capture), mirroring CR's binder
// discipline.
void
collect_free(
  CR::Term *t, size_t m, size_t depth, std::set<size_t> &out)
{
  if (!t) return;
  switch (CR::kind(t))
  {
  case CR::Kind::Var:
  {
    auto &v = std::get<CR::Var>(t->as);
    if (v.idx >= 1 && v.idx != (size_t)-1)
    {
      size_t pos = (m + depth) - v.idx;
      if (pos < m) out.insert(pos);
    }
  }
  break;
  case CR::Kind::Fun:
    collect_free(std::get<CR::Fun>(t->as).body, m, depth + 1, out);
    break;
  case CR::Kind::Let:
  {
    // the binder is in scope for both val (recursion) and body
    auto &l = std::get<CR::Let>(t->as);
    collect_free(l.val, m, depth + 1, out);
    collect_free(l.body, m, depth + 1, out);
  }
  break;
  case CR::Kind::App:
  {
    auto &a = std::get<CR::App>(t->as);
    collect_free(a.fn, m, depth, out);
    collect_free(a.arg, m, depth, out);
  }
  break;
  case CR::Kind::Case:
  {
    auto &c = std::get<CR::Case>(t->as);
    collect_free(c.scrut, m, depth, out);
    for (const CR::Alt &alt : c.alts)
      collect_free(alt.body, m, depth + alt.binders.size(), out);
    collect_free(c.deflt, m, depth, out);
  }
  break;
  case CR::Kind::Struct:
    for (const CR::FieldInit &f : std::get<CR::Struct>(t->as).fields)
      collect_free(f.val, m, depth, out);
    break;
  case CR::Kind::Field:
    collect_free(std::get<CR::Field>(t->as).record, m, depth, out);
    break;
  case CR::Kind::Variant:
    for (CR::Term *f : std::get<CR::Variant>(t->as).fields)
      collect_free(f, m, depth, out);
    break;
  case CR::Kind::Handle:
  {
    // body and the clause/els Funs are all at this scope (each Fun bumps depth
    // itself when collect_free recurses into it).
    auto &h = std::get<CR::Handle>(t->as);
    collect_free(h.body, m, depth, out);
    for (const CR::HClause &c : h.clauses) collect_free(c.fn, m, depth, out);
    collect_free(h.els, m, depth, out);
  }
  break;
  case CR::Kind::Int:
  case CR::Kind::Real:
  case CR::Kind::Str:
  case CR::Kind::Extern:
  case CR::Kind::Unk   : break;
  }
}

// The CR terms that closure-convert directly to an Atom. Everything else is a
// computation and must be hoisted into a `let` to appear in atom position.
bool
is_cr_atom(
  CR::Term *t)
{
  switch (CR::kind(t))
  {
  case CR::Kind::Int:
  case CR::Kind::Real:
  case CR::Kind::Str:
  case CR::Kind::Var:
  case CR::Kind::Fun : return true;
  default            : return false;
  }
}

struct Conv
{
  AR::Arena &arena;
  Program   &prog;

  Atom *
  mkA(
    Atom a)
  {
    return alloc_atom(arena, std::move(a));
  }
  Expr *
  mkE(
    Expr e)
  {
    return alloc_expr(arena, std::move(e));
  }

  // Convert a CR atom (ANF guarantees Var / literal / lambda here).
  Atom *
  conv_atom(
    CR::Term *t, Ctx &ctx)
  {
    switch (CR::kind(t))
    {
    case CR::Kind::Int:
      return mkA(Atom{ LitI{ std::get<CR::Int>(t->as).val } });
    case CR::Kind::Real:
      return mkA(Atom{ LitR{ std::get<CR::Real>(t->as).val } });
    case CR::Kind::Str:
      return mkA(Atom{ LitS{ std::get<CR::Str>(t->as).val } });
    case CR::Kind::Var:
    {
      auto &v = std::get<CR::Var>(t->as);
      if (v.idx == 0) return mkA(Atom{ Glob{ v.name } });
      UT_FAIL_IF(v.idx == (size_t)-1); // placeholder never appears in a body
      UT_FAIL_IF(v.idx > ctx.scope.size());
      Atom *a = ctx.scope[ctx.scope.size() - v.idx];
      UT_FAIL_IF(!a); // a non-captured enclosing slot was referenced
      return a;
    }
    case CR::Kind::Fun: return conv_fun(t, ctx);
    default:
      UT_FAIL_MSG("%s", "IR::conv_atom: non-atomic CR term (ANF violation)");
      return mkA(Atom{ LitI{ 0 } });
    }
  }

  // Lift a lambda to a Code block and return the MkClosure that builds it.
  Atom *
  conv_fun(
    CR::Term *t, Ctx &outer)
  {
    auto  &fn = std::get<CR::Fun>(t->as);
    size_t m  = outer.scope.size();

    std::set<size_t> free;
    collect_free(fn.body, m, 1, free);

    // captures, in ascending outer-position order; each free position becomes
    // an Env slot inside the new Code.
    UT::Vec<Atom *>                    captures{ arena };
    std::unordered_map<size_t, size_t> env_of;
    for (size_t pos : free)
    {
      env_of[pos] = captures.size();
      captures.push(outer.scope[pos]);
    }

    // Inner scope: enclosing binders are reachable only as Env (captured ones)
    // or unreachable (sentinel null); then the param is Local 0.
    Ctx inner;
    inner.scope.resize(m + 1, nullptr);
    for (size_t pos = 0; pos < m; ++pos)
      if (env_of.count(pos)) inner.scope[pos] = mkA(Atom{ Env{ env_of[pos] } });
    inner.scope[m]   = mkA(Atom{ Local{ 0 } });
    inner.next_local = 1;
    inner.nlocals    = 1;

    Expr  *body = conv_expr(fn.body, inner);
    size_t id   = prog.codes.size();
    prog.codes.push_back(Code{ 1, inner.nlocals, body, fn.param });

    return mkA(Atom{ MkClosure{ id, std::move(captures) } });
  }

  // Convert a handler clause `\a = \k = body` (curried CR Funs) into a SINGLE
  // 2-parameter Code: the operation argument is Local 0, the continuation k is
  // Local 1. The machine fills both slots at perform time and runs the body
  // inline (no nested application -> constant host stack). Mirrors conv_fun but
  // with two binders.
  Atom *
  conv_clause(
    CR::Term *t, Ctx &outer)
  {
    UT_FAIL_IF(CR::kind(t) != CR::Kind::Fun);
    auto &fa = std::get<CR::Fun>(t->as); // \a = ...
    UT_FAIL_IF(CR::kind(fa.body) != CR::Kind::Fun);
    auto     &fk   = std::get<CR::Fun>(fa.body->as); // \k = body
    CR::Term *body = fk.body;

    size_t           m = outer.scope.size();
    std::set<size_t> free;
    collect_free(body, m, 2, free); // a and k are the two binders in scope

    UT::Vec<Atom *>                    captures{ arena };
    std::unordered_map<size_t, size_t> env_of;
    for (size_t pos : free)
    {
      env_of[pos] = captures.size();
      captures.push(outer.scope[pos]);
    }

    Ctx inner;
    inner.scope.resize(m + 2, nullptr);
    for (size_t pos = 0; pos < m; ++pos)
      if (env_of.count(pos)) inner.scope[pos] = mkA(Atom{ Env{ env_of[pos] } });
    inner.scope[m]     = mkA(Atom{ Local{ 0 } }); // the operation argument `a`
    inner.scope[m + 1] = mkA(Atom{ Local{ 1 } }); // the continuation `k`
    inner.next_local   = 2;
    inner.nlocals      = 2;

    Expr  *cbody = conv_expr(body, inner);
    size_t id    = prog.codes.size();
    prog.codes.push_back(Code{ 2, inner.nlocals, cbody, fa.param });
    return mkA(Atom{ MkClosure{ id, std::move(captures) } });
  }

  Expr *
  conv_expr(
    CR::Term *t, Ctx &ctx)
  {
    switch (CR::kind(t))
    {
    case CR::Kind::Int:
    case CR::Kind::Real:
    case CR::Kind::Str:
    case CR::Kind::Var:
    case CR::Kind::Fun : return mkE(Expr{ Ret{ conv_atom(t, ctx) } });

    case CR::Kind::App:
    {
      auto &a  = std::get<CR::App>(t->as);
      Atom *fn = conv_atom(a.fn, ctx);
      Atom *ar = conv_atom(a.arg, ctx);
      return mkE(Expr{ App{ fn, ar, a.tail } });
    }

    case CR::Kind::Let:
    {
      // recursive: the binder is in scope while its own value is built
      auto  &l    = std::get<CR::Let>(t->as);
      size_t slot = ctx.alloc_local();
      ctx.scope.push_back(mkA(Atom{ Local{ slot } }));
      Expr *rhs  = conv_expr(l.val, ctx);
      Expr *body = conv_expr(l.body, ctx);
      ctx.scope.pop_back();
      ctx.free_local();
      return mkE(Expr{ Let{ slot, rhs, body } });
    }

    case CR::Kind::Case:
    {
      auto        &c     = std::get<CR::Case>(t->as);
      Atom        *scrut = conv_atom(c.scrut, ctx);
      UT::Vec<Alt> alts{ arena };
      for (const CR::Alt &a : c.alts)
      {
        Alt b{};
        b.kind        = a.kind;
        b.ctor        = a.ctor;
        b.tag         = a.tag;
        b.ival        = a.ival;
        b.rval        = a.rval;
        b.binders     = UT::Vec<UT::Vu>{ arena };
        b.binder_base = ctx.next_local;
        for (UT::Vu name : a.binders)
        {
          ctx.scope.push_back(mkA(Atom{ Local{ ctx.alloc_local() } }));
          b.binders.push(name);
        }
        b.body = conv_expr(a.body, ctx);
        for (size_t i = 0; i < a.binders.size(); ++i)
        {
          ctx.scope.pop_back();
          ctx.free_local();
        }
        alts.push(b);
      }
      Expr *deflt = conv_expr(c.deflt, ctx);
      return mkE(Expr{ Case{ scrut, std::move(alts), deflt } });
    }

    case CR::Kind::Struct:
    {
      auto           &s = std::get<CR::Struct>(t->as);
      UT::Vec<FieldA> fields{ arena };
      for (const CR::FieldInit &f : s.fields)
        fields.push(FieldA{ f.name, conv_atom(f.val, ctx) });
      return mkE(Expr{ MkStruct{ s.name, std::move(fields) } });
    }

    case CR::Kind::Field:
    {
      auto &f = std::get<CR::Field>(t->as);
      return mkE(Expr{ Field{ conv_atom(f.record, ctx), f.name } });
    }

    case CR::Kind::Variant:
    {
      // CR keeps variant fields inline (they were lazy thunks). Under strict
      // evaluation we ANF-normalize them here: an atomic field converts in
      // place, a computation field is hoisted into a `let` so every payload
      // slot is an atom. Fields evaluate left-to-right (outermost let first).
      auto                                  &v = std::get<CR::Variant>(t->as);
      UT::Vec<Atom *>                        fields{ arena };
      std::vector<std::pair<size_t, Expr *>> lets;
      for (CR::Term *f : v.fields)
      {
        if (is_cr_atom(f))
        {
          fields.push(conv_atom(f, ctx));
          continue;
        }
        Expr  *rhs  = conv_expr(f, ctx);
        size_t slot = ctx.alloc_local();
        lets.push_back({ slot, rhs });
        fields.push(mkA(Atom{ Local{ slot } }));
      }
      Expr *res
        = mkE(Expr{ MkVariant{ v.type_name, v.tag, std::move(fields) } });
      for (auto it = lets.rbegin(); it != lets.rend(); ++it)
        res = mkE(Expr{ Let{ it->first, it->second, res } });
      for (size_t i = 0; i < lets.size(); ++i) ctx.free_local();
      return res;
    }

    case CR::Kind::Handle:
    {
      // body converts in this activation; each clause `fn` and `els` is a CR
      // Fun, so conv_atom lifts it to a Code and yields its MkClosure.
      auto                 &h    = std::get<CR::Handle>(t->as);
      Expr                 *body = conv_expr(h.body, ctx);
      UT::Vec<HandleClause> clauses{ arena };
      for (const CR::HClause &c : h.clauses)
        clauses.push(HandleClause{ c.op, conv_clause(c.fn, ctx) });
      Atom *els = conv_atom(h.els, ctx);
      return mkE(Expr{ Handle{ body, std::move(clauses), els } });
    }

    case CR::Kind::Extern:
    {
      auto &e = std::get<CR::Extern>(t->as);
      return mkE(Expr{ Extern{ e.symbol, e.lib, e.arg_types, e.ret_type } });
    }

    case CR::Kind::Unk: return mkE(Expr{ Unk{} });
    }
    UT_FAIL_MSG("%s", "unreachable: unhandled CR::Kind in IR::conv_expr");
    return mkE(Expr{ Unk{} });
  }
};

} // namespace

Program
lower(
  const CR::StatEnv &env, AR::Arena &arena)
{
  Program prog;
  Conv    conv{ arena, prog };
  for (const auto &kv : env)
  {
    Ctx    top;
    Expr  *body = conv.conv_expr(kv.second, top);
    size_t id   = prog.codes.size();
    prog.codes.push_back(
      Code{ 0, top.nlocals, body, UT::Vu{ kv.first.data(), kv.first.size() } });
    prog.globals[kv.first] = id;
  }
  return prog;
}

/*------------------------------------------------------------------------------
 *\PPRINT
 *-----------------------------------------------------------------------------*/

namespace
{
std::string
pp_atom(
  const Atom *a)
{
  switch (akind(a))
  {
  case AKind::Local:
    return "loc[" + std::to_string(std::get<Local>(a->as).i) + "]";
  case AKind::Env : return "env[" + std::to_string(std::get<Env>(a->as).i) + "]";
  case AKind::Glob: return "@" + std::string(std::get<Glob>(a->as).name);
  case AKind::LitI: return std::to_string(std::get<LitI>(a->as).v);
  case AKind::LitR: return std::to_string(std::get<LitR>(a->as).v);
  case AKind::LitS: return "\"" + std::string(std::get<LitS>(a->as).v) + "\"";
  case AKind::Clos:
  {
    auto       &c = std::get<MkClosure>(a->as);
    std::string s = "clos#" + std::to_string(c.code) + "(";
    for (size_t i = 0; i < c.captures.size(); ++i)
      s += (i ? ", " : "") + pp_atom(c.captures[i]);
    return s + ")";
  }
  }
  return "?atom";
}

std::string
pp_expr(
  const Expr *e, int lvl)
{
  std::string pad(lvl * 2, ' ');
  switch (ekind(e))
  {
  case EKind::Ret: return pad + "ret " + pp_atom(std::get<Ret>(e->as).a);
  case EKind::Let:
  {
    auto &l = std::get<Let>(e->as);
    return pad + "let loc[" + std::to_string(l.slot) + "] =\n"
           + pp_expr(l.rhs, lvl + 1) + "\n" + pp_expr(l.body, lvl);
  }
  case EKind::App:
  {
    auto &a = std::get<App>(e->as);
    return pad + (a.tail ? "tailapp " : "app ") + pp_atom(a.fn) + " "
           + pp_atom(a.arg);
  }
  case EKind::Case:
  {
    auto       &c = std::get<Case>(e->as);
    std::string s = pad + "case " + pp_atom(c.scrut) + " of";
    for (const Alt &alt : c.alts)
    {
      s += "\n" + pad + "  alt";
      if (alt.kind == AltKind::Con) s += " ." + std::string(alt.ctor);
      if (alt.kind == AltKind::Int) s += " " + std::to_string(alt.ival);
      s += " ->\n" + pp_expr(alt.body, lvl + 2);
    }
    return s + "\n" + pad + "  else ->\n" + pp_expr(c.deflt, lvl + 2);
  }
  case EKind::MkStruct:
  {
    auto       &m = std::get<MkStruct>(e->as);
    std::string s = pad + std::string(m.name) + ".{";
    for (size_t i = 0; i < m.fields.size(); ++i)
      s += (i ? ", " : "") + std::string(m.fields[i].name) + " = "
           + pp_atom(m.fields[i].val);
    return s + "}";
  }
  case EKind::Field:
  {
    auto &f = std::get<Field>(e->as);
    return pad + pp_atom(f.rec) + "." + std::string(f.name);
  }
  case EKind::MkVariant:
  {
    auto       &v = std::get<MkVariant>(e->as);
    std::string s
      = pad + std::string(v.type_name) + "." + std::string(v.tag) + ".{";
    for (size_t i = 0; i < v.fields.size(); ++i)
      s += (i ? ", " : "") + pp_atom(v.fields[i]);
    return s + "}";
  }
  case EKind::Handle:
  {
    auto       &h = std::get<Handle>(e->as);
    std::string s = pad + "handle\n" + pp_expr(h.body, lvl + 1);
    for (const HandleClause &c : h.clauses)
      s += "\n" + pad + "  is " + std::string(c.op) + " " + pp_atom(c.fn);
    s += "\n" + pad + "  else " + pp_atom(h.els);
    return s;
  }
  case EKind::Extern:
    return pad + "@extern(" + std::string(std::get<Extern>(e->as).symbol) + ")";
  case EKind::Unk: return pad + "?unk";
  }
  return pad + "?expr";
}
} // namespace

std::string
pprint(
  const Program &p)
{
  std::string s = "=== globals ===\n";
  for (const auto &kv : p.globals)
    s += "  " + kv.first + " -> code#" + std::to_string(kv.second) + "\n";
  s += "=== code ===\n";
  for (size_t i = 0; i < p.codes.size(); ++i)
  {
    const Code &c = p.codes[i];
    s += "code#" + std::to_string(i) + " " + std::string(c.name) + " (nparams="
         + std::to_string(c.nparams) + ", nlocals=" + std::to_string(c.nlocals)
         + ")\n" + pp_expr(c.body, 1) + "\n";
  }
  return s;
}

} // namespace IR
