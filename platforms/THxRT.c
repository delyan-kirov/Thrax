/*-------------------------------------------------------------------------------
 *\file THxRT.c
 *\info The runtime core: value constructors, the apply trampoline, and the
 *      built-in operator dispatch. It mirrors the interpreter's runtime
 *      semantics (inc/ITxDATA.hpp, src/IT.cpp) in C: strict evaluation, curried
 *      builtins that fire when saturated, and tail calls that reuse the
 *      activation (constant stack) via an explicit bounce.
 *-----------------------------------------------------------------------------*/

#include "THxRT.h"

#include <string.h>

/*------------------------------------------------------------------------------
 *\CONSTRUCTORS
 *-----------------------------------------------------------------------------*/

Value *
THxRT_int(
  long long v)
{
  Value *x = THxMEM_alloc_value();
  x->tag   = T_INT;
  x->u.i   = v;
  return x;
}

Value *
THxRT_real(
  double v)
{
  Value *x = THxMEM_alloc_value();
  x->tag   = T_REAL;
  x->u.r   = v;
  return x;
}

Value *
THxRT_str(
  const char *p, size_t n)
{
  Value *x = THxMEM_alloc_value();
  x->tag   = T_STR;
  char *b  = (char *)THxMEM_alloc(n + 1);
  if (n > 0) memcpy(b, p, n);
  b[n]     = '\0';
  x->u.s.p = b;
  x->u.s.n = n;
  return x;
}

Value *
THxRT_unk(
  void)
{
  Value *x = THxMEM_alloc_value();
  x->tag   = T_UNK;
  return x;
}

Value *
THxRT_bytes(
  size_t n)
{
  Value *x = THxMEM_alloc_value();
  x->tag   = T_STR; /* the runtime, like the interpreter, does not distinguish
                       Array from Str -- both are a byte block. */
  x->u.s.p = (char *)THxMEM_alloc(n == 0 ? 1 : n);
  x->u.s.n = n;
  return x;
}

Value *
THxRT_struct(
  const char *name, size_t n, const char **fnames, Value **fields)
{
  Value *x         = THxMEM_alloc_value();
  x->tag           = T_STRUCT;
  x->u.st.name     = name;
  x->u.st.n        = n;
  const char **fns = (const char **)THxMEM_alloc(n * sizeof(const char *));
  Value      **fs  = (Value **)THxMEM_alloc(n * sizeof(Value *));
  for (size_t i = 0; i < n; ++i)
  {
    fns[i] = fnames[i];
    fs[i]  = fields[i];
    THxK_mark_escape(
      fields[i]); /* a resumption stored in a field has escaped */
  }
  x->u.st.fnames = fns;
  x->u.st.f      = fs;
  return x;
}

Value *
THxRT_variant(
  const char *tname, const char *ctor, size_t n, Value **fields)
{
  Value *x       = THxMEM_alloc_value();
  x->tag         = T_VARIANT;
  x->u.var.tname = tname;
  x->u.var.ctor  = ctor;
  x->u.var.n     = n;
  Value **fs     = (Value **)THxMEM_alloc(n * sizeof(Value *));
  for (size_t i = 0; i < n; ++i)
  {
    fs[i] = fields[i];
    THxK_mark_escape(fields[i]);
  }
  x->u.var.f = fs;
  return x;
}

Value *
THxRT_closure(
  int code, Value **captures, size_t n)
{
  Value *x       = THxMEM_alloc_value();
  x->tag         = T_CLOS;
  x->u.clos.code = code;
  x->u.clos.nenv = n;
  Value **env    = (Value **)THxMEM_alloc(n * sizeof(Value *));
  for (size_t i = 0; i < n; ++i)
  {
    env[i] = captures[i];
    THxK_mark_escape(
      captures[i]); /* a resumption captured in a closure escaped */
  }
  x->u.clos.env = env;
  return x;
}

