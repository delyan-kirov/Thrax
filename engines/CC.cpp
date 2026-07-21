/*-------------------------------------------------------------------------------
 *\file CC.cpp
 *\info Implementation of the C-code generation backend (see CC.hpp). The IR is
 *      lowered to BLOCK FUNCTIONS driven by the CEK machine in the runtime
 *      (src/THxK.c): each block runs straight-line C (atoms, pure lets, case
 *      branching) and ends by calling exactly one TERMINATOR (THxK_ret /
 *      THxK_tailcall / THxK_apply / THxK_jump / THxK_handle). A non-tail
 *      application is a suspension point: the rest of the computation becomes a
 *      continuation block, so the delimited continuation a handler captures is
 * a pure heap object rather than live C-stack frames.
 *
 * The emitter is continuation-passing: `emit_expr(e, sink)` lowers `e` so its
 * value reaches `sink` -- either delivered up the continuation (RET) or written
 * to a frame slot before jumping to a continuation block (SLOT). Pure lets are
 * still emitted inline (as before); only suspending sub-expressions split into
 * fresh blocks.
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

/*------------------------------------------------------------------------------
 *\FFI -- Thrax type <-> C ABI for direct foreign calls
 *-----------------------------------------------------------------------------*/

std::string
c_type(
  const std::string &t0, bool ret, const TG::Target &tg)
{
  UT::Vu t = tg.canon(t0);
  if (t == OP::TY_INT8) return "int8_t";
  if (t == OP::TY_INT16) return "int16_t";
  if (t == OP::TY_INT32) return "int32_t";
  if (t == OP::TY_INT64) return "int64_t";
  if (t == OP::TY_NAT8) return "uint8_t";
  if (t == OP::TY_NAT16) return "uint16_t";
  if (t == OP::TY_NAT32) return "uint32_t";
  if (t == OP::TY_NAT64) return "uint64_t";
  if (t == OP::TY_REAL64) return "double";
  if (t == OP::TY_REAL32) return "float";
  if (t == OP::TY_STR) return "char*";
  if (t == OP::TY_PTR || t == OP::TY_ARRAY) return "void*";
  if (ret && t == OP::TY_UNIT) return "void";
  return "intptr_t"; // default / type variable: word-sized, like FF::desc_of
}

std::string
marshal_arg(
  const std::string &t0, const std::string &v, const TG::Target &tg)
{
  UT::Vu t = tg.canon(t0);
  if (t == OP::TY_STR) return "(char*)THxVALUE_str(" + v + ")";
  if (t == OP::TY_ARRAY) return "(void*)THxVALUE_str(" + v + ")";
  if (t == OP::TY_PTR) return "(void*)(intptr_t)THxVALUE_as_int(" + v + ")";
  if (t == OP::TY_REAL64) return "(double)THxVALUE_as_num(" + v + ")";
  if (t == OP::TY_REAL32) return "(float)THxVALUE_as_num(" + v + ")";
  return "(" + c_type(t0, false, tg) + ")THxVALUE_as_int(" + v + ")";
}

std::string
marshal_ret(
  const std::string &t0, const std::string &r, const TG::Target &tg)
{
  UT::Vu t = tg.canon(t0);
  if (t == OP::TY_STR)
    return "return THxRT_str(" + r + " ? " + r + " : \"\", " + r + " ? strlen("
           + r + ") : 0);";
  if (t == OP::TY_REAL64 || t == OP::TY_REAL32)
    return "return THxRT_real((double)" + r + ");";
  if (t == OP::TY_PTR)
    return "return THxRT_int((long long)(intptr_t)" + r + ");";
  return "return THxRT_int((long long)" + r + ");";
}

/*------------------------------------------------------------------------------
 *\THE EMITTER
 *-----------------------------------------------------------------------------*/

// Where a computed value goes.
//  - RET  : delivered up the continuation stack (THxK_ret / tailcall).
//  - SLOT : written into frame `slot` (a let-box back-patch), then control
//  jumps
//           to continuation block `cont`.
struct Sink
{
  enum Kind
  {
    RET,
    SLOT
  } kind;
  size_t slot = 0; // valid iff SLOT
  size_t cont = 0; // continuation block id, iff SLOT
};

