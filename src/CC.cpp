/*-------------------------------------------------------------------------------
 *\file CC.cpp
 *\info Implementation of the C-code generation backend (see CC.hpp). Each IR
 *      Code becomes one C function `THx_code_<i>(env, arg)`; the ANF
 * structure maps almost one-to-one onto C statements. Tail applications emit
 *      `THxRT_tailcall` (constant stack via the runtime trampoline); non-tail ones
 *      emit `THxRT_apply`. Every value read goes through a checked accessor, so
 * the generated program fails loudly on a codegen bug rather than corrupting
 *      memory.
 *
 * Domino discipline: the IR walkers switch over IR::EKind / AKind / AltKind
 * with NO default, so adding an IR variant fails to compile here
 * (-Wswitch/-Werror) until it is handled; genuinely-unreachable arms use
 * UT_FAIL_MSG.
 *-----------------------------------------------------------------------------*/

#include "CC.hpp"

#include "OP.hpp"
#include "THxRTxAMALG.hpp" // CC::RUNTIME_C -- the runtime, baked in (generated)

#include <algorithm>
#include <cstdio>
#include <utility>
#include <vector>

namespace CC
{

namespace
{

/*------------------------------------------------------------------------------
 *\SMALL HELPERS
 *-----------------------------------------------------------------------------*/

// A C string literal (with surrounding quotes) for an arbitrary byte view.
std::string
cstr(
  UT::Vu v)
{
  std::string s = "\"";
  for (size_t i = 0; i < v.size(); ++i)
  {
    unsigned char c = (unsigned char)v.data()[i];
    switch (c)
    {
    case '"' : s += "\\\""; break;
    case '\\': s += "\\\\"; break;
    case '\n': s += "\\n"; break;
    case '\t': s += "\\t"; break;
    case '\r': s += "\\r"; break;
    default:
      if (c < 0x20 || c >= 0x7f)
      {
        char buf[8];
        std::snprintf(buf, sizeof(buf), "\\x%02x", c);
        s += buf;
        // Guard against a following hex digit extending the escape.
        if (i + 1 < v.size())
        {
          char n = v.data()[i + 1];
          if ((n >= '0' && n <= '9') || (n >= 'a' && n <= 'f')
              || (n >= 'A' && n <= 'F'))
            s += "\"\"";
        }
      }
      else
        s += (char)c;
    }
  }
  return s + "\"";
}

// A C double literal that round-trips the value.
std::string
cdbl(
  double v)
{
  char buf[64];
  std::snprintf(buf, sizeof(buf), "%.17g", v);
  return buf;
}

// Max `Env i` index used in a code body, +1 -- the closure env length, used as
// the bound passed to THxVALUE_env. Matches the MkClosure capture count by
// construction (IR::conv_fun assigns env slots densely).
struct EnvScan
{
  size_t max1 = 0; // one past the highest Env index seen
  void
  atom(
    const IR::Atom *a)
  {
    switch (IR::akind(a))
    {
    case IR::AKind::Env:
    {
      size_t i = std::get<IR::Env>(a->as).i;
      if (i + 1 > max1) max1 = i + 1;
    }
    break;
    case IR::AKind::Clos:
      for (const IR::Atom *c : std::get<IR::MkClosure>(a->as).captures) atom(c);
      break;
    case IR::AKind::Local:
    case IR::AKind::Glob:
    case IR::AKind::LitI:
    case IR::AKind::LitR:
    case IR::AKind::LitS : break;
    }
  }
  void
  expr(
    const IR::Expr *e)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret: atom(std::get<IR::Ret>(e->as).a); break;
    case IR::EKind::Let:
    {
      auto &l = std::get<IR::Let>(e->as);
      expr(l.rhs);
      expr(l.body);
    }
    break;
    case IR::EKind::App:
    {
      auto &a = std::get<IR::App>(e->as);
      atom(a.fn);
      atom(a.arg);
    }
    break;
    case IR::EKind::Case:
    {
      auto &c = std::get<IR::Case>(e->as);
      atom(c.scrut);
      for (const IR::Alt &al : c.alts) expr(al.body);
      expr(c.deflt);
    }
    break;
    case IR::EKind::MkStruct:
      for (const IR::FieldA &f : std::get<IR::MkStruct>(e->as).fields)
        atom(f.val);
      break;
    case IR::EKind::Field: atom(std::get<IR::Field>(e->as).rec); break;
    case IR::EKind::MkVariant:
      for (const IR::Atom *f : std::get<IR::MkVariant>(e->as).fields) atom(f);
      break;
    case IR::EKind::Handle:
    case IR::EKind::Extern:
    case IR::EKind::Unk   : break;
    }
  }
};

