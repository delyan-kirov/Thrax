#ifndef ITxDATA_HEADER_
#define ITxDATA_HEADER_

#include "CR.hpp"
#include "OP.hpp"

namespace IT
{

/*------------------------------------------------------------------------------
 *\RUNTIME VALUES
 *
 * A Value is what `eval` produces from a CR::Term. Unlike the Core (which is
 * immutable, arena-owned, raw-pointer code), values have dynamic lifetimes --
 * closures escape, thunks memoize, data is built and discarded -- so they are
 * REFERENCE-COUNTED (std::shared_ptr). The two worlds meet only by pointer: a
 * closure/thunk holds the CR::Term it will run (the Core arena outlives every
 * value), and forcing a Core literal copies its bytes into an owning value.
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

// A closure: the lambda body (Core) plus the environment captured where the
// lambda was evaluated. `param` is kept for diagnostics; binding is positional.
struct VClosure
{
  const CR::Term *body;
  UT::Vu          param;
  ValEnv          env;
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

// A foreign function as a first-class, curried value. `decl` points at its Core
// declaration (which carries the symbol, library and marshalling types); `args`
// accumulates operands until saturated, at which point the C call is made (see
// eval / call_extern).
struct VExtern
{
  const CR::Extern *decl;
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
// declared order. Data constructors are non-strict, so each field is a VThunk,
// forced only on demand -- this is the one place the call-by-value runtime
// defers work, so recursive / infinite values terminate.
struct VVariant
{
  UT::Vu            type_name;
  UT::Vu            tag;
  std::vector<pVal> fields;
};

// A suspended computation: a lazy data-constructor field. `expr` is the Core
// evaluated in the captured `env` the first time the thunk is forced; `memo`
// caches the (forced) result for every later demand.
struct VThunk
{
  const CR::Term *expr;
  ValEnv          env;
  pVal            memo; // null until forced
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

#define IT_VALUE_VARIANTS                                                      \
  X(Unk, VUnk)                                                                 \
  X(Int, VInt)                                                                 \
  X(Real, VReal)                                                               \
  X(Str, VStr)                                                                 \
  X(Closure, VClosure)                                                         \
  X(Builtin, VBuiltin)                                                         \
  X(Extern, VExtern)                                                           \
  X(Struct, VStruct)                                                           \
  X(Variant, VVariant)                                                         \
  X(Thunk, VThunk)                                                             \
  X(Rec, VRec)

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
pVal call_extern(const CR::Extern &e, const std::vector<pVal> &args);

std::string pprint(const pVal &v, int level = 0);

} // namespace IT

#endif // ITxDATA_HEADER_
