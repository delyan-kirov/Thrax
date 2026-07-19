/*-------------------------------------------------------------------------------
 *\file OP.hpp
 *\info The operator name vocabulary -- nothing more. Operators are surface
 *      syntax for ordinary (possibly overloaded) named functions: after parsing
 *      they are plain Var/App nodes, so no layer below shares an "operator
 * type".
 *
 * What the layers genuinely need to agree on are a handful of *strings*:
 *   - canonical names  ("+", "neg", "if", ...)  -- LX lexes them, EX desugars
 * an operator to a Var of this name, TC keys its overload table here.
 *   - type names       ("Int", "Real")          -- how TC spells operand types.
 *   - the `mono` key    "+@Int"                  -- the contract between TC
 * (which resolves an overloaded use to it) and IT (which dispatches it).
 *
 * Everything behavioural -- which char sequences lex, binding powers, the typed
 * overload signatures, the implementations -- lives in the layer that owns it
 * (LX, EX, TC, IT respectively). This header is only the shared glue, so it can
 * be included anywhere with no circular dependency.
 *-----------------------------------------------------------------------------*/

#ifndef OP_HEADER_
#define OP_HEADER_

#include "UT.hpp"

namespace OP
{

// Canonical operator spellings -- the single source of truth for these strings.
// For binary operators the spelling is also the source lexeme; the unary NEG
// and NOT are distinct names (their lexemes are SUB and BANG) so unary '-'
// never collides with binary '-'.
inline constexpr const char *ADD       = "+";
inline constexpr const char *SUB       = "-";
inline constexpr const char *MUL       = "*";
inline constexpr const char *DIV       = "/";
inline constexpr const char *MOD       = "%";
inline constexpr const char *ISEQ      = "?=";
inline constexpr const char *MORE      = "?>";
inline constexpr const char *LESS      = "?<";
inline constexpr const char *LEQ       = "<=";
inline constexpr const char *GEQ       = ">=";
inline constexpr const char *BANG      = "!";   // lexeme of unary NOT
inline constexpr const char *NEG       = "neg"; // unary '-'
inline constexpr const char *NOT       = "not"; // unary '!'
inline constexpr const char *IF        = "if";
inline constexpr const char *TY_REAL   = "@float64";
inline constexpr const char *TY_STR    = "@str";
inline constexpr const char *TY_PTR    = "@ptr";
inline constexpr const char *TY_INT8   = "@int8";
inline constexpr const char *TY_INT16  = "@int16";
inline constexpr const char *TY_INT32  = "@int32";
inline constexpr const char *TY_INT64  = "@int64";
inline constexpr const char *TY_NAT8   = "@nat8";
inline constexpr const char *TY_NAT16  = "@nat16";
inline constexpr const char *TY_NAT32  = "@nat32";
inline constexpr const char *TY_NAT64  = "@nat64";
inline constexpr const char *TY_REAL32 = "@float32";
inline constexpr const char *TY_REAL64 = "@float64";
inline constexpr const char *TY_ARRAY
  = "@array"; // a sized, contiguous block of bytes
inline constexpr const char *TY_UNIT
  = "{}"; // the empty record / unit type; runtime-represented as 0

inline constexpr const char *base_types[] = {
  TY_REAL,  TY_STR,    TY_PTR,    TY_INT8,  TY_INT16,
  TY_INT32, TY_INT64,  TY_NAT8,   TY_NAT16, TY_NAT32,
  TY_NAT64, TY_REAL32, TY_REAL64, TY_ARRAY, TY_UNIT,
};

// The prelude's FIXED transparent aliases onto the base types, as data: DR
// generates the `$ Name : @alias = target` prelude lines from this table
// (after the target-dependent `Int`/`Nat`, which come from TG::Target), and
// `canon` below folds the bare spellings back to canonical `@`-names. Single
// source of truth for both directions. Order is the prelude declaration order.
struct BaseAlias
{
  const char *name;
  const char *target;
};
inline constexpr BaseAlias base_aliases[] = {
  { "Real", TY_REAL },     { "Int8", TY_INT8 },     { "Int16", TY_INT16 },
  { "Int32", TY_INT32 },   { "Int64", TY_INT64 },   { "Nat8", TY_NAT8 },
  { "Nat16", TY_NAT16 },   { "Nat32", TY_NAT32 },   { "Nat64", TY_NAT64 },
  { "Real32", TY_REAL32 }, { "Real64", TY_REAL64 }, { "Str", TY_STR },
  { "Ptr", TY_PTR },       { "Array", TY_ARRAY },
};

// Canonical spelling of a base-type name: folds a bare prelude alias ("Str",
// "Int32", ...) to its `@`-target; any other name (already-canonical
// `@`-names, user types, type variables) passes through unchanged. NOTE:
// `Int`/`Nat` are NOT folded here -- their width is target policy, so FFI
// consumers canonicalize through TG::Target::canon, which folds them and
// defers the rest to this. (Extern signatures reach the IR with their
// *surface* spelling -- alias resolution lives in TC's sig_to_type and is
// never written back to the EX type tree.)
inline UT::Vu
canon(
  UT::Vu name)
{
  for (const BaseAlias &a : base_aliases)
    if (name == a.name) return a.target;
  return name;
}

// Internal name of the byte-block allocation primitive that `@array.{ size }`
// desugars to (an ordinary `Int -> Array` builtin: typed in TC's m_prim, run
// via IT's impls). The leading '%' cannot occur in source, so it never collides
// with a user name.
inline constexpr const char *ARR_ALLOC = "%array";

// The growable byte-vector (Str / Array) built-ins. Unlike %array these ARE
// user-callable primitives (typed in TC's m_prim, run by IT's impls / the
// native dispatch), so they carry ordinary names -- reserved, like `if`. A byte
// is an Int in 0..255. Mutators (set/push/concat/slice) are opportunistic
// in-place: they mutate their buffer when it is uniquely referenced, else copy
// (see doc/strings-and-arrays.md).
inline constexpr const char *ARR_LEN = "array_len"; // Array -> Int
inline constexpr const char *ARR_CAP = "array_cap"; // Array -> Int
inline constexpr const char *ARR_GET = "array_get"; // Array -> Int -> Int
inline constexpr const char *ARR_SET
  = "array_set"; // Array -> Int -> Int -> Array
inline constexpr const char *ARR_PUSH = "array_push"; // Array -> Int -> Array
inline constexpr const char *ARR_SLICE
  = "array_slice";                          // Array -> Int -> Int -> Array
inline constexpr const char *CONCAT = "++"; // Str/Array concat (overloaded)
// Shared impl key both `++` overloads (Str, Array) resolve to -- concatenation
// is byte-for-byte identical for both, so one implementation serves. Not in
// m_prim (unreachable except via `++` overload resolution).
inline constexpr const char *CONCAT_IMPL = "%concat";

// The `defer` cleanup intrinsic. `%`-prefixed so it is not a writable
// identifier; the surface is the `defer <cleanup> do <body>` keyword, which
// desugars to
// `%defer (\_ = body) (\_ = cleanup)`.
inline constexpr const char *DEFER = "%defer";

// Is `name` one of the built-in base types above?
inline bool
is_base_type(
  UT::Vu name)
{
  for (const char *t : base_types)
    if (name == t) return true;
  return false;
}

// Is `name` the canonical name of an overloadable (binary) operator? These are
// exactly the names a use site carries (mk_binop stores the lexeme, which
// equals the canonical name for binaries) and the keys of the type checker's
// overload_db. A user may add overloads of these via `$ @operator.{<op>}`; MR
// uses this to route an operator use through type-directed resolution, TC to
// fold the built-in candidates in beside the user's.
inline bool
is_operator(
  UT::Vu name)
{
  return name == ADD || name == SUB || name == MUL || name == DIV || name == MOD
         || name == ISEQ || name == GEQ || name == LEQ || name == MORE
         || name == LESS || name == CONCAT;
}

// Monomorphic implementation key
inline std::string
mono(
  const char *name, const char *ty)
{
  if (ty && ty[0] == '@') ty += 1;
  return std::string{ name } + "@" + ty;
}

} // namespace OP

#endif // OP_HEADER_
