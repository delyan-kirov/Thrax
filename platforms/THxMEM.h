/*-------------------------------------------------------------------------------
 *\file THxMEM.h
 *\info The memory / value-lifetime seam. The runtime and generated code touch
 *      allocation ONLY through this interface, so the strategy is swappable.
 *      Two engines implement it:
 *
 *      - THxMEMRC.c (the DEFAULT): precise reference counting. Values carry
 *        Value::rc; every heap store retains (+1) and every un-store releases
 *        (-1), freeing at zero and recursively releasing children (via
 *        THxVALUE_destroy). Fresh values are registered in a TEMP POOL that the
 *        CEK driver drains after each block bounce, so per-iteration garbage in
 *        long-running loops is reclaimed. A live-allocation counter
 *        (THxMEM_live) lets a program assert it exits clean.
 *
 *      - THxMEMBUMP.c (compile with -DTHX_MEM_BUMP): the v1 bump arena that
 *        never frees; retain/release/pool are no-ops and THxMEM_live is 0.
 *        Kept as the debugging fallback (memory bugs disappear under it, which
 *        is itself diagnostic).
 *-----------------------------------------------------------------------------*/

#ifndef THxMEM_H_
#define THxMEM_H_

#include "THxVALUE.h"

#include <stddef.h>

/* Allocate `size` zeroed bytes with value lifetime. Never returns NULL (a
 * failed allocation aborts via THxCHECK_FAIL). */
void *THxMEM_alloc(size_t size);

/* Free a block obtained from THxMEM_alloc (raw payload arrays: env/fields/args,
 * string bytes, frames, resumption segments). No-op under the bump engine. Only
 * the owning value's destructor calls this. */
void THxMEM_free(void *p);

/* Allocate one zeroed Value, registered in the temp pool (the pool holds its
 * initial reference; anything that keeps the value must retain it). */
Value *THxMEM_alloc_value(void);

/* Lifetime hooks. No-ops under the bump allocator; the ref-counting engine
 * makes them adjust Value::rc and free at zero (releasing children through
 * THxVALUE_destroy). Both tolerate NULL. */
void THxMEM_retain(Value *v);
void THxMEM_release(Value *v);

/* Is `v` provably uniquely owned -- safe to mutate in place (its buffer is
 * observed by no one but the caller)? True under the ref-counting engine iff
 * rc == 1 (every durable holder -- slot, field, kont -- retains, so anything
 * shared is rc >= 2; a value still in the temp pool carries the pool's extra
 * ref, so it reads as non-unique -- conservative but sound). ALWAYS false under
 * the bump engine, which tracks no counts. Backs opportunistic in-place
 * mutation (array_push / array_set / ++); see doc/strings-and-arrays.md. */
int THxMEM_unique(Value *v);

/* The temp pool: every THxMEM_alloc_value is registered here with the pool
 * owning its initial reference. The CEK driver brackets each block bounce with
 * mark/drain, releasing the bounce's temporaries; whatever was stored (slot,
 * field, kont, cache) survives via its own retain. Re-entrant runs nest safely
 * because each drains only to its own mark. */
size_t THxMEM_pool_mark(void);
void   THxMEM_pool_drain(size_t mark);

/* Live allocation count (values + raw blocks). 0 under the bump engine, so a
 * leak check passes trivially there. */
size_t THxMEM_live(void);

#endif /* THxMEM_H_ */
