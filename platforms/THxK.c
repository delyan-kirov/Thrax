/*-------------------------------------------------------------------------------
 *\file THxK.c
 *\info The reified-K (CEK) driver: a C port of the interpreter's abstract
 *      machine (engines/IT.cpp). It owns the explicit continuation stack
 *      `kont`, applies values, unwinds returns, and implements algebraic
 *      effects -- `perform` (capture a continuation slice from a prompt to here
 *      and run the clause below the prompt), `resume` (splice the slice back,
 *      affine), and `defer` (deferred cleanups). Generated blocks are leaves
 *      the driver calls; they never recurse into each other on the C stack, so
 *      a captured continuation is a pure heap object.
 *
 *      OWNERSHIP (the RC discipline; all no-ops under -DTHX_MEM_BUMP):
 *      - A frame slot owns its value (THxK_setlocal retains/releases).
 *      - Frames are refcounted: the driver's current activation holds one
 *        reference, every KRet (live or captured) holds one. A frame retains
 *        the closure it runs (`clos`), since it borrows that closure's env.
 *      - Every kont frame owns what it stores: KRet its frame, KPrompt its
 *        clauses + els, KDefer its cleanup, KThunkRet its saved value,
 *        KAfterClause its kval. Pushing retains (or transfers), popping
 *        releases once the payload has been handed on.
 *      - Capturing a continuation slice MOVES its kont frames into the
 *        resumption segment; resuming moves them back. A segment dropped
 *        without resuming releases its contents (the abort path). Segments
 *        carry their own count: a T_RESUMP value owns one reference, and
 *        content-copying a resumption into a let box (THxVALUE_patch_box)
 *        aliases it.
 *      - `do_ret`/`do_apply` hold one owned in-flight reference on the value /
 *        callee / argument they move between frames, so a frame switch cannot
 *        free what is still being delivered.
 *      - The temp pool (THxMEM_pool_*) owns every freshly constructed value;
 *        run_loop drains it after each block bounce, reclaiming per-iteration
 *        garbage -- this is what keeps long-running loops flat.
 *
 * The logic mirrors IT::Machine::run and its `ret` closure one-to-one; see that
 * file for the prose on each step.
 *-----------------------------------------------------------------------------*/

#include "THxK.h"

#include "THxCHECK.h"
#include "THxMEM.h"
#include "THxRT.h"

#include <stdlib.h>
#include <string.h>

/*------------------------------------------------------------------------------
 *\CONTINUATION FRAMES + RESUMPTIONS
 *-----------------------------------------------------------------------------*/

typedef enum
{
  K_RET,        /* resume a suspended activation with the returned value */
  K_PROMPT,     /* a handler delimiter */
  K_DEFER,      /* a deferred cleanup marker */
  K_THUNKRET,   /* deliver `saved`, discarding the incoming value */
  K_AFTERCLAUSE /* clause boundary: decides a captured k's defer fate */
} KTag;

typedef struct
{
  KTag tag;
  union
  {
    struct
    {
      Frame  *frame;
      BlockFn cont;
      size_t  slot;
    } ret;
    struct
    {
      const char **ops;
      Value      **clauses;
      size_t       n;
      Value       *els;
    } prompt;
    struct
    {
      Value *cleanup;
    } defer;
    struct
    {
      Value *saved;
    } thunkret;
    struct
    {
      Value *kval;
    } afterclause;
  } u;
} KFrame;

/* A captured continuation slice. Affine: `used` guards a second resume.
 * `escaped` records that the clause stored the resumption in a heap structure
 * (see THxK_mark_escape) -- the C analog of the interpreter's shared_ptr
 * use-count test, so a stored k is not finalized as if discarded. `rc` counts
 * the T_RESUMP values that own the segment (content-copying a resumption into
 * a let box aliases it). */
struct THxK_Resump
{
  unsigned rc;
  KFrame  *seg;
  size_t   n;
  int      used;
  int      escaped;
};

/*------------------------------------------------------------------------------
 *\THE CONTINUATION STACK (one, shared; re-entrant runs use a base marker)
 *-----------------------------------------------------------------------------*/

static KFrame *g_kont = NULL;
static size_t  g_kn   = 0;
static size_t  g_kcap = 0;

