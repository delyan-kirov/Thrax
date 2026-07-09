/*-------------------------------------------------------------------------------
 *\file IR.hpp
 *\info The intermediate representation -- the Core (CR) after CLOSURE
 *      CONVERSION. This is the real IR (not to be confused with CR, the typed
 *      desugared Core): every lambda is lifted to a top-level closed `Code`
 *      block, and a lambda expression becomes an explicit `MkClosure(code,
 *      captures)`. Variable access is split three ways -- `Local` (a slot in
 * the current activation), `Env` (a field of the current closure's captured
 *      record), `Glob` (a top-level binding) -- which is exactly the
 *      closure-record vs stack-frame distinction a C backend needs, and which
 *      replaces CR's De-Bruijn search with O(1) array indexing.
 *
 * The IR is in A-normal form (inherited from CR): every operator/operand is an
 * `Atom`, and every non-trivial computation is named by a `Let`. The reified-K
 * machine (the runtime, forthcoming) and a future C backend both consume this.
 *
 * Memory: like CR, IR nodes are arena-allocated raw pointers; strings are `Vu`
 * views (here they alias CR's arena, since the IR is built from CR in the same
 * arena). See docs/effect-system-design.md for the surrounding design.
 *-----------------------------------------------------------------------------*/

#ifndef IR_HEADER_
#define IR_HEADER_

#include "CR.hpp"
#include "UT.hpp"

#include <unordered_set>