size_t
env_size(
  const IR::Code &c)
{
  EnvScan s;
  s.expr(c.body);
  return s.max1;
}

/*------------------------------------------------------------------------------
 *\UNSUPPORTED-FEATURE SCAN
 *-----------------------------------------------------------------------------*/

struct Reject
{
  const IR::Program         &prog;
  std::optional<std::string> reason;

  void
  set(
    std::string r)
  {
    if (!reason) reason = std::move(r);
  }

  void
  atom(
    const IR::Atom *a)
  {
    switch (IR::akind(a))
    {
    case IR::AKind::Glob:
    {
      std::string n(std::get<IR::Glob>(a->as).name);
      if (n == OP::DEFER)
        set("`defer` cleanup");
      else if (prog.operations.count(n))
        set("effect operation `" + n + "`");
    }
    break;
    case IR::AKind::Clos:
      for (const IR::Atom *c : std::get<IR::MkClosure>(a->as).captures) atom(c);
      break;
    case IR::AKind::Local:
    case IR::AKind::Env:
    case IR::AKind::LitI:
    case IR::AKind::LitR:
    case IR::AKind::LitS : break;
    }
  }

  void
  expr(
    const IR::Expr *e)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret: atom(std::get<IR::Ret>(e->as).a); break;
    case IR::EKind::Let:
    {
      auto &l = std::get<IR::Let>(e->as);
      expr(l.rhs);
      expr(l.body);
    }
    break;
    case IR::EKind::App:
    {
      auto &a = std::get<IR::App>(e->as);
      atom(a.fn);
      atom(a.arg);
    }
    break;
    case IR::EKind::Case:
    {
      auto &c = std::get<IR::Case>(e->as);
      atom(c.scrut);
      for (const IR::Alt &al : c.alts) expr(al.body);
      expr(c.deflt);
    }
    break;
    case IR::EKind::MkStruct:
      for (const IR::FieldA &f : std::get<IR::MkStruct>(e->as).fields)
        atom(f.val);
      break;
    case IR::EKind::Field: atom(std::get<IR::Field>(e->as).rec); break;
    case IR::EKind::MkVariant:
      for (const IR::Atom *f : std::get<IR::MkVariant>(e->as).fields) atom(f);
      break;
    case IR::EKind::Handle: set("effect handlers (`do`/`ctl`)"); break;
    case IR::EKind::Extern: set("foreign calls (`@extern` / FFI)"); break;
    case IR::EKind::Unk   : break;
    }
  }
};

/*------------------------------------------------------------------------------
 *\THE EMITTER
 *-----------------------------------------------------------------------------*/

// Where a computed value goes: either returned (tail position) or assigned to a
// named C variable (the value was demanded as a let-binding / case result).
struct Dest
{
  bool        tail;
  std::string var; // valid iff !tail
};

struct Emitter
{
  const IR::Program &prog;
  std::string        out;
  size_t             tmp     = 0;
  size_t             nlocals = 0; // of the code currently being emitted
  size_t             nenv    = 0;

  explicit Emitter(
    const IR::Program &p)
      : prog{ p }
  {
  }

  std::string
  fresh(
    const char *p)
  {
    return std::string(p) + std::to_string(tmp++);
  }