struct Emitter
{
  const IR::Program &prog;
  // One entry per emitted block: its full C function text. Filled out of order
  // (a block is reserved, then set once its body is built), so this is a vector
  // of finished function definitions indexed by block id.
  std::vector<std::string>        blocks;
  std::vector<size_t>             code_entry; // code index -> entry block id
  std::vector<const IR::Extern *> externs;    // each @extern site, in order
  size_t                          tmp = 0;

  explicit Emitter(
    const IR::Program &p)
      : prog{ p }
  {
    code_entry.assign(p.codes.size(), 0);
  }

  std::string
  fresh(
    const char *p)
  {
    return std::string(p) + std::to_string(tmp++);
  }

  size_t
  reserve_block()
  {
    size_t id = blocks.size();
    blocks.push_back(std::string());
    return id;
  }

  void
  set_block(
    size_t id, const std::string &body)
  {
    blocks[id] = "static void blk_" + std::to_string(id)
                 + "(Frame* fr, Value* in) {\n  (void)fr; (void)in;\n" + body
                 + "}\n\n";
  }

  // ---- atoms: each is a single, side-effect-light C expression ----
  std::string
  atom(
    const IR::Atom *a)
  {
    switch (IR::akind(a))
    {
    case IR::AKind::Local:
      return "THxVALUE_local(fr->locals, fr->nlocals, "
             + std::to_string(std::get<IR::Local>(a->as).i) + ")";
    case IR::AKind::Env:
      return "THxVALUE_env(fr->env, fr->nenv, "
             + std::to_string(std::get<IR::Env>(a->as).i) + ")";
    case IR::AKind::Glob:
    {
      UT::Vu      nm = std::get<IR::Glob>(a->as).name;
      std::string n(nm);
      if (n == OP::DEFER) return "THxK_defer()";
      if (prog.operations.count(n)) return "THxK_op(" + cstr(nm) + ")";
      return "THxRT_glob(" + cstr(nm) + ")";
    }
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

  // A single C expression for a value-producing, non-suspending, non-branching
  // expression (Ret / aggregates / Unk / Extern). Case/Let/App/Handle are
  // structural and never reach here.
  std::string
  value_expr(
    const IR::Expr *e)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret: return atom(std::get<IR::Ret>(e->as).a);
    case IR::EKind::MkStruct:
    {
      auto &m = std::get<IR::MkStruct>(e->as);
      if (m.fields.size() == 0)
        return "THxRT_struct(" + cstr(m.name) + ", 0, NULL, NULL)";
      std::string names = "(const char*[]){ ", vals = "(Value*[]){ ";
      for (const IR::FieldA &f : m.fields)
      {
        names += cstr(f.name) + ", ";
        vals += atom(f.val) + ", ";
      }
      return "THxRT_struct(" + cstr(m.name) + ", "
             + std::to_string(m.fields.size()) + ", " + names + "}, " + vals
             + "})";
    }
    case IR::EKind::Field:
    {
      auto &f = std::get<IR::Field>(e->as);
      return "THxVALUE_field(" + atom(f.rec) + ", " + cstr(f.name) + ")";
    }
    case IR::EKind::MkVariant:
    {
      auto &mv = std::get<IR::MkVariant>(e->as);
      if (mv.fields.size() == 0)
        return "THxRT_variant(" + cstr(mv.type_name) + ", " + cstr(mv.tag)
               + ", 0, NULL)";
      std::string vals = "(Value*[]){ ";
      for (const IR::Atom *f : mv.fields) vals += atom(f) + ", ";
      return "THxRT_variant(" + cstr(mv.type_name) + ", " + cstr(mv.tag) + ", "
             + std::to_string(mv.fields.size()) + ", " + vals + "})";
    }
    case IR::EKind::Unk: return "THxRT_unk()";
    case IR::EKind::Extern:
    {
      auto  &x   = std::get<IR::Extern>(e->as);
      size_t idx = externs.size();
      externs.push_back(&x);
      return "THxRT_extern(" + std::to_string(idx) + ", "
             + std::to_string(x.arg_types.size()) + ")";
    }
    case IR::EKind::Let:
    case IR::EKind::App:
    case IR::EKind::Case:
    case IR::EKind::Handle:
      UT_FAIL_MSG("%s", "CC: value_expr on a non-value expression");
    }
    UT_FAIL_MSG("%s", "CC: unhandled IR::Expr in value_expr");
    return "THxRT_unk()";
  }

