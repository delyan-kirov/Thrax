/*-------------------------------------------------------------------------------
 *\file THxK.h
 *\info The reified-K (CEK) driver -- the C port of the interpreter's abstract
 *      machine (src/IT.cpp). The continuation is an EXPLICIT heap stack of
 *      frames, so a handler can capture and splice the delimited continuation
 *      between a prompt and a `perform`; the C call stack no longer holds
 *      pending work across a possible effect. This subsumes the old
 * direct-style apply trampoline (THxRT_apply) and, as a side effect, makes deep
 * non-tail recursion grow the heap rather than the C stack.
 *
 * Generated code is compiled to BLOCK FUNCTIONS. A block runs straight-line C
 * (atoms, pure lets, case branching) and ends by calling exactly one TERMINATOR
 * (THxK_ret / THxK_tailcall / THxK_apply / THxK_jump / THxK_handle), which
 * records the next machine action; the driver acts on it. An activation's
 * locals/env live in a heap `Frame` (not C autos), so a block can be re-entered
 * after a suspension. See doc/native-backend.md.
 *-----------------------------------------------------------------------------*/

#ifndef THxK_H_
#define THxK_H_

#include "THxVALUE.h"

#include <stddef.h>

/* One activation: the local slot array (params + let/case binders) and the
 * current closure's captured record (`Env i`). Heap-allocated so it survives a
 * suspension captured in the continuation stack.
 *
 * Lifetime: frames are reference-counted (`rc`) -- the driver's current
 * activation holds one reference and every KRet on the continuation stack
 * (including captured segments) holds one. `env` is BORROWED from the closure
 * the frame was created from; `clos` keeps that closure alive (+1) for the
 * frame's lifetime (NULL for a nullary CAF frame). Slots own their values
 * (+1 each, via THxK_setlocal). */
typedef struct Frame
{
  unsigned rc;
  Value   *clos;
  Value  **locals;
  size_t   nlocals;
  Value  **env;
  size_t   nenv;
} Frame;

/* A compiled basic block. `in` is the value delivered to a resumed block (the
 * argument for a code entry, the produced value for a continuation); blocks
 * re-read their live variables from `fr`, so `in` is usually unused. */
typedef void (*BlockFn)(Frame *fr, Value *in);

/*------------------------------------------------------------------------------
 *\PROVIDED BY THE GENERATED PROGRAM
 *-----------------------------------------------------------------------------*/

extern BlockFn      THxRT_code_table[];   /* entry block of each lifted Code */
extern const size_t THxRT_code_nlocals[]; /* activation slot count per Code */
extern const size_t THxRT_code_count;

/*------------------------------------------------------------------------------
 *\BLOCK TERMINATORS -- a block ends by calling exactly one
 *-----------------------------------------------------------------------------*/

/* Deliver `v` up the continuation stack (the block's value is the answer). */
void THxK_ret(Value *v);

/* Tail application: apply `fn` to `arg`; the result flows to the continuation
 * (constant stack -- no KRet is pushed). */
void THxK_tailcall(Value *fn, Value *arg);

/* Non-tail application: push a return continuation that, when `fn arg` yields a
 * value, back-patches frame `fr`'s `slot` box with it and resumes at block
 * `cont`; then apply. The box must already sit in `slot` (see THxK_setbox). */
void THxK_apply(Frame *fr, Value *fn, Value *arg, BlockFn cont, size_t slot);

/* Local transfer to a continuation block in the SAME frame (a case join / the
 * body of a let whose value was produced without suspending). */
void THxK_jump(BlockFn cont);

/* Install a handler: push a return continuation (`fr`/`cont`/`slot`, as
 * THxK_apply) for the handled expression's value, then a prompt holding the
 * operation clauses (parallel `ops`/`clauses` arrays) and the value clause
 * `els`, then run `body` under it. Deep handler: `els` runs on the body's
 * normal completion. */
void THxK_handle(Frame       *fr,
                 BlockFn      cont,
                 size_t       slot,
                 const char **ops,
                 Value      **clauses,
                 size_t       nclauses,
                 Value       *els,
                 BlockFn      body);

/*------------------------------------------------------------------------------
 *\LET-BOX HELPERS (recursive-let back-patch)
 *-----------------------------------------------------------------------------*/

/* Bind `v` into frame `fr`'s `slot`: the slot takes ownership (+1), releasing
 * whatever it previously held. The sanctioned way to write a slot (case payload
 * binds, argument placement). */
void THxK_setlocal(Frame *fr, size_t slot, Value *v);

/* Place a fresh placeholder box in `slot`, so a recursive binding's closures
 * can capture it before the value exists. */
void THxK_setbox(Frame *fr, size_t slot);

/* Back-patch the box in `slot` with `v` (copies the value into the placeholder,
 * so earlier captures observe it). Used on the non-suspending completion path;
 * the suspending path back-patches in the driver's KRet handling. */
void THxK_backpatch(Frame *fr, size_t slot, Value *v);

/*------------------------------------------------------------------------------
 *\EFFECT VALUE CONSTRUCTORS
 *-----------------------------------------------------------------------------*/

/* A first-class effect operation (by canonical "Effect.op" name): performs when
 * applied to its argument. */
Value *THxK_op(const char *name);

/* A fresh `defer` intrinsic value (curried; collects the action then the
 * cleanup thunk before installing the cleanup). */
Value *THxK_defer(void);

/* Note that `v`, if a resumption, is now reachable from a heap structure (a
 * closure capture, struct/variant field). A stored resumption's `defer`
 * cleanups are not finalized at the clause boundary. Called by the THxRT value
 * constructors on each stored capture/field. */
void THxK_mark_escape(Value *v);

/*------------------------------------------------------------------------------
 *\DRIVER ENTRY POINTS
 *-----------------------------------------------------------------------------*/

/* Run a nullary Code (a CAF body) to completion and return its value. */
Value *THxK_run_code(size_t code);

/* Apply `f` to `arg`, running to completion; returns the result value. */
Value *THxK_call(Value *f, Value *arg);

#endif /* THxK_H_ */