  // ---- atoms: each is a single, side-effect-light C expression ----
  std::string
  atom(
    const IR::Atom *a)
  {
    switch (IR::akind(a))
    {
    case IR::AKind::Local:
      return "THxVALUE_local(locals, " + std::to_string(nlocals) + ", "
             + std::to_string(std::get<IR::Local>(a->as).i) + ")";
    case IR::AKind::Env:
      return "THxVALUE_env(env, " + std::to_string(nenv) + ", "
             + std::to_string(std::get<IR::Env>(a->as).i) + ")";
    case IR::AKind::Glob:
      return "THxRT_glob(" + cstr(std::get<IR::Glob>(a->as).name) + ")";
    case IR::AKind::LitI:
      return "THxRT_int(" + std::to_string(std::get<IR::LitI>(a->as).v) + "LL)";
    case IR::AKind::LitR:
      return "THxRT_real(" + cdbl(std::get<IR::LitR>(a->as).v) + ")";
    case IR::AKind::LitS:
    {
      UT::Vu v = std::get<IR::LitS>(a->as).v;
      return "THxRT_str(" + cstr(v) + ", " + std::to_string(v.size()) + ")";
    }
    case IR::AKind::Clos:
    {
      auto       &c = std::get<IR::MkClosure>(a->as);
      std::string s = "THxRT_closure(" + std::to_string(c.code) + ", ";
      if (c.captures.size() == 0)
        s += "NULL, 0)";
      else
      {
        s += "(Value*[]){ ";
        for (const IR::Atom *cap : c.captures) s += atom(cap) + ", ";
        s += "}, " + std::to_string(c.captures.size()) + ")";
      }
      return s;
    }
    }
    UT_FAIL_MSG("%s", "CC: unhandled IR::Atom kind");
    return "THxRT_unk()";
  }

  // Deliver a finished value expression to its destination.
  void
  finish(
    const std::string &val, const Dest &d)
  {
    if (d.tail)
      out += "  return " + val + ";\n";
    else
      out += "  " + d.var + " = " + val + ";\n";
  }

  // ---- expressions ----
  void
  expr(
    const IR::Expr *e, const Dest &d)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret: finish(atom(std::get<IR::Ret>(e->as).a), d); break;

    case IR::EKind::App:
    {
      auto       &a   = std::get<IR::App>(e->as);
      std::string fn  = atom(a.fn);
      std::string arg = atom(a.arg);
      if (d.tail)
        out += "  return THxRT_tailcall(" + fn + ", " + arg + ");\n";
      else
        out += "  " + d.var + " = THxRT_apply(" + fn + ", " + arg + ");\n";
    }
    break;

    case IR::EKind::Let:
    {
      auto       &l   = std::get<IR::Let>(e->as);
      std::string box = fresh("box");
      std::string v   = fresh("v");
      // A recursive let: bind the slot to a placeholder box first, build the
      // rhs (whose closures may capture the box), then copy the value into the
      // box so those captures observe it. Mirrors the interpreter's back-patch.
      out += "  Value* " + box + " = THxRT_unk();\n";
      out += "  locals[" + std::to_string(l.slot) + "] = " + box + ";\n";
      out += "  Value* " + v + ";\n";
      expr(l.rhs, Dest{ false, v });
      out += "  *" + box + " = *" + v + ";\n";
      expr(l.body, d);
    }
    break;

    case IR::EKind::Case:
    {
      auto       &c = std::get<IR::Case>(e->as);
      std::string s = fresh("scrut");
      out += "  Value* " + s + " = " + atom(c.scrut) + ";\n";
      bool first = true;
      for (const IR::Alt &al : c.alts)
      {
        std::string cond;
        switch (al.kind)
        {
        case IR::AltKind::Int:
          cond = "THxVALUE_as_int(" + s + ") == " + std::to_string(al.ival) + "LL";
          break;
        case IR::AltKind::Real:
          cond = "THxVALUE_as_num(" + s + ") == " + cdbl(al.rval);
          break;
        case IR::AltKind::Con:
          cond = "strcmp(THxVALUE_ctor(" + s + "), " + cstr(al.ctor) + ") == 0";
          break;
        }
        out += std::string("  ") + (first ? "if" : "else if") + " (" + cond
               + ") {\n";
        if (al.kind == IR::AltKind::Con)
          for (size_t i = 0; i < al.binders.size(); ++i)
            out += "  locals[" + std::to_string(al.binder_base + i)
                   + "] = THxVALUE_variant_field(" + s + ", " + std::to_string(i)
                   + ");\n";
        expr(al.body, d);
        out += "  }\n";
        first = false;
      }
      out += first ? "  {\n" : "  else {\n";
      expr(c.deflt, d);
      out += "  }\n";
    }
    break;

