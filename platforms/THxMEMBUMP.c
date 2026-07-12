/*-------------------------------------------------------------------------------
 *\file THxMEMBUMP.c
 *\info The fallback memory engine (-DTHX_MEM_BUMP): a growing chain of bump
 *      blocks that is never freed. Deliberately the simplest correct strategy;
 *      retain/release/pool are no-ops, THxMEM_live is always 0 (a leak check
 *      passes trivially). The default engine is THxMEMRC.c (reference
 *      counting); this one is kept for debugging -- a memory bug that
 *      disappears under bump is an RC-discipline bug. See
 *      doc/native-backend.md.
 *-----------------------------------------------------------------------------*/

#ifdef THX_MEM_BUMP

#include "THxMEM.h"

#include "THxCHECK.h"

#include <stdlib.h>
#include <string.h>

/* One bump block. Blocks are chained newest-first; we never free. */
typedef struct Block
{
  struct Block *prev;
  size_t        len; /* bytes handed out */
  size_t        cap; /* bytes available in `data` */
  unsigned char data[];
} Block;

#define RT_BLOCK_MIN (1u << 20) /* 1 MiB default block */

static Block *g_cur = NULL;

/* Round `n` up so successive allocations stay aligned for any Value member. */
static size_t
align_up(
  size_t n)
{
  const size_t a = sizeof(void *) > 8 ? sizeof(void *) : 8;
  return (n + (a - 1)) & ~(a - 1);
}

static Block *
new_block(
  size_t need)
{
  size_t cap = need > RT_BLOCK_MIN ? need : RT_BLOCK_MIN;
  Block *b   = (Block *)malloc(sizeof(Block) + cap);
  if (b == NULL)
    THxCHECK_FAILF("THxMEM_alloc: out of memory (needed %zu bytes)", need);
  b->prev = g_cur;
  b->len  = 0;
  b->cap  = cap;
  return b;
}

void *
THxMEM_alloc(
  size_t size)
{
  size_t need = align_up(size == 0 ? 1 : size);
  if (g_cur == NULL || g_cur->len + need > g_cur->cap) g_cur = new_block(need);
  void *p = g_cur->data + g_cur->len;
  g_cur->len += need;
  memset(p, 0, need);
  return p;
}

Value *
THxMEM_alloc_value(
  void)
{
  return (Value *)THxMEM_alloc(sizeof(Value));
}

void
THxMEM_free(
  void *p)
{
  (void)p; /* no-op: the bump engine never frees */
}

void
THxMEM_retain(
  Value *v)
{
  (void)v; /* no-op: the bump engine never frees */
}

void
THxMEM_release(
  Value *v)
{
  (void)v; /* no-op: the bump engine never frees */
}

int
THxMEM_unique(
  Value *v)
{
  (void)v;  /* no ref counts here: never claim uniqueness, so a mutator
             * always copies (correct; the bump engine never frees anyway) */
  return 0;
}

size_t
THxMEM_pool_mark(
  void)
{
  return 0;
}

void
THxMEM_pool_drain(
  size_t mark)
{
  (void)mark;
}

size_t
THxMEM_live(
  void)
{
  return 0; /* nothing is tracked, so a leak check passes trivially */
}

#endif /* THX_MEM_BUMP */