static void
kont_push(
  KFrame f)
{
  if (g_kn == g_kcap)
  {
    g_kcap = g_kcap ? g_kcap * 2 : 64;
    g_kont = (KFrame *)realloc(g_kont, g_kcap * sizeof(KFrame));
    THxCHECK_ASSERT(g_kont != NULL, "THxK: kont stack allocation failed");
  }
  g_kont[g_kn++] = f;
}

/*------------------------------------------------------------------------------
 *\FRAMES (refcounted activations)
 *-----------------------------------------------------------------------------*/

/* A fresh activation for `clos` (retained; its env is borrowed for the frame's
 * lifetime). NULL `clos` = a nullary CAF frame with no captured record. The
 * caller owns the single reference. */
static Frame *
frame_new(
  size_t nlocals, Value *clos)
{
  Frame *f = (Frame *)THxMEM_alloc(sizeof(Frame));
  f->rc    = 1;
  f->clos  = clos;
  if (clos)
  {
    THxMEM_retain(clos);
    f->env  = clos->u.clos.env;
    f->nenv = clos->u.clos.nenv;
  }
  else
  {
    f->env  = NULL;
    f->nenv = 0;
  }
  f->locals
    = nlocals ? (Value **)THxMEM_alloc(nlocals * sizeof(Value *)) : NULL;
  f->nlocals = nlocals;
  return f;
}

static void
frame_retain(
  Frame *f)
{
  if (f) ++f->rc;
}

static void
frame_release(
  Frame *f)
{
  if (f == NULL) return;
  THxCHECK_ASSERT(f->rc > 0, "THxK frame_release: releasing a freed frame");
  if (--f->rc > 0) return;
  for (size_t i = 0; i < f->nlocals; ++i) THxMEM_release(f->locals[i]);
  THxMEM_free(f->locals);
  THxMEM_release(f->clos);
  THxMEM_free(f);
}

/*------------------------------------------------------------------------------
 *\KONT-FRAME + SEGMENT LIFETIME
 *-----------------------------------------------------------------------------*/

/* Release everything a popped (or discarded) kont frame owns. */
static void
kframe_release(
  KFrame *kf)
{
  switch (kf->tag)
  {
  case K_RET: frame_release(kf->u.ret.frame); return;
  case K_PROMPT:
    for (size_t j = 0; j < kf->u.prompt.n; ++j)
      THxMEM_release(kf->u.prompt.clauses[j]);
    THxMEM_free((void *)kf->u.prompt.ops);
    THxMEM_free(kf->u.prompt.clauses);
    THxMEM_release(kf->u.prompt.els);
    return;
  case K_DEFER      : THxMEM_release(kf->u.defer.cleanup); return;
  case K_THUNKRET   : THxMEM_release(kf->u.thunkret.saved); return;
  case K_AFTERCLAUSE: THxMEM_release(kf->u.afterclause.kval); return;
  }
  THxCHECK_FAIL("THxK kframe_release: unhandled continuation frame");
}

/* Segment lifetime, driven by the owning T_RESUMP values (see THxVALUE.c). An
 * alias (a resumption content-copied into a let box) also counts as STORED --
 * the same judgement the interpreter reads off its shared_ptr use-count. */
void
THxK_resump_addref(
  THxK_Resump *seg)
{
  if (seg == NULL) return;
  ++seg->rc;
  seg->escaped = 1;
}

void
THxK_resump_release(
  THxK_Resump *seg)
{
  if (seg == NULL) return;
  THxCHECK_ASSERT(seg->rc > 0, "THxK_resump_release: releasing a freed slice");
  if (--seg->rc > 0) return;
  if (!seg->used)
    for (size_t i = 0; i < seg->n; ++i) kframe_release(&seg->seg[i]);
  THxMEM_free(seg->seg); /* NULL after a resume: a no-op */
  THxMEM_free(seg);
}

/*------------------------------------------------------------------------------
 *\NEXT-ACTION REGISTERS (set by a block's terminator, read by the driver)
 *-----------------------------------------------------------------------------*/

typedef enum
{
  ACT_NONE,
  ACT_RET,
  ACT_APPLY,
  ACT_JUMP
} Act;

static Act     g_act;
static Value  *g_v;   /* ACT_RET value */
static Value  *g_fn;  /* ACT_APPLY callee */
static Value  *g_arg; /* ACT_APPLY argument */
static BlockFn g_blk; /* ACT_JUMP target */

static Value
  *g_result; /* set when a run completes (kont drained to its base) */

/*------------------------------------------------------------------------------
 *\BLOCK TERMINATORS
 *-----------------------------------------------------------------------------*/