    case IR::EKind::MkStruct:
    {
      auto       &m = std::get<IR::MkStruct>(e->as);
      std::string v;
      if (m.fields.size() == 0)
        v = "THxRT_struct(" + cstr(m.name) + ", 0, NULL, NULL)";
      else
      {
        std::string names = "(const char*[]){ ", vals = "(Value*[]){ ";
        for (const IR::FieldA &f : m.fields)
        {
          names += cstr(f.name) + ", ";
          vals += atom(f.val) + ", ";
        }
        v = "THxRT_struct(" + cstr(m.name) + ", " + std::to_string(m.fields.size())
            + ", " + names + "}, " + vals + "})";
      }
      finish(v, d);
    }
    break;

    case IR::EKind::Field:
    {
      auto &f = std::get<IR::Field>(e->as);
      finish("THxVALUE_field(" + atom(f.rec) + ", " + cstr(f.name) + ")", d);
    }
    break;

    case IR::EKind::MkVariant:
    {
      auto       &mv = std::get<IR::MkVariant>(e->as);
      std::string v;
      if (mv.fields.size() == 0)
        v = "THxRT_variant(" + cstr(mv.type_name) + ", " + cstr(mv.tag)
            + ", 0, NULL)";
      else
      {
        std::string vals = "(Value*[]){ ";
        for (const IR::Atom *f : mv.fields) vals += atom(f) + ", ";
        v = "THxRT_variant(" + cstr(mv.type_name) + ", " + cstr(mv.tag) + ", "
            + std::to_string(mv.fields.size()) + ", " + vals + "})";
      }
      finish(v, d);
    }
    break;

    case IR::EKind::Unk: finish("THxRT_unk()", d); break;

    case IR::EKind::Handle:
    case IR::EKind::Extern:
      // Rejected by CC::unsupported before emit is ever called.
      UT_FAIL_MSG("%s",
                  "CC: effect/FFI node reached codegen (unsupported gate "
                  "should have caught it)");
      break;
    }
  }

  void
  emit_code(
    size_t id)
  {
    const IR::Code &c = prog.codes[id];
    nlocals           = c.nlocals;
    nenv              = env_size(c);
    tmp               = 0;

    out += "static Value* THx_code_" + std::to_string(id)
           + "(Value** env, Value* arg) {\n";
    out += "  (void)env; (void)arg;\n";
    if (c.nlocals > 0)
    {
      out += "  Value* locals[" + std::to_string(c.nlocals) + "] = {0};\n";
      if (c.nparams >= 1) out += "  locals[0] = arg;\n";
    }
    expr(c.body, Dest{ true, {} });
    out += "}\n\n";
  }
};

} // namespace

/*------------------------------------------------------------------------------
 *\PUBLIC API
 *-----------------------------------------------------------------------------*/

std::optional<std::string>
unsupported(
  const IR::Program &prog)
{
  Reject r{ prog, std::nullopt };
  for (const IR::Code &c : prog.codes) r.expr(c.body);
  // A 2-parameter Code is a handler clause (IR::conv_clause); its presence
  // means effects even if every Handle node were somehow elided.
  for (const IR::Code &c : prog.codes)
    if (c.nparams >= 2) r.set("effect handler clauses");
  return r.reason;
}

