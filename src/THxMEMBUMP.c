/*-------------------------------------------------------------------------------
 *\file THxMEMBUMP.c
 *\info The v1 memory engine: a growing chain of bump blocks that is never
 *      freed. This is deliberately the simplest correct strategy -- finite
 *      programs (every test and example that the native backend targets today)
 *      run to completion well within memory, and never freeing means generated
 *      code needs no retain/release discipline and recursive-let cycles are a
 *      non-issue. retain/release are no-ops here; swap this file for
 *      ext/THxMEMRC.c to get reference counting. See doc/native-backend.md.
 *-----------------------------------------------------------------------------*/

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
  if (b == NULL) THxCHECK_FAILF("THxMEM_alloc: out of memory (needed %zu bytes)", need);
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