void
THxK_ret(
  Value *v)
{
  g_act = ACT_RET;
  g_v   = v;
}

void
THxK_tailcall(
  Value *fn, Value *arg)
{
  g_act = ACT_APPLY;
  g_fn  = fn;
  g_arg = arg;
}

void
THxK_apply(
  Frame *fr, Value *fn, Value *arg, BlockFn cont, size_t slot)
{
  KFrame kf;
  kf.tag         = K_RET;
  kf.u.ret.frame = fr;
  kf.u.ret.cont  = cont;
  kf.u.ret.slot  = slot;
  frame_retain(fr); /* the kont keeps the suspended activation alive */
  kont_push(kf);
  g_act = ACT_APPLY;
  g_fn  = fn;
  g_arg = arg;
}

void
THxK_jump(
  BlockFn cont)
{
  g_act = ACT_JUMP;
  g_blk = cont;
}

void
THxK_handle(
  Frame       *fr,
  BlockFn      cont,
  size_t       slot,
  const char **ops,
  Value      **clauses,
  size_t       nclauses,
  Value       *els,
  BlockFn      body)
{
  /* KRet for the handled expression's eventual value (sits below the prompt, so
   * the value clause's result flows to it). When `cont` is NULL the handler is
   * in tail position: the value flows straight to the enclosing continuation,
   * so no KRet is added. */
  if (cont)
  {
    KFrame kr;
    kr.tag         = K_RET;
    kr.u.ret.frame = fr;
    kr.u.ret.cont  = cont;
    kr.u.ret.slot  = slot;
    frame_retain(fr);
    kont_push(kr);
  }

  /* Copy the clause tables into runtime-lifetime memory (the generated arrays
   * are C compound literals with block lifetime). The prompt owns them. */
  const char **ops2   = (const char **)THxMEM_alloc(nclauses * sizeof(char *));
  Value      **claus2 = (Value **)THxMEM_alloc(nclauses * sizeof(Value *));
  for (size_t i = 0; i < nclauses; ++i)
  {
    ops2[i]   = ops[i];
    claus2[i] = clauses[i];
    THxMEM_retain(claus2[i]);
  }
  KFrame kp;
  kp.tag              = K_PROMPT;
  kp.u.prompt.ops     = ops2;
  kp.u.prompt.clauses = claus2;
  kp.u.prompt.n       = nclauses;
  kp.u.prompt.els     = els;
  THxMEM_retain(els);
  kont_push(kp);

  g_act = ACT_JUMP;
  g_blk = body;
}

void
THxK_setlocal(
  Frame *fr, size_t slot, Value *v)
{
  THxCHECK_ASSERT(slot < fr->nlocals, "THxK_setlocal: slot out of range");
  THxMEM_retain(v); /* retain-before-release: v may be the slot's old value */
  THxMEM_release(fr->locals[slot]);
  fr->locals[slot] = v;
}

void
THxK_setbox(
  Frame *fr, size_t slot)
{
  THxK_setlocal(fr, slot, THxRT_unk());
}

void
THxK_backpatch(
  Frame *fr, size_t slot, Value *v)
{
  THxCHECK_ASSERT(slot < fr->nlocals, "THxK_backpatch: slot out of range");
  THxCHECK_ASSERT(fr->locals[slot] != NULL, "THxK_backpatch: no box in slot");
  THxVALUE_patch_box(fr->locals[slot], v);
}

/*------------------------------------------------------------------------------
 *\EFFECT VALUE CONSTRUCTORS
 *-----------------------------------------------------------------------------*/

Value *
THxK_op(
  const char *name)
{
  Value *x     = THxMEM_alloc_value();
  x->tag       = T_OP;
  x->u.op.name = name;
  return x;
}

static Value *
mk_resump(
  THxK_Resump *seg)
{
  Value *x        = THxMEM_alloc_value();
  x->tag          = T_RESUMP;
  x->u.resump.seg = seg; /* the value owns the segment's single reference */
  return x;
}

Value *
THxK_defer(
  void)
{
  Value *x         = THxMEM_alloc_value();
  x->tag           = T_DEFER;
  x->u.defer.nargs = 0;
  x->u.defer.args  = NULL;
  return x;
}

