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
