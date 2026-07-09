/*-------------------------------------------------------------------------------
 *\file THxVALUE.c
 *\info The checked accessors and tag naming. Each accessor asserts the tag (and
 *      any index bounds) before touching the union, so a codegen or runtime bug
 *      aborts with a precise message at the point of misuse.
 *-----------------------------------------------------------------------------*/

#include "THxVALUE.h"

#include "THxCHECK.h"

#include <string.h>

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
