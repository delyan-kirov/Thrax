/*-------------------------------------------------------------------------------
 *\file CR.hpp
 *\info The Core: the typed, desugared lambda-calculus the front end (EX -> LL
 * -> MR -> TC) compiles to -- Var / App / Fun / Let / Case / literals / struct
 * / variant / extern. It is NOT a low-level IR (no basic blocks, no flattened
 * control flow); a real IR would be a later lowering target. The Core is what
 * the tree-walking interpreter (IT) evaluates today, and what a future C
 * backend (or IR lowering) would consume.
 *
 * The Core is deliberately backend-agnostic and free of runtime concerns -- it
 * carries no environments, closures, thunks or reference counts. A CR::Term is
 * an immutable node ARENA-allocated (raw `Term *`, never individually freed),
 * the same way the parser's EX::Expr is.
 *
 * The Core owns its data. The front-end arena (lexer/parser/type-checker) lives
 * only for the front end, so `build` COPIES every name it keeps out of that
 * arena and into the Core's own arena (`UT::strdup`). Nothing in a Term owns
 * heap memory -- only `Vu` views into the Core arena and arena-backed `UT::Vec`
 * -- so the arena need never run a destructor, and the Core outlives the front
 * end that produced it.
 *
 * Following EX/LX, a node is a tagged union built from one X-macro list of
 * (name, payload-struct) pairs. Unlike them it stores NO separate tag field:
 * the `std::variant` is the discriminant, and `kind()` reads it back (so the
 * two can never fall out of sync). The runtime VALUES produced when IT
 * evaluates a Term are a separate, reference-counted type living in IT (see
 * ITxDATA.hpp).
 *-----------------------------------------------------------------------------*/

#ifndef CR_HEADER_
#define CR_HEADER_

#include "EX.hpp"
#include "OP.hpp"
#include "UT.hpp"

