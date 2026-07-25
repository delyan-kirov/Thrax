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
inline constexpr const char *ADD        = "+";
inline constexpr const char *SUB        = "-";
inline constexpr const char *MUL        = "*";
inline constexpr const char *DIV        = "/";
inline constexpr const char *MOD        = "%";
inline constexpr const char *ISEQ       = "?=";
inline constexpr const char *MORE       = "?>";
inline constexpr const char *LESS       = "?<";
inline constexpr const char *LEQ        = "<=";
inline constexpr const char *GEQ        = ">=";
inline constexpr const char *BANG       = "!";   // lexeme of unary NOT
inline constexpr const char *NEG        = "neg"; // unary '-'
inline constexpr const char *NOT        = "not"; // unary '!'
inline constexpr const char *IF         = "if";
inline constexpr const char *TY_REAL    = "@float64";
inline constexpr const char *TY_STR     = "@str";
inline constexpr const char *TY_PTR     = "@ptr";
inline constexpr const char *TY_INT8    = "@int8";
inline constexpr const char *TY_INT16   = "@int16";
inline constexpr const char *TY_INT32   = "@int32";
inline constexpr const char *TY_INT64   = "@int64";
inline constexpr const char *TY_NAT8    = "@nat8";
inline constexpr const char *TY_NAT16   = "@nat16";
inline constexpr const char *TY_NAT32   = "@nat32";
inline constexpr const char *TY_NAT64   = "@nat64";
inline constexpr const char *TY_REAL32  = "@float32";
inline constexpr const char *TY_REAL64  = "@float64";
inline constexpr const char *TY_ARRAY   = "@array";
inline constexpr const char *TY_UNIT    = "{}";
inline constexpr const char *TY_BOOL    = "@bool";
inline constexpr const char *BOOL_TRUE  = "@true";
inline constexpr const char *BOOL_FALSE = "@false";
inline constexpr const char *TY_VEC     = "Vec";
inline constexpr const char *VEC_REP    = "%vec";
inline constexpr const char *VEC_NEW    = "vec_new";
inline constexpr const char *VEC_FILL   = "vec_fill";
inline constexpr const char *VEC_LEN    = "vec_len";
inline constexpr const char *VEC_GET    = "vec_get";
inline constexpr const char *VEC_SET    = "vec_set";
inline constexpr const char *VEC_PUSH   = "vec_push";

inline constexpr const char *TUPLE_PREFIX = "%tuple";
inline std::string
tuple_name(
  size_t n)
{
  return std::string{ TUPLE_PREFIX } + std::to_string(n);
}
inline bool
is_tuple_name(
  UT::Vu name)
{
  UT::Vu p{ TUPLE_PREFIX };
  return name.size() > p.size() && name.substr(0, p.size()) == p;
}

inline constexpr const char *base_types[] = {
  TY_REAL,   TY_STR,   TY_PTR,   TY_INT8,  TY_INT16, TY_INT32,
  TY_INT64,  TY_NAT8,  TY_NAT16, TY_NAT32, TY_NAT64, TY_REAL32,
  TY_REAL64, TY_ARRAY, TY_UNIT,  TY_BOOL,
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
  { "Ptr", TY_PTR },       { "Array", TY_ARRAY },   { "Bool", TY_BOOL },
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

// The `@`-intrinsic registry for the Term and Decl namespaces: the single
// source of truth for every non-type `@`-sigil built-in the parser recognizes.
// (The Type namespace is `base_types[]` above, since it also carries the
// non-`@` unit type `{}` and is consumed elsewhere; `is_base_type` is its
// query.) An `@`-name belongs to exactly one namespace, which fixes *where* it
// is legal:
//   - Type  -- type position only, becomes a TyCon (base_types[], above).
//   - Term  -- expression position (a `pat_ok` subset also patterns).
//   - Decl  -- the head of a `$`-global only; produces a top-level declaration.
// `@array` is a Term intrinsic here AND a base type above; each parse site knows
// which namespace it wants, so the dual role is unambiguous. `form` records how
// a Term intrinsic parses its arguments; `id` is the routing key each parse site
// switches on (see doc/at-intrinsics.md).
enum class AtNs
{
  Type,
  Term,
  Decl
};
enum class AtForm
{
  None,     // decls (routed by id, not form)
  Nullary,  // `@true`        -- a bare atom
  DotBlock, // `@array.{ e }` -- a `.{ ... }` block
  StrArg,   // `@char "a"`    -- a following string literal
};
enum class AtId
{
  True,
  False,
  Char,
  ArrayLit,
  Mod,
  Struct,
  Union,
  Alias,
  Effect,
  Operator,
  Private,
  Public,
  Extern,
  Run,
  Assert,
};
struct AtIntrinsic
{
  const char *name;
  AtNs        ns;
  AtForm      form;
  AtId        id;
  bool        pat_ok; // Term only: also legal in a pattern (`@true`/`@false`)
};
inline constexpr AtIntrinsic at_intrinsics[] = {
  { BOOL_TRUE, AtNs::Term, AtForm::Nullary, AtId::True, true },
  { BOOL_FALSE, AtNs::Term, AtForm::Nullary, AtId::False, true },
  { "@char", AtNs::Term, AtForm::StrArg, AtId::Char, false },
  { TY_ARRAY, AtNs::Term, AtForm::DotBlock, AtId::ArrayLit, false },
  { "@mod", AtNs::Decl, AtForm::None, AtId::Mod, false },
  { "@struct", AtNs::Decl, AtForm::None, AtId::Struct, false },
  { "@union", AtNs::Decl, AtForm::None, AtId::Union, false },
  { "@alias", AtNs::Decl, AtForm::None, AtId::Alias, false },
  { "@effect", AtNs::Decl, AtForm::None, AtId::Effect, false },
  { "@operator", AtNs::Decl, AtForm::None, AtId::Operator, false },
  { "@private", AtNs::Decl, AtForm::None, AtId::Private, false },
  { "@public", AtNs::Decl, AtForm::None, AtId::Public, false },
  { "@extern", AtNs::Decl, AtForm::None, AtId::Extern, false },
  { "@run", AtNs::Decl, AtForm::None, AtId::Run, false },
  { "@assert", AtNs::Decl, AtForm::None, AtId::Assert, false },
};

// Look a Term/Decl `@`-name up in a specific namespace (positions are
// namespace-typed, so callers know which they want). Returns nullptr if `name`
// is not an intrinsic of that namespace -- e.g. an unknown name, or `@struct`
// asked for as a Term. For the Type namespace use `is_base_type`.
inline const AtIntrinsic *
at_lookup(
  UT::Vu name, AtNs ns)
{
  for (const AtIntrinsic &a : at_intrinsics)
    if (a.ns == ns && name == a.name) return &a;
  return nullptr;
}

// A human-readable name for a namespace, for "wrong position" diagnostics.
inline const char *
at_ns_name(
  AtNs ns)
{
  switch (ns)
  {
  case AtNs::Type: return "type";
  case AtNs::Term: return "expression";
  case AtNs::Decl: return "declaration";
  }
  return "";
}

// Classify a known `@`-name for diagnostics ("`@struct` is a declaration form,
// not valid here"). Writes the namespace and returns true when `name` matches an
// intrinsic; returns false for an unknown name. `@array` reports Term here (its
// registry row); base types report Type.
inline bool
at_classify(
  UT::Vu name, AtNs &out)
{
  for (const AtIntrinsic &a : at_intrinsics)
    if (name == a.name)
    {
      out = a.ns;
      return true;
    }
  if (is_base_type(name))
  {
    out = AtNs::Type;
    return true;
  }
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
