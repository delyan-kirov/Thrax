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
inline constexpr const char *ADD  = "+";
inline constexpr const char *SUB  = "-";
inline constexpr const char *MUL  = "*";
inline constexpr const char *DIV  = "/";
inline constexpr const char *MOD  = "%";
inline constexpr const char *ISEQ = "?=";
inline constexpr const char *MORE = "?>";
inline constexpr const char *LESS = "?<";
inline constexpr const char *LEQ  = "<=";
inline constexpr const char *GEQ  = ">=";
inline constexpr const char *BANG = "!";   // lexeme of unary NOT
inline constexpr const char *NEG  = "neg"; // unary '-'
inline constexpr const char *NOT  = "not"; // unary '!'
inline constexpr const char *IF   = "if";

// Base (scalar) type names -- the built-in, non-aggregate types the compiler
// knows, spelled exactly as the type checker spells them (con("Int") etc.).
// `Int`/`Nat`/`Real` are the platform-word numerics; the sized variants pin a
// width (and signedness); `Str`/`Ptr` are pointer-shaped. This block is the
// single source of truth: `base_types` lists them all, TC registers them as
// nullary type constructors, and FF maps each to its C ABI descriptor.
inline constexpr const char *TY_INT    = "Int";
inline constexpr const char *TY_REAL   = "Real";
inline constexpr const char *TY_STR    = "Str";
inline constexpr const char *TY_NAT    = "Nat";
inline constexpr const char *TY_PTR    = "Ptr";
inline constexpr const char *TY_INT8   = "Int8";
inline constexpr const char *TY_INT16  = "Int16";
inline constexpr const char *TY_INT32  = "Int32";
inline constexpr const char *TY_INT64  = "Int64";
inline constexpr const char *TY_NAT8   = "Nat8";
inline constexpr const char *TY_NAT16  = "Nat16";
inline constexpr const char *TY_NAT32  = "Nat32";
inline constexpr const char *TY_NAT64  = "Nat64";
inline constexpr const char *TY_REAL32 = "Real32";
inline constexpr const char *TY_REAL64 = "Real64";
inline constexpr const char *TY_ARRAY
  = "Array"; // a sized, contiguous block of bytes
inline constexpr const char *TY_UNIT
  = "{}"; // the empty record / unit type; runtime-represented as 0

inline constexpr const char *base_types[] = {
  TY_INT,   TY_NAT,    TY_REAL,   TY_STR,   TY_PTR,   TY_INT8,
  TY_INT16, TY_INT32,  TY_INT64,  TY_NAT8,  TY_NAT16, TY_NAT32,
  TY_NAT64, TY_REAL32, TY_REAL64, TY_ARRAY, TY_UNIT,
};

// Internal name of the byte-block allocation primitive that `@array.{ size }`
// desugars to (an ordinary `Int -> Array` builtin: typed in TC's m_prim, run
// via IT's impls). The leading '%' cannot occur in source, so it never collides
// with a user name.
inline constexpr const char *ARR_ALLOC = "%array";
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
         || name == LESS;
}

// The monomorphic implementation key for one resolved overload: the operator
// name tagged with the type that selects the implementation, e.g. "+@Int". TC
// rewrites a resolved use to this string; IT dispatches on it. Both ends build
// it through this one helper, so a typo is impossible rather than silent.
inline std::string
mono(
  const char *name, const char *ty)
{
  return std::string{ name } + "@" + ty;
}

} // namespace OP

#endif // OP_HEADER_