/* A fresh defer value with `x` appended to `d`'s accumulated thunks. */
static Value *
defer_push(
  Value *d, Value *x)
{
  size_t n          = d->u.defer.nargs;
  Value *nd         = THxMEM_alloc_value();
  nd->tag           = T_DEFER;
  nd->u.defer.nargs = n + 1;
  Value **args      = (Value **)THxMEM_alloc((n + 1) * sizeof(Value *));
  for (size_t i = 0; i < n; ++i) args[i] = d->u.defer.args[i];
  args[n] = x;
  for (size_t i = 0; i <= n; ++i)
    THxMEM_retain(args[i]); /* the new value owns its thunk row */
  nd->u.defer.args = args;
  return nd;
}

/* Mark a resumption reachable from a stored heap structure, so its `defer`
 * cleanups are not finalized at the clause boundary (they run on later resume).
 * Called by the value constructors in THxRT.c on each stored capture/field. */
void
THxK_mark_escape(
  Value *v)
{
  if (v && v->tag == T_RESUMP) v->u.resump.seg->escaped = 1;
}

/*------------------------------------------------------------------------------
 *\THE MACHINE
 *-----------------------------------------------------------------------------*/

static Value *
unit(
  void)
{
  return THxRT_unk();
}

/* Jump into a 1-argument closure `clo` with `arg` in slot 0 (value clause / a
 * defer cleanup). Sets the driver's control to the closure's entry block. The
 * new frame retains `clo` (so it survives the caller releasing its kont edge)
 * and its slot retains `arg`; the outgoing activation is released. */
static int
jump1(
  BlockFn *cur, Frame **fr, Value **in, Value *clo, Value *arg)
{
  THxCHECK_ASSERT(clo != NULL && clo->tag == T_CLOS,
                  "THxK jump1: not a closure");
  size_t code = (size_t)clo->u.clos.code;
  THxCHECK_ASSERT(code < THxRT_code_count, "THxK jump1: code out of range");
  Frame *nf = frame_new(THxRT_code_nlocals[code], clo);
  if (nf->nlocals > 0) THxK_setlocal(nf, 0, arg);
  frame_release(*fr);
  *cur = THxRT_code_table[code];
  *fr  = nf;
  *in  = arg;
  return 1;
}

/* Hand a finished value to the continuation stack (the interpreter's `ret`).
 * Returns 1 to continue the driver loop (control updated), 0 when the run's
 * base is reached (g_result set, holding a reference the caller owns).
 * Maintains exactly one owned in-flight reference on the value being delivered
 * until a store (box, slot, kont) has taken its own. */
static int
do_ret(
  BlockFn *cur, Frame **fr, Value **in, Value *v, size_t base)
{
  THxMEM_retain(v); /* the in-flight reference */
  for (;;)
  {
    if (g_kn == base)
    {
      frame_release(*fr);
      *fr      = NULL;
      g_result = v; /* transfer the in-flight reference to the caller */
      return 0;
    }
    KFrame kf = g_kont[--g_kn];
    switch (kf.tag)
    {
    case K_RET:
    {
      Value *box = kf.u.ret.frame->locals[kf.u.ret.slot];
      THxCHECK_ASSERT(box != NULL, "THxK ret: no box in resumed slot");
      THxVALUE_patch_box(box, v);
      *cur = kf.u.ret.cont;
      frame_release(*fr);
      *fr = kf.u.ret.frame; /* adopt the KRet's frame reference */
      *in = v;
      THxMEM_release(v); /* delivered: the box owns copies of v's children */
      return 1;
    }
    case K_PROMPT:
    {
      /* body finished normally -> run the value clause on the result */
      int r = jump1(cur, fr, in, kf.u.prompt.els, v);
      kframe_release(&kf); /* els survives via the new frame's clos ref */
      THxMEM_release(v);   /* delivered: retained in the clause's slot 0 */
      return r;
    }
    case K_DEFER:
    {
      /* normal completion through a defer: run cleanup, then deliver v */
      KFrame tr;
      tr.tag              = K_THUNKRET;
      tr.u.thunkret.saved = v; /* park the in-flight reference in the kont */
      kont_push(tr);
      int r = jump1(cur, fr, in, kf.u.defer.cleanup, unit());
      kframe_release(&kf); /* cleanup survives via the new frame's clos ref */
      return r;
    }
    case K_THUNKRET:
      THxMEM_release(v);
      v = kf.u.thunkret.saved; /* adopt the kont's reference as in-flight */
      continue;
    case K_AFTERCLAUSE:
    {
      THxK_Resump *res     = kf.u.afterclause.kval->u.resump.seg;
      int          resumed = res->used;
      int          stored  = res->escaped;
      if (resumed || stored)
      {
        kframe_release(&kf); /* drop the kval edge; deliver v unchanged */
        continue;
      }
      /* discarded (exception/abort): run the captured defer cleanups now, on
       * the live stack with enclosing handlers installed, then re-deliver v. */
      KFrame tr;
      tr.tag              = K_THUNKRET;
      tr.u.thunkret.saved = v; /* park the in-flight reference in the kont */
      kont_push(tr);
      for (size_t i = 0; i < res->n; ++i)
        if (res->seg[i].tag == K_DEFER)
        {
          KFrame kd;
          kd.tag             = K_DEFER;
          kd.u.defer.cleanup = res->seg[i].u.defer.cleanup;
          THxMEM_retain(kd.u.defer.cleanup); /* the kont stores it */
          kont_push(kd);
        }
      kframe_release(&kf); /* drop kval: may free the discarded segment */
      v = unit();          /* start unwinding the freshly pushed KDefers */
      THxMEM_retain(v);    /* the new in-flight reference */
      continue;
    }
    }
    THxCHECK_FAIL("THxK do_ret: unhandled continuation frame");
  }
}

