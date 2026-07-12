/*-------------------------------------------------------------------------------
 *\file THxMEMRC.c
 *\info The default memory engine: precise reference counting over Value::rc.
 *      Strict + pure data is acyclic by construction (see doc/native-backend.md
 *      and doc/effect-system-design.md section 8), so plain RC is complete; the
 * one cycle -- a recursive-let closure capturing its own box -- is a weak self
 *      edge handled in THxVALUE_destroy / THxVALUE_patch_box.
 *
 *      Release is ITERATIVE: dropping the last reference to a long list must
 *      not recurse on the C stack, so dead values go on a worklist that the
 *      outermost release drains (THxVALUE_destroy re-enters THxMEM_release for
 *      children, which only enqueues).
 *
 *      The TEMP POOL gives fresh values a definite owner: every allocated value
 *      is registered with the pool holding its initial reference, and the CEK
 *      driver drains to a mark after each block bounce. Whatever a bounce
 *      stored (frame slot, kont frame, constructor child, global cache) took
 *      its own retain and survives; the rest is per-bounce garbage -- exactly
 *      the allocations a long-running loop must recycle.
 *
 *      THxMEM_live counts values + raw blocks still allocated; a program that
 *      released everything exits with live == 0 (the generated main asserts
 *      this -- the built-in leak check).
 *-----------------------------------------------------------------------------*/

#ifndef THX_MEM_BUMP

#include "THxMEM.h"

#include "THxCHECK.h"

#include <stdlib.h>
#include <string.h>

/* Defined in THxVALUE.c: releases v's children and frees its owned raw
 * payload blocks. Called exactly once, when v's count reaches zero. */
void THxVALUE_destroy(Value *v);

static size_t g_live = 0; /* live allocations: values + raw blocks */

void *
THxMEM_alloc(
  size_t size)
{
  void *p = calloc(1, size == 0 ? 1 : size);
  if (p == NULL)
    THxCHECK_FAILF("THxMEM_alloc: out of memory (needed %zu bytes)", size);
  ++g_live;
  return p;
}

void
THxMEM_free(
  void *p)
{
  if (p == NULL) return;
  THxCHECK_ASSERT(g_live > 0, "THxMEM_free: more frees than allocations");
  --g_live;
  free(p);
}

/*------------------------------------------------------------------------------
 *\THE TEMP POOL
 *-----------------------------------------------------------------------------*/

static Value **g_pool = NULL;
static size_t  g_pn = 0, g_pcap = 0;

Value *
THxMEM_alloc_value(
  void)
{
  Value *v = (Value *)THxMEM_alloc(sizeof(Value));
  v->rc    = 1; /* the pool's reference */
  if (g_pn == g_pcap)
  {
    g_pcap = g_pcap ? g_pcap * 2 : 256;
    g_pool = (Value **)realloc(g_pool, g_pcap * sizeof(Value *));
    THxCHECK_ASSERT(g_pool != NULL, "THxMEM: temp pool allocation failed");
  }
  g_pool[g_pn++] = v;
  return v;
}

size_t
THxMEM_pool_mark(
  void)
{
  return g_pn;
}

void
THxMEM_pool_drain(
  size_t mark)
{
  THxCHECK_ASSERT(mark <= g_pn, "THxMEM_pool_drain: mark above pool top");
  while (g_pn > mark) THxMEM_release(g_pool[--g_pn]);
}

/*------------------------------------------------------------------------------
 *\RETAIN / RELEASE (iterative destruction)
 *-----------------------------------------------------------------------------*/

void
THxMEM_retain(
  Value *v)
{
  if (v == NULL) return;
  THxCHECK_ASSERT(v->rc > 0, "THxMEM_retain: retaining a freed value");
  ++v->rc;
}

int
THxMEM_unique(
  Value *v)
{
  return v != NULL && v->rc == 1;
}

/* Dead-value worklist: destruction never recurses on the C stack. */
static Value **g_dead = NULL;
static size_t  g_dn = 0, g_dcap = 0;
static int     g_draining = 0;

static void
dead_push(
  Value *v)
{
  if (g_dn == g_dcap)
  {
    g_dcap = g_dcap ? g_dcap * 2 : 64;
    g_dead = (Value **)realloc(g_dead, g_dcap * sizeof(Value *));
    THxCHECK_ASSERT(g_dead != NULL, "THxMEM: dead worklist allocation failed");
  }
  g_dead[g_dn++] = v;
}

void
THxMEM_release(
  Value *v)
{
  if (v == NULL) return;
  THxCHECK_ASSERT(v->rc > 0, "THxMEM_release: releasing a freed value");
  if (--v->rc > 0) return;
  dead_push(v);
  if (g_draining) return; /* an outer drain loop will get to it */
  g_draining = 1;
  while (g_dn > 0)
  {
    Value *d = g_dead[--g_dn];
    THxVALUE_destroy(d); /* re-enters THxMEM_release: enqueues only */
    THxMEM_free(d);
  }
  g_draining = 0;
}

size_t
THxMEM_live(
  void)
{
  return g_live;
}

#endif /* !THX_MEM_BUMP */
