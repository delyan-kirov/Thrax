#ifndef ITxDATA_HEADER_
#define ITxDATA_HEADER_

#include "CR.hpp"
#include "IR.hpp"
#include "OP.hpp"
#include "TG.hpp"

namespace IT
{
/*------------------------------------------------------------------------------
 *\RUNTIME VALUES
 *
 * A Value is what the reified-K machine (ITxMACHINE) produces from the IR.
 * Unlike the IR/Core (immutable, arena-owned, raw-pointer code), values have
 * dynamic lifetimes -- closures escape, data is built and discarded -- so they
 * are REFERENCE-COUNTED (std::shared_ptr). The two worlds meet only by index/
 * pointer: a closure (VCode) holds the IR code it will run (the arena outlives
 * every value), and a literal is copied into an owning value when evaluated.
 * Evaluation is strict: there are no thunks.
 *
 * Like the Core, a Value is a tagged union with no stored tag -- the variant is
 * the discriminant and `kind()` reads it back.
 *-----------------------------------------------------------------------------*/

struct Value;
using pVal   = std::shared_ptr<Value>;
using ValEnv = std::vector<pVal>;

struct VUnk
{
};

struct VInt
{
  ssize_t val;
};

struct VReal
{
  double val;
};

struct VStr
{
  std::string val; // owned: literals are copied in, FFI results are fresh
};

// A built-in operation as a first-class, curried value. `impl` is the
// monomorphic key the type checker resolved an overloaded use to (e.g. "+@Int",
// a view into the Core arena); `args` accumulates applied operands until it
// reaches `arity`, at which point the implementation runs (see eval).
struct VBuiltin
{
  UT::Vu            impl;
  size_t            arity;
  std::vector<pVal> args;
};

// A foreign function as a first-class, curried value. `decl` points at its IR
// node (which carries the symbol, library and marshalling types); `args`
// accumulates operands until saturated, at which point the C call is made (see
// the machine's App handling / call_extern).
struct VExtern
{
  const IR::Extern *decl;
  std::vector<pVal> args;
};

// A struct value: the type name plus its fields in declaration order, every
// field already forced.
struct VStruct
{
  UT::Vu                               name;
  std::vector<std::pair<UT::Vu, pVal>> fields;
};

// A sum value: the union type, the variant `tag`, and the payload `fields` in
// declared order. Evaluation is strict -- every field is already a forced value
// (no thunks). Coinductive / infinite structures are `@codata`, a separate kind
// (see docs/effect-system-design.md section 1a), not lazy data.
struct VVariant
{
  UT::Vu            type_name;
  UT::Vu            tag;
  std::vector<pVal> fields;
};

// A self-reference cell for a `let` binding. A binding's value (typically a
// closure) captures the environment the binding lives in, which would otherwise
// hold a shared_ptr back to the value -- a cycle that leaks. While the value is
// built the binding is held *weakly* through one of these; eval's VAR lookup
// locks `target` to recover it (always live: the body keeps it in scope).
struct VRec
{
  std::weak_ptr<Value> target;
};

// A closure built by the reified-K machine (ITxMACHINE): the lifted `code`
// (index into IR::Program::codes) plus its captured environment, read
// positional as `Env i`.
struct VCode
{
  size_t code;
  ValEnv env;
};

// An effect operation as a first-class value (the surface "calls" an operation
// to perform it). Applying it searches the continuation stack for the nearest
// handler with a clause for `name`, captures the continuation, and runs that
// clause. Operations are unary.
struct VOp
{
  std::string name;
};

// A captured, first-class continuation (resumption) -- the slice of the reified
// continuation stack from a handler's prompt up to a perform point. Applying it
// resumes the suspended computation with the supplied value. AFFINE: it may be
// applied at most once (enforced in the machine via Resumption::used). `seg`
// holds the captured K-frames; the frame type lives in IT.hpp, so it is opaque
// here (a shared_ptr to the forward-declared Resumption).
struct Resumption;
struct VResump
{
  std::shared_ptr<Resumption> seg;
};

// The `defer` intrinsic as a first-class, curried value: `defer action
// cleanup` runs `action {}` with `cleanup` registered to run when the action's
// scope exits -- on normal completion (a value returning through the installed
// KDefer) or on discard (the continuation dropped un-resumed). `args` collects
// the two thunks before the machine installs the cleanup and runs the action.
struct VDefer
{
  std::vector<pVal> args;
};

#define IT_VALUE_VARIANTS                                                      \
  X(Unk, VUnk)                                                                 \
  X(Int, VInt)                                                                 \
  X(Real, VReal)                                                               \
  X(Str, VStr)                                                                 \
  X(Builtin, VBuiltin)                                                         \
  X(Extern, VExtern)                                                           \
  X(Struct, VStruct)                                                           \
  X(Variant, VVariant)                                                         \
  X(Rec, VRec)                                                                 \
  X(Code, VCode)                                                               \
  X(Op, VOp)                                                                   \
  X(Resump, VResump)                                                           \
  X(Defer, VDefer)

enum class VKind
{
#define X(name, type) name,
  IT_VALUE_VARIANTS
#undef X
};

using ValData =
#define X(name, type) type,
  std::variant<IT_VALUE_VARIANTS std::monostate>
#undef X
  ;

struct Value
{
  ValData as;
};

inline VKind
kind(
  const Value *v)
{
  return static_cast<VKind>(v->as.index());
}
inline VKind
kind(
  const pVal &v)
{
  return static_cast<VKind>(v->as.index());
}

inline pVal
mk(
  Value v)
{
  return std::make_shared<Value>(std::move(v));
}

/*------------------------------------------------------------------------------
 *\BUILTINS
 *-----------------------------------------------------------------------------*/

// Operand extraction / result construction. The type checker resolved the
// overload, so an "@Int" impl only sees Ints. An "@Real" impl may see an Int on
// one side (mixed Int/Real combinations route here), so it reads via `as_num`,
// which coerces an Int operand to double.
ssize_t as_int(const pVal &v);
double  as_num(const pVal &v);
pVal    mk_int(ssize_t v);
pVal    mk_real(double v);
// An Array value: `n` zeroed bytes, backed by the byte-bearing VStr value (the
// type checker keeps Array and Str distinct; the runtime, like every value,
// does not).
pVal mk_bytes(size_t n);

// One built-in implementation: its arity and a function over the already
// evaluated, saturated operands.
struct Impl
{
  size_t arity;
  pVal (*fn)(const std::vector<pVal> &);
};

// The dispatch table, keyed by the monomorphic key the type checker resolved
// each overloaded use to (see TC's overload_db). Const, so it has internal
// linkage and may live in this header.
const std::unordered_map<std::string, Impl> impls{
  // `@array.{ n }` -- allocate n zeroed bytes (see EX::parse_array,
  // OP::ARR_ALLOC).
  { OP::ARR_ALLOC,
    { 1,
      [](const std::vector<pVal> &a) {
        ssize_t n = as_int(a[0]);
        return mk_bytes(n < 0 ? 0 : (size_t)n);
      } } },
  // Growable byte-vector reads (see doc/strings-and-arrays.md). The Array/Str
  // bytes live in the VStr string; a byte is an Int in 0..255.
  { OP::ARR_LEN,
    { 1,
      [](const std::vector<pVal> &a) {
        return mk_int((ssize_t)std::get<VStr>(a[0]->as).val.size());
      } } },
  { OP::ARR_CAP,
    { 1,
      [](const std::vector<pVal> &a) {
        return mk_int((ssize_t)std::get<VStr>(a[0]->as).val.capacity());
      } } },
  { OP::ARR_GET,
    { 2,
      [](const std::vector<pVal> &a) {
        const std::string &s = std::get<VStr>(a[0]->as).val;
        ssize_t            i = as_int(a[1]);
        UT_FAIL_IF(i < 0 || (size_t)i >= s.size()); // out-of-bounds read
        return mk_int((unsigned char)s[(size_t)i]);
      } } },
  { OP::VEC_NEW,
    { 1,
      [](const std::vector<pVal> &) {
        return mk(Value{
          VVariant{ UT::Vu{ OP::VEC_REP }, UT::Vu{ OP::VEC_REP }, {} } });
      } } },
  { OP::VEC_FILL,
    { 2,
      [](const std::vector<pVal> &a) {
        ssize_t           n = as_int(a[0]);
        std::vector<pVal> fs((size_t)(n < 0 ? 0 : n), a[1]);
        return mk(Value{ VVariant{
          UT::Vu{ OP::VEC_REP }, UT::Vu{ OP::VEC_REP }, std::move(fs) } });
      } } },
  { OP::VEC_LEN,
    { 1,
      [](const std::vector<pVal> &a) {
        return mk_int((ssize_t)std::get<VVariant>(a[0]->as).fields.size());
      } } },
  { OP::VEC_GET,
    { 2,
      [](const std::vector<pVal> &a) {
        const std::vector<pVal> &fs = std::get<VVariant>(a[0]->as).fields;
        ssize_t                  i  = as_int(a[1]);
        UT_FAIL_IF(i < 0 || (size_t)i >= fs.size()); // out-of-bounds read
        return fs[(size_t)i];
      } } },
  { OP::VEC_SET,
    { 3,
      [](const std::vector<pVal> &a) {
        std::vector<pVal> fs = std::get<VVariant>(a[0]->as).fields; // copy
        ssize_t           i  = as_int(a[1]);
        UT_FAIL_IF(i < 0 || (size_t)i >= fs.size());
        fs[(size_t)i] = a[2];
        return mk(Value{ VVariant{
          UT::Vu{ OP::VEC_REP }, UT::Vu{ OP::VEC_REP }, std::move(fs) } });
      } } },
  { OP::VEC_PUSH,
    { 2,
      [](const std::vector<pVal> &a) {
        std::vector<pVal> fs = std::get<VVariant>(a[0]->as).fields; // copy
        fs.push_back(a[1]);
        return mk(Value{ VVariant{
          UT::Vu{ OP::VEC_REP }, UT::Vu{ OP::VEC_REP }, std::move(fs) } });
      } } },
  // Growable byte-vector mutators. The interpreter is the semantic reference:
  // it always returns a fresh value (value semantics are identical to the
  // native backend's opportunistic in-place, which is a pure optimization).
  { OP::ARR_PUSH,
    { 2,
      [](const std::vector<pVal> &a) {
        std::string s = std::get<VStr>(a[0]->as).val; // copy
        s.push_back((char)(unsigned char)as_int(a[1]));
        return mk(Value{ VStr{ std::move(s) } });
      } } },
  { OP::ARR_SET,
    { 3,
      [](const std::vector<pVal> &a) {
        std::string s = std::get<VStr>(a[0]->as).val; // copy
        ssize_t     i = as_int(a[1]);
        UT_FAIL_IF(i < 0 || (size_t)i >= s.size());
        s[(size_t)i] = (char)(unsigned char)as_int(a[2]);
        return mk(Value{ VStr{ std::move(s) } });
      } } },
  { OP::ARR_SLICE,
    { 3,
      [](const std::vector<pVal> &a) {
        const std::string &s   = std::get<VStr>(a[0]->as).val;
        ssize_t            beg = as_int(a[1]);
        ssize_t            end = as_int(a[2]);
        if (beg < 0) beg = 0;
        if (end > (ssize_t)s.size()) end = (ssize_t)s.size();
        if (end < beg) end = beg;
        return mk(Value{ VStr{ s.substr((size_t)beg, (size_t)(end - beg)) } });
      } } },
  // `++` concatenation (Str or Array -- one byte-concat impl). Interpreter
  // copies (reference semantics); the native backend reuses the lhs buffer when
  // unique.
  { OP::CONCAT_IMPL,
    { 2,
      [](const std::vector<pVal> &a) {
        std::string s = std::get<VStr>(a[0]->as).val; // copy lhs
        s += std::get<VStr>(a[1]->as).val;            // append rhs bytes
        return mk(Value{ VStr{ std::move(s) } });
      } } },
  // Str byte-equality (`?=@Str`), backing exact string pattern matching.
  { OP::mono(OP::ISEQ, OP::TY_STR),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(
          std::get<VStr>(a[0]->as).val == std::get<VStr>(a[1]->as).val ? 1 : 0);
      } } },
  // The integer families. Both widths are always registered -- the mono key
  // the type checker wrote (which folds `Int` to the TARGET word) selects
  // the semantics, so compile-time evaluation of a wasm32 program observes
  // 32-bit wrap on this 64-bit host, exactly like the emitted program will
  // (THxPLAT.h's truncate-on-store). The @int32 arithmetic wraps its result;
  // comparisons need no wrap (in-range operands, checked literals).
  { OP::mono(OP::ADD, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) + as_int(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) - as_int(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) * as_int(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) / as_int(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) % as_int(a[1]));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) == as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) >= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) <= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) > as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_INT64),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) < as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::ADD, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int((int32_t)(as_int(a[0]) + as_int(a[1])));
      } } },
  { OP::mono(OP::SUB, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int((int32_t)(as_int(a[0]) - as_int(a[1])));
      } } },
  { OP::mono(OP::MUL, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int((int32_t)(as_int(a[0]) * as_int(a[1])));
      } } },
  { OP::mono(OP::DIV, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int((int32_t)(as_int(a[0]) / as_int(a[1])));
      } } },
  { OP::mono(OP::MOD, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int((int32_t)(as_int(a[0]) % as_int(a[1])));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) == as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) >= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) <= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) > as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_INT32),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) < as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::ADD, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_real(as_num(a[0]) + as_num(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_real(as_num(a[0]) - as_num(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_real(as_num(a[0]) * as_num(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_real(as_num(a[0]) / as_num(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_real(std::fmod(as_num(a[0]), as_num(a[1])));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_num(a[0]) == as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_num(a[0]) >= as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_num(a[0]) <= as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_num(a[0]) > as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_REAL),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_num(a[0]) < as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::NEG, OP::TY_INT64),
    { 1, [](const std::vector<pVal> &a) { return mk_int(-as_int(a[0])); } } },
  { OP::mono(OP::NEG, OP::TY_INT32),
    { 1,
      [](const std::vector<pVal> &a) {
        return mk_int((int32_t)-as_int(a[0]));
      } } },
  { OP::mono(OP::NEG, OP::TY_REAL),
    { 1, [](const std::vector<pVal> &a) { return mk_real(-as_num(a[0])); } } },
  { OP::mono(OP::NOT, OP::TY_INT64),
    { 1,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) == 0 ? 1 : 0);
      } } },
  { OP::mono(OP::NOT, OP::TY_INT32),
    { 1,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) == 0 ? 1 : 0);
      } } },
};

// Marshal the saturated arguments and make the foreign call described by `e`.
pVal call_extern(const IR::Extern &e, const std::vector<pVal> &args);

std::string pprint(const pVal &v, int level = 0);

} // namespace IT

#endif // ITxDATA_HEADER_
