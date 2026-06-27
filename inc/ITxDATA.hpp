#ifndef ITxDATA_HEADER_
#define ITxDATA_HEADER_

#include "CR.hpp"
#include "IR.hpp"
#include "OP.hpp"

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
// (see docs/effect-system-design.md §1a), not lazy data.
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
  X(Resump, VResump)

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
  { OP::mono(OP::ADD, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) + as_int(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) - as_int(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) * as_int(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) / as_int(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) % as_int(a[1]));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) == as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) >= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) <= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_INT),
    { 2,
      [](const std::vector<pVal> &a) {
        return mk_int(as_int(a[0]) > as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_INT),
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
  { OP::mono(OP::NEG, OP::TY_INT),
    { 1, [](const std::vector<pVal> &a) { return mk_int(-as_int(a[0])); } } },
  { OP::mono(OP::NEG, OP::TY_REAL),
    { 1, [](const std::vector<pVal> &a) { return mk_real(-as_num(a[0])); } } },
  { OP::mono(OP::NOT, OP::TY_INT),
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
