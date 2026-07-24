/*-------------------------------------------------------------------------------
 *\file MR.cpp
 *\info Module resolution ("link") layer -- see MR.hpp.
 *-----------------------------------------------------------------------------*/

#include "MR.hpp"
#include "ER.hpp"
#include "EXxDATA.hpp"
#include "UT.hpp"

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

// One module-level TYPE symbol (a struct/union/alias/effect name). `resolved`
// is its program-wide identity -- `MOD/Name` for a user module, or the bare
// source name for the exempt prelude/`C` modules whose types stay global
// (`List`, `Int`, ...). `is_effect` marks an effect name, which also owns
// operations.
struct TypeSym
{
  UT::Vu resolved;
  bool   is_public;
  bool   is_effect;
};

// All fragments of one module, merged across files: its symbols and the imports
// every fragment requested. A name maps to a *list* of definitions: a module
// may overload a term name, giving each definition a distinct mangled global
// and letting the type checker pick the one that fits each use.
struct ModuleData
{
  UT::Vu                                                name;
  std::unordered_map<std::string, std::vector<SymInfo>> symbols;
  std::unordered_map<std::string, TypeSym>              types;
  std::vector<EX::ExImport>                             imports;
};

// A module's name-resolution scope: the unqualified candidates a bare name may
// resolve to, the prefixes usable for qualified `PFX.x` access, and the visible
// type/effect names (bare source name -> program-wide `resolved` identity) used
// to rewrite every type reference to its owning module.
struct Scope
{
  std::unordered_map<std::string, std::vector<UT::Vu>> unq;
  std::unordered_map<std::string, UT::Vu>              alias;
  std::unordered_map<std::string, UT::Vu>              tvis;
  std::unordered_map<std::string, std::vector<UT::Vu>> tamb;
};

using Locals = std::vector<UT::Vu>;

// Append every variable a pattern binds (in scope for its arm/body). Literal
// and wildcard patterns bind nothing; nested struct/variant/prefix patterns
// recurse.
void
collect_pat_binders(
  EX::Pattern *p, Locals &out)
{
  switch (p->tag)
  {
  case EX::PatTag::Wild:
  case EX::PatTag::Int:
  case EX::PatTag::Bool:
  case EX::PatTag::Real:
  case EX::PatTag::Str : return;
  case EX::PatTag::Var : out.push_back(std::get<EX::PatVar>(p->as).name); return;
  case EX::PatTag::StrPrefix:
    collect_pat_binders(std::get<EX::PatStrPrefix>(p->as).rest, out);
    return;
  case EX::PatTag::Struct:
    for (EX::FieldPat &f : std::get<EX::PatStruct>(p->as).fields)
      collect_pat_binders(f.pat, out);
    return;
  case EX::PatTag::Variant:
    for (EX::FieldPat &f : std::get<EX::PatVariant>(p->as).fields)
      collect_pat_binders(f.pat, out);
    return;
  case EX::PatTag::Seq:
  {
    auto &pq = std::get<EX::PatSeq>(p->as);
    for (size_t i = 0; i < pq.elems.size(); ++i)
      collect_pat_binders(pq.elems[i], out);
    if (pq.rest) collect_pat_binders(pq.rest, out);
    return;
  }
  }
  UT_FAIL_MSG("%s", "collect_pat_binders: unhandled PatTag");
}

struct Linker
{
  AR::Arena                                  &arena;
  std::vector<MR::Unit>                      &units;
  std::unordered_map<std::string, ModuleData> modules;
  std::vector<std::pair<UT::Vu, Expr *>>      decls; // (module, type decl)
  std::vector<std::pair<UT::Vu, Expr *>>      defs;  // (module, Def node)
  std::vector<ER::Diagnostic>                 diags;
  std::unordered_map<std::string, std::vector<std::string>> edges;
  std::string                                               m_current;

  // Declared effects: RESOLVED effect name (`MOD/Eff`) -> (operation source
  // name
  // -> its canonical `MOD/Eff.op` identity). Keyed by the program-wide resolved
  // name, so same-named effects in different modules stay distinct. Used to
  // resolve an effect-qualified `Eff.op` and a handler clause's operation (the
  // bare `Eff` is mapped to its resolved name through the module's `tvis`).
  std::unordered_map<std::string, std::unordered_map<std::string, UT::Vu>>
    effects;

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

  // `MOD/name`, copied into the arena so it outlives this pass. An overloaded
  // name's second and later definitions get a `#k` suffix (`MOD/name#1`, ...)
  // so every definition has a unique global; the '#' cannot occur in a source
  // identifier, so these never collide with a real name.
  UT::Vu
  mangle(
    UT::Vu mod, UT::Vu name, size_t dup = 0)
  {
    std::string s;
    s.reserve(mod.size() + 1 + name.size() + 4);
    s.append(mod.data(), mod.size());
    s.push_back('/');
    s.append(name.data(), name.size());
    if (dup) s += "#" + std::to_string(dup);
    return UT::strdup(arena, UT::Vu{ s.data(), s.size() });
  }

