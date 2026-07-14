#include "EXxDATA.hpp"
#include "EX.hpp"

namespace EX
{
const char *
tok_desc(
  const LX::Token &t)
{
  return LX::TokenTag::Eof == t.tag ? "end of input" : nullptr;
}

/*-------------------------------------------------------------------------------
 *\PPRINT
 *------------------------------------------------------------------------------*/

std::string
pprint(
  ExprTag t, int level)
{
  std::string pad(level * 2, ' ');
  switch (t)
  {
#define X(tag, type)                                                           \
  case ExprTag::tag: return pad + #tag;
    EX_EXPR_VARIANTS
#undef X
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

static std::string
pprint_ty(
  Ty *t)
{
  if (!t) return "?";
  switch (t->tag)
  {
  case TyTag::Con:
  {
    auto       &c   = std::get<TyCon>(t->as);
    std::string out = std::string(c.name);
    for (Ty *arg : c.args)
    {
      // Parenthesize an applied-con argument so `List (Maybe Int)` round-trips.
      bool nest = arg && TyTag::Con == arg->tag
                  && !std::get<TyCon>(arg->as).args.empty();
      out += " ";
      out += nest ? "(" + pprint_ty(arg) + ")" : pprint_ty(arg);
    }
    return out;
  }
  case TyTag::Var: return "`" + std::string(std::get<TyVar>(t->as).name);
  case TyTag::Arrow:
  {
    auto &ar = std::get<TyArrow>(t->as);
    return pprint_ty(ar.from) + " -> " + pprint_ty(ar.to);
  }
  }
  UT_FAIL_MSG("%s", "unhandled TyTag in pprint_ty");
  return "?";
}

std::string
pprint(
  Expr *e, int level)
{
  if (!e) return std::string(level * 2, ' ') + "(null)";

  std::string pad(level * 2, ' ');
  switch (e->tag)
  {
  case ExprTag::Int: return pad + std::to_string(std::get<ExInt>(e->as).value);
  case ExprTag::Real:
    return pad + std::to_string(std::get<ExReal>(e->as).value);
  case ExprTag::Var:
  {
    auto &v = std::get<ExVar>(e->as);
    return pad + (v.qualifier.size() ? std::string(v.qualifier) + "." : "")
           + std::string(v.name);
  }
  case ExprTag::Str:
    return pad + "\"" + std::string(std::get<ExStr>(e->as).value) + "\"";
  case ExprTag::Unit: return pad + "{}";
  case ExprTag::EffectDecl:
  {
    auto       &ed = std::get<ExEffectDecl>(e->as);
    std::string r  = pad + "$" + std::string(ed.name) + " : @effect =";
    for (auto &op : ed.ops)
      r += "\n" + std::string((level + 1) * 2, ' ') + std::string(op.name)
           + " : " + pprint_ty(op.ty);
    return r;
  }
  case ExprTag::Let:
  {
    auto       &lt     = std::get<ExLet>(e->as);
    std::string binder = lt.pat ? pprint(lt.pat) : std::string(lt.var);
    return pad + "let " + binder + " =\n" + pprint(lt.val, level + 1) + "\n"
           + pad + "in\n" + pprint(lt.body, level + 1);
  }
  case ExprTag::Handle:
  {
    auto       &h = std::get<ExHandle>(e->as);
    std::string r = pad + "do\n" + pprint(h.body, level + 1) + "\n" + pad
                    + "ctl " + std::string(h.k);
    std::string ipad((level + 1) * 2, ' ');
    for (auto &c : h.clauses)
      r += "\n" + ipad + "is " + std::string(c.op) + " " + std::string(c.arg)
           + " =\n" + pprint(c.body, level + 2);
    if (h.else_body)
      r += "\n" + ipad + "else " + std::string(h.else_var) + " =\n"
           + pprint(h.else_body, level + 2);
    return r;
  }
  case ExprTag::If:
    return pad + "if " + pprint(std::get<ExIf>(e->as).cond, 0) + " then\n"
           + pprint(std::get<ExIf>(e->as).then, level + 1) + "\n" + pad
           + "else\n" + pprint(std::get<ExIf>(e->as).alt, level + 1);
  case ExprTag::Match:
  {
    auto       &m = std::get<ExMatch>(e->as);
    std::string s = pad + "when " + pprint(m.scrut, 0);
    for (size_t i = 0; i < m.arms.size(); ++i)
    {
      s += "\n" + pad + "is " + pprint(m.arms[i].pat);
      if (m.arms[i].guard) s += " if " + pprint(m.arms[i].guard, 0);
      s += " then\n" + pprint(m.arms[i].body, level + 1);
    }
    return s + "\n" + pad + "else\n" + pprint(m.alt, level + 1);
  }
  case ExprTag::Case:
  {
    auto       &cs = std::get<ExCase>(e->as);
    std::string s  = pad + "case " + pprint(cs.scrut, 0);
    for (size_t i = 0; i < cs.alts.size(); ++i)
      s += "\n" + pad + "alt ->\n" + pprint(cs.alts[i].body, level + 1);
    return s + "\n" + pad + "else\n" + pprint(cs.deflt, level + 1);
  }
  case ExprTag::FnDef:
  {
    auto       &fn = std::get<ExFnDef>(e->as);
    std::string binder
      = fn.param_pat ? pprint(fn.param_pat) : std::string(fn.param);
    return pad + "\\" + binder + " =\n" + pprint(fn.body, level + 1);
  }
  case ExprTag::App:
  {
    auto &app = std::get<ExApp>(e->as);
    return pad + "(app\n" + pprint(app.fn, level + 1) + "\n"
           + pprint(app.arg, level + 1) + ")";
  }
  case ExprTag::Def:
    return pad + "$" + std::string(std::get<ExDef>(e->as).name) + " =\n"
           + pprint(std::get<ExDef>(e->as).def, level + 1);
  case ExprTag::Extern:
    return pad + "@extern.{\"" + std::string(std::get<ExExtern>(e->as).symbol)
           + "\", \"" + std::string(std::get<ExExtern>(e->as).lib) + "\"}";
  case ExprTag::StructDecl:
  {
    auto       &sd = std::get<ExStructDecl>(e->as);
    std::string s  = pad + "$" + std::string(sd.name) + " : @struct";
    for (size_t i = 0; i < sd.fields.size(); ++i)
      s += "\n" + std::string((level + 1) * 2, ' ')
           + std::string(sd.fields[i].name) + " : "
           + pprint_ty(sd.fields[i].ty);
    return s;
  }
  case ExprTag::StructLit:
  {
    auto       &sl = std::get<ExStructLit>(e->as);
    std::string s  = pad + std::string(sl.type_name) + ".{";
    for (size_t i = 0; i < sl.fields.size(); ++i)
      s += "\n" + std::string((level + 1) * 2, ' ') + "."
           + std::string(sl.fields[i].name) + " =\n"
           + pprint(sl.fields[i].val, level + 2);
    return s + ")";
  }
  case ExprTag::Field:
  {
    auto &fa = std::get<ExField>(e->as);
    return pad + "(field " + std::string(fa.field) + "\n"
           + pprint(fa.record, level + 1) + ")";
  }
  case ExprTag::UnionDecl:
  {
    auto       &ud = std::get<ExUnionDecl>(e->as);
    std::string s  = pad + "$" + std::string(ud.name) + " : @union";
    for (size_t i = 0; i < ud.variants.size(); ++i)
    {
      s += "\n" + std::string((level + 1) * 2, ' ')
           + std::string(ud.variants[i].tag) + ": {";
      auto &fs = ud.variants[i].fields;
      for (size_t k = 0; k < fs.size(); ++k)
        s += (k ? ", " : "")
             + (fs[k].name.size() ? std::string(fs[k].name) + ": " : "")
             + pprint_ty(fs[k].ty);
      s += "}";
    }
    return s;
  }
  case ExprTag::AliasDecl:
  {
    auto &ad = std::get<ExAliasDecl>(e->as);
    return pad + "$" + std::string(ad.name)
           + " : @alias = " + pprint_ty(ad.target);
  }
  case ExprTag::VariantLit:
  {
    auto       &vl = std::get<ExVariantLit>(e->as);
    std::string s
      = pad + std::string(vl.type_name) + "." + std::string(vl.tag) + ".{";
    for (size_t i = 0; i < vl.fields.size(); ++i)
      s += "\n" + std::string((level + 1) * 2, ' ')
           + (vl.fields[i].name.size()
                ? "." + std::string(vl.fields[i].name) + " =\n"
                : "\n")
           + pprint(vl.fields[i].val, level + 2);
    return s + ")";
  }
  case ExprTag::SeqLit:
  {
    auto       &sl = std::get<ExSeqLit>(e->as);
    std::string s  = pad + (sl.is_array ? "@[" : "[");
    for (size_t i = 0; i < sl.elems.size(); ++i)
      s += "\n" + pprint(sl.elems[i], level + 1);
    return s + "]";
  }
  case ExprTag::ModDecl:
    return pad + "@mod " + std::string(std::get<ExModDecl>(e->as).name);
  case ExprTag::Import:
  {
    auto       &im = std::get<ExImport>(e->as);
    std::string s  = pad + "with ";
    if (im.lhs_prefix.size()) s += std::string(im.lhs_prefix) + ".";
    s += std::string(im.lhs_name);
    if (im.has_eq)
    {
      s += " = ";
      if (im.rhs_prefix.size()) s += std::string(im.rhs_prefix) + ".";
      s += std::string(im.rhs_name);
    }
    return s;
  }
  case ExprTag::Vis:
    return pad + (std::get<ExVis>(e->as).is_private ? "@private" : "@public");
  case ExprTag::Overload:
  {
    auto       &ov = std::get<ExOverload>(e->as);
    std::string s  = pad + "overload " + std::string(ov.name) + " {";
    for (size_t i = 0; i < ov.candidates.size(); ++i)
      s += (i ? ", " : "") + std::string(ov.candidates[i]);
    return s + "}";
  }
  case ExprTag::Unknown: return pad + "?unknown";
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

std::string
pprint(
  Pattern *p)
{
  if (!p) return "(null-pat)";
  switch (p->tag)
  {
  case PatTag::Wild: return "_";
  case PatTag::Var : return std::string(std::get<PatVar>(p->as).name);
  case PatTag::Int : return std::to_string(std::get<PatInt>(p->as).value);
  case PatTag::Real: return std::to_string(std::get<PatReal>(p->as).value);
  case PatTag::Str:
    return "\"" + std::string(std::get<PatStr>(p->as).value) + "\"";
  case PatTag::StrPrefix:
  {
    auto &pp = std::get<PatStrPrefix>(p->as);
    return "\"" + std::string(pp.prefix) + "\" ++ " + pprint(pp.rest);
  }
  case PatTag::Struct:
  {
    auto       &ps = std::get<PatStruct>(p->as);
    std::string s  = std::string(ps.type_name) + ".{";
    for (size_t i = 0; i < ps.fields.size(); ++i)
    {
      if (i) s += ", ";
      if (ps.fields[i].name.size())
        s += "." + std::string(ps.fields[i].name) + " = ";
      s += pprint(ps.fields[i].pat);
    }
    return s + "}";
  }
  case PatTag::Variant:
  {
    auto       &pv = std::get<PatVariant>(p->as);
    std::string s
      = std::string(pv.type_name) + "." + std::string(pv.tag) + ".{";
    for (size_t i = 0; i < pv.fields.size(); ++i)
    {
      if (i) s += ", ";
      if (pv.fields[i].name.size())
        s += "." + std::string(pv.fields[i].name) + " = ";
      s += pprint(pv.fields[i].pat);
    }
    return s + "}";
  }
  }
  UT_FAIL_IF("UNREACHABLE");
  return "";
}

/*-------------------------------------------------------------------------------
 *\EXPR CTORS
 *------------------------------------------------------------------------------*/

ExFnDef::ExFnDef(
  UT::Vu param, AR::Arena &arena)
    : param{ param }
{
  this->body = (EX::Expr *)arena.alloc<EX::Expr>(1);
}

Expr::Expr(ExprTag tag)
    : tag{ tag } {};

Expr::Expr(
  ExprTag tag, AR::Arena &arena)
    : tag{ tag }
{
  switch (tag)
  {
  case ExprTag::FnDef:
  {
    ExFnDef fndef;
    fndef.body = (EX::Expr *)arena.alloc<EX::Expr>(1);
    this->as   = fndef;
  }
  break;
  case ExprTag::If:
  {
    this->as = ExIf{ (Expr *)arena.alloc<Expr>(1),
                     (Expr *)arena.alloc<Expr>(1),
                     (Expr *)arena.alloc<Expr>(1) };
  }
  break;
  default: UT_FAIL_IF("Invalid tag for this constructor");
  }
};

} // namespace EX
