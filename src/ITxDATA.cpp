#include "ITxDATA.hpp"
#include "FF.hpp"

namespace IT
{

ssize_t
as_int(
  const pVal &v)
{
  return std::get<VInt>(v->as).val;
}
double
as_num(
  const pVal &v)
{
  return VKind::Real == kind(v) ? std::get<VReal>(v->as).val
                                : (double)std::get<VInt>(v->as).val;
}
pVal
mk_int(
  ssize_t v)
{
  return mk(Value{ VInt{ v } });
}
pVal
mk_real(
  double v)
{
  return mk(Value{ VReal{ v } });
}
pVal
mk_bytes(
  size_t n)
{
  return mk(Value{ VStr{ std::string(n, '\0') } });
}

pVal
call_extern(
  const IR::Extern &e, const std::vector<pVal> &args)
{
  // The marshalling type names, minus any unit `{}` slots: a `{}` parameter is
  // "no argument" (lets a nullary C function be written `{} -> Int`), so it
  // contributes neither a type nor a word to the C call.
  std::vector<std::string> arg_types;
  std::vector<ssize_t>     words;
  arg_types.reserve(e.arg_types.size());
  words.reserve(args.size());

  UT_FAIL_IF(args.size() != e.arg_types.size());
  for (size_t i = 0; i < args.size(); ++i)
  {
    std::string ty(e.arg_types[i]);
    const pVal &a = args[i];
    UT_FAIL_IF(!a);
    if (ty == OP::TY_UNIT) continue; // `{}` arg: skipped, not passed

    arg_types.push_back(ty);
    if (VKind::Int == kind(a))
      words.push_back(std::get<VInt>(a->as).val);
    else if (VKind::Real == kind(a))
    {
      double   d = std::get<VReal>(a->as).val;
      ssize_t  w;
      std::memcpy(&w, &d, sizeof(w)); // FF reads Real words as double bits
      words.push_back(w);
    }
    else if (VKind::Str == kind(a))
      words.push_back((ssize_t)(intptr_t)std::get<VStr>(a->as).val.c_str());
    else
      UT_FAIL_MSG("FFI argument to '%s' must be Int, Real or Str",
                  std::string(e.symbol).c_str());
  }

  std::string ret_type(e.ret_type);
  ssize_t     r = FF::call(e.lib, e.symbol, arg_types, ret_type, words);

  if ("Str" == ret_type)
  {
    const char *p = (const char *)(intptr_t)r;
    return mk(Value{ VStr{ p ? std::string(p) : std::string() } });
  }
  if ("Real" == ret_type || "Real32" == ret_type)
  {
    double d;
    std::memcpy(&d, &r, sizeof(d));
    return mk_real(d);
  }
  if (ret_type == OP::TY_UNIT) return mk(Value{ VUnk{} });
  return mk_int(r);
}

std::string
pprint(
  const pVal &v, int level)
{
  std::string pad(level * 2, ' ');
  if (!v) return pad + "(null)";

  switch (kind(v))
  {
  case VKind::Int : return pad + std::to_string(std::get<VInt>(v->as).val);
  case VKind::Real: return pad + std::to_string(std::get<VReal>(v->as).val);
  case VKind::Str : return pad + "\"" + std::get<VStr>(v->as).val + "\"";
  case VKind::Builtin:
  {
    auto       &b = std::get<VBuiltin>(v->as);
    std::string s = pad + std::string(b.impl);
    for (const pVal &a : b.args) s += "\n" + pprint(a, level + 1);
    return s;
  }
  case VKind::Extern:
    return pad + "@extern(" + std::string(std::get<VExtern>(v->as).decl->symbol)
           + ")";
  case VKind::Struct:
  {
    auto       &s = std::get<VStruct>(v->as);
    std::string r = pad + std::string(s.name) + ".{";
    for (const auto &f : s.fields)
      r += "\n" + std::string((level + 1) * 2, ' ') + std::string(f.first)
           + " =\n" + pprint(f.second, level + 2);
    return r + ")";
  }
  case VKind::Variant:
  {
    auto       &vr = std::get<VVariant>(v->as);
    std::string r
      = pad + std::string(vr.type_name) + "." + std::string(vr.tag) + ".{";
    for (const pVal &f : vr.fields) r += "\n" + pprint(f, level + 1);
    return r + ")";
  }
  case VKind::Rec: return pad + "<rec>";
  case VKind::Code:
    return pad + "<code#" + std::to_string(std::get<VCode>(v->as).code) + ">";
  case VKind::Op: return pad + "<op " + std::get<VOp>(v->as).name + ">";
  case VKind::Resump : return pad + "<resumption>";
  case VKind::Finally: return pad + "<finally>";
  case VKind::Unk    : return pad + "?unknown";
  }
  return pad + "?unreachable";
}

} // namespace IT
