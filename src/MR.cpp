/*-------------------------------------------------------------------------------
 *\file MR.cpp
 *\info Module resolution ("link") layer -- see MR.hpp.
 *-----------------------------------------------------------------------------*/

#include "MR.hpp"
#include "ER.hpp"
#include "EXxDATA.hpp"
#include "UT.hpp"

#include <string>
#include <unordered_map>
#include <vector>

namespace MR
{
namespace
{

using EX::Expr;
using EX::ExprTag;

// A module name: an uppercase initial, then uppercase letters, digits, '_', and
// the inert lowercase 'x'. No other lowercase (which is what lets the `_impl`
// filename marker never collide with a real module name).
bool
is_module_name(
  UT::Vu s)
{
  if (s.empty()) return false;
  char c0 = s.data()[0];
  if (c0 < 'A' || c0 > 'Z') return false;
  for (size_t i = 1; i < s.size(); ++i)
  {
    char c  = s.data()[i];
    bool ok = (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_'
              || c == 'x';
    if (!ok) return false;
  }
  return true;
}

// One module-level term symbol: where it lives after mangling, whether
// importers may see it, and the EX::Def node (for the entry-point signature
// check).
struct SymInfo
{
  UT::Vu mangled;
  bool   is_public;
  Expr  *node;
};

// All fragments of one module, merged across files: its symbols and the imports
// every fragment requested.
struct ModuleData
{
  UT::Vu                                   name;
  std::unordered_map<std::string, SymInfo> symbols;
  std::vector<EX::ExImport>                imports;
};

// A module's name-resolution scope: the unqualified candidates a bare name may
// resolve to, and the prefixes usable for qualified `PFX.x` access.
struct Scope
{
  std::unordered_map<std::string, std::vector<UT::Vu>> unq;
  std::unordered_map<std::string, UT::Vu>              alias;
};

struct Linker
{
  AR::Arena                                  &arena;
  std::vector<MR::Unit>                      &units;
  std::unordered_map<std::string, ModuleData> modules;
  std::vector<Expr *>                         decls; // struct/union (global)
  std::vector<std::pair<UT::Vu, Expr *>>      defs;  // (module, Def node)
  std::vector<ER::Diagnostic>                 diags;

  Linker(
    AR::Arena &a, std::vector<MR::Unit> &u)
      : arena{ a },
        units{ u }
  {
  }

  // 1-based source line of `anchor`, found in whichever unit owns it (0 if
  // none).
  size_t
  line_of(
    UT::Vu anchor)
  {
    for (auto &u : units)
    {
      const char *b = u.content.data();
      if (anchor.data() && b && anchor.data() >= b
          && anchor.data() < b + u.content.size())
      {
        size_t off  = (size_t)(anchor.data() - b);
        size_t line = 1;
        for (size_t i = 0; i < off; ++i)
          if (b[i] == '\n') line += 1;
        return line;
      }
    }
    return 0;
  }

  void
  err(
    ER::Code code, UT::Vu anchor, const std::string &msg)
  {
    UT::Vu m = UT::strdup(arena, UT::Vu{ msg.data(), msg.size() });
    diags.push_back(ER::mk_root(arena, code, anchor, line_of(anchor), m));
  }

  // `MOD/name`, copied into the arena so it outlives this pass.
  UT::Vu
  mangle(
    UT::Vu mod, UT::Vu name)
  {
    std::string s;
    s.reserve(mod.size() + 1 + name.size());
    s.append(mod.data(), mod.size());
    s.push_back('/');
    s.append(name.data(), name.size());
    return UT::strdup(arena, UT::Vu{ s.data(), s.size() });
  }

  // The filename lint (Option B): the file must be `NAME.thx` or
  // `NAME_impl<Tag>.thx`, with a non-empty tag. Filenames never *resolve*
  // anything -- this is purely a consistency check against the `@mod` header.
  void
  check_filename(
    UT::Vu file, UT::Vu M)
  {
    std::string f(file);
    size_t      slash = f.find_last_of("/\\");
    std::string base  = (slash == std::string::npos) ? f : f.substr(slash + 1);
    size_t      dot   = base.find_last_of('.');
    std::string stem  = (dot == std::string::npos) ? base : base.substr(0, dot);
    std::string Ms(M);

    if (stem == Ms) return;
    std::string prefix = Ms + "_impl";
    if (stem.rfind(prefix, 0) == 0 && stem.size() > prefix.size()) return;

    err(ER::Code::FILENAME_MISMATCH,
        M,
        "file '" + base + "' does not match module '" + Ms + "': name it '" + Ms
          + ".thx' or '" + Ms + "_impl<Tag>.thx'");
  }

  // Pass 1: split each file into its module, mangle every term definition, and
  // gather symbols and imports. All symbols are collected before any reference
  // is resolved, so definitions may appear in any order.
  void
  collect()
  {
    for (auto &u : units)
    {
      if (u.exprs.size() == 0) continue;
      Expr *head = &u.exprs[0];
      if (head->tag != ExprTag::ModDecl)
      {
        err(ER::Code::EXPECTED_GLOBAL,
            {},
            "a source file must begin with a '@mod NAME' header");
        continue;
      }

      UT::Vu M = std::get<EX::ExModDecl>(head->as).name;
      if (!is_module_name(M))
        err(ER::Code::BAD_MODULE_NAME,
            M,
            "'" + std::string(M)
              + "' is not a valid module name (uppercase, digits, '_' and the "
                "letter 'x' only)");
      check_filename(u.filename, M);

      ModuleData &md = modules[std::string(M)];
      md.name        = M;

      bool priv = false; // visibility resets at the start of each file
      for (size_t i = 1; i < u.exprs.size(); ++i)
      {
        Expr *e = &u.exprs[i];
        switch (e->tag)
        {
        case ExprTag::Vis: priv = std::get<EX::ExVis>(e->as).is_private; break;
        case ExprTag::Import:
          md.imports.push_back(std::get<EX::ExImport>(e->as));
          break;
        case ExprTag::Def:
        {
          auto       &d    = std::get<EX::ExDef>(e->as);
          UT::Vu      orig = d.name;
          std::string key(orig);
          if (md.symbols.count(key))
          {
            err(ER::Code::DUPLICATE_SYMBOL,
                orig,
                "symbol '" + key + "' is already defined in module "
                  + std::string(M));
            break;
          }
          UT::Vu mg       = mangle(M, orig);
          md.symbols[key] = { mg, !priv, e };
          d.origin        = orig; // keep the source name for diagnostics
          d.name = mg; // the rest of the pipeline sees the mangled name
          defs.push_back({ M, e });
          break;
        }
        case ExprTag::StructDecl:
        case ExprTag::UnionDecl : decls.push_back(e); break;
        default                 : break; // nothing else is valid at top level
        }
      }
    }
  }

  void
  add_alias(
    Scope &sc, UT::Vu prefix, UT::Vu mod)
  {
    std::string k(prefix);
    auto        it = sc.alias.find(k);
    if (it != sc.alias.end() && std::string(it->second) != std::string(mod))
    {
      err(ER::Code::DUPLICATE_SYMBOL,
          prefix,
          "import prefix '" + k + "' is already bound to another module");
      return;
    }
    sc.alias[k] = mod;
  }

  // Pass 2a: turn a module's imports into a resolution scope, validating each.
  Scope
  build_scope(
    ModuleData &md)
  {
    Scope sc;
    // Own symbols (including private ones) are visible unqualified within the
    // module, and qualified resolution handles `Mkey.x` directly.
    for (auto &kv : md.symbols) sc.unq[kv.first].push_back(kv.second.mangled);

    for (auto &im : md.imports)
    {
      UT::Vu srcMod, srcSym;
      if (im.has_eq)
      {
        if (im.rhs_prefix.size())
        {
          srcMod = im.rhs_prefix;
          srcSym = im.rhs_name;
        }
        else
        {
          srcMod = im.rhs_name;
        }
      }
      else
      {
        srcMod = im.lhs_name;
      }

      std::string srcKey(srcMod);
      auto        mit = modules.find(srcKey);
      if (mit == modules.end())
      {
        err(
          ER::Code::UNKNOWN_MODULE, srcMod, "no module named '" + srcKey + "'");
        continue;
      }
      ModuleData &src = mit->second;

      if (srcSym.empty())
      {
        // Whole-module import.
        if (!im.has_eq)
        {
          // `with MOD` -- unqualified public symbols, plus `MOD.x`.
          for (auto &kv : src.symbols)
            if (kv.second.is_public)
              sc.unq[kv.first].push_back(kv.second.mangled);
          add_alias(sc, im.lhs_name, srcMod);
        }
        else
        {
          // `with ALIAS = MOD` / `with MOD = MOD` -- qualified only.
          if (im.lhs_prefix.size())
          {
            err(ER::Code::BAD_MODULE_NAME,
                im.lhs_name,
                "a module alias must be a bare name");
            continue;
          }
          add_alias(sc, im.lhs_name, srcMod);
        }
      }
      else
      {
        // Single-symbol import `... = MOD.sym`.
        std::string symKey(srcSym);
        auto        sit = src.symbols.find(symKey);
        if (sit == src.symbols.end())
        {
          err(ER::Code::UNKNOWN_SYMBOL,
              srcSym,
              "module '" + srcKey + "' has no symbol '" + symKey + "'");
          continue;
        }
        if (!sit->second.is_public)
        {
          err(ER::Code::PRIVATE_SYMBOL,
              srcSym,
              "symbol '" + symKey + "' is private to module '" + srcKey + "'");
          continue;
        }
        // `with name = MOD.sym` binds it unqualified; `with MOD.sym = MOD.sym`
        // is qualified-only (and `MOD.sym` already works), so just validate it.
        if (im.lhs_prefix.empty())
          sc.unq[std::string(im.lhs_name)].push_back(sit->second.mangled);
      }
    }
    return sc;
  }

  // Resolve a qualified `PFX.name` use to its mangled global, in place.
  void
  resolve_qualified(
    EX::ExVar &v, const std::string &Mkey, Scope &sc)
  {
    std::string PFX(v.qualifier);
    std::string nm(v.name);

    UT::Vu T;
    bool   ownAccess = false;
    auto   ait       = sc.alias.find(PFX);
    if (ait != sc.alias.end())
    {
      T = ait->second;
    }
    else if (PFX == Mkey)
    {
      T         = modules[Mkey].name;
      ownAccess = true;
    }
    else if (modules.count(PFX))
    {
      T = modules[PFX].name;
    }
    else
    {
      err(ER::Code::UNKNOWN_MODULE,
          v.qualifier,
          "no module named '" + PFX + "' in '" + PFX + "." + nm + "'");
      return;
    }

    ModuleData &tm  = modules[std::string(T)];
    auto        sit = tm.symbols.find(nm);
    if (sit == tm.symbols.end())
    {
      err(ER::Code::UNKNOWN_SYMBOL,
          v.name,
          "module '" + std::string(T) + "' has no symbol '" + nm + "'");
      return;
    }
    if (!ownAccess && !sit->second.is_public)
    {
      err(ER::Code::PRIVATE_SYMBOL,
          v.name,
          "symbol '" + nm + "' is private to module '" + std::string(T) + "'");
      return;
    }
    v.name      = sit->second.mangled;
    v.qualifier = UT::Vu{};
  }

  // Pass 2b: rewrite every variable reference in a body. Locals
  // (lambda/let/case binders) shadow module symbols and are left untouched; an
  // unqualified name not found in scope (operators, builtins, genuine typos) is
  // also left for TC.
  void
  resolve(
    Expr *e, const std::string &Mkey, Scope &sc, std::vector<UT::Vu> &locals)
  {
    switch (e->tag)
    {
    case ExprTag::Var:
    {
      auto &v = std::get<EX::ExVar>(e->as);
      if (v.qualifier.size())
      {
        resolve_qualified(v, Mkey, sc);
        return;
      }
      for (auto &l : locals)
        if (l == v.name) return; // a local binder shadows
      auto it = sc.unq.find(std::string(v.name));
      if (it == sc.unq.end()) return; // operator / builtin / unknown
      if (it->second.size() == 1)
      {
        v.name = it->second[0];
        return;
      }
      std::string msg
        = "'" + std::string(v.name) + "' is ambiguous; provided by";
      for (size_t i = 0; i < it->second.size(); ++i)
        msg += (i ? "," : "") + (" " + std::string(it->second[i]));
      msg += " -- qualify it (e.g. MOD." + std::string(v.name) + ")";
      err(ER::Code::AMBIGUOUS_NAME, v.name, msg);
      return;
    }
    case ExprTag::App:
    {
      auto &a = std::get<EX::ExApp>(e->as);
      resolve(a.fn, Mkey, sc, locals);
      resolve(a.arg, Mkey, sc, locals);
      return;
    }
    case ExprTag::FnDef:
    {
      auto &f = std::get<EX::ExFnDef>(e->as);
      locals.push_back(f.param);
      resolve(f.body, Mkey, sc, locals);
      locals.pop_back();
      return;
    }
    case ExprTag::Let:
    {
      auto &l = std::get<EX::ExLet>(e->as);
      locals.push_back(l.var); // in scope for the value too (recursive let)
      resolve(l.val, Mkey, sc, locals);
      resolve(l.body, Mkey, sc, locals);
      locals.pop_back();
      return;
    }
    case ExprTag::If:
    {
      auto &i = std::get<EX::ExIf>(e->as);
      resolve(i.cond, Mkey, sc, locals);
      resolve(i.then, Mkey, sc, locals);
      resolve(i.alt, Mkey, sc, locals);
      return;
    }
    case ExprTag::Case:
    {
      auto &c = std::get<EX::ExCase>(e->as);
      resolve(c.scrut, Mkey, sc, locals);
      for (size_t k = 0; k < c.alts.size(); ++k)
      {
        size_t base = locals.size();
        // Only a Con alt binds names; `binders` is unset for Int/Real alts.
        if (c.alts[k].kind == EX::AltKind::Con)
          for (size_t j = 0; j < c.alts[k].binders.size(); ++j)
            locals.push_back(c.alts[k].binders[j]);
        resolve(c.alts[k].body, Mkey, sc, locals);
        locals.resize(base);
      }
      resolve(c.deflt, Mkey, sc, locals);
      return;
    }
    case ExprTag::Field:
      resolve(std::get<EX::ExField>(e->as).record, Mkey, sc, locals);
      return;
    case ExprTag::StructLit:
    {
      auto &sl = std::get<EX::ExStructLit>(e->as);
      for (size_t k = 0; k < sl.fields.size(); ++k)
        resolve(sl.fields[k].val, Mkey, sc, locals);
      return;
    }
    case ExprTag::VariantLit:
    {
      auto &vl = std::get<EX::ExVariantLit>(e->as);
      for (size_t k = 0; k < vl.fields.size(); ++k)
        resolve(vl.fields[k].val, Mkey, sc, locals);
      return;
    }
    default: return; // Int / Real / Str / Extern / Unknown: nothing to resolve
    }
  }

  // The entry signature must be `Int` (a value) or `Str -> Int`.
  bool
  check_entry_sig(
    EX::Ty *sig, bool &takes_arg)
  {
    if (!sig) return false;
    if (sig->tag == EX::TyTag::Con)
    {
      auto &c = std::get<EX::TyCon>(sig->as);
      if (c.name == "Int" && c.args.size() == 0)
      {
        takes_arg = false;
        return true;
      }
      return false;
    }
    if (sig->tag == EX::TyTag::Arrow)
    {
      auto &a = std::get<EX::TyArrow>(sig->as);
      if (a.from->tag == EX::TyTag::Con && a.to->tag == EX::TyTag::Con)
      {
        auto &f = std::get<EX::TyCon>(a.from->as);
        auto &t = std::get<EX::TyCon>(a.to->as);
        if (f.name == "Str" && f.args.size() == 0 && t.name == "Int"
            && t.args.size() == 0)
        {
          takes_arg = true;
          return true;
        }
      }
      return false;
    }
    return false;
  }

  Result
  run()
  {
    collect();

    std::unordered_map<std::string, Scope> scopes;
    for (auto &kv : modules) scopes[kv.first] = build_scope(kv.second);

    for (auto &pr : defs)
    {
      Scope              &sc = scopes[std::string(pr.first)];
      auto               &d  = std::get<EX::ExDef>(pr.second->as);
      std::vector<UT::Vu> locals;
      resolve(d.def, std::string(pr.first), sc, locals);
    }

    Result r;
    r.program = EX::Exprs{ arena };
    for (auto *dnode : decls) r.program.push(*dnode);
    for (auto &pr : defs) r.program.push(*pr.second);

    // Entry point: module MAIN's `main`, with a supported signature. Absence of
    // an entry is not an error here (a library or test snippet need not have
    // one) -- it leaves `entry` empty and the driver decides whether to require
    // it. A MAIN/main that exists but has the wrong signature IS an error.
    auto mit = modules.find("MAIN");
    if (mit != modules.end())
    {
      auto sit = mit->second.symbols.find("main");
      if (sit != mit->second.symbols.end())
      {
        auto &d         = std::get<EX::ExDef>(sit->second.node->as);
        bool  takes_arg = false;
        if (!check_entry_sig(d.sig, takes_arg))
          err(ER::Code::ENTRY_SIGNATURE,
              mit->second.name,
              "'main' must be annotated ': Int' or ': Str -> Int'");
        else
        {
          r.entry           = sit->second.mangled;
          r.entry_takes_arg = takes_arg;
        }
      }
    }

    r.diags = std::move(diags);
    return r;
  }
};

} // namespace

Result
link(
  std::vector<Unit> &units, AR::Arena &arena)
{
  Linker l{ arena, units };
  return l.run();
}

} // namespace MR

/*-------------------------------------------------------------------------------
 *\EOF
 *-----------------------------------------------------------------------------*/
