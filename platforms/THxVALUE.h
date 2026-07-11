/*-------------------------------------------------------------------------------
 *\file THxVALUE.h
 *\info The runtime value -- a pointer-boxed tagged union, the C counterpart of
 *      the interpreter's IT::Value (see inc/ITxDATA.hpp). Like the interpreter
 *      it is strict: there are no thunks; every field of a struct/variant is an
 *      already-evaluated value.
 *
 * Generated code NEVER reads a union arm directly. Every access goes through a
 * checked accessor below, which asserts the tag (and bounds) via
 * THxCHECK_ASSERT first. This keeps the boxed representation honest and makes a
 * codegen bug abort with a precise message instead of corrupting memory.
 *
 * The `rc` field drives the reference-counting engine (THxMEMRC.c, the
 * default); the bump fallback (-DTHX_MEM_BUMP) leaves it untouched. See
 * doc/native-backend.md.
 *-----------------------------------------------------------------------------*/

#ifndef THxVALUE_H_
#define THxVALUE_H_

#include <stddef.h>

/* The value discriminant. Adding a tag here is the head of a domino: the
 * accessors, THxVALUE_tag_name, THxRT_apply's dispatch and the codegen all
 * switch on it, so an unhandled case trips -Wswitch (-Werror) or a
 * THxCHECK_FAIL. */
typedef enum
{
  T_INT,     /* machine integer (also Unit, booleans 0/1) */
  T_REAL,    /* double */
  T_STR,     /* owned byte block: Str literal or Array bytes */
  T_STRUCT,  /* record: named fields in declaration order */
  T_VARIANT, /* sum value: type name + constructor tag + payload */
  T_CLOS,    /* closure: an IR code index + captured environment */
  T_BUILTIN, /* a (possibly partially applied) built-in operator */
  T_EXTERN,  /* a (possibly partially applied) foreign C function (FFI) */
  T_OP,      /* an effect operation, first-class; performs when applied */
  T_RESUMP,  /* a captured continuation (resumption); affine -- resumes once */
  T_DEFER,   /* the `defer` intrinsic, curried over its two thunks */
  T_UNK      /* the unknown/placeholder value (recursive-let box, unit) */
} Tag;

typedef struct Value Value;

/* A captured continuation segment; defined by the CEK driver (THxK). Only a
 * pointer is stored here, so THxVALUE need not know its layout. */
typedef struct THxK_Resump THxK_Resump;

/* Every lifted IR Code is a curried, arity-1 function: it takes its captured
 * environment and one argument and returns a value. */
typedef Value *(*CodeFn)(Value **env, Value *arg);

struct Value
{
  Tag      tag;
  unsigned rc; /* RESERVED for ref counting; unused by the bump allocator */
  union
  {
    long long i; /* T_INT */
    double    r; /* T_REAL */
    struct
    {
      char  *p;
      size_t n;
    } s; /* T_STR */
    struct
    {
      const char  *name;
      const char **fnames;
      Value      **f;
      size_t       n;
    } st; /* T_STRUCT */
    struct
    {
      const char *tname;
      const char *ctor;
      Value     **f;
      size_t      n;
    } var; /* T_VARIANT */
    struct
    {
      int     code;
      Value **env;
      size_t  nenv;
    } clos; /* T_CLOS */
    struct
    {
      const char *impl;
      size_t      arity;
      size_t      nargs;
      Value     **args;
    } bi; /* T_BUILTIN */
    struct
    {
      int     idx; /* index into the generated THxRT_extern_table */
      size_t  arity;
      size_t  nargs;
      Value **args;
    } ext; /* T_EXTERN */
    struct
    {
      const char *name;
    } op; /* T_OP: the operation's canonical "Effect.op" name */
    struct
    {
      THxK_Resump *seg;
    } resump; /* T_RESUMP: the captured continuation slice */
    struct
    {
      size_t  nargs;
      Value **args;
    } defer; /* T_DEFER: accumulates the two thunks (action, cleanup) */
  } u;
};

/* Human-readable tag name, for diagnostics. */
const char *THxVALUE_tag_name(Tag t);

/*------------------------------------------------------------------------------
 *\CHECKED ACCESSORS -- the only sanctioned way to read a value
 *-----------------------------------------------------------------------------*/

long long THxVALUE_as_int(Value *v); /* fails unless T_INT */
double    THxVALUE_as_num(Value *v); /* T_REAL, or T_INT coerced to double */

/* Bounds-checked reads of an activation's local slot / a closure's env field.
 */
Value *THxVALUE_local(Value **locals, size_t n, size_t i);
Value *THxVALUE_env(Value **env, size_t n, size_t i);

/* T_STR byte pointer (NUL-terminated), for passing to foreign functions. */
char *THxVALUE_str(Value *v);

/* T_STRUCT field by name (fails, naming the field, if absent). */
Value *THxVALUE_field(Value *rec, const char *name);

/* T_VARIANT constructor tag and positional payload (bounds-checked). */
const char *THxVALUE_ctor(Value *v);
Value      *THxVALUE_variant_field(Value *v, size_t i);

/*------------------------------------------------------------------------------
 *\LIFETIME (used by the RC memory engine and the CEK driver)
 *-----------------------------------------------------------------------------*/

/* Release v's children and free its owned raw payload blocks (arrays, string
 * bytes, resumption segment). Called exactly once by the RC engine when v's
 * count reaches zero; never call it directly. A child pointer EQUAL TO v is
 * skipped -- the weak self edge of a recursive-let closure that captured its
 * own box (the one RC cycle; see doc/native-backend.md). */
void THxVALUE_destroy(Value *v);

/* Back-patch a let box: make `box` a copy of `v`, preserving box's own count.
 * Pointer-array payloads (fields/env/args) and string bytes are DEEP-copied
 * into fresh blocks and the children retained (a shared array would be freed
 * twice), so box and v afterwards have independent lifetimes; a resumption's
 * segment is shared via its own count. A child equal to `box` is left
 * unretained (the weak self edge, matching THxVALUE_destroy). */
void THxVALUE_patch_box(Value *box, Value *v);

#endif /* THxVALUE_H_ */