  // Does `e` perform a call (any App) or install a Handle anywhere in the value
  // it produces? If so a `let` RHS must deliver through a continuation block
  // rather than be emitted inline -- emit_pure_into has no way to make a call.
  // This counts *tail* Apps too: a `let` RHS is never in tail position, so a
  // tail App bound there (a lazy constructor field the ANF marked "like a
  // lambda body") is really a normal call.
  bool
  has_call(
    const IR::Expr *e)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret:
    case IR::EKind::MkStruct:
    case IR::EKind::Field:
    case IR::EKind::MkVariant:
    case IR::EKind::Unk:
    case IR::EKind::Extern   : return false;
    case IR::EKind::App      :
    case IR::EKind::Handle   : return true;
    case IR::EKind::Let:
    {
      auto &l = std::get<IR::Let>(e->as);
      return has_call(l.rhs) || has_call(l.body);
    }
    case IR::EKind::Case:
    {
      auto &c = std::get<IR::Case>(e->as);
      for (const IR::Alt &al : c.alts)
        if (has_call(al.body)) return true;
      return has_call(c.deflt);
    }
    }
    return false;
  }

  // Deliver a finished value C-expression `val` to its sink.
  void
  deliver(
    const std::string &val, const Sink &sink, std::string &out)
  {
    if (sink.kind == Sink::RET)
      out += "  THxK_ret(" + val + ");\n";
    else
    {
      out += "  THxK_backpatch(fr, " + std::to_string(sink.slot) + ", " + val
             + ");\n";
      out += "  THxK_jump(blk_" + std::to_string(sink.cont) + ");\n";
    }
  }

  // Compute a pure (suspension-free) expression's value into frame `slot` (a
  // let-box back-patch), emitted inline. Never reaches App/Handle.
  void
  emit_pure_into(
    const IR::Expr *e, size_t slot, std::string &out)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret:
    case IR::EKind::MkStruct:
    case IR::EKind::Field:
    case IR::EKind::MkVariant:
    case IR::EKind::Unk:
    case IR::EKind::Extern:
      out += "  THxK_backpatch(fr, " + std::to_string(slot) + ", "
             + value_expr(e) + ");\n";
      break;
    case IR::EKind::Case: emit_case(e, slot_sink_pure(slot), out); break;
    case IR::EKind::Let:
    {
      auto &l = std::get<IR::Let>(e->as);
      out += "  THxK_setbox(fr, " + std::to_string(l.slot) + ");\n";
      emit_pure_into(l.rhs, l.slot, out);
      emit_pure_into(l.body, slot, out);
    }
    break;
    case IR::EKind::App:
    case IR::EKind::Handle:
      UT_FAIL_MSG("%s", "CC: emit_pure_into on a suspending expression");
    }
  }

  // A sink that means "back-patch this slot then fall through" (used by a pure
  // Case's branches). Encoded as SLOT with cont == NO_CONT so `deliver` knows
  // to omit the jump. See deliver / emit_case usage.
  static constexpr size_t NO_CONT = (size_t)-1;
  Sink
  slot_sink_pure(
    size_t slot)
  {
    return Sink{ Sink::SLOT, slot, NO_CONT };
  }

  // ---- the core continuation-passing lowering ----
  void
  emit_expr(
    const IR::Expr *e, const Sink &sink, std::string &out)
  {
    switch (IR::ekind(e))
    {
    case IR::EKind::Ret:
    case IR::EKind::MkStruct:
    case IR::EKind::Field:
    case IR::EKind::MkVariant:
    case IR::EKind::Unk:
    case IR::EKind::Extern   : deliver(value_expr(e), sink, out); break;

    case IR::EKind::App:
    {
      auto       &a  = std::get<IR::App>(e->as);
      std::string fn = atom(a.fn), arg = atom(a.arg);
      // Tail-ness follows the sink, not the ANF `tail` flag: only a RET sink is
      // a genuine tail position. A call the ANF marked tail (a lazy constructor
      // field, normalized "like a lambda body") but which IR lowering hoisted
      // into a `let` is delivered into a slot -- a normal, non-tail call.
      if (sink.kind == Sink::RET)
        out += "  THxK_tailcall(" + fn + ", " + arg + ");\n";
      else
        out += "  THxK_apply(fr, " + fn + ", " + arg + ", blk_"
               + std::to_string(sink.cont) + ", " + std::to_string(sink.slot)
               + ");\n";
    }
    break;

    case IR::EKind::Let   : emit_let(e, sink, out); break;
    case IR::EKind::Case  : emit_case(e, sink, out); break;
    case IR::EKind::Handle: emit_handle(e, sink, out); break;
    }
  }

  void
  emit_let(
    const IR::Expr *e, const Sink &sink, std::string &out)
  {
    auto &l = std::get<IR::Let>(e->as);
    out += "  THxK_setbox(fr, " + std::to_string(l.slot) + ");\n";
    if (has_call(l.rhs))
    {
      // The body becomes a continuation block; the rhs delivers into the slot
      // then control transfers there.
      size_t body_blk = reserve_block();
      emit_expr(l.rhs, Sink{ Sink::SLOT, l.slot, body_blk }, out);
      std::string body_out;
      emit_expr(l.body, sink, body_out);
      set_block(body_blk, body_out);
    }
    else
    {
      // Pure rhs: compute into the slot inline, then continue with the body in
      // the same block.
      emit_pure_into(l.rhs, l.slot, out);
      emit_expr(l.body, sink, out);
    }
  }

  void
  emit_case(
    const IR::Expr *e, const Sink &sink, std::string &out)
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
        cond
          = "THxVALUE_as_int(" + s + ") == " + std::to_string(al.ival) + "LL";
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
          out += "  THxK_setlocal(fr, " + std::to_string(al.binder_base + i)
                 + ", THxVALUE_variant_field(" + s + ", " + std::to_string(i)
                 + "));\n";
      emit_branch(al.body, sink, out);
      out += "  }\n";
      first = false;
    }
    out += first ? "  {\n" : "  else {\n";
    emit_branch(c.deflt, sink, out);
    out += "  }\n";
  }

  // A case branch: either a real sink (RET / SLOT-with-cont) or the "pure-into"
  // sink (SLOT with NO_CONT, meaning back-patch and fall through -- the
  // enclosing pure let continues after the if/else).
  void
  emit_branch(
    const IR::Expr *e, const Sink &sink, std::string &out)
  {
    if (sink.kind == Sink::SLOT && sink.cont == NO_CONT)
      emit_pure_into(e, sink.slot, out);
    else
      emit_expr(e, sink, out);
  }

  void
  emit_handle(
    const IR::Expr *e, const Sink &sink, std::string &out)
  {
    auto       &h     = std::get<IR::Handle>(e->as);
    size_t      hbody = reserve_block();
    std::string ops = "(const char*[]){ ", cls = "(Value*[]){ ";
    for (const IR::HandleClause &c : h.clauses)
    {
      ops += cstr(c.op) + ", ";
      cls += atom(c.fn) + ", ";
    }
    ops += "}";
    cls += "}";
    std::string n   = std::to_string(h.clauses.size());
    std::string els = atom(h.els);
    // cont == NULL means "deliver the handled value to the current
    // continuation" (a RET sink); otherwise push a KRet for (cont, slot).
    std::string cont
      = (sink.kind == Sink::RET) ? "NULL" : "blk_" + std::to_string(sink.cont);
    std::string slot = std::to_string(sink.kind == Sink::RET ? 0 : sink.slot);
    if (h.clauses.size() == 0) // no clauses -> empty tables are still valid
    {
      ops = "NULL";
      cls = "NULL";
    }
    out += "  THxK_handle(fr, " + cont + ", " + slot + ", " + ops + ", " + cls
           + ", " + n + ", " + els + ", blk_" + std::to_string(hbody) + ");\n";
    std::string body_out;
    emit_expr(h.body, Sink{ Sink::RET }, body_out);
    set_block(hbody, body_out);
  }

  // Emit one code as its entry block (plus any continuation blocks it spawns).
  void
  emit_code(
    size_t id)
  {
    size_t entry   = reserve_block();
    code_entry[id] = entry;
    std::string out;
    emit_expr(prog.codes[id].body, Sink{ Sink::RET }, out);
    set_block(entry, out);
  }

  // Emit one foreign-call wrapper per collected extern. Each foreign symbol
  // is a DIRECT call resolved by the system linker (static or dynamic per
  // the link line -- no dlopen in generated programs): the declaration uses
  // an asm label so its C identifier (`THx_sym_N`) cannot collide with the
  // libc headers the inlined runtime already included, while the assembler
  // symbol is the real one. The prototype is the Thrax-widened signature --
  // the same ABI gamble the old cast-through-dlsym made, made honest per
  // target when C-int marshalling lands. Call after all code is emitted.
  void
  emit_externs(
    std::string &out, const TG::Target &tg)
  {
    for (size_t i = 0; i < externs.size(); ++i)
    {
      const IR::Extern &e    = *externs[i];
      std::string       ret  = std::string(e.ret_type);
      std::string       cret = c_type(ret, true, tg);

      std::vector<std::string> cargs;
      std::vector<size_t>      pos;
      for (size_t j = 0; j < e.arg_types.size(); ++j)
      {
        std::string t(e.arg_types[j]);
        if (t == OP::TY_UNIT) continue; // `{}` arg: not passed
        cargs.push_back(c_type(t, false, tg));
        pos.push_back(j);
      }
      std::string proto;
      for (size_t k = 0; k < cargs.size(); ++k)
        proto += (k ? ", " : "") + cargs[k];
      if (proto.empty()) proto = "void";

      std::string id  = std::to_string(i);
      std::string sym = "THx_sym_" + id;
      out += "extern " + cret + " " + sym + "(" + proto + ") __asm__("
             + cstr(e.symbol) + ");\n";
      out += "static Value* THx_extern_" + id + "(Value** args) {\n";
      out += "  (void)args;\n";

      std::string callargs;
      for (size_t k = 0; k < pos.size(); ++k)
      {
        std::string t(e.arg_types[pos[k]]);
        std::string a = "a" + std::to_string(k);
        out += "  " + cargs[k] + " " + a + " = "
               + marshal_arg(t, "args[" + std::to_string(pos[k]) + "]", tg)
               + ";\n";
        callargs += (k ? ", " : "") + a;
      }

      if (ret == OP::TY_UNIT)
      {
        out += "  " + sym + "(" + callargs + ");\n";
        out += "  return THxRT_unk();\n";
      }
      else
      {
        out += "  " + cret + " r = " + sym + "(" + callargs + ");\n";
        out += "  " + marshal_ret(ret, "r", tg) + "\n";
      }
      out += "}\n\n";
    }
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
  // The native backend now compiles the whole IR -- the strict subset, C FFI,
  // and algebraic effects (handlers / operations / resumptions / `defer`) via
  // the CEK driver. Nothing is statically rejected; affine (one-shot)
  // resumption use is enforced at runtime. The seam remains so a future
  // not-yet-lowered feature can be reported here.
  (void)prog;
  return std::nullopt;
}

