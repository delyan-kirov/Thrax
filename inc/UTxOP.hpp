/*-------------------------------------------------------------------------------
 *\file UTxOP.hpp
 *\info The operator catalogue. One shared definition of every operator -- its
 *      arity, and (for the evaluable ones) a function pointer that computes it.
 *      LX, EX, IT and TC all look operators up here instead of hardcoding them.
 *
 * Operators are keyed by their canonical name. For binary operators the name is
 * the source lexeme ("+", "?=", ...); for the unary ones it is a distinct word
 * ("neg", "not") so that unary '-' never collides with binary '-'.
 *-----------------------------------------------------------------------------*/

#ifndef UTXOP_HEADER
#define UTXOP_HEADER

#include "UT.hpp"

namespace OP
{

// Canonical operator spellings -- the single source of truth for these strings.
// Use these everywhere instead of bare "?=" / "neg" / ... literals. For binary
// operators the spelling is also the source lexeme; for the unary operators NEG
// and NOT are distinct names whose lexemes are SUB and BANG.
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

// Number of operands an operator takes. `if` is the sole ternary.
enum class Arity
{
  Unary   = 1,
  Binary  = 2,
  Ternary = 3,
};

using UnaryFn  = ssize_t (*)(ssize_t);
using BinaryFn = ssize_t (*)(ssize_t, ssize_t);

using UnaryTable  = std::unordered_map<std::string, UnaryFn>;
using BinaryTable = std::unordered_map<std::string, BinaryFn>;
using ArityTable  = std::unordered_map<std::string, Arity>;

// Unary operators -> evaluator. Keyed by canonical name ("neg", "not").
extern const UnaryTable unary_db;

// Binary operators -> evaluator. '/' and '%' trap on a zero divisor.
extern const BinaryTable binary_db;

// Every operator (unary, binary, and the ternary `if`) -> its arity.
extern const ArityTable arity_db;

} // namespace OP

#endif // UTXOP_HEADER