std::string
emit(
  const IR::Program &prog, UT::Vu entry, bool entry_takes_arg)
{
  Emitter em{ prog };

  em.out += "/* Generated by the Thrax CC backend. Do not edit.\n";
  em.out += " * Self-contained -- the runtime is inlined below. Build: cc -O2 "
            "prog.c -o prog */\n\n";
  // The whole C runtime, baked into the compiler and emitted inline, so the
  // generated program is a single self-contained translation unit.
  em.out += RUNTIME_C;
  em.out += "\n#include <string.h>\n\n";

  // Forward declarations, then definitions (codes call each other freely).
  for (size_t i = 0; i < prog.codes.size(); ++i)
    em.out += "static Value* THx_code_" + std::to_string(i)
              + "(Value** env, Value* arg);\n";
  em.out += "\n";
  for (size_t i = 0; i < prog.codes.size(); ++i) em.emit_code(i);

  // The dispatch table the runtime calls back through.
  em.out += "CodeFn THxRT_code_table[] = {\n";
  for (size_t i = 0; i < prog.codes.size(); ++i)
    em.out += "  THx_code_" + std::to_string(i) + ",\n";
  em.out += "};\n";
  em.out += "const size_t THxRT_code_count = " + std::to_string(prog.codes.size())
            + ";\n\n";

  // Globals: name -> CAF code index, lazily forced and memoized (matches
  // IT::glob). Stable iteration order for reproducible output.
  std::vector<std::pair<std::string, size_t>> globs(prog.globals.begin(),
                                                    prog.globals.end());
  std::sort(globs.begin(), globs.end());
  size_t ng = globs.size();

  if (ng > 0)
  {
    em.out += "static Value* g_cache[" + std::to_string(ng) + "];\n";
    em.out += "static char g_inited[" + std::to_string(ng) + "];\n";
    em.out += "static char g_inprog[" + std::to_string(ng) + "];\n\n";
  }

  em.out += "Value* THxRT_glob(const char* name) {\n";
  for (size_t k = 0; k < ng; ++k)
  {
    UT::Vu nm{ globs[k].first.data(), globs[k].first.size() };
    em.out += "  if (strcmp(name, " + cstr(nm) + ") == 0) {\n";
    em.out += "    if (g_inited[" + std::to_string(k) + "]) return g_cache["
              + std::to_string(k) + "];\n";
    em.out += "    if (g_inprog[" + std::to_string(k)
              + "]) THxCHECK_FAILF(\"cyclic value global '%s'\", name);\n";
    em.out += "    g_inprog[" + std::to_string(k) + "] = 1;\n";
    em.out += "    g_cache[" + std::to_string(k) + "] = THxRT_force_code("
              + std::to_string(globs[k].second) + ");\n";
    em.out += "    g_inited[" + std::to_string(k) + "] = 1;\n";
    em.out += "    g_inprog[" + std::to_string(k) + "] = 0;\n";
    em.out += "    return g_cache[" + std::to_string(k) + "];\n";
    em.out += "  }\n";
  }
  em.out += "  return THxRT_builtin(name);\n";
  em.out += "}\n\n";

  // Force every global -- the compiled mirror of the interpreter smoke test
  // (tst/TS.cpp), so a runtime fault in any top-level definition surfaces.
  em.out += "static void THx_force_all(void) {\n";
  for (size_t k = 0; k < ng; ++k)
  {
    UT::Vu nm{ globs[k].first.data(), globs[k].first.size() };
    em.out += "  (void)THxRT_glob(" + cstr(nm) + ");\n";
  }
  em.out += "}\n\n";

  // Entry: force all globals, then run the entry point if there is one. The
  // entry's Int result is the process exit code.
  em.out += "int main(void) {\n";
  em.out += "  THx_force_all();\n";
  if (entry.size() > 0)
  {
    em.out += "  Value* e = THxRT_glob(" + cstr(entry) + ");\n";
    if (entry_takes_arg) em.out += "  e = THxRT_apply(e, THxRT_str(\"\", 0));\n";
    em.out += "  return (int)THxVALUE_as_int(e);\n";
  }
  else
    em.out += "  return 0;\n";
  em.out += "}\n";

  return em.out;
}

} // namespace CC
