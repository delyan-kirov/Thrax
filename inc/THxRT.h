/*-------------------------------------------------------------------------------
 *\file THxRT.h
 *\info The Thrax native runtime entry points used by generated C. A program
 *      emitted by the CC backend includes this header; it supplies the value
 *      constructors, the apply trampoline (which preserves the language's tail
 *      calls in constant stack), and the built-in operator dispatch.
 *
 * The generated translation unit supplies, in turn, the program-specific tables
 * the runtime calls back into: `THxRT_code_table` / `THxRT_code_count` (the
 * lifted IR functions) and `THxRT_glob` (top-level CAF + builtin resolution).
 * See doc/native-backend.md.
 *-----------------------------------------------------------------------------*/

#ifndef THxRT_H_
#define THxRT_H_

#include "THxCHECK.h"
#include "THxMEM.h"
#include "THxVALUE.h"

/*------------------------------------------------------------------------------
 *\PROVIDED BY THE GENERATED PROGRAM
 *-----------------------------------------------------------------------------*/

extern CodeFn       THxRT_code_table[]; /* one entry per lifted IR Code */
extern const size_t THxRT_code_count;   /* length of THxRT_code_table */

/* Resolve a top-level name: a memoized CAF (the generated chain) or a built-in
 * operator key (falls through to THxRT_builtin). */
Value *THxRT_glob(const char *name);

/*------------------------------------------------------------------------------
 *\VALUE CONSTRUCTORS
 *-----------------------------------------------------------------------------*/

Value *THxRT_int(long long v);
Value *THxRT_real(double v);
Value *THxRT_str(const char *p, size_t n); /* copies n bytes */
Value *THxRT_unk(void);
Value *THxRT_bytes(size_t n); /* Array: n zeroed bytes (the %array primitive) */

/* All three copy the passed arrays into value-lifetime memory, so callers may
 * pass stack compound literals. */
Value *
THxRT_struct(const char *name, size_t n, const char **fnames, Value **fields);
Value *
THxRT_variant(const char *tname, const char *ctor, size_t n, Value **fields);
Value *THxRT_closure(int code, Value **captures, size_t n);

/* A first-class (curried) built-in operator value, by its monomorphic key
 * (e.g. "+@Int"); aborts on an unknown key. */
Value *THxRT_builtin(const char *impl);

/*------------------------------------------------------------------------------
 *\APPLICATION (the trampoline)
 *-----------------------------------------------------------------------------*/

/* Apply `f` to one argument, driving any tail-call bounces to completion. */
Value *THxRT_apply(Value *f, Value *arg);

/* Request a tail call: record (f, arg) for the enclosing trampoline and return
 * the sentinel. Generated tail positions emit `return THxRT_tailcall(f, arg);`. */
Value *THxRT_tailcall(Value *f, Value *arg);

/* Run a nullary IR Code (a CAF body) to completion through the trampoline. */
Value *THxRT_force_code(size_t code);

#endif /* THxRT_H_ */