namespace
{

// Collect the symbolic library names of every foreign binding under `e`.
// Externs nest only in Let/Case/Handle bodies (aggregate fields are atoms).
void
collect_libs(
  const IR::Expr *e, std::vector<std::string> &libs)
{
  switch (IR::ekind(e))
  {
  case IR::EKind::Extern:
  {
    std::string lib{ std::get<IR::Extern>(e->as).lib };
    for (const std::string &l : libs)
      if (l == lib) return;
    libs.push_back(lib);
    return;
  }
  case IR::EKind::Let:
  {
    auto &l = std::get<IR::Let>(e->as);
    collect_libs(l.rhs, libs);
    collect_libs(l.body, libs);
    return;
  }
  case IR::EKind::Case:
  {
    auto &c = std::get<IR::Case>(e->as);
    for (const IR::Alt &a : c.alts) collect_libs(a.body, libs);
    collect_libs(c.deflt, libs);
    return;
  }
  case IR::EKind::Handle:
    collect_libs(std::get<IR::Handle>(e->as).body, libs);
    return;
  case IR::EKind::Ret:
  case IR::EKind::App:
  case IR::EKind::MkStruct:
  case IR::EKind::Field:
  case IR::EKind::MkVariant:
  case IR::EKind::Unk      : return;
  }
}

} // namespace