/*------------------------------------------------------------------------------
 *\BUILT-IN OPERATORS
 *
 * The finite dispatch table reimplemented from inc/ITxDATA.hpp's `impls`, keyed
 * by the monomorphic strings OP::mono produces ("+@Int", "?<@Real", ...). The
 * arity table and the dispatch chain below must list the same keys; a key in
 * one but not the other trips a THxCHECK_FAIL (domino).
 *-----------------------------------------------------------------------------*/

typedef struct
{
  const char *key;
  size_t      arity;
} BuiltinArity;

static const BuiltinArity g_builtins[] = {
  { "%array", 1 },  { "neg@Int", 1 }, { "neg@Real", 1 }, { "not@Int", 1 },
  { "+@Int", 2 },   { "-@Int", 2 },   { "*@Int", 2 },    { "/@Int", 2 },
  { "%@Int", 2 },   { "?=@Int", 2 },  { "?>@Int", 2 },   { "?<@Int", 2 },
  { "<=@Int", 2 },  { ">=@Int", 2 },  { "+@Real", 2 },   { "-@Real", 2 },
  { "*@Real", 2 },  { "/@Real", 2 },  { "%@Real", 2 },   { "?=@Real", 2 },
  { "?>@Real", 2 }, { "?<@Real", 2 }, { "<=@Real", 2 },  { ">=@Real", 2 },
};

static size_t
builtin_arity(
  const char *key)
{
  for (size_t i = 0; i < sizeof(g_builtins) / sizeof(g_builtins[0]); ++i)
    if (strcmp(g_builtins[i].key, key) == 0) return g_builtins[i].arity;
  THxCHECK_FAILF("THxRT_builtin: unknown built-in '%s'", key);
}

Value *
THxRT_builtin(
  const char *impl)
{
  Value *x      = THxMEM_alloc_value();
  x->tag        = T_BUILTIN;
  x->u.bi.impl  = impl;
  x->u.bi.arity = builtin_arity(impl);
  x->u.bi.nargs = 0;
  x->u.bi.args  = NULL;
  return x;
}

Value *
THxRT_extern(
  int idx, size_t arity)
{
  THxCHECK_ASSERT((size_t)idx < THxRT_extern_count,
                  "THxRT_extern: wrapper index out of range");
  Value *x       = THxMEM_alloc_value();
  x->tag         = T_EXTERN;
  x->u.ext.idx   = idx;
  x->u.ext.arity = arity;
  x->u.ext.nargs = 0;
  x->u.ext.args  = NULL;
  return x;
}

