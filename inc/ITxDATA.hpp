#ifndef ITxDATA_HEADER_
#define ITxDATA_HEADER_

#include "EX.hpp"
#include "OP.hpp"

namespace IT
{

struct Lm;
using pLm     = std::shared_ptr<Lm>;
using StatEnv = std::unordered_map<std::string, pLm>;
using DynEnv  = std::vector<pLm>;

struct Int
{
  ssize_t unwrap;
};

struct Real
{
  double unwrap;
};

// An application `fn arg`. After the ANF pass (see IT.cpp) `fn` and `arg` are
// always atomic (a variable, literal, or lambda), and `tail` records whether
// the call sits in program tail position -- the property the trampoline in eval
// relies on and that a future optimizer (e.g. fusion over a normalized core)
// can read directly.
struct App
{
  pLm  fn;
  pLm  arg;
  bool tail = false;
};

struct Var
{
  std::string unwrap;
  size_t      idx;
  DynEnv      env;
};

struct Str
{
  std::string unwrap;
};

struct Fun
{
  Var var;
  pLm body;
};

// A (possibly recursive) local binding. `var` is in scope while `val` is
// evaluated, so a value that refers to itself resolves once `val` is
// back-patched into the binding slot (see eval).
struct Let
{
  Var var;
  pLm val;
  pLm body;
};

// A foreign binding. `arg_types`/`ret_type` are Thrax type names taken from the
// signature; `args` accumulates curried arguments until the function is
// saturated, at which point the C call is made (see eval / call_extern).
struct Extern
{
  std::string              symbol;
  std::string              lib;
  std::vector<std::string> arg_types;
  std::string              ret_type;
  std::vector<pLm>         args;
};

// A built-in operation as a first-class, curried value. `impl` is the key the
// type checker resolved an overloaded use to (e.g. "+@Int"); `args` accumulates
// applied arguments until it reaches `arity`, at which point the implementation
// runs (see eval). Because it is an ordinary value, a built-in can be partially
// applied and passed around like any function.
struct Builtin
{
  std::string      impl;
  size_t           arity;
  std::vector<pLm> args;
};

// The kind of head an alternative matches on -- shared with the parser AST.
using AltKind = EX::AltKind;

// One alternative of a `Case`. It is taken when the forced scrutinee's head
// matches `kind`: constructor index `tag` (binding the payload positionally to
// `binders`), or the literal `ival` / `rval`. `binders` entries are De Bruijn
// locals in `body`, in payload order; an empty string ignores a slot.
struct CaseAlt
{
  AltKind                  kind;
  std::string              ctor; // Con: constructor name (matched at runtime)
  size_t                   tag  = 0; // Con: constructor index within its type
  ssize_t                  ival = 0; // Int
  double                   rval = 0; // Real
  std::vector<std::string> binders;  // Con: payload binder names, positional
  pLm                      body;
};

// `case scrut of alt... else deflt`. Like the former `If`, this is a dedicated
// lazy node, NOT a function: eval forces `scrut`, then runs only the taken
// alternative's body (binding a Con alt's payload first), or `deflt` if none
// match. `if` and every match lower to a `Case` -- it is the one brancher.
// `if c then t else e` is the single Int alternative `{ 0 -> e }` over `t`.
struct Case
{
  pLm                  scrut;
  std::vector<CaseAlt> alts;
  pLm                  deflt;
};

// A self-reference cell for a `let` binding. A binding's value (typically a
// closure) captures the environment the binding lives in, which would otherwise
// hold a shared_ptr back to the value -- a cycle that leaks. While the value is
// evaluated the binding is therefore held *weakly* through one of these; eval's
// VAR lookup locks `target` to recover it. The value is always live when
// locked: the body keeps it strongly in scope, and a call site keeps the
// closure alive.
struct Rec
{
  std::weak_ptr<Lm> target;
};

// A struct value: the type name plus its fields in declaration order. Built
// from a `Type.{...}` literal; eval forces every field.
struct Struct
{
  std::string                              name;
  std::vector<std::pair<std::string, pLm>> fields;
};

// Field access `record.field`. eval forces the record (a Struct) and returns
// the named field; the type checker guarantees the field exists.
struct Field
{
  pLm         record;
  std::string name;
};

// A sum value: the union type, the variant `tag`, and the payload `fields` in
// the variant's declared order (LL normalizes named construction to
// positional). Built from a `Type.Tag.{...}` literal. Thrax is otherwise
// call-by-value, but data constructors are non-strict: each field is a Thunk,
// forced only on demand, so recursive / infinite values terminate. A `Case` Con
// alternative matches on `tag` and binds `fields` positionally (the thunks
// themselves, so the binders stay lazy until used).
struct Variant
{
  std::string      type_name;
  std::string      tag;
  std::vector<pLm> fields;
};

// A suspended computation -- a lazy data-constructor field, the one place the
// otherwise call-by-value runtime defers work. `expr` is evaluated in the
// captured `env` the first time the thunk is forced; `memo` caches the (forced)
// result for every later demand. Runtime-only: never built by exprs2pLm/ANF.
struct Thunk
{
  pLm    expr;
  DynEnv env;
  pLm    memo; // null until forced
};

#define IT_L_VARIANTS                                                          \
  X(INT, Int)                                                                  \
  X(REAL, Real)                                                                \
  X(STR, Str)                                                                  \
  X(APP, App)                                                                  \
  X(FUN, Fun)                                                                  \
  X(LET, Let)                                                                  \
  X(EXTERN, Extern)                                                            \
  X(BUILTIN, Builtin)                                                          \
  X(CASE, Case)                                                                \
  X(REC, Rec)                                                                  \
  X(STRUCT, Struct)                                                            \
  X(FIELD, Field)                                                              \
  X(VARIANT, Variant)                                                          \
  X(THUNK, Thunk)                                                              \
  X(VAR, Var)

enum class LTag
{
#define X(tag, variant) tag,
  IT_L_VARIANTS
#undef X
    UNK,
};

using LmUnion =
#define X(tag, variant) variant,
  std::variant<IT_L_VARIANTS std::monostate>
#undef X
  ;

struct Lm
{
  LTag    tag;
  LmUnion as;
};

// One built-in implementation: its arity and a function over the already
// evaluated, saturated operands.
struct Impl
{
  size_t arity;
  pLm (*fn)(const std::vector<pLm> &);
};

// Operand extraction / result construction. The type checker resolved the
// overload, so an "@Int" impl only sees Ints. An "@Real" impl may see an Int on
// one side (mixed Int/Real combinations route here), so it reads via `as_num`,
// which coerces an Int operand to double.
ssize_t as_int(const pLm &v);
double  as_num(const pLm &v);
pLm     mk_int(ssize_t v);
pLm     mk_real(double v);
// An Array value: `n` zeroed bytes. Backed by the byte-bearing STR value, so an
// Array is a sized, mutable block of bytes (the type checker keeps Array and
// Str distinct; the runtime, like every other value, does not).
pLm mk_bytes(size_t n);

// The dispatch table, keyed by the monomorphic key the type checker resolved
// each overloaded use to (see TC's overload_db). An "@Int" impl only sees Ints;
// an "@Real" impl may see an Int on one side (mixed Int/Real combinations route
// here), so it reads operands via as_num, which coerces an Int to double.
const std::unordered_map<std::string, Impl> impls{
  // `@array.{ n }` -- allocate n zeroed bytes (see EX::parse_array,
  // OP::ARR_ALLOC).
  { OP::ARR_ALLOC,
    { 1,
      [](const std::vector<pLm> &a) {
        ssize_t n = as_int(a[0]);
        return mk_bytes(n < 0 ? 0 : (size_t)n);
      } } },
  { OP::mono(OP::ADD, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) + as_int(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) - as_int(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) * as_int(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) / as_int(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        UT_FAIL_IF(as_int(a[1]) == 0);
        return mk_int(as_int(a[0]) % as_int(a[1]));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) == as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) >= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) <= as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) > as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_INT),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) < as_int(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::ADD, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) + as_num(a[1]));
      } } },
  { OP::mono(OP::SUB, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) - as_num(a[1]));
      } } },
  { OP::mono(OP::MUL, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) * as_num(a[1]));
      } } },
  { OP::mono(OP::DIV, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(as_num(a[0]) / as_num(a[1]));
      } } },
  { OP::mono(OP::MOD, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_real(std::fmod(as_num(a[0]), as_num(a[1])));
      } } },
  { OP::mono(OP::ISEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) == as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::GEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) >= as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LEQ, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) <= as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::MORE, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) > as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::LESS, OP::TY_REAL),
    { 2,
      [](const std::vector<pLm> &a) {
        return mk_int(as_num(a[0]) < as_num(a[1]) ? 1 : 0);
      } } },
  { OP::mono(OP::NEG, OP::TY_INT),
    { 1, [](const std::vector<pLm> &a) { return mk_int(-as_int(a[0])); } } },
  { OP::mono(OP::NEG, OP::TY_REAL),
    { 1, [](const std::vector<pLm> &a) { return mk_real(-as_num(a[0])); } } },
  { OP::mono(OP::NOT, OP::TY_INT),
    { 1,
      [](const std::vector<pLm> &a) {
        return mk_int(as_int(a[0]) == 0 ? 1 : 0);
      } } },
};

pLm lookup(const StatEnv &env, const std::string &name);
Var mkVar(UT::Vu s);
Str mkStr(UT::Vu s);
Fun mkFun(UT::Vu s, pLm body);

using NameStack = std::vector<std::string>;
void assign_id(pLm node, NameStack &env);

// A foreign type name as written in a signature. Type variables and function
// arguments are opaque, word-sized values, so they marshal as pointers.
std::string ffi_type_name(EX::Ty *t);

// Split a signature `A -> B -> R` into argument types [A, B] and result R.
void flatten_sig(EX::Ty *t, std::vector<std::string> &args, std::string &ret);

pLm call_extern(const Extern &e);

} // namespace IT

#endif // ITxDATA_HEADER_