namespace CR
{

struct Term;

// The global environment: a resolved (mangled) global name to the Core of its
// body. Built by `build` (one entry per `$ name = ...`), read by the
// interpreter when a global variable is forced. Keyed by std::string because
// the names are mangled `MOD/name` strings the interpreter looks up by value.
using StatEnv = std::unordered_map<std::string, Term *>;

struct Unk
{
};

struct Int
{
  ssize_t val;
};

struct Real
{
  double val;
};

struct Str
{
  UT::Vu val; // a view into the Core arena; copied into a value when forced
};

// A variable reference. `idx` is a De-Bruijn index assigned by assign_id: a
// positive index counts outward through the runtime environment to a local
// binder; 0 marks a global (resolved by `name` in the StatEnv); (size_t)-1 is a
// global *definition* placeholder (the value a `$ name = ...` form evaluates
// to, never looked up).
struct Var
{
  UT::Vu name;
  size_t idx = 0;
};

// An application `fn arg`. After ANF both `fn` and `arg` are atoms (a variable,
// literal, or lambda) and `tail` records whether the call sits in program tail
// position -- read by the interpreter's trampoline and available to a compiling
// backend.
struct App
{
  Term *fn;
  Term *arg;
  bool  tail = false;
};

struct Fun
{
  UT::Vu param; // binder name (kept for diagnostics; binding is positional)
  Term  *body;
};

// A (possibly recursive) local binding `let var = val in body`. `var` is in
// scope in both `val` (so a value may refer to itself) and `body`.
struct Let
{
  UT::Vu var;
  Term  *val;
  Term  *body;
};

// Field access `record.field`; `record` evaluates to a struct value.
struct Field
{
  Term  *record;
  UT::Vu name;
};

// One `field = val` entry of a struct literal.
struct FieldInit
{
  UT::Vu name;
  Term  *val;
};

// A struct literal `Type.{ field = val, ... }`, fields in declaration order.
struct Struct
{
  UT::Vu             name;
  UT::Vec<FieldInit> fields;
};

// A union construction `Type.Tag.{ payload... }`, payload positional in the
// variant's declared order. The interpreter suspends each field lazily.
struct Variant
{
  UT::Vu          type_name;
  UT::Vu          tag;
  UT::Vec<Term *> fields;
};

// The kind of head a Case alternative matches on -- shared with the parser.
using AltKind = EX::AltKind;

// One alternative of a Case: matched when the forced scrutinee's head agrees
// with `kind` -- a constructor `ctor` (binding the payload positionally to
// `binders`, an empty binder ignoring a slot) or an Int/Real literal.
struct Alt
{
  AltKind         kind;
  UT::Vu          ctor{};   // Con: constructor name (matched at runtime)
  size_t          tag  = 0; // Con: constructor index within its type
  ssize_t         ival = 0; // Int
  double          rval = 0; // Real
  UT::Vec<UT::Vu> binders;  // Con: payload binder names, positional
  Term           *body = nullptr;
};

// `case scrut of alt... else deflt` -- the one branching form. `if` and every
// match lower to this; the interpreter forces `scrut`, runs the taken
// alternative's body (binding a Con alt's payload first), or `deflt`.
struct Case
{
  Term        *scrut;
  UT::Vec<Alt> alts;
  Term        *deflt;
};

// A handler: run `body`; an operation performed within it dispatches to the
// matching clause. Each clause's `fn` is a 2-argument curried lambda
// `\arg = \k = e` and `els` is a 1-argument lambda `\x = e` (the value clause,
// synthesized as the identity `\x = x` when no `else` was written). Modelling
// the clauses/else as ordinary `Fun`s lets assign_id and the closure converter
// handle their binders with no special cases. Installed at runtime as a prompt
// on the continuation stack; see IT.
struct HClause
{
  UT::Vu op;
  Term  *fn;
};
struct Handle
{
  Term            *body;
  UT::Vec<HClause> clauses;
  Term            *els;
};

// A foreign binding `@extern "C" "symbol" "lib"`: the body of an FFI global.
// Its argument / result types come from the enclosing signature, not the
// body. `lib` is symbolic; consumers resolve it (FF at dlopen time, CC on the
// link line). The interpreter wraps it in a runtime value and calls it once
// saturated.
struct Extern
{
  UT::Vu          abi;
  UT::Vu          symbol;
  UT::Vu          lib;
  UT::Vec<UT::Vu> arg_types;
  UT::Vu          ret_type;
};

// `Unk` is first so the all-zero bytes the arena hands back name a valid
// (trivially constructible) alternative -- the variant move-assignment in
// `alloc` then reads a well-formed discriminant.
#define CR_TERM_VARIANTS                                                       \
  X(Unk, Unk)                                                                  \
  X(Int, Int)                                                                  \
  X(Real, Real)                                                                \
  X(Str, Str)                                                                  \
  X(Var, Var)                                                                  \
  X(App, App)                                                                  \
  X(Fun, Fun)                                                                  \
  X(Let, Let)                                                                  \
  X(Field, Field)                                                              \
  X(Struct, Struct)                                                            \
  X(Variant, Variant)                                                          \
  X(Case, Case)                                                                \
  X(Handle, Handle)                                                            \
  X(Extern, Extern)

// `Kind` exists only as a readable view of the variant's discriminant: it lists
// the alternatives in their declared order, so `kind()` is just `as.index()`
// cast back. It is never stored on a node.
enum class Kind
{
#define X(name, type) name,
  CR_TERM_VARIANTS
#undef X
};

using TermData =
#define X(name, type) type,
  std::variant<CR_TERM_VARIANTS std::monostate>
#undef X
  ;

struct Term
{
  TermData as;
};

inline Kind
kind(
  const Term *t)
{
  return static_cast<Kind>(t->as.index());
}

/*------------------------------------------------------------------------------
 *\API
 *-----------------------------------------------------------------------------*/

// Allocate a Term in `arena` holding `t`. The arena hands back zeroed bytes (a
// valid `Unk` alternative), so move-assigning an arbitrary alternative into it
// is well-formed.
Term *alloc(AR::Arena &arena, Term t);

// Compile `expr` to Core in `arena`, normalize it (ANF) and assign De-Bruijn
// indices. A global definition (`$ name = ...`) is finalized, stored into `env`
// and yields a placeholder Var; every other form returns its finished Core.
// This is the whole EX::Expr -> Core lowering. Names are copied into `arena`,
// so the result is independent of the front-end arena `expr` came from.
Term *build(EX::Expr *expr, StatEnv &env, AR::Arena &arena);

// Assign each Var its De-Bruijn index against the binder stack `names`.
void assign_id(Term *node, std::vector<UT::Vu> &names);

std::string pprint(Term *t, int level = 0);

} // namespace CR

#endif // CR_HEADER_