/* Apply `fn` to `arg` (the interpreter's App dispatch). Returns 1 to continue
 * (control updated), 0 when the run completed. Holds owned in-flight references
 * on `fn` and `arg` across the frame switch (either may live in the dying
 * activation). */
static int
do_apply(
  BlockFn *cur, Frame **fr, Value **in, Value *fn, Value *arg, size_t base)
{
  THxCHECK_ASSERT(fn != NULL, "THxK apply: null callee");
  THxMEM_retain(fn);
  THxMEM_retain(arg);
  int r = 0;
  switch (fn->tag)
  {
  case T_CLOS:
  {
    size_t code = (size_t)fn->u.clos.code;
    THxCHECK_ASSERT(code < THxRT_code_count, "THxK apply: code out of range");
    Frame *nf = frame_new(THxRT_code_nlocals[code], fn);
    if (nf->nlocals > 0) THxK_setlocal(nf, 0, arg);
    frame_release(*fr);
    *cur = THxRT_code_table[code];
    *fr  = nf;
    *in  = arg;
    r    = 1;
    break;
  }
  case T_BUILTIN:
    r = do_ret(cur, fr, in, THxRT_apply_builtin(fn, arg), base);
    break;
  case T_EXTERN:
    r = do_ret(cur, fr, in, THxRT_apply_extern(fn, arg), base);
    break;
  case T_OP:
  {
    /* perform: find the nearest prompt (above `base`) with a clause for this
     * op, capture the slice [prompt, here] (INCLUDING the prompt -> deep
     * handler), truncate to below the prompt, and run the clause there. */
    const char *op     = fn->u.op.name;
    size_t      p      = g_kn;
    Value      *clause = NULL;
    for (size_t i = g_kn; i-- > base;)
    {
      if (g_kont[i].tag != K_PROMPT) continue;
      for (size_t j = 0; j < g_kont[i].u.prompt.n; ++j)
        if (strcmp(g_kont[i].u.prompt.ops[j], op) == 0)
        {
          clause = g_kont[i].u.prompt.clauses[j];
          p      = i;
          break;
        }
      if (clause) break;
    }
    if (!clause) THxCHECK_FAILF("unhandled effect operation '%s'", op);

    /* The slice's kont frames MOVE into the segment (their owned references
     * move with them; no counts change). */
    size_t       n   = g_kn - p;
    THxK_Resump *seg = (THxK_Resump *)THxMEM_alloc(sizeof(THxK_Resump));
    seg->rc          = 1; /* owned by the kval below */
    seg->seg         = (KFrame *)THxMEM_alloc(n * sizeof(KFrame));
    memcpy(seg->seg, &g_kont[p], n * sizeof(KFrame));
    seg->n       = n;
    seg->used    = 0;
    seg->escaped = 0;
    g_kn         = p; /* the clause runs below the prompt (outside it) */

    Value *kval = mk_resump(seg);
    KFrame ac;
    ac.tag                = K_AFTERCLAUSE;
    ac.u.afterclause.kval = kval;
    THxMEM_retain(kval); /* the kont stores it */
    kont_push(ac);

    /* enter the 2-arg clause: op argument in slot 0, resumption k in slot 1.
     * The clause closure is owned by the CAPTURED prompt (inside seg); the new
     * frame's clos reference keeps it alive however the segment ends. */
    THxCHECK_ASSERT(clause->tag == T_CLOS,
                    "THxK perform: clause not a closure");
    size_t code = (size_t)clause->u.clos.code;
    THxCHECK_ASSERT(code < THxRT_code_count, "THxK perform: clause code range");
    Frame *nf = frame_new(THxRT_code_nlocals[code], clause);
    THxCHECK_ASSERT(nf->nlocals >= 2, "THxK perform: clause needs 2 slots");
    THxK_setlocal(nf, 0, arg);
    THxK_setlocal(nf, 1, kval);
    frame_release(*fr);
    *cur = THxRT_code_table[code];
    *fr  = nf;
    *in  = arg;
    r    = 1;
    break;
  }
  case T_RESUMP:
  {
    /* resume: splice the captured slice back on, deliver arg to the suspended
     * point. Affine -- at most once. The kont frames MOVE back; the emptied
     * segment stays behind (owned by its values) as a used husk. */
    THxK_Resump *seg = fn->u.resump.seg;
    if (seg->used) THxCHECK_FAIL("resumption used more than once (affine)");
    seg->used = 1;
    for (size_t i = 0; i < seg->n; ++i) kont_push(seg->seg[i]);
    THxMEM_free(seg->seg);
    seg->seg = NULL;
    seg->n   = 0;
    r        = do_ret(cur, fr, in, arg, base);
    break;
  }
  case T_DEFER:
  {
    Value *nd = defer_push(fn, arg);
    if (nd->u.defer.nargs < 2)
    {
      r = do_ret(cur, fr, in, nd, base);
      break;
    }
    /* both thunks in hand: install cleanup, run action {}. */
    KFrame kd;
    kd.tag             = K_DEFER;
    kd.u.defer.cleanup = nd->u.defer.args[1];
    THxMEM_retain(kd.u.defer.cleanup); /* the kont stores it */
    kont_push(kd);
    r = jump1(cur, fr, in, nd->u.defer.args[0], unit());
    break;
  }
  case T_INT:
  case T_REAL:
  case T_STR:
  case T_STRUCT:
  case T_VARIANT:
  case T_UNK:
    THxCHECK_FAILF("THxK apply: callee is not a function (%s)",
                   THxVALUE_tag_name(fn->tag));
  }
  THxMEM_release(fn);
  THxMEM_release(arg);
  return r;
}

