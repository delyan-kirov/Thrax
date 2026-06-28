/*-------------------------------------------------------------------------------
 *\file IT.hpp
 *\info The reified-K abstract machine: the strict runtime that executes the IR
 *      (closure-converted Core). Unlike a tree-walker, the
 *      continuation is an EXPLICIT heap stack of frames, so deep non-tail
 *      recursion grows the heap rather than the C stack, and tail calls reuse
 *      the activation (constant stack) -- the trampoline, generalized. This is
 *      the substrate the effect machinery (handlers, first-class resumptions)
 *      will extend; see docs/effect-system-design.md.
 *
 * It is strict: data constructors are eager (no thunks). Values are the
 * reference-counted IT::Value, with closures represented as VCode (an IR code
 * index + captured env). Globals are lazy-memoized CAFs.
 *-----------------------------------------------------------------------------*/
#ifndef IT_HEADER_
#define IT_HEADER_

#include "IR.hpp"
#include "ITxDATA.hpp"

#include <variant>

namespace IT
{

// One activation: the local slot array (params + let/case binders) and the
// current closure's captured record (`Env i`).
struct Frame
{
  ValEnv locals;
  ValEnv env;
};
using FrameP = std::shared_ptr<Frame>;

// A return continuation: when the current computation yields a value `v`,
// back-patch the let's placeholder box with `v`, bind it into the saved frame's
// slot, and resume `cont`. (The box lets a recursive binding's closure, which
// captured the placeholder weakly, observe the finished value.)
struct KRet
{
  pVal            box;
  size_t          slot;
  const IR::Expr *cont;
  FrameP          frame;
};

// A handler installed by `do <body> ctl k ...`. Each operation's clause is a
// 2-argument curried closure (the machine applies it to the operation's
// argument then the resumption), keyed by operation name; `els` is the
// 1-argument value clause, run on the body's normal result.
struct Handler
{
  std::vector<std::pair<std::string, pVal>> clauses;
  pVal                                      els;
};

// A prompt delimiter on the continuation stack -- where a handler is installed.
// `perform` searches down for the nearest one whose handler has the operation;
// `ret` meeting one runs `els` (the body finished without performing).
struct KPrompt
{
  Handler handler;
};

// A `defer`/deferred-cleanup marker on the continuation stack. When a value
// returns *through* it (normal completion of the protected computation), the
// machine runs `cleanup` and then delivers the value. When it rides inside a
// captured continuation that the handler discards, its cleanup is run at the
// clause boundary instead (see KAfterClause).
struct KDefer
{
  pVal cleanup; // a nullary `{} -> {}` closure
};

// "Deliver `saved`, ignoring the incoming value." Pushed under a KDefer cleanup
// so the cleanup's own result is discarded and the protected value continues.
struct KThunkRet
{
  pVal saved;
};

// A marker pushed just below a handler operation clause, holding the clause's
// resumption `k` (as the value `kval`). When the clause finishes, this decides
// the fate of any `defer` cleanups captured in `k`: if `k` was resumed they
// run on the resumed computation's completion (via their KDefer); if `k` was
// neither resumed nor stored (the exception/abort case) its cleanups run HERE,
// on the live stack with the enclosing handlers still installed -- which is
// what makes `defer` exception-safe, and why it must happen at the clause
// boundary, not whenever `k` is later freed. "Stored" is detected by an extra
// live reference to kval.
struct KAfterClause
{
  pVal kval;
};

// One frame of the reified continuation stack. `perform` captures a contiguous
// slice of these.
using KFrame = std::variant<KRet, KPrompt, KDefer, KThunkRet, KAfterClause>;

// A captured continuation: the KFrame slice from a prompt up to a perform
// point. Affine -- `used` guards against resuming twice. Held opaquely by a
// VResump.
struct Resumption
{
  std::vector<KFrame> seg;
  bool                used = false;
};

// The machine. Holds the program and the global (CAF) memo table; one instance
// runs a whole program. Reused across nested CAF evaluations.
struct Machine
{
  const IR::Program                    &prog;
  std::unordered_map<std::string, pVal> memo;
  std::unordered_set<std::string>       in_progress;

  explicit Machine(
    const IR::Program &p)
      : prog{ p }
  {
  }

  // Evaluate `code` (with the given initial locals and captured env) to a
  // value, running the machine loop until its continuation stack empties.
  pVal run(size_t code, ValEnv locals, ValEnv env);

  // Resolve a top-level name to a value: a built-in operator key, or a
  // lazy-memoized global CAF (evaluated once on first demand).
  pVal glob(UT::Vu name);

  // Apply a value (closure or builtin) to one argument, to completion.
  pVal apply(pVal callee, pVal arg);

  // Read an atom in `frame` WITHOUT forcing a recursive cell (so a closure
  // capture keeps the weak self-reference); call `deref` where a real value is
  // needed.
  pVal eval_atom(const IR::Atom *a, const FrameP &frame);

  // Collapse a chain of recursive cells (VRec) to the underlying value.
  pVal deref(pVal v);
};

// Run the program's entry point: `glob(entry)`, applied to "" when it takes an
// argument; returns the Int result (or 0). Mirrors IT::eval's use in DR.
int machine_main(const IR::Program &prog, UT::Vu entry, bool takes_arg);

} // namespace IT

#endif // IT_HEADER_

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
