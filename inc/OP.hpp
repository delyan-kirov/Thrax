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

// Operand type names, as the type checker spells them (con("Int") etc.).
inline constexpr const char *TY_INT  = "Int";
inline constexpr const char *TY_REAL = "Real";

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