/*------------------------------------------------------------------------------
 *\THE DRIVER LOOP
 *-----------------------------------------------------------------------------*/

static Value *
run_loop(
  BlockFn cur, Frame *fr, Value *in, size_t base)
{
  for (;;)
  {
    size_t pm = THxMEM_pool_mark(); /* this bounce's temporaries */
    g_act     = ACT_NONE;
    cur(fr, in);
    int cont = 1;
    switch (g_act)
    {
    case ACT_JUMP : cur = g_blk; break; /* same frame; in is unused by blocks */
    case ACT_RET  : cont = do_ret(&cur, &fr, &in, g_v, base); break;
    case ACT_APPLY: cont = do_apply(&cur, &fr, &in, g_fn, g_arg, base); break;
    case ACT_NONE : THxCHECK_FAIL("THxK: block set no terminator");
    }
    /* Everything this bounce constructed and did not store dies here; what was
     * stored (slot, kont, box children, g_result) took its own reference. */
    THxMEM_pool_drain(pm);
    if (!cont) return g_result;
  }
}

/*------------------------------------------------------------------------------
 *\DRIVER ENTRY POINTS
 *-----------------------------------------------------------------------------*/

Value *
THxK_run_code(
  size_t code)
{
  THxCHECK_ASSERT(code < THxRT_code_count, "THxK_run_code: code out of range");
  size_t base = g_kn;
  Frame *fr   = frame_new(THxRT_code_nlocals[code], NULL);
  return run_loop(THxRT_code_table[code], fr, NULL, base);
}

Value *
THxK_call(
  Value *f, Value *arg)
{
  size_t  base = g_kn;
  BlockFn cur  = NULL;
  Frame  *fr   = NULL;
  Value  *in   = NULL;
  if (!do_apply(&cur, &fr, &in, f, arg, base)) return g_result;
  return run_loop(cur, fr, in, base);
}
