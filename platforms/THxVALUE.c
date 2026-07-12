/*-------------------------------------------------------------------------------
 *\file THxVALUE.c
 *\info The checked accessors, tag naming, and the value lifetime helpers
 *      (destroy / patch_box) the RC memory engine and the CEK driver use. Each
 *      accessor asserts the tag (and any index bounds) before touching the
 *      union, so a codegen or runtime bug aborts with a precise message at the
 *      point of misuse.
 *-----------------------------------------------------------------------------*/

#include "THxVALUE.h"

#include "THxCHECK.h"
#include "THxMEM.h"

#include <string.h>

/* Resumption segment lifetime, owned by the CEK driver (THxK.c). A segment is
 * shared by every T_RESUMP value that aliases it (patch_box), so it carries its
 * own count. */
void THxK_resump_addref(THxK_Resump *seg);
void THxK_resump_release(THxK_Resump *seg);

const char *
THxVALUE_tag_name(
  Tag t)
{
  switch (t)
  {
  case T_INT    : return "Int";
  case T_REAL   : return "Real";
  case T_STR    : return "Str";
  case T_STRUCT : return "Struct";
  case T_VARIANT: return "Variant";
  case T_CLOS   : return "Closure";
  case T_BUILTIN: return "Builtin";
  case T_EXTERN : return "Extern";
  case T_OP     : return "Op";
  case T_RESUMP : return "Resumption";
  case T_DEFER  : return "Defer";
  case T_UNK    : return "Unk";
  }
  THxCHECK_FAIL("THxVALUE_tag_name: unhandled tag");
}

long long
THxVALUE_as_int(
  Value *v)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_as_int: null value");
  if (v->tag != T_INT)
    THxCHECK_FAILF("THxVALUE_as_int: expected Int, got %s",
                   THxVALUE_tag_name(v->tag));
  return v->u.i;
}

double
THxVALUE_as_num(
  Value *v)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_as_num: null value");
  if (v->tag == T_INT) return (double)v->u.i;
  if (v->tag == T_REAL) return v->u.r;
  THxCHECK_FAILF("THxVALUE_as_num: expected Int or Real, got %s",
                 THxVALUE_tag_name(v->tag));
}

Value *
THxVALUE_local(
  Value **locals, size_t n, size_t i)
{
  THxCHECK_ASSERT(locals != NULL, "THxVALUE_local: null activation");
  if (i >= n)
    THxCHECK_FAILF("THxVALUE_local: slot %zu out of range (have %zu)", i, n);
  THxCHECK_ASSERT(locals[i] != NULL, "THxVALUE_local: read of an unbound slot");
  return locals[i];
}

Value *
THxVALUE_env(
  Value **env, size_t n, size_t i)
{
  if (i >= n)
    THxCHECK_FAILF("THxVALUE_env: field %zu out of range (have %zu)", i, n);
  THxCHECK_ASSERT(env[i] != NULL, "THxVALUE_env: read of an unbound capture");
  return env[i];
}

char *
THxVALUE_str(
  Value *v)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_str: null value");
  if (v->tag != T_STR)
    THxCHECK_FAILF("THxVALUE_str: expected Str, got %s",
                   THxVALUE_tag_name(v->tag));
  return v->u.s.p;
}

size_t
THxVALUE_str_len(
  Value *v)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_str_len: null value");
  if (v->tag != T_STR)
    THxCHECK_FAILF("THxVALUE_str_len: expected Str, got %s",
                   THxVALUE_tag_name(v->tag));
  return v->u.s.n;
}

size_t
THxVALUE_str_cap(
  Value *v)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_str_cap: null value");
  if (v->tag != T_STR)
    THxCHECK_FAILF("THxVALUE_str_cap: expected Str, got %s",
                   THxVALUE_tag_name(v->tag));
  return v->u.s.cap;
}

Value *
THxVALUE_field(
  Value *rec, const char *name)
{
  THxCHECK_ASSERT(rec != NULL, "THxVALUE_field: null record");
  if (rec->tag != T_STRUCT)
    THxCHECK_FAILF("THxVALUE_field: expected Struct, got %s",
                   THxVALUE_tag_name(rec->tag));
  for (size_t k = 0; k < rec->u.st.n; ++k)
    if (strcmp(rec->u.st.fnames[k], name) == 0) return rec->u.st.f[k];
  THxCHECK_FAILF("THxVALUE_field: no field '%s' on struct '%s'",
                 name,
                 rec->u.st.name ? rec->u.st.name : "<anon>");
}

const char *
THxVALUE_ctor(
  Value *v)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_ctor: null value");
  if (v->tag != T_VARIANT)
    THxCHECK_FAILF("THxVALUE_ctor: expected Variant, got %s",
                   THxVALUE_tag_name(v->tag));
  return v->u.var.ctor;
}