namespace IR
{

struct Atom;
struct Expr;

/*------------------------------------------------------------------------------
 *\ATOMS -- trivial value-expressions (no evaluation step)
 *-----------------------------------------------------------------------------*/

// A slot in the current activation's local array (params + let/case binders).
struct Local
{
  size_t i;
};

// A field of the current closure's captured record.
struct Env
{
  size_t i;
};

// A top-level binding, by mangled name (also covers built-in operator keys like
// "+@Int", which the machine recognizes by name).
struct Glob
{
  UT::Vu name;
};

struct LitI
{
  ssize_t v;
};
struct LitR
{
  double v;
};
struct LitS
{
  UT::Vu v;
};

// Allocate a closure: the lifted `code` (index into Program::codes) plus the
// captured atoms, read from the *enclosing* activation, that become the new
// closure's `Env`. Treated as an atom (like a literal) so it is never hoisted.
struct MkClosure
{
  size_t          code;
  UT::Vec<Atom *> captures;
};

#define IR_ATOM_VARIANTS                                                       \
  X(Local, Local)                                                              \
  X(Env, Env)                                                                  \
  X(Glob, Glob)                                                                \
  X(LitI, LitI)                                                                \
  X(LitR, LitR)                                                                \
  X(LitS, LitS)                                                                \
  X(Clos, MkClosure)

enum class AKind
{
#define X(name, type) name,
  IR_ATOM_VARIANTS
#undef X
};

using AtomData =
#define X(name, type) type,
  std::variant<IR_ATOM_VARIANTS std::monostate>
#undef X
  ;

struct Atom
{
  AtomData as;
};

inline AKind
akind(
  const Atom *a)
{
  return static_cast<AKind>(a->as.index());
}

/*------------------------------------------------------------------------------
 *\EXPRESSIONS -- computations (each takes an evaluation step)
 *-----------------------------------------------------------------------------*/

// Return an atom (the ANF tail position / answer).
struct Ret
{
  Atom *a;
};

// `let slot = rhs in body` -- evaluate rhs, store its value in local `slot`,
// then run body. The slot index is assigned at closure-conversion time.
struct Let
{
  size_t slot;
  Expr  *rhs;
  Expr  *body;
};

// Apply a closure (or builtin/extern) to one atom. `tail` marks tail position.
struct App
{
  Atom *fn;
  Atom *arg;
  bool  tail;
};

// A struct literal field (post-ANF the value is an atom).
struct FieldA
{
  UT::Vu name;
  Atom  *val;
};
struct MkStruct
{
  UT::Vu          name;
  UT::Vec<FieldA> fields;
};

// Field access `record.name` (record is an atom).
struct Field
{
  Atom  *rec;
  UT::Vu name;
};

// A variant construction (eager now -- no thunks; fields are atoms).
struct MkVariant
{
  UT::Vu          type_name;
  UT::Vu          tag;
  UT::Vec<Atom *> fields;
};

using AltKind = EX::AltKind;

// One Case alternative. A Con alt binds its payload positionally into the local
// slots [binder_base .. binder_base + binders.size()).
struct Alt
{
  AltKind         kind;
  UT::Vu          ctor{};
  size_t          tag         = 0;
  ssize_t         ival        = 0;
  double          rval        = 0;
  size_t          binder_base = 0;
  UT::Vec<UT::Vu> binders;
  Expr           *body = nullptr;
};

struct Case
{
  Atom        *scrut;
  UT::Vec<Alt> alts;
  Expr        *deflt;
};

// A handler. `body` runs under the installed prompt. Each clause's `fn` is the
// MkClosure of a 2-argument curried clause (`\arg = \k = e`, applied by the
// machine to the operation's argument then the continuation); `els` is the
// MkClosure of the 1-argument value clause (`\x = e`), run on the body's normal
// result. Operations are matched against `clauses` by name.
struct HandleClause
{
  UT::Vu op;
  Atom  *fn;
};
struct Handle
{
  Expr                 *body;
  UT::Vec<HandleClause> clauses;
  Atom                 *els;
};

// A foreign binding (the body of an FFI global), carried through unchanged.
struct Extern
{
  UT::Vu          symbol;
  UT::Vu          lib;
  UT::Vec<UT::Vu> arg_types;
  UT::Vu          ret_type;
};

struct Unk
{
};

#define IR_EXPR_VARIANTS                                                       \
  X(Ret, Ret)                                                                  \
  X(Let, Let)                                                                  \
  X(App, App)                                                                  \
  X(Case, Case)                                                                \
  X(MkStruct, MkStruct)                                                        \
  X(Field, Field)                                                              \
  X(MkVariant, MkVariant)                                                      \
  X(Handle, Handle)                                                            \
  X(Extern, Extern)                                                            \
  X(Unk, Unk)

enum class EKind
{
#define X(name, type) name,
  IR_EXPR_VARIANTS
#undef X
};

using ExprData =
#define X(name, type) type,
  std::variant<IR_EXPR_VARIANTS std::monostate>
#undef X
  ;

struct Expr
{
  ExprData as;
};

inline EKind
ekind(
  const Expr *e)
{
  return static_cast<EKind>(e->as.index());
}

/*------------------------------------------------------------------------------
 *\PROGRAM
 *-----------------------------------------------------------------------------*/

// A lifted, closed function. `nparams` is its arity at this `Code` (curried
// lambdas nest, so a multi-arg source function is several `Code`s); `nlocals`
// is the activation's slot count (params occupy slots [0..nparams)); globals
// are nullary `Code`s (CAFs) whose body computes the global's value.
struct Code
{
  size_t nparams;
  size_t nlocals;
  Expr  *body;
  UT::Vu name; // debug label (source name / lambda origin)
};

// The whole program: a pool of code blocks and the global table (name -> the
// nullary code that computes it).
struct Program
{
  std::vector<Code>                       codes;
  std::unordered_map<std::string, size_t> globals;
  // Names of effect operations (from `@effect` declarations). The machine
  // resolves a Glob naming one of these to an operation value that performs
  // when applied. Populated by the driver from the program's effect
  // declarations.
  std::unordered_set<std::string> operations;
};

/*------------------------------------------------------------------------------
 *\API
 *-----------------------------------------------------------------------------*/

Atom *alloc_atom(AR::Arena &arena, Atom a);
Expr *alloc_expr(AR::Arena &arena, Expr e);

// Closure-convert a whole CR global environment into an IR Program, allocating
// IR nodes in `arena` (which must be the arena the CR came from, since IR
// reuses CR's string views).
Program lower(const CR::StatEnv &env, AR::Arena &arena);

std::string pprint(const Program &p);

} // namespace IR

#endif // IR_HEADER_