std::vector<std::string>
link_flags(
  const IR::Program &prog)
{
  std::vector<std::string> libs;
  for (const IR::Code &c : prog.codes) collect_libs(c.body, libs);

  std::vector<std::string> flags;
  for (const std::string &lib : libs)
  {
    if (lib == "libc") continue; // always linked
    if (lib.find('/') != std::string::npos
        || lib.find('.') != std::string::npos)
    {
      flags.push_back(lib); // explicit path/soname: verbatim, user's problem
      continue;
    }
    flags.push_back("-l" + (lib.starts_with("lib") ? lib.substr(3) : lib));
  }
  return flags;
}

std::string
emit(
  const IR::Program &prog,
  UT::Vu             entry,
  bool               entry_takes_arg,
  const TG::Target  &tg)
{
  Emitter em{ prog };
  for (size_t i = 0; i < prog.codes.size(); ++i) em.emit_code(i);

  std::string out;
  out += "/* Generated by the Thrax CC backend for target " + tg.name()
         + ". Do not edit.\n";
  out += " * Self-contained -- the runtime is inlined below. Build:\n";
  std::string flags;
  for (const std::string &f : link_flags(prog)) flags += " " + f;
  out += " *   cc -O2 prog.c -o prog" + flags + " */\n\n";
  out += "#define THX_INT \"" + std::string(tg.int_ty()) + "\"\n";
  out += "_Static_assert(sizeof(void*) == " + std::to_string(tg.ptr_bits() / 8)
         + ", \"this program was generated for " + tg.name()
         + "; compile it with a C compiler targeting it\");\n\n";
  // The whole C runtime, baked into the compiler and emitted inline, so the
  // generated program is a single self-contained translation unit.
  out += RUNTIME_C;
  out += "\n#include <string.h>\n\n";

  // Forward declarations of every block, then the block definitions (blocks
  // reference each other's continuations freely).
  for (size_t i = 0; i < em.blocks.size(); ++i)
    out += "static void blk_" + std::to_string(i) + "(Frame*, Value*);\n";
  out += "\n";
  for (const std::string &b : em.blocks) out += b;

  // Foreign-call wrappers (only if the program uses `@extern`): direct
  // linker-resolved calls, no dlopen anywhere in generated code.
  if (!em.externs.empty())
  {
    out += "#include <stdint.h>\n\n"; // intptr_t, for Ptr marshalling
    em.emit_externs(out, tg);
  }

  // The foreign-wrapper table the runtime dispatches T_EXTERN through (always
  // defined; a 1-element dummy when there are no foreign calls).
  if (em.externs.empty())
    out += "ExternFn THxRT_extern_table[1] = { 0 };\n";
  else
  {
    out += "ExternFn THxRT_extern_table[] = {\n";
    for (size_t i = 0; i < em.externs.size(); ++i)
      out += "  THx_extern_" + std::to_string(i) + ",\n";
    out += "};\n";
  }
  out += "const size_t THxRT_extern_count = "
         + std::to_string(em.externs.size()) + ";\n\n";

  // The dispatch tables the runtime calls back through: entry block + slot
  // count per lifted Code.
  out += "BlockFn THxRT_code_table[] = {\n";
  for (size_t i = 0; i < prog.codes.size(); ++i)
    out += "  blk_" + std::to_string(em.code_entry[i]) + ",\n";
  out += "};\n";
  out += "const size_t THxRT_code_nlocals[] = {\n";
  for (size_t i = 0; i < prog.codes.size(); ++i)
    out += "  " + std::to_string(prog.codes[i].nlocals) + ",\n";
  out += "};\n";
  out += "const size_t THxRT_code_count = " + std::to_string(prog.codes.size())
         + ";\n\n";

  // Globals: name -> CAF code index, lazily forced and memoized (matches
  // IT::glob). Stable iteration order for reproducible output.
  std::vector<std::pair<std::string, size_t>> globs(prog.globals.begin(),
                                                    prog.globals.end());
  std::sort(globs.begin(), globs.end());
  size_t ng = globs.size();

  if (ng > 0)
  {
    out += "static Value* g_cache[" + std::to_string(ng) + "];\n";
    out += "static char g_inited[" + std::to_string(ng) + "];\n";
    out += "static char g_inprog[" + std::to_string(ng) + "];\n\n";
  }

  out += "Value* THxRT_glob(const char* name) {\n";
  for (size_t k = 0; k < ng; ++k)
  {
    UT::Vu nm{ globs[k].first.data(), globs[k].first.size() };
    out += "  if (strcmp(name, " + cstr(nm) + ") == 0) {\n";
    out += "    if (g_inited[" + std::to_string(k) + "]) return g_cache["
           + std::to_string(k) + "];\n";
    out += "    if (g_inprog[" + std::to_string(k)
           + "]) THxCHECK_FAILF(\"cyclic value global '%s'\", name);\n";
    out += "    g_inprog[" + std::to_string(k) + "] = 1;\n";
    out += "    g_cache[" + std::to_string(k) + "] = THxK_run_code("
           + std::to_string(globs[k].second) + ");\n";
    out += "    g_inited[" + std::to_string(k) + "] = 1;\n";
    out += "    g_inprog[" + std::to_string(k) + "] = 0;\n";
    out += "    return g_cache[" + std::to_string(k) + "];\n";
    out += "  }\n";
  }
  out += "  return THxRT_builtin(name);\n";
  out += "}\n\n";

  // Force every global -- the compiled mirror of the interpreter smoke test
  // (tst/TS.cpp), so a runtime fault in any top-level definition surfaces.
  out += "static void THx_force_all(void) {\n";
  for (size_t k = 0; k < ng; ++k)
  {
    UT::Vu nm{ globs[k].first.data(), globs[k].first.size() };
    out += "  (void)THxRT_glob(" + cstr(nm) + ");\n";
  }
  out += "}\n\n";

  // Release the memoized CAF cache (each entry owns its value), so a clean run
  // ends with zero live allocations and the leak check below can be exact.
  out += "static void THx_release_globals(void) {\n";
  for (size_t k = 0; k < ng; ++k)
  {
    std::string ks = std::to_string(k);
    out += "  if (g_inited[" + ks + "]) { THxMEM_release(g_cache[" + ks
           + "]); g_inited[" + ks + "] = 0; }\n";
  }
  out += "}\n\n";

  // Entry: force all globals, then run the entry point if there is one; its
  // Int result is the process exit code. Epilogue: release the globals and the
  // temp pool, then assert nothing is still allocated -- the built-in LEAK
  // CHECK. Under the RC engine a leak exits 97 (so every native test doubles
  // as a leak regression test); under -DTHX_MEM_BUMP live() is 0 and the check
  // passes trivially.
  out += "int main(void) {\n";
  out += "  THx_force_all();\n";
  out += "  int code = 0;\n";
  if (entry.size() > 0)
  {
    out += "  Value* e = THxRT_glob(" + cstr(entry) + ");\n";
    if (entry_takes_arg)
    {
      out += "  Value* r = THxK_call(e, THxRT_str(\"\", 0));\n";
      out += "  code = (int)THxVALUE_as_int(r);\n";
      out += "  THxMEM_release(r);\n";
    }
    else
      out += "  code = (int)THxVALUE_as_int(e);\n"; // a cache borrow
  }
  out += "  THx_release_globals();\n";
  out += "  THxMEM_pool_drain(0);\n";
  out += "  size_t leaked = THxMEM_live();\n";
  out += "  if (leaked) {\n";
  out += "    fprintf(stderr, \"THxMEM: %zu allocation(s) leaked\\n\", "
         "leaked);\n";
  out += "    return 97;\n";
  out += "  }\n";
  out += "  return code;\n";
  out += "}\n";

  return out;
}

} // namespace CC