Value *
THxVALUE_variant_field(
  Value *v, size_t i)
{
  THxCHECK_ASSERT(v != NULL, "THxVALUE_variant_field: null value");
  if (v->tag != T_VARIANT)
    THxCHECK_FAILF("THxVALUE_variant_field: expected Variant, got %s",
                   THxVALUE_tag_name(v->tag));
  if (i >= v->u.var.n)
    THxCHECK_FAILF("THxVALUE_variant_field: field %zu out of range (have %zu)",
                   i,
                   v->u.var.n);
  return v->u.var.f[i];
}

/*------------------------------------------------------------------------------
 *\LIFETIME
 *-----------------------------------------------------------------------------*/

/* Release the children in `f[0..n)` (skipping the weak self edge, child == v)
 * and free the array itself. */
static void
release_children(
  Value *v, Value **f, size_t n)
{
  if (f == NULL) return;
  for (size_t i = 0; i < n; ++i)
    if (f[i] != v) THxMEM_release(f[i]);
  THxMEM_free(f);
}

/* Free v's owned payload: release children, free raw blocks. Shared by
 * THxVALUE_destroy (rc reached zero) and THxVALUE_patch_box (overwriting a
 * box's previous contents). Leaves the union in a don't-care state; the caller
 * frees v or overwrites it. */
static void
payload_destroy(
  Value *v)
{
  switch (v->tag)
  {
  case T_INT:
  case T_REAL:
  case T_OP:
  case T_UNK: return;
  case T_STR: THxMEM_free(v->u.s.p); return;
  case T_STRUCT:
    THxMEM_free((void *)v->u.st.fnames); /* the array; the names are static */
    release_children(v, v->u.st.f, v->u.st.n);
    return;
  case T_VARIANT: release_children(v, v->u.var.f, v->u.var.n); return;
  case T_CLOS   : release_children(v, v->u.clos.env, v->u.clos.nenv); return;
  case T_BUILTIN: release_children(v, v->u.bi.args, v->u.bi.nargs); return;
  case T_EXTERN : release_children(v, v->u.ext.args, v->u.ext.nargs); return;
  case T_DEFER  : release_children(v, v->u.defer.args, v->u.defer.nargs); return;
  case T_RESUMP : THxK_resump_release(v->u.resump.seg); return;
  }
  THxCHECK_FAIL("THxVALUE payload_destroy: unhandled tag");
}

void
THxVALUE_destroy(
  Value *v)
{
  payload_destroy(v);
}

/* A fresh array holding f[0..n), each child retained except the weak self edge
 * (child == self). NULL when n == 0. */
static Value **
copy_children(
  Value *self, Value **f, size_t n)
{
  if (n == 0) return NULL;
  Value **c = (Value **)THxMEM_alloc(n * sizeof(Value *));
  for (size_t i = 0; i < n; ++i)
  {
    c[i] = f[i];
    if (c[i] != self) THxMEM_retain(c[i]);
  }
  return c;
}

void
THxVALUE_patch_box(
  Value *box, Value *v)
{
  THxCHECK_ASSERT(box != NULL && v != NULL, "THxVALUE_patch_box: null value");
  if (box == v) return;

  payload_destroy(box); /* a box is normally T_UNK: a no-op */
  unsigned saved = box->rc;
  *box           = *v;
  box->rc        = saved;

  /* The bitwise copy above SHARES v's payload blocks; give box its own (a
   * shared array would be freed by both destructors). */
  switch (box->tag)
  {
  case T_INT:
  case T_REAL:
  case T_OP:
  case T_UNK: break;
  case T_STR:
  {
    char *b = (char *)THxMEM_alloc(box->u.s.n + 1);
    if (box->u.s.n > 0) memcpy(b, v->u.s.p, box->u.s.n);
    b[box->u.s.n] = '\0';
    box->u.s.p    = b;
    box->u.s.cap  = box->u.s.n; /* the copy owns exactly its bytes */
    break;
  }
  case T_STRUCT:
  {
    size_t n = box->u.st.n;
    if (n == 0)
    {
      box->u.st.fnames = NULL;
      box->u.st.f      = NULL;
      break;
    }
    const char **fns = (const char **)THxMEM_alloc(n * sizeof(const char *));
    for (size_t i = 0; i < n; ++i) fns[i] = v->u.st.fnames[i];
    box->u.st.fnames = fns;
    box->u.st.f      = copy_children(box, v->u.st.f, n);
    break;
  }
  case T_VARIANT:
    box->u.var.f = copy_children(box, v->u.var.f, box->u.var.n);
    break;
  case T_CLOS:
    box->u.clos.env = copy_children(box, v->u.clos.env, box->u.clos.nenv);
    break;
  case T_BUILTIN:
    box->u.bi.args = copy_children(box, v->u.bi.args, box->u.bi.nargs);
    break;
  case T_EXTERN:
    box->u.ext.args = copy_children(box, v->u.ext.args, box->u.ext.nargs);
    break;
  case T_DEFER:
    box->u.defer.args = copy_children(box, v->u.defer.args, box->u.defer.nargs);
    break;
  case T_RESUMP: THxK_resump_addref(box->u.resump.seg); break;
  }
}