static Value *
builtin_dispatch(
  Value *b)
{
  const char *k = b->u.bi.impl;
  Value     **a = b->u.bi.args;

  if (!strcmp(k, "%array"))
  {
    long long n = THxVALUE_as_int(a[0]);
    return THxRT_bytes(n < 0 ? 0 : (size_t)n);
  }

  if (!strcmp(k, "neg@Int")) return THxRT_int(-THxVALUE_as_int(a[0]));
  if (!strcmp(k, "neg@Real")) return THxRT_real(-THxVALUE_as_num(a[0]));
  if (!strcmp(k, "not@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) == 0 ? 1 : 0);

  if (!strcmp(k, "+@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) + THxVALUE_as_int(a[1]));
  if (!strcmp(k, "-@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) - THxVALUE_as_int(a[1]));
  if (!strcmp(k, "*@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) * THxVALUE_as_int(a[1]));
  if (!strcmp(k, "/@Int"))
  {
    long long d = THxVALUE_as_int(a[1]);
    if (d == 0) THxCHECK_FAIL("division by zero (/@Int)");
    return THxRT_int(THxVALUE_as_int(a[0]) / d);
  }
  if (!strcmp(k, "%@Int"))
  {
    long long d = THxVALUE_as_int(a[1]);
    if (d == 0) THxCHECK_FAIL("division by zero (%@Int)");
    return THxRT_int(THxVALUE_as_int(a[0]) % d);
  }
  if (!strcmp(k, "?=@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) == THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, "?>@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) > THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, "?<@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) < THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, "<=@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) <= THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, ">=@Int"))
    return THxRT_int(THxVALUE_as_int(a[0]) >= THxVALUE_as_int(a[1]) ? 1 : 0);

  if (!strcmp(k, "+@Real"))
    return THxRT_real(THxVALUE_as_num(a[0]) + THxVALUE_as_num(a[1]));
  if (!strcmp(k, "-@Real"))
    return THxRT_real(THxVALUE_as_num(a[0]) - THxVALUE_as_num(a[1]));
  if (!strcmp(k, "*@Real"))
    return THxRT_real(THxVALUE_as_num(a[0]) * THxVALUE_as_num(a[1]));
  if (!strcmp(k, "/@Real"))
    return THxRT_real(THxVALUE_as_num(a[0]) / THxVALUE_as_num(a[1]));
  if (!strcmp(k, "%@Real"))
  {
    double x = THxVALUE_as_num(a[0]), y = THxVALUE_as_num(a[1]);
    return THxRT_real(x - y * (double)(long long)(x / y));
  }
  if (!strcmp(k, "?=@Real"))
    return THxRT_int(THxVALUE_as_num(a[0]) == THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, "?>@Real"))
    return THxRT_int(THxVALUE_as_num(a[0]) > THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, "?<@Real"))
    return THxRT_int(THxVALUE_as_num(a[0]) < THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, "<=@Real"))
    return THxRT_int(THxVALUE_as_num(a[0]) <= THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, ">=@Real"))
    return THxRT_int(THxVALUE_as_num(a[0]) >= THxVALUE_as_num(a[1]) ? 1 : 0);

  THxCHECK_FAILF("builtin_dispatch: unimplemented built-in '%s'", k);
}

/* A fresh builtin value with `x` appended to `b`'s accumulated arguments. */
static Value *
builtin_push(
  Value *b, Value *x)
{
  size_t n       = b->u.bi.nargs;
  Value *nb      = THxMEM_alloc_value();
  nb->tag        = T_BUILTIN;
  nb->u.bi.impl  = b->u.bi.impl;
  nb->u.bi.arity = b->u.bi.arity;
  nb->u.bi.nargs = n + 1;
  Value **args   = (Value **)THxMEM_alloc((n + 1) * sizeof(Value *));
  for (size_t i = 0; i < n; ++i) args[i] = b->u.bi.args[i];
  args[n]       = x;
  nb->u.bi.args = args;
  return nb;
}

/* A fresh foreign value with `x` appended to `e`'s accumulated arguments. */
static Value *
extern_push(
  Value *e, Value *x)
{
  size_t n        = e->u.ext.nargs;
  Value *ne       = THxMEM_alloc_value();
  ne->tag         = T_EXTERN;
  ne->u.ext.idx   = e->u.ext.idx;
  ne->u.ext.arity = e->u.ext.arity;
  ne->u.ext.nargs = n + 1;
  Value **args    = (Value **)THxMEM_alloc((n + 1) * sizeof(Value *));
  for (size_t i = 0; i < n; ++i) args[i] = e->u.ext.args[i];
  args[n]        = x;
  ne->u.ext.args = args;
  return ne;
}

/*------------------------------------------------------------------------------
 *\APPLICATION -- builtin / foreign operand accumulation
 *
 * The CEK driver (THxK) drives application of closures/operations/resumptions;
 * these two handle the non-suspending callees. Each appends one operand and, on
 * saturation, runs the operator / foreign call, returning the result value (or
 * the still-partial callee).
 *-----------------------------------------------------------------------------*/

Value *
THxRT_apply_builtin(
  Value *f, Value *arg)
{
  Value *b = builtin_push(f, arg);
  if (b->u.bi.nargs >= b->u.bi.arity) return builtin_dispatch(b);
  return b;
}

Value *
THxRT_apply_extern(
  Value *f, Value *arg)
{
  Value *e = extern_push(f, arg);
  if (e->u.ext.nargs >= e->u.ext.arity)
  {
    THxCHECK_ASSERT((size_t)e->u.ext.idx < THxRT_extern_count,
                    "THxRT_apply_extern: extern index out of range");
    return THxRT_extern_table[e->u.ext.idx](e->u.ext.args);
  }
  return e;
}
