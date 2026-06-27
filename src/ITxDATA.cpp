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
  const CR::Extern &e, const std::vector<pVal> &args)
{
  std::vector<std::string> arg_types;
  arg_types.reserve(e.arg_types.size());
  for (UT::Vu t : e.arg_types) arg_types.push_back(std::string(t));
  std::string ret_type(e.ret_type);

  std::vector<ssize_t> words;
  words.reserve(args.size());
  for (const pVal &a : args)
  {
    UT_FAIL_IF(!a);
    if (VKind::Int == kind(a))
    {
      words.push_back(std::get<VInt>(a->as).val);
    }
    else if (VKind::Str == kind(a))
    {
      const std::string &s = std::get<VStr>(a->as).val;
      words.push_back((ssize_t)(intptr_t)s.c_str());
    }
    else
    {
      UT_FAIL_MSG("FFI argument to '%s' must be Int or Str",
                  std::string(e.symbol).c_str());
    }
  }

  ssize_t r = FF::call(e.lib, e.symbol, arg_types, ret_type, words);

  if ("Str" == ret_type)
  {
    const char *p = (const char *)(intptr_t)r;
    return mk(Value{ VStr{ p ? std::string(p) : std::string() } });
  }
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
  case VKind::Resump: return pad + "<resumption>";
  case VKind::Unk: return pad + "?unknown";
  }
  return pad + "?unreachable";
}

} // namespace IT