  // `MOD/Eff.op`, the canonical program-wide identity of an operation, copied
  // into the arena. `eff` is the effect's already-resolved name; the '.'
  // separator cannot occur in a mangled global (`MOD/name`), so an operation
  // identity never collides with one.
  UT::Vu
  op_identity(
    UT::Vu eff, UT::Vu op)
  {
    std::string s;
    s.reserve(eff.size() + 1 + op.size());
    s.append(eff.data(), eff.size());
    s.push_back('.');
    s.append(op.data(), op.size());
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

  // The prelude and the `C` libc namespace own the built-in-ish global types
  // (`List`, the numeric aliases, ...) whose names are hard-wired into the rest
  // of the pipeline (CR's `List`/`Cons`/`Nil`, TC's alias table, ...). Their
  // type names stay bare and program-global; every other module's types are
  // namespaced to `MOD/Name`.
  static bool
  type_exempt(
    UT::Vu M)
  {
    return M == "PRELUDE" || M == "C";
  }

  // Register a type/effect declaration under its module: rewrite its name to
  // the program-wide `resolved` identity (mangled for a user module, bare for
  // an exempt one), keep the source name as `origin` for diagnostics, and
  // record it in `md.types`. Returns the resolved name.
  UT::Vu
  register_type(
    ModuleData &md, UT::Vu &name, UT::Vu &origin, bool priv, bool is_effect)
  {
    origin               = name;
    UT::Vu      resolved = type_exempt(md.name) ? name : mangle(md.name, name);
    std::string key(name);
    if (md.types.count(key))
      err(ER::Code::DUPLICATE_SYMBOL,
          name,
          "type '" + key + "' is declared more than once in module '"
            + std::string(md.name) + "'");
    md.types[key] = { resolved, !priv, is_effect };
    name          = resolved;
    return resolved;
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
          // A repeated name is an overload, not an error: each definition gets
          // its own mangled global (the first keeps the plain `MOD/name`).
          auto  &cands = md.symbols[key];
          UT::Vu mg    = mangle(M, orig, cands.size());
          cands.push_back({ mg, !priv, e });
          d.origin = orig; // keep the source name for diagnostics
          d.name   = mg;   // the rest of the pipeline sees the mangled name
          defs.push_back({ M, e });
          break;
        }
        case ExprTag::EffectDecl:
        {
          // Namespace the effect (`MOD/Eff`), then register each operation as a
          // module symbol whose identity is the canonical `MOD/Eff.op`: a bare
          // op resolves through the ordinary scope/overload path and same-named
          // ops from different effects get distinct identities. Also record the
          // effect in the program-wide table (keyed by the resolved name) for
          // qualified `Eff.op` and clause-op resolution.
          auto  &ed       = std::get<EX::ExEffectDecl>(e->as);
          UT::Vu resolved = register_type(md, ed.name, ed.origin, priv, true);
          auto  &opmap    = effects[std::string(resolved)];
          for (auto &op : ed.ops)
          {
            UT::Vu id                   = op_identity(resolved, op.name);
            opmap[std::string(op.name)] = id;
            md.symbols[std::string(op.name)].push_back({ id, !priv, e });
          }
          decls.push_back({ M, e });
          break;
        }
        // Per-module types: a struct/union/alias name is namespaced to
        // `MOD/Name` (except the exempt prelude/`C` globals) and every
        // reference
        // -- `Ty::Con` in signatures, constructor/field literals, and variant
        // patterns -- is rewritten to it in the type-resolution pass below.
        case ExprTag::StructDecl:
        {
          auto &sd = std::get<EX::ExStructDecl>(e->as);
          register_type(md, sd.name, sd.origin, priv, false);
          decls.push_back({ M, e });
          break;
        }
        case ExprTag::UnionDecl:
        {
          auto &ud = std::get<EX::ExUnionDecl>(e->as);
          register_type(md, ud.name, ud.origin, priv, false);
          decls.push_back({ M, e });
          break;
        }
        case ExprTag::AliasDecl:
        {
          auto &ad = std::get<EX::ExAliasDecl>(e->as);
          register_type(md, ad.name, ad.origin, priv, false);
          decls.push_back({ M, e });
          break;
        }
        default: break; // nothing else is valid at top level
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
    // module, and qualified resolution handles `Mkey.x` directly. An overloaded
    // name contributes every one of its definitions.
    for (auto &kv : md.symbols)
      for (auto &si : kv.second) sc.unq[kv.first].push_back(si.mangled);

    // Own types (including private ones) are visible unqualified by their bare
    // source name; a reference is rewritten to the resolved identity. An own
    // type shadows any imported one of the same name (which stays reachable
    // qualified `A.Type`).
    std::unordered_set<std::string> own_ty;
    for (auto &kv : md.types)
    {
      sc.tvis[kv.first] = kv.second.resolved;
      own_ty.insert(kv.first);
    }

    // Import a module's public types. A name already bound to a DIFFERENT type
    // (imported from another module) is ambiguous: record every distinct
    // candidate in `tamb` so an unqualified use is rejected with a suggestion
    // to qualify. Re-importing the same type (e.g. via two paths) is not a
    // clash.
    auto import_types = [&](ModuleData &src) {
      for (auto &kv : src.types)
      {
        if (!kv.second.is_public || own_ty.count(kv.first)) continue;
        UT::Vu r  = kv.second.resolved;
        auto   it = sc.tvis.find(kv.first);
        if (it == sc.tvis.end())
        {
          sc.tvis[kv.first] = r;
          continue;
        }
        if (std::string(it->second) == std::string(r)) continue;
        auto &amb = sc.tamb[kv.first];
        if (amb.empty()) amb.push_back(it->second);
        bool seen = false;
        for (UT::Vu c : amb)
          if (std::string(c) == std::string(r)) seen = true;
        if (!seen) amb.push_back(r);
      }
    };

    auto pit = modules.find("PRELUDE");
    if (pit != modules.end() && &pit->second != &md)
    {
      for (auto &kv : pit->second.symbols)
        for (auto &si : kv.second)
          if (si.is_public) sc.unq[kv.first].push_back(si.mangled);
      // The prelude's global types (`List`, the numeric aliases, ...) are in
      // scope everywhere, unqualified.
      import_types(pit->second);
    }

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
          // `with MOD` -- unqualified public symbols and types, plus `MOD.x`.
          for (auto &kv : src.symbols)
            for (auto &si : kv.second)
              if (si.is_public) sc.unq[kv.first].push_back(si.mangled);
          import_types(src);
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
        // An overloaded name imports every public definition (one or many).
        bool any_public = false;
        for (auto &si : sit->second)
          if (si.is_public)
          {
            any_public = true;
            // `with name = MOD.sym` binds it unqualified; `with MOD.sym =
            // MOD.sym` is qualified-only (and `MOD.sym` already works), so for
            // that form there is nothing to add -- just validate visibility.
            if (im.lhs_prefix.empty())
              sc.unq[std::string(im.lhs_name)].push_back(si.mangled);
          }
        if (!any_public)
        {
          err(ER::Code::PRIVATE_SYMBOL,
              srcSym,
              "symbol '" + symKey + "' is private to module '" + srcKey + "'");
          continue;
        }
      }
    }
    return sc;
  }

  // Collect the mangled globals a qualified `PFX.name` use may refer to. One
  // candidate is the ordinary case; an overloaded name yields several, left for
  // the type checker to pick from. Emits a diagnostic (and leaves `out` empty)
  // for an unknown module/symbol or an entirely-private match.
  void
  resolve_qualified(
    EX::ExVar &v, const std::string &Mkey, Scope &sc, std::vector<UT::Vu> &out)
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
      // Not a module/alias: an effect-qualified operation `Eff.op`? Map the
      // bare effect name through the module's visible types to its resolved
      // identity.
      auto tv = sc.tvis.find(PFX);
      if (tv != sc.tvis.end())
      {
        auto eff = effects.find(std::string(tv->second));
        if (eff != effects.end())
        {
          auto oit = eff->second.find(nm);
          if (oit == eff->second.end())
            err(ER::Code::UNKNOWN_SYMBOL,
                v.name,
                "effect '" + PFX + "' has no operation '" + nm + "'");
          else
            out.push_back(oit->second);
          return;
        }
      }
      err(ER::Code::UNKNOWN_MODULE,
          v.qualifier,
          "no module or effect named '" + PFX + "' in '" + PFX + "." + nm
            + "'");
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
    for (auto &si : sit->second)
      if (ownAccess || si.is_public) out.push_back(si.mangled);
    if (out.empty())
      err(ER::Code::PRIVATE_SYMBOL,
          v.name,
          "symbol '" + nm + "' is private to module '" + std::string(T) + "'");
  }

  // Settle a variable use against the candidates a name resolved to. A single
  // candidate rewrites the Var to its mangled global; several turn the node
  // into an ExOverload the type checker resolves by type (and reports as
  // ambiguous only if no -- or more than one -- candidate fits). `anchor` is
  // the use's source slice (its original, still-unmangled name). `is_operator`
  // forces the ExOverload form even for a single candidate: an operator always
  // has its built-in meanings too, which TC folds in beside the user's, so the
  // choice is never settled here.
  void
  install_resolution(
    Expr                *e,
    UT::Vu               name,
    UT::Vu               anchor,
    std::vector<UT::Vu> &cands,
    bool                 is_operator = false)
  {
    // Drop duplicates (the same global reachable via own + import).
    std::vector<UT::Vu> uniq;
    for (UT::Vu c : cands)
    {
      bool seen = false;
      for (UT::Vu u : uniq)
        if (u == c) seen = true;
      if (!seen) uniq.push_back(c);
    }
    if (uniq.empty()) return;
    for (UT::Vu c : uniq) edges[m_current].push_back(std::string(c));
    if (uniq.size() == 1 && !is_operator)
    {
      auto &v     = std::get<EX::ExVar>(e->as);
      v.name      = uniq[0];
      v.qualifier = UT::Vu{};
      return;
    }
    EX::ExOverload ov;
    ov.name       = name;
    ov.anchor     = anchor;
    ov.candidates = UT::Vec<UT::Vu>{ arena };
    for (UT::Vu c : uniq) ov.candidates.push(c);
    e->tag = ExprTag::Overload;
    e->as  = std::move(ov);
  }

  // Resolve a handler clause's operation to its canonical `MOD/Eff.op`
  // identity, rewriting `c.op` (and clearing `c.qualifier`). A clause names
  // exactly one operation, so -- unlike a perform site -- it is never an
  // ExOverload and cannot be settled by type: an unqualified op shared by
  // several effects visible in this module must be qualified `Eff.op`. Effects
  // visible to the module are its `tvis` entries that name an effect.
  void
  resolve_clause_op(
    EX::HandlerClause &c, Scope &sc)
  {
    std::string op(c.op);
    if (c.qualifier.size())
    {
      std::string eff(c.qualifier);
      auto        tv  = sc.tvis.find(eff);
      auto        eit = tv == sc.tvis.end() ? effects.end()
                                            : effects.find(std::string(tv->second));
      if (eit == effects.end())
        return (void)err(ER::Code::UNKNOWN_MODULE,
                         c.qualifier,
                         "no effect named '" + eff + "'");
      auto oit = eit->second.find(op);
      if (oit == eit->second.end())
        return (void)err(ER::Code::UNKNOWN_SYMBOL,
                         c.op,
                         "effect '" + eff + "' has no operation '" + op + "'");
      c.op        = oit->second;
      c.qualifier = UT::Vu{};
      return;
    }

    // Bare: gather every effect VISIBLE to this module that declares this op.
    std::vector<std::string> owners; // bare names, for the diagnostic
    UT::Vu                   hit{};
    for (auto &tv : sc.tvis)
    {
      auto eit = effects.find(std::string(tv.second));
      if (eit == effects.end()) continue;
      auto oit = eit->second.find(op);
      if (oit != eit->second.end())
      {
        owners.push_back(tv.first);
        hit = oit->second;
      }
    }
    if (owners.empty())
      return (void)err(ER::Code::UNKNOWN_SYMBOL,
                       c.op,
                       "no effect declares an operation '" + op + "'");
    if (owners.size() > 1)
    {
      std::string qs;
      for (size_t i = 0; i < owners.size(); ++i)
        qs += (i ? ", " : "") + owners[i] + "." + op;
      return (void)err(ER::Code::AMBIGUOUS_NAME,
                       c.op,
                       "operation '" + op
                         + "' is declared by several effects; qualify it (" + qs
                         + ")");
    }
    c.op = hit;
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
      auto  &v      = std::get<EX::ExVar>(e->as);
      UT::Vu anchor = v.name; // the original (still-unmangled) source slice
      if (v.qualifier.size())
      {
        std::vector<UT::Vu> cands;
        resolve_qualified(v, Mkey, sc, cands);
        install_resolution(e, anchor, anchor, cands);
        return;
      }
      for (auto &l : locals)
        if (l == v.name) return; // a local binder shadows
      auto it = sc.unq.find(std::string(v.name));
      if (it == sc.unq.end())
        return; // operator / builtin / unknown -- left for TC. (A built-in
                // operator with no user overload in scope stays a plain Var and
                // resolves through TC's overload_db, exactly as before.)
      install_resolution(
        e, anchor, anchor, it->second, OP::is_operator(v.name));
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
      auto  &f    = std::get<EX::ExFnDef>(e->as);
      size_t base = locals.size();
      if (f.param_pat)
        collect_pat_binders(f.param_pat, locals);
      else
        locals.push_back(f.param);
      resolve(f.body, Mkey, sc, locals);
      locals.resize(base);
      return;
    }
    case ExprTag::Let:
    {
      auto &l = std::get<EX::ExLet>(e->as);
      // A pattern let destructures its value; the pattern's binders are in
      // scope only in the body, not the value. A plain let's name is recursive.
      if (l.pat)
      {
        resolve(l.val, Mkey, sc, locals);
        size_t base = locals.size();
        collect_pat_binders(l.pat, locals);
        resolve(l.body, Mkey, sc, locals);
        locals.resize(base);
        return;
      }
      locals.push_back(l.var); // in scope for the value too (recursive let)
      resolve(l.val, Mkey, sc, locals);
      resolve(l.body, Mkey, sc, locals);
      locals.pop_back();
      return;
    }
    case ExprTag::Match:
    {
      auto &m = std::get<EX::ExMatch>(e->as);
      resolve(m.scrut, Mkey, sc, locals);
      for (size_t i = 0; i < m.arms.size(); ++i)
      {
        size_t base = locals.size();
        collect_pat_binders(m.arms[i].pat, locals);
        if (m.arms[i].guard) resolve(m.arms[i].guard, Mkey, sc, locals);
        resolve(m.arms[i].body, Mkey, sc, locals);
        locals.resize(base);
      }
      if (m.alt) resolve(m.alt, Mkey, sc, locals); // null when `else` omitted
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
    case ExprTag::Handle:
    {
      auto &h = std::get<EX::ExHandle>(e->as);
      resolve(h.body, Mkey, sc, locals); // k is NOT in scope in the body
      for (size_t c = 0; c < h.clauses.size(); ++c)
      {
        resolve_clause_op(h.clauses[c],
                          sc); // rewrite op to its `MOD/Eff.op` identity
        size_t base = locals.size();
        locals.push_back(h.clauses[c].arg); // the operation's argument
        locals.push_back(h.k);              // the shared continuation
        resolve(h.clauses[c].body, Mkey, sc, locals);
        locals.resize(base);
      }
      if (h.else_body)
      {
        size_t base = locals.size();
        locals.push_back(h.else_var);
        resolve(h.else_body, Mkey, sc, locals);
        locals.resize(base);
      }
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
    case ExprTag::SeqLit:
    {
      auto &sl = std::get<EX::ExSeqLit>(e->as);
      for (size_t k = 0; k < sl.elems.size(); ++k)
        resolve(sl.elems[k], Mkey, sc, locals);
      return;
    }
    default: return; // Int / Real / Str / Extern / Unknown: nothing to resolve
    }
  }

  /*--- Pass 2c: type-reference rewriting -------------------------------------
   * With every type's resolved identity now known, rewrite each reference --
   * `Ty::Con` names (and effect-row labels) in signatures, constructor/field
   * literals, and variant/struct patterns -- to its owning module's `MOD/Name`.
   * A name not in `tvis` (a `@`-sigil builtin, a type variable, or a genuine
   * unknown) is left untouched for TC to interpret or reject. */

  // A resolved `MOD/Name` shown as the user would qualify it (`MOD.Name`).
  std::string
  friendly_qual(
    UT::Vu resolved)
  {
    std::string s(resolved);
    size_t      slash = s.find('/');
    if (slash != std::string::npos) s[slash] = '.';
    return s;
  }

  // Resolve a bare type/effect name to its resolved identity. An ambiguous name
  // (imported from several modules) is rejected with a suggestion to qualify;
  // an unknown name (a `@`-sigil builtin, a type variable, a genuine unknown)
  // is left untouched for TC. `anchor` locates the diagnostic at the source
  // use.
  UT::Vu
  rewrite_ty_name(
    UT::Vu name, UT::Vu anchor, Scope &sc)
  {
    if (!name.size()) return name;
    std::string key(name);
    auto        amb = sc.tamb.find(key);
    if (amb != sc.tamb.end())
    {
      std::string qs;
      for (size_t i = 0; i < amb->second.size(); ++i)
        qs += (i ? ", " : "") + friendly_qual(amb->second[i]);
      err(ER::Code::AMBIGUOUS_NAME,
          anchor,
          "type '" + key + "' is imported from several modules; qualify it ("
            + qs + ")");
      return name;
    }
    auto it = sc.tvis.find(key);
    return it == sc.tvis.end() ? name : it->second;
  }

  // Resolve a qualified type `A.Name` to its owning module's resolved identity
  // (like `resolve_qualified` for terms: any program module or import alias may
  // be the qualifier). Emits a diagnostic and returns empty on failure.
  UT::Vu
  resolve_qualified_type(
    UT::Vu qual, UT::Vu name, const std::string &Mkey, Scope &sc)
  {
    std::string PFX(qual);
    UT::Vu      T;
    bool        ownAccess = false;
    auto        ait       = sc.alias.find(PFX);
    if (ait != sc.alias.end())
      T = ait->second;
    else if (PFX == Mkey)
    {
      T         = modules[Mkey].name;
      ownAccess = true;
    }
    else if (modules.count(PFX))
      T = modules[PFX].name;
    else
    {
      err(ER::Code::UNKNOWN_MODULE,
          qual,
          "no module named '" + PFX + "' in qualified type '" + PFX + "."
            + std::string(name) + "'");
      return {};
    }

    ModuleData &tm  = modules[std::string(T)];
    auto        tit = tm.types.find(std::string(name));
    if (tit == tm.types.end())
    {
      err(ER::Code::UNKNOWN_SYMBOL,
          name,
          "module '" + std::string(T) + "' has no type '" + std::string(name)
            + "'");
      return {};
    }
    if (!ownAccess && !tit->second.is_public)
    {
      err(ER::Code::PRIVATE_SYMBOL,
          name,
          "type '" + std::string(name) + "' is private to module '"
            + std::string(T) + "'");
      return {};
    }
    return tit->second.resolved;
  }

  // Does `name` denote a type visible (by its bare name) in this scope?
  bool
  is_visible_type(
    UT::Vu name, Scope &sc)
  {
    std::string s(name);
    return sc.tvis.count(s) || sc.tamb.count(s);
  }

  // Does `name` denote a module (own, an import alias, or any program module)?
  bool
  is_module_ref(
    UT::Vu name, const std::string &Mkey, Scope &sc)
  {
    std::string s(name);
    return sc.alias.count(s) || s == Mkey || modules.count(s);
  }

  void
  rewrite_ty(
    EX::Ty *t, const std::string &Mkey, Scope &sc)
  {
    if (!t) return;
    switch (t->tag)
    {
    case EX::TyTag::Con:
    {
      auto &c = std::get<EX::TyCon>(t->as);
      if (c.qualifier.size())
      {
        UT::Vu r = resolve_qualified_type(c.qualifier, c.name, Mkey, sc);
        if (r.size()) c.name = r;
        c.qualifier = UT::Vu{}; // consumed
      }
      else
        c.name = rewrite_ty_name(c.name, c.name, sc);
      for (EX::Ty *a : c.args) rewrite_ty(a, Mkey, sc);
      return;
    }
    case EX::TyTag::Arrow:
    {
      auto &a = std::get<EX::TyArrow>(t->as);
      rewrite_ty(a.from, Mkey, sc);
      rewrite_ty(a.to, Mkey, sc);
      // Effect-row labels name effects; the tail is a row variable (untouched).
      for (size_t i = 0; i < a.eff_labels.size(); ++i)
        a.eff_labels[i] = rewrite_ty_name(a.eff_labels[i], a.eff_labels[i], sc);
      return;
    }
    case EX::TyTag::Var: return;
    }
  }

  void
  rewrite_pat_types(
    EX::Pattern *p, const std::string &Mkey, Scope &sc)
  {
    switch (p->tag)
    {
    case EX::PatTag::Struct:
    {
      auto &ps = std::get<EX::PatStruct>(p->as);
      if (ps.qualifier.size())
      {
        UT::Vu r = resolve_qualified_type(ps.qualifier, ps.type_name, Mkey, sc);
        if (r.size()) ps.type_name = r;
        ps.qualifier = UT::Vu{};
      }
      else
        ps.type_name = rewrite_ty_name(ps.type_name, ps.type_name, sc);
      for (EX::FieldPat &f : ps.fields) rewrite_pat_types(f.pat, Mkey, sc);
      return;
    }
    case EX::PatTag::Variant:
    {
      auto &pv = std::get<EX::PatVariant>(p->as);
      if (pv.qualifier.size())
      {
        UT::Vu r = resolve_qualified_type(pv.qualifier, pv.type_name, Mkey, sc);
        if (r.size()) pv.type_name = r;
        pv.qualifier = UT::Vu{};
      }
      else if (!is_visible_type(pv.type_name, sc)
               && is_module_ref(pv.type_name, Mkey, sc))
      {
        // `M.Type.{..}` parsed as a variant but `M` is a module: it is really a
        // module-qualified STRUCT pattern. Rebuild it as one.
        UT::Vu r = resolve_qualified_type(pv.type_name, pv.tag, Mkey, sc);
        EX::PatStruct ps;
        ps.type_name = r.size() ? r : pv.tag;
        ps.fields    = std::move(pv.fields);
        ps.anchor    = pv.anchor;
        ps.line      = pv.line;
        p->tag       = EX::PatTag::Struct;
        p->as        = std::move(ps);
        for (EX::FieldPat &f : std::get<EX::PatStruct>(p->as).fields)
          rewrite_pat_types(f.pat, Mkey, sc);
        return;
      }
      else
        pv.type_name = rewrite_ty_name(pv.type_name, pv.type_name, sc);
      for (EX::FieldPat &f : pv.fields) rewrite_pat_types(f.pat, Mkey, sc);
      return;
    }
    case EX::PatTag::StrPrefix:
      rewrite_pat_types(std::get<EX::PatStrPrefix>(p->as).rest, Mkey, sc);
      return;
    case EX::PatTag::Seq:
    {
      auto &pq = std::get<EX::PatSeq>(p->as);
      for (size_t i = 0; i < pq.elems.size(); ++i)
        rewrite_pat_types(pq.elems[i], Mkey, sc);
      if (pq.rest) rewrite_pat_types(pq.rest, Mkey, sc);
      return;
    }
    default: return; // Wild / Var / Int / Real / Str: no type name
    }
  }

  void
  rewrite_types_expr(
    Expr *e, const std::string &Mkey, Scope &sc)
  {
    switch (e->tag)
    {
    case ExprTag::StructLit:
    {
      auto &sl = std::get<EX::ExStructLit>(e->as);
      if (sl.qualifier.size())
      {
        UT::Vu r = resolve_qualified_type(sl.qualifier, sl.type_name, Mkey, sc);
        if (r.size()) sl.type_name = r;
        sl.qualifier = UT::Vu{};
      }
      else
        sl.type_name = rewrite_ty_name(sl.type_name, sl.type_name, sc);
      for (size_t k = 0; k < sl.fields.size(); ++k)
        rewrite_types_expr(sl.fields[k].val, Mkey, sc);
      return;
    }
    case ExprTag::VariantLit:
    {
      auto &vl = std::get<EX::ExVariantLit>(e->as);
      if (vl.qualifier.size())
      {
        // `A.Type.Tag` -- resolve the qualified union type, keep the tag.
        UT::Vu r = resolve_qualified_type(vl.qualifier, vl.type_name, Mkey, sc);
        if (r.size()) vl.type_name = r;
        vl.qualifier = UT::Vu{};
      }
      else if (!is_visible_type(vl.type_name, sc)
               && is_module_ref(vl.type_name, Mkey, sc))
      {
        // `M.Type.{..}` was parsed as a variant (`M` type, `Type` tag), but `M`
        // is a module: it is really a module-qualified STRUCT literal. Rebuild
        // the node as one and resolve the qualified struct type.
        UT::Vu r = resolve_qualified_type(vl.type_name, vl.tag, Mkey, sc);
        EX::ExStructLit sl;
        sl.type_name = r.size() ? r : vl.tag;
        sl.fields    = std::move(vl.fields);
        e->tag       = ExprTag::StructLit;
        e->as        = std::move(sl);
        auto &nsl    = std::get<EX::ExStructLit>(e->as);
        for (size_t k = 0; k < nsl.fields.size(); ++k)
          rewrite_types_expr(nsl.fields[k].val, Mkey, sc);
        return;
      }
      else
        vl.type_name = rewrite_ty_name(vl.type_name, vl.type_name, sc);
      for (size_t k = 0; k < vl.fields.size(); ++k)
        rewrite_types_expr(vl.fields[k].val, Mkey, sc);
      return;
    }
    case ExprTag::App:
    {
      auto &a = std::get<EX::ExApp>(e->as);
      rewrite_types_expr(a.fn, Mkey, sc);
      rewrite_types_expr(a.arg, Mkey, sc);
      return;
    }
    case ExprTag::FnDef:
    {
      auto &f = std::get<EX::ExFnDef>(e->as);
      if (f.param_pat) rewrite_pat_types(f.param_pat, Mkey, sc);
      rewrite_types_expr(f.body, Mkey, sc);
      return;
    }
    case ExprTag::Let:
    {
      auto &l = std::get<EX::ExLet>(e->as);
      if (l.pat) rewrite_pat_types(l.pat, Mkey, sc);
      if (l.sig) rewrite_ty(l.sig, Mkey, sc);
      rewrite_types_expr(l.val, Mkey, sc);
      rewrite_types_expr(l.body, Mkey, sc);
      return;
    }
    case ExprTag::Match:
    {
      auto &m = std::get<EX::ExMatch>(e->as);
      rewrite_types_expr(m.scrut, Mkey, sc);
      for (size_t i = 0; i < m.arms.size(); ++i)
      {
        rewrite_pat_types(m.arms[i].pat, Mkey, sc);
        if (m.arms[i].guard) rewrite_types_expr(m.arms[i].guard, Mkey, sc);
        rewrite_types_expr(m.arms[i].body, Mkey, sc);
      }
      if (m.alt) rewrite_types_expr(m.alt, Mkey, sc); // null when no `else`
      return;
    }
    case ExprTag::If:
    {
      auto &i = std::get<EX::ExIf>(e->as);
      rewrite_types_expr(i.cond, Mkey, sc);
      rewrite_types_expr(i.then, Mkey, sc);
      rewrite_types_expr(i.alt, Mkey, sc);
      return;
    }
    case ExprTag::Handle:
    {
      auto &h = std::get<EX::ExHandle>(e->as);
      rewrite_types_expr(h.body, Mkey, sc);
      for (size_t c = 0; c < h.clauses.size(); ++c)
        rewrite_types_expr(h.clauses[c].body, Mkey, sc);
      if (h.else_body) rewrite_types_expr(h.else_body, Mkey, sc);
      return;
    }
    case ExprTag::Field:
      rewrite_types_expr(std::get<EX::ExField>(e->as).record, Mkey, sc);
      return;
    case ExprTag::SeqLit:
    {
      auto &sl = std::get<EX::ExSeqLit>(e->as);
      for (size_t k = 0; k < sl.elems.size(); ++k)
        rewrite_types_expr(sl.elems[k], Mkey, sc);
      return;
    }
    default: return; // Var / Int / Real / Str / Extern / Overload / Unknown
    }
  }

  // Rewrite the type references embedded in a top-level type declaration's
  // field / variant-payload / alias-target / operation signatures.
  void
  rewrite_decl_types(
    Expr *e, const std::string &Mkey, Scope &sc)
  {
    switch (e->tag)
    {
    case ExprTag::StructDecl:
      for (auto &f : std::get<EX::ExStructDecl>(e->as).fields)
        rewrite_ty(f.ty, Mkey, sc);
      return;
    case ExprTag::UnionDecl:
      for (auto &v : std::get<EX::ExUnionDecl>(e->as).variants)
        for (auto &f : v.fields) rewrite_ty(f.ty, Mkey, sc);
      return;
    case ExprTag::AliasDecl:
      rewrite_ty(std::get<EX::ExAliasDecl>(e->as).target, Mkey, sc);
      return;
    case ExprTag::EffectDecl:
      for (auto &op : std::get<EX::ExEffectDecl>(e->as).ops)
        rewrite_ty(op.ty, Mkey, sc);
      return;
    default: return;
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
      m_current = std::string(d.name);
      resolve(d.def, std::string(pr.first), sc, locals);
    }

    // Pass 2c: rewrite type references (decl signatures, def signatures, and
    // the constructor/pattern references in every body) to their resolved
    // names.
    for (auto &pr : decls)
    {
      std::string Mkey(pr.first);
      rewrite_decl_types(pr.second, Mkey, scopes[Mkey]);
    }
    for (auto &pr : defs)
    {
      std::string Mkey(pr.first);
      Scope      &sc = scopes[Mkey];
      auto       &d  = std::get<EX::ExDef>(pr.second->as);
      rewrite_ty(d.sig, Mkey, sc);
      rewrite_types_expr(d.def, Mkey, sc);
    }

    Result r;

    // Entry point: module MAIN's `main`, with a supported signature. Absence of
    // an entry is not an error here (a library or test snippet need not have
    // one) -- it leaves `entry` empty and the driver decides whether to require
    // it. A MAIN/main that exists but has the wrong signature IS an error.
    auto mit = modules.find("MAIN");
    if (mit != modules.end())
    {
      auto sit = mit->second.symbols.find("main");
      if (sit != mit->second.symbols.end() && !sit->second.empty())
      {
        SymInfo &mainSym   = sit->second.front();
        auto    &d         = std::get<EX::ExDef>(mainSym.node->as);
        bool     takes_arg = false;
        if (!check_entry_sig(d.sig, takes_arg))
          err(ER::Code::ENTRY_SIGNATURE,
              mit->second.name,
              "'main' must be annotated ': Int' or ': Str -> Int'");
        else
        {
          r.entry           = mainSym.mangled;
          r.entry_takes_arg = takes_arg;
        }
      }
    }

    // Dead-global elimination
    std::vector<std::string> roots;
    if (r.entry.size()) roots.push_back(std::string(r.entry));
    for (auto &pr : defs)
    {
      auto &d = std::get<EX::ExDef>(pr.second->as);
      if (d.ctime_assert)
      {
        roots.push_back(std::string(d.name));
        r.ctime_asserts.push_back({ d.name, d.assert_anchor });
      }
      if (d.ctime_run)
      {
        roots.push_back(std::string(d.name));
        r.ctime_runs.push_back({ d.name, d.assert_anchor });
      }
    }

    std::unordered_set<std::string> live;
    {
      std::vector<std::string> stack = roots;
      for (const std::string &g : roots) live.insert(g);
      while (stack.size())
      {
        std::string g = std::move(stack.back());
        stack.pop_back();
        auto it = edges.find(g);
        if (it == edges.end()) continue;
        for (const std::string &t : it->second)
          if (live.insert(t).second) stack.push_back(t);
      }
    }

    r.program = EX::Exprs{ arena };
    for (auto &pr : decls) r.program.push(*pr.second);
    for (auto &pr : defs)
    {
      auto &d = std::get<EX::ExDef>(pr.second->as);
      if (roots.empty() || live.count(std::string(d.name)))
        r.program.push(*pr.second);
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
