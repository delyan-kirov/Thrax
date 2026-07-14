#include "CR.hpp"
#include "CRxANF.hpp"
#include "OP.hpp"

namespace CR
{

Term *
alloc(
  AR::Arena &arena, Term t)
{
  Term *p = (Term *)arena.alloc<Term>();
  *p      = std::move(t);
  return p;
}

namespace
{

// Normalize to ANF, then assign De-Bruijn indices -- the finishing steps every
// definition goes through before it is stored or evaluated.
Term *
finalize(
  Term *node, AR::Arena &arena)
{
  Anf anf{ arena };
  node = anf.term(node);
  std::vector<UT::Vu> names;
  assign_id(node, names);
  return node;
}

// Split a foreign signature `A -> B -> R` into argument types [A, B] and result
// R, copying each type name into `arena`. A type variable or function argument
// is opaque and word-sized, so it marshals as a pointer ("Ptr").
UT::Vu
ffi_type_name(
  EX::Ty *t, AR::Arena &arena)
{
  switch (t->tag)
  {
  case EX::TyTag::Con:
    return UT::strdup(arena, std::get<EX::TyCon>(t->as).name);
  case EX::TyTag::Var:
  case EX::TyTag::Arrow: return UT::strdup(arena, "Ptr");
  }
  UT_FAIL_MSG("%s", "unreachable: unknown EX::TyTag in ffi_type_name");
  return {};
}

void
flatten_sig(
  EX::Ty *t, UT::Vec<UT::Vu> &args, UT::Vu &ret, AR::Arena &arena)
{
  while (t && EX::TyTag::Arrow == t->tag)
  {
    auto &ar = std::get<EX::TyArrow>(t->as);
    args.push(ffi_type_name(ar.from, arena));
    t = ar.to;
  }
  ret = t ? ffi_type_name(t, arena) : UT::strdup(arena, "Int");
}

// The recursive EX::Expr -> Core worker. `env` collects the globals
// encountered; `arena` owns every node and every interned name.
Term *
go(
  EX::Expr *expr, StatEnv &env, AR::Arena &arena)
{
  switch (expr->tag)
  {
  case EX::ExprTag::If:
  {
    // `if c then t else e`  ==>  case c of { 0 -> e } else t
    EX::ExIf &ifexpr = std::get<EX::ExIf>(expr->as);
    Alt       zero{}; // value-init: `binders` is a valid empty vector
    zero.kind = AltKind::Int;
    zero.ival = 0;
    zero.body = go(ifexpr.alt, env, arena);
    Case cs;
    cs.scrut = go(ifexpr.cond, env, arena);
    cs.alts  = UT::Vec<Alt>{ arena };
    cs.alts.push(zero);
    cs.deflt = go(ifexpr.then, env, arena);
    return alloc(arena, Term{ cs });
  }

  case EX::ExprTag::Case:
  {
    auto &ec = std::get<EX::ExCase>(expr->as);
    Case  cs;
    cs.scrut = go(ec.scrut, env, arena);
    cs.deflt = go(ec.deflt, env, arena);
    cs.alts  = UT::Vec<Alt>{ arena };
    for (const EX::CaseAlt &a : ec.alts)
    {
      Alt b{}; // value-init: `binders` is a valid empty vector
      b.kind = a.kind;
      b.tag  = a.tag;
      b.ival = a.ival;
      b.rval = a.rval;
      if (a.kind == EX::AltKind::Con) // ctor / binders are Con-only
      {
        b.ctor    = UT::strdup(arena, a.ctor);
        b.binders = UT::Vec<UT::Vu>{ arena };
        for (UT::Vu binder : a.binders)
          b.binders.push(UT::strdup(arena, binder));
      }
      b.body = go(a.body, env, arena);
      cs.alts.push(b);
    }
    return alloc(arena, Term{ cs });
  }

  case EX::ExprTag::Let:
  {
    auto &lt = std::get<EX::ExLet>(expr->as);
    Let   l{ UT::strdup(arena, lt.var),
           go(lt.val, env, arena),
           go(lt.body, env, arena) };
    return alloc(arena, Term{ l });
  }

  case EX::ExprTag::App:
  {
    auto &app = std::get<EX::ExApp>(expr->as);
    App   a{ go(app.fn, env, arena), go(app.arg, env, arena), false };
    return alloc(arena, Term{ a });
  }

  case EX::ExprTag::FnDef:
  {
    auto &fn = std::get<EX::ExFnDef>(expr->as);
    Fun   f{ UT::strdup(arena, fn.param), go(fn.body, env, arena) };
    return alloc(arena, Term{ f });
  }

  case EX::ExprTag::Str:
  {
    return alloc(
      arena,
      Term{ Str{ UT::strdup(arena, std::get<EX::ExStr>(expr->as).value) } });
  }

  case EX::ExprTag::Var:
  {
    auto  &v    = std::get<EX::ExVar>(expr->as);
    UT::Vu name = v.resolved.data() ? v.resolved : v.name;
    return alloc(arena, Term{ Var{ UT::strdup(arena, name), 0 } });
  }

  case EX::ExprTag::Int:
    return alloc(arena, Term{ Int{ std::get<EX::ExInt>(expr->as).value } });

  // The unit value `{}` -- represented at runtime as the integer 0 (the type
  // checker keeps the unit type distinct from Int; the runtime does not).
  case EX::ExprTag::Unit: return alloc(arena, Term{ Int{ 0 } });

  case EX::ExprTag::Real:
    return alloc(arena, Term{ Real{ std::get<EX::ExReal>(expr->as).value } });

  case EX::ExprTag::Def:
  {
    auto       &gd   = std::get<EX::ExDef>(expr->as);
    std::string name = std::string{ gd.name };

    Term *def;
    if (EX::ExprTag::Extern == gd.def->tag)
    {
      // Foreign binding: the call types come from the signature, not the body.
      auto  &ex = std::get<EX::ExExtern>(gd.def->as);
      Extern e;
      e.symbol    = UT::strdup(arena, ex.symbol);
      e.lib       = UT::strdup(arena, ex.lib);
      e.arg_types = UT::Vec<UT::Vu>{ arena };
      if (gd.sig)
        flatten_sig(gd.sig, e.arg_types, e.ret_type, arena);
      else
        e.ret_type = UT::strdup(arena, "Int");
      def = alloc(arena, Term{ e });
    }
    else
    {
      def = go(gd.def, env, arena);
    }

    env[name] = finalize(def, arena);
    return alloc(arena, Term{ Var{ UT::strdup(arena, gd.name), (size_t)-1 } });
  }

  case EX::ExprTag::StructLit:
  {
    auto  &sl = std::get<EX::ExStructLit>(expr->as);
    Struct s;
    s.name   = UT::strdup(arena, sl.type_name);
    s.fields = UT::Vec<FieldInit>{ arena };
    for (const EX::FieldInit &f : sl.fields)
      s.fields.push(
        FieldInit{ UT::strdup(arena, f.name), go(f.val, env, arena) });
    return alloc(arena, Term{ s });
  }

  case EX::ExprTag::Field:
  {
    auto &fa = std::get<EX::ExField>(expr->as);
    Field f{ go(fa.record, env, arena), UT::strdup(arena, fa.field) };
    return alloc(arena, Term{ f });
  }

  case EX::ExprTag::VariantLit:
  {
    auto   &vl = std::get<EX::ExVariantLit>(expr->as);
    Variant v;
    v.type_name = UT::strdup(arena, vl.type_name);
    v.tag       = UT::strdup(arena, vl.tag);
    v.fields    = UT::Vec<Term *>{ arena };
    // Fields are positional in declared order (LL normalized any named form).
    for (const EX::FieldInit &f : vl.fields)
      v.fields.push(go(f.val, env, arena));
    return alloc(arena, Term{ v });
  }

  case EX::ExprTag::SeqLit:
  {
    // TC settled the container (ExSeqLit::is_array). Desugar accordingly:
    //   Array : array_push (.. (array_push (%array 0) e0) ..) en   (byte
    //   vector) List  : List.Cons.{ e0, .. List.Cons.{ en, List.Nil } }
    //   (blessed List)
    auto &sl = std::get<EX::ExSeqLit>(expr->as);

    if (sl.is_array)
    {
      App   alloc0{ alloc(arena,
                        Term{ Var{ UT::strdup(arena, OP::ARR_ALLOC), 0 } }),
                  alloc(arena, Term{ Int{ 0 } }),
                  false };
      Term *acc = alloc(arena, Term{ alloc0 });
      for (size_t i = 0; i < sl.elems.size(); ++i)
      {
        Term *push
          = alloc(arena, Term{ Var{ UT::strdup(arena, OP::ARR_PUSH), 0 } });
        Term *pa = alloc(arena, Term{ App{ push, acc, false } });
        acc
          = alloc(arena, Term{ App{ pa, go(sl.elems[i], env, arena), false } });
      }
      return acc;
    }

    Variant nil;
    nil.type_name = UT::strdup(arena, "List");
    nil.tag       = UT::strdup(arena, "Nil");
    nil.fields    = UT::Vec<Term *>{ arena };
    Term *acc     = alloc(arena, Term{ nil });
    for (size_t i = sl.elems.size(); i-- > 0;)
    {
      Variant cons;
      cons.type_name = UT::strdup(arena, "List");
      cons.tag       = UT::strdup(arena, "Cons");
      cons.fields    = UT::Vec<Term *>{ arena };
      cons.fields.push(go(sl.elems[i], env, arena));
      cons.fields.push(acc);
      acc = alloc(arena, Term{ cons });
    }
    return acc;
  }

  case EX::ExprTag::Handle:
  {
    auto            &h    = std::get<EX::ExHandle>(expr->as);
    Term            *body = go(h.body, env, arena);
    UT::Vec<HClause> clauses{ arena };
    for (auto &c : h.clauses)
    {
      // \arg = \k = <clause body> -- the curried Fun nesting gives `arg` and
      // `k` their binders, which assign_id / closure conversion then handle.
      Term *cb    = go(c.body, env, arena);
      Term *inner = alloc(arena, Term{ Fun{ UT::strdup(arena, h.k), cb } });
      Term *outer
        = alloc(arena, Term{ Fun{ UT::strdup(arena, c.arg), inner } });
      clauses.push(HClause{ UT::strdup(arena, c.op), outer });
    }
    Term *els;
    if (h.else_body)
      els = alloc(arena,
                  Term{ Fun{ UT::strdup(arena, h.else_var),
                             go(h.else_body, env, arena) } });
    else
    {
      // No else clause: the value clause is the identity `\x = x`.
      UT::Vu x = UT::strdup(arena, "%x");
      els = alloc(arena, Term{ Fun{ x, alloc(arena, Term{ Var{ x, 0 } }) } });
    }
    return alloc(arena, Term{ Handle{ body, clauses, els } });
  }

  // A struct / union / alias *declaration* is a type-level form with no runtime
  // value.
  case EX::ExprTag::StructDecl:
  case EX::ExprTag::UnionDecl:
  case EX::ExprTag::AliasDecl:
  case EX::ExprTag::EffectDecl:
  case EX::ExprTag::Unknown   : return alloc(arena, Term{ Unk{} });

  case EX::ExprTag::Overload:
  {
    // TC resolved the overload set, writing the chosen mangled global to
    // `chosen`; from here it is an ordinary variable reference.
    auto &ov = std::get<EX::ExOverload>(expr->as);
    UT_FAIL_IF(!ov.chosen.data()); // TC leaves it set, or fails before Core
    return alloc(arena, Term{ Var{ UT::strdup(arena, ov.chosen), 0 } });
  }

  case EX::ExprTag::Extern:
    UT_FAIL_MSG("%s", "@extern is only valid as a global definition");
    break;
  case EX::ExprTag::Match:
    UT_FAIL_MSG("%s", "match should have been lowered by TC");
    break;
  case EX::ExprTag::ModDecl:
  case EX::ExprTag::Import:
  case EX::ExprTag::Vis:
    UT_FAIL_MSG("%s", "module directive should have been removed by MR");
    break;
  }

  UT_FAIL_MSG("%s", "unreachable: unhandled EX::ExprTag in CR::build");
  return alloc(arena, Term{ Unk{} });
}

} // namespace

Term *
build(
  EX::Expr *expr, StatEnv &env, AR::Arena &arena)
{
  return finalize(go(expr, env, arena), arena);
}

void
assign_id(
  Term *node, std::vector<UT::Vu> &names)
{
  UT_FAIL_IF(!node);

  switch (kind(node))
  {
  case Kind::Var:
  {
    auto &v = std::get<Var>(node->as);
    for (size_t i = names.size(); i > 0; --i)
    {
      if (names[i - 1] == v.name)
      {
        v.idx = names.size() - (i - 1);
        break;
      }
    }
  }
  break;

  case Kind::Fun:
  {
    auto &v = std::get<Fun>(node->as);
    names.push_back(v.param);
    assign_id(v.body, names);
    names.pop_back();
  }
  break;

  case Kind::Let:
  {
    // the bound name is in scope for both the value (recursion) and the body
    auto &v = std::get<Let>(node->as);
    names.push_back(v.var);
    assign_id(v.val, names);
    assign_id(v.body, names);
    names.pop_back();
  }
  break;

  case Kind::App:
  {
    auto &v = std::get<App>(node->as);
    assign_id(v.fn, names);
    assign_id(v.arg, names);
  }
  break;

  case Kind::Case:
  {
    auto &v = std::get<Case>(node->as);
    assign_id(v.scrut, names);
    for (Alt &alt : v.alts)
    {
      for (UT::Vu b : alt.binders) names.push_back(b);
      assign_id(alt.body, names);
      for (size_t i = 0; i < alt.binders.size(); ++i) names.pop_back();
    }
    assign_id(v.deflt, names);
  }
  break;

  case Kind::Struct:
  {
    auto &v = std::get<Struct>(node->as);
    for (FieldInit &f : v.fields) assign_id(f.val, names);
  }
  break;

  case Kind::Field: assign_id(std::get<Field>(node->as).record, names); break;

  case Kind::Variant:
  {
    // A constructor binds nothing; index each payload field in place (it is
    // suspended into a thunk capturing this same scope at eval time).
    auto &v = std::get<Variant>(node->as);
    for (Term *f : v.fields) assign_id(f, names);
  }
  break;

  case Kind::Handle:
  {
    // body, each clause `fn` (a Fun, which pushes its own binders), and the
    // value clause `els` (also a Fun) are all indexed in this same scope.
    auto &h = std::get<Handle>(node->as);
    assign_id(h.body, names);
    for (HClause &c : h.clauses) assign_id(c.fn, names);
    assign_id(h.els, names);
  }
  break;

  case Kind::Int:
  case Kind::Real:
  case Kind::Str:
  case Kind::Extern:
  case Kind::Unk   : break;
  }
}

std::string
pprint(
  Term *t, int level)
{
  if (!t) return std::string(level * 2, ' ') + "(null)";

  std::string pad(level * 2, ' ');
  switch (kind(t))
  {
  case Kind::Int : return pad + std::to_string(std::get<Int>(t->as).val);
  case Kind::Real: return pad + std::to_string(std::get<Real>(t->as).val);
  case Kind::Str:
    return pad + "\"" + std::string(std::get<Str>(t->as).val) + "\"";
  case Kind::Var:
  {
    auto       &v = std::get<Var>(t->as);
    std::string s = pad + std::string(v.name);
    if (v.idx != (size_t)-1) s += "[" + std::to_string(v.idx) + "]";
    return s;
  }
  case Kind::App:
  {
    auto &v = std::get<App>(t->as);
    return pad + "(app\n" + pprint(v.fn, level + 1) + "\n"
           + pprint(v.arg, level + 1) + ")";
  }
  case Kind::Fun:
  {
    auto &v = std::get<Fun>(t->as);
    return pad + "(fn " + std::string(v.param) + "\n"
           + pprint(v.body, level + 1) + ")";
  }
  case Kind::Let:
  {
    auto &v = std::get<Let>(t->as);
    return pad + "(let " + std::string(v.var) + "\n" + pprint(v.val, level + 1)
           + "\n" + pprint(v.body, level + 1) + ")";
  }
  case Kind::Extern:
  {
    auto &v = std::get<Extern>(t->as);
    return pad + "@extern(" + std::string(v.symbol) + " in "
           + std::string(v.lib) + ")";
  }
  case Kind::Case:
  {
    auto       &v    = std::get<Case>(t->as);
    std::string pad1 = std::string((level + 1) * 2, ' ');
    std::string s    = pad + "(case\n" + pprint(v.scrut, level + 1);
    for (const Alt &alt : v.alts)
      s += "\n" + pad1 + "alt ->\n" + pprint(alt.body, level + 2);
    return s + "\n" + pad1 + "else ->\n" + pprint(v.deflt, level + 2) + ")";
  }
  case Kind::Handle:
  {
    auto       &v    = std::get<Handle>(t->as);
    std::string pad1 = std::string((level + 1) * 2, ' ');
    std::string s    = pad + "(handle\n" + pprint(v.body, level + 1);
    for (const HClause &c : v.clauses)
      s += "\n" + pad1 + "is " + std::string(c.op) + " ->\n"
           + pprint(c.fn, level + 2);
    return s + "\n" + pad1 + "else ->\n" + pprint(v.els, level + 2) + ")";
  }
  case Kind::Struct:
  {
    auto       &v = std::get<Struct>(t->as);
    std::string s = pad + std::string(v.name) + ".{";
    for (const FieldInit &f : v.fields)
      s += "\n" + std::string((level + 1) * 2, ' ') + std::string(f.name)
           + " =\n" + pprint(f.val, level + 2);
    return s + ")";
  }
  case Kind::Field:
  {
    auto &v = std::get<Field>(t->as);
    return pad + "(field " + std::string(v.name) + "\n"
           + pprint(v.record, level + 1) + ")";
  }
  case Kind::Variant:
  {
    auto       &v = std::get<Variant>(t->as);
    std::string s
      = pad + std::string(v.type_name) + "." + std::string(v.tag) + ".{";
    for (Term *f : v.fields) s += "\n" + pprint(f, level + 1);
    return s + ")";
  }
  case Kind::Unk: return pad + "?unknown";
  }
  return pad + "?unreachable";
}

} // namespace CR
