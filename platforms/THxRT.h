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
 *
 * The lifted-code tables (THxRT_code_table / THxRT_code_nlocals / count) live
 * in THxK.h, since the CEK driver owns application.
 *-----------------------------------------------------------------------------*/

/* One foreign-call wrapper per `@extern` site: it marshals the collected,
 * already-saturated arguments, makes the direct typed C call, and marshals the
 * result back to a Value. Generated, with the FFI machinery, by the CC backend
 * (the table is always defined -- empty when the program has no foreign calls).
 */
typedef Value *(*ExternFn)(Value **args);
extern ExternFn     THxRT_extern_table[];
extern const size_t THxRT_extern_count;

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

/* A first-class (curried) foreign function value: `idx` selects its wrapper in
 * THxRT_extern_table, `arity` is the number of curried arguments it consumes.
 */
Value *THxRT_extern(int idx, size_t arity);

/*------------------------------------------------------------------------------
 *\APPLICATION -- builtin / foreign operand accumulation
 *
 * Application proper is the CEK driver's job (THxK); these apply the non-
 * suspending callees. Each appends `arg` to the (curried) callee and, when
 * saturated, runs the operator / foreign call and returns the result value;
 * otherwise it returns the partially-applied callee.
 *-----------------------------------------------------------------------------*/

Value *THxRT_apply_builtin(Value *b, Value *arg);
Value *THxRT_apply_extern(Value *e, Value *arg);

#endif /* THxRT_H_ */
