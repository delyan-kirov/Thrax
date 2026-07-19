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
  b[n]       = '\0';
  x->u.s.p   = b;
  x->u.s.n   = n;
  x->u.s.cap = n; /* the +1 NUL slot is not counted as capacity */
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
  x->u.s.p   = (char *)THxMEM_alloc(n == 0 ? 1 : n);
  x->u.s.n   = n;
  x->u.s.cap = n;
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
    THxMEM_retain(fields[i]); /* the struct owns its fields */
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
    THxMEM_retain(fields[i]); /* the variant owns its payload */
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
    THxMEM_retain(captures[i]); /* the closure owns its captured record */
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

#ifndef THX_INT
#error "THX_INT must be defined by the emitter (see engines/CC.cpp)"
#endif
// FIXME : How do we know float64 is supported on that platform?
#define THX_REAL "@float64"
#define THX_STR "@str"

static const BuiltinArity g_builtins[] = {
  { "array_len", 1 },   { "array_cap", 1 },    { "array_get", 2 },
  { "array_push", 2 },  { "array_set", 3 },    { "array_slice", 3 },
  { "%concat", 2 },     { "?=" THX_STR, 2 },   { "%array", 1 },
  { "neg" THX_INT, 1 }, { "neg" THX_REAL, 1 }, { "not" THX_INT, 1 },
  { "+" THX_INT, 2 },   { "-" THX_INT, 2 },    { "*" THX_INT, 2 },
  { "/" THX_INT, 2 },   { "%" THX_INT, 2 },    { "?=" THX_INT, 2 },
  { "?>" THX_INT, 2 },  { "?<" THX_INT, 2 },   { "<=" THX_INT, 2 },
  { ">=" THX_INT, 2 },  { "+" THX_REAL, 2 },   { "-" THX_REAL, 2 },
  { "*" THX_REAL, 2 },  { "/" THX_REAL, 2 },   { "%" THX_REAL, 2 },
  { "?=" THX_REAL, 2 }, { "?>" THX_REAL, 2 },  { "?<" THX_REAL, 2 },
  { "<=" THX_REAL, 2 }, { ">=" THX_REAL, 2 },
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

// FIXME: This implementation is garbage
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

  /* Growable byte-vector reads (see doc/strings-and-arrays.md). */
  if (!strcmp(k, "array_len"))
    return THxRT_int((long long)THxVALUE_str_len(a[0]));
  if (!strcmp(k, "array_cap"))
    return THxRT_int((long long)THxVALUE_str_cap(a[0]));
  if (!strcmp(k, "array_get"))
  {
    long long i = THxVALUE_as_int(a[1]);
    THxCHECK_ASSERT(i >= 0 && (size_t)i < THxVALUE_str_len(a[0]),
                    "array_get: index out of bounds");
    return THxRT_int((unsigned char)THxVALUE_str(a[0])[i]);
  }

  /* Mutators -- opportunistic in-place: mutate a[0]'s buffer when it is
   * uniquely owned (THxMEM_unique), else build a fresh value. When mutating in
   * place we retain a[0] and return it, so it survives the caller builtin
   * releasing its operand row. Value semantics are preserved either way. */
  if (!strcmp(k, "array_push"))
  {
    Value        *v    = a[0];
    unsigned char byte = (unsigned char)THxVALUE_as_int(a[1]);
    size_t        n    = v->u.s.n;
    if (THxMEM_unique(v))
    {
      if (n + 1 > v->u.s.cap)
      {
        size_t ncap = v->u.s.cap ? v->u.s.cap * 2 : 8;
        if (ncap < n + 1) ncap = n + 1;
        char *nb = (char *)THxMEM_alloc(ncap + 1);
        if (n) memcpy(nb, v->u.s.p, n);
        THxMEM_free(v->u.s.p);
        v->u.s.p   = nb;
        v->u.s.cap = ncap;
      }
      v->u.s.p[n]     = (char)byte;
      v->u.s.p[n + 1] = '\0';
      v->u.s.n        = n + 1;
      THxMEM_retain(v);
      return v;
    }
    size_t ncap = (n + 1 < 8) ? 8 : n + 1;
    Value *x    = THxMEM_alloc_value();
    x->tag      = T_STR;
    char *nb    = (char *)THxMEM_alloc(ncap + 1);
    if (n) memcpy(nb, v->u.s.p, n);
    nb[n]      = (char)byte;
    nb[n + 1]  = '\0';
    x->u.s.p   = nb;
    x->u.s.n   = n + 1;
    x->u.s.cap = ncap;
    return x;
  }
  if (!strcmp(k, "array_set"))
  {
    Value        *v    = a[0];
    long long     i    = THxVALUE_as_int(a[1]);
    unsigned char byte = (unsigned char)THxVALUE_as_int(a[2]);
    size_t        n    = v->u.s.n;
    THxCHECK_ASSERT(i >= 0 && (size_t)i < n, "array_set: index out of bounds");
    if (THxMEM_unique(v))
    {
      v->u.s.p[i] = (char)byte;
      THxMEM_retain(v);
      return v;
    }
    Value *x    = THxRT_str(v->u.s.p, n); /* fresh copy */
    x->u.s.p[i] = (char)byte;
    return x;
  }
  if (!strcmp(k, "array_slice"))
  {
    Value    *v   = a[0];
    long long beg = THxVALUE_as_int(a[1]);
    long long end = THxVALUE_as_int(a[2]);
    size_t    n   = v->u.s.n;
    if (beg < 0) beg = 0;
    if (end > (long long)n) end = (long long)n;
    if (end < beg) end = beg;
    return THxRT_str(v->u.s.p + beg, (size_t)(end - beg));
  }

  /* `++` concatenation (Str/Array): append rhs to lhs. Opportunistic in-place
   * on the lhs buffer when it is unique, else a fresh block. */
  if (!strcmp(k, "%concat"))
  {
    Value *l    = a[0];
    Value *r    = a[1];
    size_t ln   = l->u.s.n;
    size_t rn   = r->u.s.n;
    size_t need = ln + rn;
    if (THxMEM_unique(l))
    {
      if (need > l->u.s.cap)
      {
        size_t ncap = l->u.s.cap ? l->u.s.cap * 2 : 8;
        if (ncap < need) ncap = need;
        char *nb = (char *)THxMEM_alloc(ncap + 1);
        if (ln) memcpy(nb, l->u.s.p, ln);
        THxMEM_free(l->u.s.p);
        l->u.s.p   = nb;
        l->u.s.cap = ncap;
      }
      if (rn) memcpy(l->u.s.p + ln, r->u.s.p, rn);
      l->u.s.p[need] = '\0';
      l->u.s.n       = need;
      THxMEM_retain(l);
      return l;
    }
    size_t ncap = need < 8 ? 8 : need;
    Value *x    = THxMEM_alloc_value();
    x->tag      = T_STR;
    char *nb    = (char *)THxMEM_alloc(ncap + 1);
    if (ln) memcpy(nb, l->u.s.p, ln);
    if (rn) memcpy(nb + ln, r->u.s.p, rn);
    nb[need]   = '\0';
    x->u.s.p   = nb;
    x->u.s.n   = need;
    x->u.s.cap = ncap;
    return x;
  }
  /* Str byte-equality. */
  if (!strcmp(k, "?=" THX_STR))
  {
    size_t ln = a[0]->u.s.n;
    size_t rn = a[1]->u.s.n;
    int eq = ln == rn && (ln == 0 || memcmp(a[0]->u.s.p, a[1]->u.s.p, ln) == 0);
    return THxRT_int(eq ? 1 : 0);
  }

  if (!strcmp(k, "neg" THX_INT)) return THxRT_int(-THxVALUE_as_int(a[0]));
  if (!strcmp(k, "neg" THX_REAL)) return THxRT_real(-THxVALUE_as_num(a[0]));
  if (!strcmp(k, "not" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) == 0 ? 1 : 0);

  if (!strcmp(k, "+" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) + THxVALUE_as_int(a[1]));
  if (!strcmp(k, "-" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) - THxVALUE_as_int(a[1]));
  if (!strcmp(k, "*" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) * THxVALUE_as_int(a[1]));
  if (!strcmp(k, "/" THX_INT))
  {
    long long d = THxVALUE_as_int(a[1]);
    if (d == 0) THxCHECK_FAIL("division by zero (/" THX_INT ")");
    return THxRT_int(THxVALUE_as_int(a[0]) / d);
  }
  if (!strcmp(k, "%" THX_INT))
  {
    long long d = THxVALUE_as_int(a[1]);
    if (d == 0) THxCHECK_FAIL("division by zero (%" THX_INT ")");
    return THxRT_int(THxVALUE_as_int(a[0]) % d);
  }
  if (!strcmp(k, "?=" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) == THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, "?>" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) > THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, "?<" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) < THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, "<=" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) <= THxVALUE_as_int(a[1]) ? 1 : 0);
  if (!strcmp(k, ">=" THX_INT))
    return THxRT_int(THxVALUE_as_int(a[0]) >= THxVALUE_as_int(a[1]) ? 1 : 0);

  if (!strcmp(k, "+" THX_REAL))
    return THxRT_real(THxVALUE_as_num(a[0]) + THxVALUE_as_num(a[1]));
  if (!strcmp(k, "-" THX_REAL))
    return THxRT_real(THxVALUE_as_num(a[0]) - THxVALUE_as_num(a[1]));
  if (!strcmp(k, "*" THX_REAL))
    return THxRT_real(THxVALUE_as_num(a[0]) * THxVALUE_as_num(a[1]));
  if (!strcmp(k, "/" THX_REAL))
    return THxRT_real(THxVALUE_as_num(a[0]) / THxVALUE_as_num(a[1]));
  if (!strcmp(k, "%" THX_REAL))
  {
    double x = THxVALUE_as_num(a[0]), y = THxVALUE_as_num(a[1]);
    return THxRT_real(x - y * (double)(long long)(x / y));
  }
  if (!strcmp(k, "?=" THX_REAL))
    return THxRT_int(THxVALUE_as_num(a[0]) == THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, "?>" THX_REAL))
    return THxRT_int(THxVALUE_as_num(a[0]) > THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, "?<" THX_REAL))
    return THxRT_int(THxVALUE_as_num(a[0]) < THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, "<=" THX_REAL))
    return THxRT_int(THxVALUE_as_num(a[0]) <= THxVALUE_as_num(a[1]) ? 1 : 0);
  if (!strcmp(k, ">=" THX_REAL))
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
  args[n] = x;
  for (size_t i = 0; i <= n; ++i)
    THxMEM_retain(args[i]); /* the new callee owns its operand row */
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
  args[n] = x;
  for (size_t i = 0; i <= n; ++i)
    THxMEM_retain(args[i]); /* the new callee owns its operand row */
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
