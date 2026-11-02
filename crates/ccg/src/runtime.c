#include <math.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* THRAX_THREADS is defined by the emitter for a program that spawns parallel
 * tasks (`@par`) on a target with threads. Without it the machine state is plain
 * globals and every count is non-atomic, so a sequential program pays nothing.
 * The thread headers and `THX_TLS` come from portable.h, emitted ahead of this. */

typedef struct Value Value;
typedef struct Frame Frame;
typedef struct Resump Resump;
typedef struct Task Task;
typedef struct Chan Chan;
typedef struct TVar TVar;

/* A compiled basic block. `in` is the value delivered to a resumed block; blocks
 * re-read their live variables from `fr`, so `in` is usually unused. */
typedef void (*BlockFn)(Frame *fr, Value *in);

/* Provided by the generated program. */
extern BlockFn THxRT_code_table[];
extern const size_t THxRT_code_nlocals[];
/* Per code, a bit per parameter the body only borrows (frontend
 * `Code::borrowed`): it never moves or drops one. */
extern const unsigned char THxRT_code_borrowed[];
extern const size_t THxRT_code_count;

/* One foreign-call wrapper per distinct `@extern`: it marshals the collected
 * arguments across the C ABI and calls the symbol. Provided by the generated
 * program (see the ccg `emit_extern_table`). */
typedef Value *(*ExternFn)(Value **args);
extern ExternFn THxRT_extern_table[];
extern const size_t THxRT_extern_count;

typedef enum {
  T_INT,
  T_REAL,
  T_REAL32, /* single-precision real (@Float32); stored in u.r as a double */
  T_STR, /* byte vector: both Str and Array */
  T_BOOL,
  T_UNIT,
  T_TUPLE,
  T_STRUCT,
  T_VARIANT,
  T_VEC,       /* Vec `T (boxed elements) */
  T_CLOS,      /* closure: an IR code index + captured environment */
  T_BUILTIN,   /* a (possibly partially applied) built-in operator */
  T_EXTERN,    /* a (possibly partially applied) foreign C function (@extern) */
  T_OP,        /* an effect operation, first-class; performs when applied */
  T_RESUMP,    /* a captured continuation (a delimited stack slice) */
  T_THUNK,     /* an unforced lazy slot: a nullary closure, forced in place */
  T_FUTURE,    /* a parallel task's handle (`@par`); only with THRAX_THREADS */
  T_BLACKHOLE, /* a shared thunk another thread is forcing right now */
  T_CHAN,      /* a channel's handle (`@Chan`) */
  T_TVAR       /* a transactional variable's handle (`@TVar`) */
} Tag;

typedef struct {
  const char *name;
  Value *val;
} Field;

/* `shared` says who can reach a value. SH_LOCAL: one thread, plain counts.
 * SH_SHARED: several threads, atomic counts. SH_IMMORTAL: reachable from a
 * global, so alive until exit: no counting at all (a hot global closure would
 * otherwise bounce its count between cores), and freed by one sweep at exit. A
 * value starts local and is marked, with everything it reaches, when it crosses
 * to another thread; marking only ever raises the level.
 *
 * With THRAX_THREADS the level is also encoded in the sign of `rc` (Lean 4's
 * scheme), so the counting fast path is one load and one compare: `rc > 0` is a
 * local count, `rc < 0` a shared one (`-rc` owners, atomic), and `rc == 0` is
 * immortal, or freed. A shared value made immortal keeps its negative count.
 * `shared` stays the authority on the slow paths. Without THRAX_THREADS `rc` is
 * only ever positive. */
enum { SH_LOCAL = 0, SH_SHARED = 1, SH_IMMORTAL = 2 };
struct Value {
  uint8_t tag;
  uint8_t shared;
  int rc;
  union {
    int64_t i;
    double r;
    bool b;
    struct {
      uint8_t *data;
      size_t len;
      size_t cap; /* bytes `data` holds before its trailing NUL; cap >= len */
    } str;
    struct {
      Value **items;
      size_t len;
      size_t cap; /* slots allocated in `items`, at least `len` */
    } seq; /* tuple and vec */
    struct {
      const char *name;
      Field *fields;
      size_t len;
    } strct;
    struct {
      const char *ty;
      const char *tag;
      Value **fields;
      size_t len;
    } variant;
    struct {
      int code;
      Value **env;
      size_t nenv;
    } clos;
    struct {
      int id; /* a THxB_* constant */
      size_t arity;
      Value **args;
      size_t nargs;
    } builtin;
    struct {
      int idx; /* index into the generated THxRT_extern_table */
      size_t arity;
      Value **args;
      size_t nargs;
    } ext;
    struct {
      const char *effect; /* NULL for an ambient (unqualified) operation */
      const char *op;
    } op;
    Resump *resump;
    Value *thunk; /* the nullary closure an unforced lazy slot holds */
    Task *future;
    Chan *chan;
    TVar *tvar;
  } u;
};

/* One activation: the local slot array (params + let/case binders) and the
 * current closure's captured record. Refcounted and heap-allocated so it
 * survives a suspension captured in the continuation stack. */
struct Frame {
  unsigned rc;
  /* A bit per local slot holding a lent value: the slot owns no reference, the
   * caller that lent it keeps it alive. Only parameters are ever lent. */
  unsigned lent;
  Value *clos; /* retained; NULL for a nullary CAF */
  Value **locals;
  size_t nlocals;
  Value **env; /* borrowed from `clos` */
  size_t nenv;
};

static inline int frame_lent(Frame *f, size_t i) { return i < 32 && (f->lent >> i & 1u); }

/* Continuation frames (the reified-K stack) and captured resumptions. */
typedef enum {
  K_RET,        /* resume a suspended activation with the returned value */
  K_PROMPT,     /* a handler delimiter */
  K_DEFER,      /* a deferred cleanup marker */
  K_THUNKRET,   /* deliver `saved`, discarding the incoming value */
  K_AFTERCLAUSE, /* clause boundary: decides a captured k's defer fate */
  K_CLEANUPS    /* cleanups of a slice abandoned without being captured */
} KTag;

/* Whether a handler clause needs its continuation (the frontend's ResumeUse): a
 * clause that cannot resume needs no capture at all. */
typedef enum { RESUME_NEVER = 0, RESUME_CAPTURED = 1 } ResumeUse;

typedef struct {
  KTag tag;
  union {
    struct {
      Frame *frame;
      BlockFn cont;
      size_t slot;
    } ret;
    struct {
      const char **effs;
      const char **ops;
      const int *uses; /* ResumeUse per clause */
      Value **clauses;
      size_t n;
      Value *els;
    } prompt;
    struct {
      Value *cleanup;
    } defer;
    struct {
      Value *saved;
    } thunkret;
    struct {
      Resump *res;
    } afterclause;
    struct {
      Value **cleanups;
      size_t n;
    } cleanups;
  } u;
} KFrame;

/* A captured continuation slice. `rc` counts its owners: the T_RESUMP values
 * holding it (a resumption content-copied into a let box aliases it) plus the
 * K_AFTERCLAUSE frames of its clause boundary, of which there are `nbound`. A
 * boundary never resumes, so the slice is uniquely owned when exactly one value
 * holds it (`rc == nbound + 1`); a resume through that value then moves the
 * frames out and leaves `seg` NULL behind, and any other resume splices a COPY
 * (`segment_copy`). `used` means "resumed at least once", which the boundary
 * consults to decide whether an abandoned slice's `defer` cleanups still have
 * to run. `shared` marks a slice other threads can reach (see `mark_level`):
 * its frames are then never moved, only copied, so it can be resumed on any
 * thread, and with threads its counts are atomic. */
struct Resump {
  unsigned rc;
  unsigned nbound;
  KFrame *seg;
  size_t n;
  int used;
  int shared;
};

#ifdef THRAX_THREADS
#define RES_INC(p) __atomic_add_fetch((p), 1, __ATOMIC_RELAXED)
#define RES_DEC(p) __atomic_sub_fetch((p), 1, __ATOMIC_ACQ_REL)
#define RES_GET(p) __atomic_load_n((p), __ATOMIC_ACQUIRE)
#define RES_SET(p, v) __atomic_store_n((p), (v), __ATOMIC_RELEASE)
#else
#define RES_INC(p) (++*(p))
#define RES_DEC(p) (--*(p))
#define RES_GET(p) (*(p))
#define RES_SET(p, v) (*(p) = (v))
#endif

enum { TASK_PENDING, TASK_RUNNING, TASK_DONE };

/* A parallel task, owned by the T_FUTURE cells naming it (`rc`, atomic). `fn`
 * is the nullary closure until the task has run; `result` is written once,
 * before `state` turns TASK_DONE with release order. `io` marks a `@spawn`
 * task, which the program waits for before it exits. */
struct Task {
  unsigned rc;
  int state;
  int io;
  Value *fn;
  Value *result;
};

/* A channel (`@Chan`), owned by the T_CHAN cells naming it (`rc`). A sent
 * value waits in the ring `buf` (`len` of `cap` slots from `head`), holding a
 * reference. With threads, the ring changes only under `mu`. */
struct Chan {
  unsigned rc;
  Value **buf;
  size_t head, len, cap;
#ifdef THRAX_THREADS
  pthread_mutex_t mu;
#endif
};

/* A transactional variable (`@TVar`), owned by the T_TVAR cells naming it and
 * by the transaction logs holding it (`rc`). `version` counts the commits
 * that wrote it. With threads, `val` and `version` change only under
 * `g_stm_mu`. */
struct TVar {
  unsigned rc;
  Value *val;
  uint64_t version;
};

/* The count of a channel or a transactional variable, which any thread may
 * hold. */
static void obj_retain(unsigned *rc) {
#ifdef THRAX_THREADS
  __atomic_fetch_add(rc, 1, __ATOMIC_RELAXED);
#else
  ++*rc;
#endif
}

/* Whether this release dropped the last reference. */
static int obj_release(unsigned *rc) {
#ifdef THRAX_THREADS
  return __atomic_sub_fetch(rc, 1, __ATOMIC_ACQ_REL) == 0;
#else
  return --*rc == 0;
#endif
}

/* Uncounted scratch allocation (transient buffers, FFI C-strings): not part of
 * a value's lifetime, so not tracked by the leak check. */
static void *xmalloc(size_t n) {
  void *p = malloc(n ? n : 1);
  if (!p) {
    fprintf(stderr, "thrax: out of memory\n");
    exit(70);
  }
  return p;
}

_Noreturn static void thrax_fault(const char *msg) {
  fprintf(stderr, "thrax: runtime fault: %s\n", msg);
  exit(1);
}

/*------------------------------------------------------------------------------
 *\THE MEMORY ENGINE (precise reference counting; port of THxMEMRC.c)
 *-----------------------------------------------------------------------------*/

/* Releases v's children and frees its owned raw payload blocks; called once,
 * when v's count reaches zero. */
static void THxVALUE_destroy(Value *v);

/* Live allocations (values + raw blocks) made minus freed by this thread. A
 * block may be freed by a thread other than the one that allocated it, so one
 * thread's count can go negative; a worker folds its count into
 * `g_live_folded` when it exits, and the sum stays exact. */
static THX_TLS long long g_live = 0;
#ifdef THRAX_THREADS
static long long g_live_folded = 0;
#endif

/* Built with -DTHRAX_ALLOC_STATS, a program reports on stderr at exit how many
 * counted blocks it allocated in all, for measuring allocation behaviour. */
#ifdef THRAX_ALLOC_STATS
static long long g_allocs_total = 0, g_reuses_total = 0;
static void alloc_stats_report(void) {
  fprintf(stderr, "thrax: %lld allocations, %lld cells reused\n",
          __atomic_load_n(&g_allocs_total, __ATOMIC_RELAXED),
          __atomic_load_n(&g_reuses_total, __ATOMIC_RELAXED));
}
#endif

#ifdef THRAX_RC_STATS
/* Every count change on a value, reported at exit: build the emitted C with
 * -DTHRAX_RC_STATS to measure reference-count traffic. Thread-local, so a
 * threaded program reports the main thread's share. */
static THX_TLS unsigned long long g_rc_retains = 0, g_rc_releases = 0;
static void rc_stats_report(void) {
  fprintf(stderr, "thrax: %llu retains, %llu releases\n", g_rc_retains, g_rc_releases);
}
__attribute__((constructor)) static void rc_stats_init(void) { atexit(rc_stats_report); }
#define THX_RC_COUNT(n) (++(n))
#else
#define THX_RC_COUNT(n) ((void)0)
#endif

/* Allocate `size` zeroed bytes with value lifetime (counted). */
static void *THxMEM_alloc(size_t size) {
  void *p = calloc(1, size ? size : 1);
  if (!p) thrax_fault("out of memory");
  ++g_live;
#ifdef THRAX_ALLOC_STATS
  if (__atomic_fetch_add(&g_allocs_total, 1, __ATOMIC_RELAXED) == 0) atexit(alloc_stats_report);
#endif
  return p;
}

/* Resize a THxMEM_alloc'd block (or allocate one for NULL). Bytes past the
 * old size are not zeroed. */
static void *THxMEM_realloc(void *p, size_t size) {
  void *q = realloc(p, size ? size : 1);
  if (!q) thrax_fault("out of memory");
  if (!p) ++g_live;
  return q;
}

static void THxMEM_free(void *p) {
  if (!p) return;
#ifndef THRAX_THREADS
  if (g_live == 0) thrax_fault("THxMEM_free: more frees than allocations");
#endif
  --g_live;
  free(p);
}

/* Recycled small blocks: value cells, frames and the child arrays of tuples,
 * records, variants, closures and partial applications. A freed block goes on
 * its size class's list for this thread instead of back to malloc, and the
 * next allocation of that size takes it. Every size is a multiple of 8 and a
 * class holds blocks of exactly its size, so a block from THxMEM_alloc may be
 * freed here and a block from here by THxMEM_free: both are plain malloc
 * blocks of at least the size asked for. A block on a list is not live, so the
 * leak check stays exact. A block may be freed, and so recycled, by a thread
 * other than the one that allocated it; each list is touched only by its
 * owning thread. */
#define THX_CELL_CLASSES 33 /* sizes 8 .. 256 */
#define THX_CELL_KEEP 8192  /* blocks a class keeps before handing back to malloc */
typedef struct FreeCell {
  struct FreeCell *next;
} FreeCell;
static THX_TLS FreeCell *g_cells[THX_CELL_CLASSES];
static THX_TLS unsigned g_ncells[THX_CELL_CLASSES];

/* `size` (a multiple of 8) zeroed bytes with value lifetime (counted). */
static inline void *THxMEM_cell(size_t size) {
  size_t k = size >> 3;
  if (k < THX_CELL_CLASSES && g_cells[k]) {
    FreeCell *c = g_cells[k];
    g_cells[k] = c->next;
    --g_ncells[k];
    ++g_live;
    memset(c, 0, size);
    return c;
  }
  return THxMEM_alloc(size);
}

static inline void THxMEM_cell_free(void *p, size_t size) {
  size_t k = size >> 3;
  if (!p) return;
  if (k == 0 || k >= THX_CELL_CLASSES || g_ncells[k] >= THX_CELL_KEEP) {
    THxMEM_free(p);
    return;
  }
#ifndef THRAX_THREADS
  if (g_live == 0) thrax_fault("THxMEM_free: more frees than allocations");
#endif
  --g_live;
  FreeCell *c = p;
  c->next = g_cells[k];
  g_cells[k] = c;
  ++g_ncells[k];
}

/* Hand this thread's recycled blocks back to malloc (they are not live). */
static void THxMEM_cells_release(void) {
  for (size_t k = 0; k < THX_CELL_CLASSES; k++) {
    while (g_cells[k]) {
      FreeCell *c = g_cells[k];
      g_cells[k] = c->next;
      free(c);
    }
    g_ncells[k] = 0;
  }
}

/* The temp pool: every fresh value is registered here holding the pool's
 * initial reference; the driver drains to a mark after each block bounce. */
static THX_TLS Value **g_pool = NULL;
static THX_TLS size_t g_pn = 0, g_pcap = 0;

static void THxMEM_release(Value *v);

/* Hand one reference on `v` to the pool, which releases it at the next drain. */
static void pool_adopt(Value *v) {
  if (g_pn == g_pcap) {
    g_pcap = g_pcap ? g_pcap * 2 : 256;
    g_pool = realloc(g_pool, g_pcap * sizeof(Value *));
    if (!g_pool) thrax_fault("temp pool allocation failed");
  }
  g_pool[g_pn++] = v;
}

static Value *alloc_value(Tag t) {
  Value *v = THxMEM_cell(sizeof(Value));
  v->tag = t;
  v->rc = 1; /* the pool's reference */
  pool_adopt(v);
  return v;
}

static size_t THxMEM_pool_mark(void) { return g_pn; }

static void THxMEM_pool_drain(size_t mark) {
  if (mark > g_pn) thrax_fault("pool_drain: mark above pool top");
  while (g_pn > mark) THxMEM_release(g_pool[--g_pn]);
}

#ifdef THRAX_THREADS
/* A shared count changes under other threads, so even the sign test reads it
 * atomically; relaxed is a plain load. */
static inline int rc_load(Value *v) { return __atomic_load_n(&v->rc, __ATOMIC_RELAXED); }
/* Marking a shared value immortal raises its level while other threads count
 * it, so the level is read and written atomically too. */
static inline uint8_t sh_load(Value *v) { return __atomic_load_n(&v->shared, __ATOMIC_RELAXED); }

/* The counting of a value other threads can reach, kept out of line so the
 * thread-local fast path stays small enough to inline. */
__attribute__((noinline)) static void retain_slow(Value *v) {
  uint8_t sh = sh_load(v);
  if (sh == SH_IMMORTAL) return;
  if (sh == SH_LOCAL || __atomic_fetch_sub(&v->rc, 1, __ATOMIC_RELAXED) >= 0)
    thrax_fault("retain: retaining a freed value");
}

/* Whether this release dropped the last reference. */
__attribute__((noinline)) static int release_slow(Value *v) {
  uint8_t sh = sh_load(v);
  if (sh == SH_IMMORTAL) return 0;
  int old = sh == SH_LOCAL ? 0 : __atomic_fetch_add(&v->rc, 1, __ATOMIC_ACQ_REL);
  if (old >= 0) thrax_fault("release: releasing a freed value");
  return old == -1;
}
#endif

static inline void THxMEM_retain(Value *v) {
  if (!v) return;
  THX_RC_COUNT(g_rc_retains);
#ifdef THRAX_THREADS
  int rc = rc_load(v);
  if (__builtin_expect(rc > 0, 1))
    v->rc = rc + 1;
  else
    retain_slow(v);
#else
  if (v->rc == 0) thrax_fault("retain: retaining a freed value");
  ++v->rc;
#endif
}

/* The acquire load pairs with the releasing decrement of every other owner, so
 * an in-place update after this test sees their last writes. */
static int THxMEM_unique(Value *v) {
  if (!v) return 0;
#ifdef THRAX_THREADS
  int rc = rc_load(v);
  if (rc > 0) return rc == 1;
  return rc < 0 && sh_load(v) != SH_IMMORTAL && __atomic_load_n(&v->rc, __ATOMIC_ACQUIRE) == -1;
#else
  return v->rc == 1;
#endif
}

/* Whether `v` is held once, by this thread alone. A value other threads can
 * reach is never reused: its children would have to stay marked shared. */
static inline int unique_local(Value *v) {
#ifdef THRAX_THREADS
  return rc_load(v) == 1;
#else
  return v->rc == 1;
#endif
}

/* Dead-value worklist: destruction never recurses on the C stack. */
static THX_TLS Value **g_dead = NULL;
static THX_TLS size_t g_dn = 0, g_dcap = 0;
static THX_TLS int g_draining = 0;

static void dead_push(Value *v) {
  if (g_dn == g_dcap) {
    g_dcap = g_dcap ? g_dcap * 2 : 64;
    g_dead = realloc(g_dead, g_dcap * sizeof(Value *));
    if (!g_dead) thrax_fault("dead worklist allocation failed");
  }
  g_dead[g_dn++] = v;
}

static void THxMEM_release(Value *v) {
  if (!v) return;
  THX_RC_COUNT(g_rc_releases);
#ifdef THRAX_THREADS
  int rc = rc_load(v);
  if (__builtin_expect(rc > 1, 1)) {
    v->rc = rc - 1;
    return;
  }
  if (rc == 1)
    v->rc = 0;
  else if (!release_slow(v))
    return;
#else
  if (v->rc == 0) thrax_fault("release: releasing a freed value");
  if (--v->rc > 0) return;
#endif
  dead_push(v);
  if (g_draining) return; /* an outer drain loop will get to it */
  g_draining = 1;
  while (g_dn > 0) {
    Value *d = g_dead[--g_dn];
    THxVALUE_destroy(d); /* re-enters THxMEM_release: enqueues only */
    THxMEM_cell_free(d, sizeof(Value));
  }
  g_draining = 0;
}

static size_t THxMEM_live(void) {
#ifdef THRAX_THREADS
  return (size_t)(g_live + __atomic_load_n(&g_live_folded, __ATOMIC_ACQUIRE));
#else
  return (size_t)g_live;
#endif
}

/* -- constructors -------------------------------------------------------- */

/* Shared constants: unit, the booleans and the small integers are one static
 * cell each, never counted down to zero and never freed. With THRAX_THREADS
 * they are immortal (`rc == 0`, the counting slow path ignores them, and
 * `mark_level` stops at them); without, their count starts out of reach of
 * zero. A cell that is patched in place (a let box, a forced thunk) must never
 * be one of these, which is what `fresh_box` is for. */
#ifdef THRAX_THREADS
#define THX_CONST_RC 0
#else
#define THX_CONST_RC (1 << 30)
#endif
#define THX_CONST(t, init) {(uint8_t)(t), SH_IMMORTAL, THX_CONST_RC, {init}}
#define THX_SMALL_MIN (-128)
#define THX_SMALL_MAX 1023
#define THX_SI(n) THX_CONST(T_INT, .i = (n))
#define THX_SI4(n) THX_SI(n), THX_SI((n) + 1), THX_SI((n) + 2), THX_SI((n) + 3)
#define THX_SI16(n) THX_SI4(n), THX_SI4((n) + 4), THX_SI4((n) + 8), THX_SI4((n) + 12)
#define THX_SI64(n) THX_SI16(n), THX_SI16((n) + 16), THX_SI16((n) + 32), THX_SI16((n) + 48)
#define THX_SI256(n) THX_SI64(n), THX_SI64((n) + 64), THX_SI64((n) + 128), THX_SI64((n) + 192)
static Value g_small_ints[THX_SMALL_MAX - THX_SMALL_MIN + 1] = {
    THX_SI64(-128), THX_SI64(-64), THX_SI256(0), THX_SI256(256), THX_SI256(512), THX_SI256(768)};
static Value g_unit = THX_CONST(T_UNIT, .i = 0);
static Value g_false = THX_CONST(T_BOOL, .b = false);
static Value g_true = THX_CONST(T_BOOL, .b = true);
_Static_assert(sizeof(g_small_ints) / sizeof(Value) == THX_SMALL_MAX - THX_SMALL_MIN + 1,
               "small integer table size");
_Static_assert(sizeof(Value) % 8 == 0, "a value cell is a recyclable size");
#undef THX_SI256
#undef THX_SI64
#undef THX_SI16
#undef THX_SI4
#undef THX_SI

Value *THxRT_int(long long n) {
  if (n >= THX_SMALL_MIN && n <= THX_SMALL_MAX) return &g_small_ints[n - THX_SMALL_MIN];
  Value *v = alloc_value(T_INT);
  v->u.i = (int64_t)n;
  return v;
}
Value *THxRT_real(double r) {
  Value *v = alloc_value(T_REAL);
  v->u.r = r;
  return v;
}
/* An @Float32 value: the exact f32 is held in the double slot (every float is
 * representable as a double), so reads are lossless and display rounds to f32. */
Value *THxRT_real32(float r) {
  Value *v = alloc_value(T_REAL32);
  v->u.r = (double)r;
  return v;
}
Value *THxRT_bool(int b) { return b ? &g_true : &g_false; }
Value *THxRT_unit(void) { return &g_unit; }
/* A placeholder a let's value is later patched into. */
static Value *fresh_box(void) { return alloc_value(T_UNIT); }

/* Byte vectors carry a trailing NUL that is not counted in `len`, so
 * THxVALUE_str is a valid C string for the FFI seam (matching the C++
 * runtime). `len` is still the byte length; the NUL is slack. */
Value *THxRT_str(const char *data, size_t len) {
  Value *v = alloc_value(T_STR);
  v->u.str.data = THxMEM_alloc(len + 1);
  memcpy(v->u.str.data, data, len);
  v->u.str.data[len] = 0;
  v->u.str.len = len;
  v->u.str.cap = len;
  return v;
}
/* Take ownership of an already-THxMEM_alloc'd buffer. */
static Value *mk_str_owned(uint8_t *data, size_t len) {
  Value *v = alloc_value(T_STR);
  v->u.str.data = data;
  v->u.str.len = len;
  v->u.str.cap = len;
  return v;
}

Value *THxRT_tuple(Value **items, size_t len) {
  Value *v = alloc_value(T_TUPLE);
  v->u.seq.items = len ? THxMEM_cell(len * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.seq.items[i] = items[i];
    THxMEM_retain(items[i]);
  }
  v->u.seq.len = len;
  v->u.seq.cap = len;
  return v;
}
static Value *mk_vec(Value **items, size_t len) {
  Value *v = alloc_value(T_VEC);
  v->u.seq.items = len ? THxMEM_alloc(len * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.seq.items[i] = items[i];
    THxMEM_retain(items[i]);
  }
  v->u.seq.len = len;
  v->u.seq.cap = len;
  return v;
}
Value *THxRT_struct(const char *name, size_t len, const char **fnames,
                    Value **vals) {
  Value *v = alloc_value(T_STRUCT);
  v->u.strct.name = name;
  v->u.strct.fields = len ? THxMEM_cell(len * sizeof(Field)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.strct.fields[i].name = fnames[i];
    v->u.strct.fields[i].val = vals[i];
    THxMEM_retain(vals[i]);
  }
  v->u.strct.len = len;
  return v;
}
Value *THxRT_variant(const char *ty, const char *tag, size_t len,
                     Value **fields) {
  Value *v = alloc_value(T_VARIANT);
  v->u.variant.ty = ty;
  v->u.variant.tag = tag;
  v->u.variant.fields = len ? THxMEM_cell(len * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.variant.fields[i] = fields[i];
    THxMEM_retain(fields[i]);
  }
  v->u.variant.len = len;
  return v;
}
Value *THxRT_closure(int code, Value **captures, size_t n) {
  Value *v = alloc_value(T_CLOS);
  v->u.clos.code = code;
  v->u.clos.env = n ? THxMEM_cell(n * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < n; i++) {
    v->u.clos.env[i] = captures[i];
    THxMEM_retain(captures[i]);
  }
  v->u.clos.nenv = n;
  return v;
}
/* Suspend a lazy slot. `f` is the nullary closure lowering built around the
 * slot's expression; THxVALUE_force runs it. */
Value *THxRT_thunk(Value *f) {
  Value *v = alloc_value(T_THUNK);
  v->u.thunk = f;
  THxMEM_retain(f);
  return v;
}
/* Field names and constructor tags are mostly the generated program's string
 * literals, which the C compiler pools, so pointer equality usually decides a
 * match before strcmp has to. */
static inline int same_name(const char *a, const char *b) {
  return a == b || strcmp(a, b) == 0;
}
Value *THxRT_builtin(int id, size_t arity) {
  Value *v = alloc_value(T_BUILTIN);
  v->u.builtin.id = id;
  v->u.builtin.arity = arity;
  v->u.builtin.args = NULL;
  v->u.builtin.nargs = 0;
  return v;
}
Value *THxRT_extern(int idx, size_t arity) {
  Value *v = alloc_value(T_EXTERN);
  v->u.ext.idx = idx;
  v->u.ext.arity = arity;
  v->u.ext.args = NULL;
  v->u.ext.nargs = 0;
  return v;
}

Value *THxVALUE_force(Value *v);

/* A struct built from a base (record update): base's fields seeded, then the
 * listed overrides. An empty `name` keeps the base's type name. */
Value *THxRT_struct_update(Value *base, const char *name, size_t nextra,
                           const char **fnames, Value **vals) {
  base = THxVALUE_force(base); /* a base read from a lazy slot */
  if (base->tag != T_STRUCT) thrax_fault("record update of a non-struct value");
  size_t cap = base->u.strct.len + nextra;
  Field *fields = THxMEM_alloc((cap ? cap : 1) * sizeof(Field));
  size_t len = base->u.strct.len;
  memcpy(fields, base->u.strct.fields, len * sizeof(Field));
  for (size_t i = 0; i < nextra; i++) {
    bool found = false;
    for (size_t j = 0; j < len; j++)
      if (same_name(fields[j].name, fnames[i])) {
        fields[j].val = vals[i];
        found = true;
        break;
      }
    if (!found) {
      fields[len].name = fnames[i];
      fields[len].val = vals[i];
      len++;
    }
  }
  Value *v = alloc_value(T_STRUCT);
  v->u.strct.name = (name && name[0]) ? name : base->u.strct.name;
  v->u.strct.fields = fields;
  v->u.strct.len = len;
  for (size_t i = 0; i < len; i++) {
    THxMEM_retain(fields[i].val); /* the fresh struct owns every field */
  }
  return v;
}

/* -- checked accessors (the only sanctioned way to read a value) --------- */

long long THxVALUE_as_int(Value *v) {
  if (v->tag != T_INT) thrax_fault("expected an integer");
  return (long long)v->u.i;
}
double THxVALUE_as_num(Value *v) {
  if (v->tag == T_INT) return (double)v->u.i;
  if (v->tag == T_REAL || v->tag == T_REAL32) return v->u.r;
  thrax_fault("expected a number");
}
int THxVALUE_as_bool(Value *v) {
  if (v->tag != T_BOOL) thrax_fault("expected a boolean");
  return v->u.b ? 1 : 0;
}
Value *THxVALUE_local(Value **locals, size_t n, size_t i) {
  if (i >= n) thrax_fault("local slot out of range");
  if (!locals[i]) thrax_fault("read of an empty local slot");
  return locals[i];
}
Value *THxVALUE_env(Value **env, size_t n, size_t i) {
  if (i >= n) thrax_fault("env field out of range");
  if (!env[i]) thrax_fault("read of an empty env field");
  return env[i];
}
/* A local's last use: its reference moves to the temp pool, so the value lives
 * out this bounce while the slot no longer counts as an owner. */
Value *THxVALUE_take(Frame *fr, size_t i) {
  Value *v = THxVALUE_local(fr->locals, fr->nlocals, i);
  fr->locals[i] = NULL;
  if (frame_lent(fr, i)) {
    fr->lent &= ~(1u << i);
    THxMEM_retain(v);
  }
  pool_adopt(v);
  return v;
}
/* A captured field's last use. The field moves out only when this activation
 * is the closure's sole owner and the closure cannot reach itself (a recursive
 * closure's weak self edge would let the body enter it again, and the next
 * activation would find the field gone); otherwise it is read as a borrow. */
Value *THxVALUE_take_env(Frame *fr, size_t i) {
  Value *v = THxVALUE_env(fr->env, fr->nenv, i);
  Value *clos = fr->clos;
  if (!THxMEM_unique(clos)) return v;
  for (size_t j = 0; j < fr->nenv; j++)
    if (fr->env[j] == clos) return v;
  fr->env[i] = NULL;
  pool_adopt(v);
  return v;
}
char *THxVALUE_str(Value *v) {
  if (v->tag != T_STR) thrax_fault("expected a byte vector");
  return (char *)v->u.str.data;
}
static Value *struct_field(Value *v, const char *name) {
  for (size_t i = 0; i < v->u.strct.len; i++)
    if (same_name(v->u.strct.fields[i].name, name))
      return v->u.strct.fields[i].val;
  return NULL;
}
static void THxVALUE_patch_box(Value *box, Value *v);
Value *THxK_call(Value *f, Value *arg);

/* Run an unforced lazy slot and patch the cell with the result, so the work
 * happens once however many times the value is looked at: every holder of the
 * cell sees the forced value. Nesting collapses here, not at each use. */
#ifdef THRAX_THREADS
static Value *force_shared(Value *v);
#endif
static Value *THxK_call_owned(Value *f, Value *arg);
static int steal_payload(Value *box, Value *v);
Value *THxVALUE_force(Value *v) {
#ifdef THRAX_THREADS
  if (v && __builtin_expect(rc_load(v) <= 0, 0)) return force_shared(v);
#endif
  while (v && v->tag == T_THUNK) {
    Value *f = v->u.thunk;
    Value *forced;
#ifndef THRAX_THREADS
    if (unique_local(f)) {
      /* The cell is the closure's only owner, and patching it below would
       * drop the closure anyway, so the call takes it over: the activation
       * is then its sole owner and can move the captures out (a lazy tail
       * reaches the next call uniquely held, ready for reuse). Meanwhile the
       * cell is a black hole, so a value that needs itself faults. */
      v->tag = T_BLACKHOLE;
      v->u.thunk = NULL;
      Value *u = THxRT_unit();
      THxMEM_retain(u);
      forced = THxK_call_owned(f, u); /* owned */
      if (forced == v) thrax_fault("a lazy value depends on itself");
    } else
#endif
    {
      /* Hold the closure across the call: patching the cell below destroys
       * the thunk payload, which is what otherwise owns it. */
      THxMEM_retain(f);
      forced = THxK_call(f, THxRT_unit()); /* owned */
      THxMEM_release(f);
      if (forced == v) break;
    }
    if (!steal_payload(v, forced)) {
      THxVALUE_patch_box(v, forced); /* deep-copies, so the result is ours to drop */
      THxMEM_release(forced);
    }
  }
#ifndef THRAX_THREADS
  if (v && v->tag == T_BLACKHOLE) thrax_fault("a lazy value depends on itself");
#endif
  return v;
}

/* `record.field`: a struct field by name, or a tuple element by index. */
Value *THxVALUE_field(Value *v, const char *name) {
  v = THxVALUE_force(v);
  if (v->tag == T_STRUCT) {
    Value *f = struct_field(v, name);
    if (f) return f;
    thrax_fault("no such field");
  }
  if (v->tag == T_TUPLE) {
    char *end;
    long idx = strtol(name, &end, 10);
    if (*end == '\0' && idx >= 0 && (size_t)idx < v->u.seq.len)
      return v->u.seq.items[idx];
    thrax_fault("no such tuple index");
  }
  thrax_fault("field access on a non-record");
}
const char *THxVALUE_ctor(Value *v) {
  if (v->tag != T_VARIANT) thrax_fault("expected a variant");
  return v->u.variant.tag;
}
static inline int THxVALUE_ctor_is(Value *v, const char *tag) {
  return same_name(THxVALUE_ctor(v), tag);
}
Value *THxVALUE_variant_field(Value *v, size_t i) {
  if (v->tag != T_VARIANT) thrax_fault("expected a variant");
  if (i >= v->u.variant.len) thrax_fault("variant field out of range");
  return v->u.variant.fields[i];
}

/* -- coercions (faults on the wrong kind, like the interpreter) ---------- */

static int64_t as_i64(Value *v) {
  if (v->tag != T_INT) thrax_fault("expected an integer");
  return v->u.i;
}
static double as_f64(Value *v) {
  if (v->tag == T_INT) return (double)v->u.i;
  if (v->tag == T_REAL || v->tag == T_REAL32) return v->u.r;
  thrax_fault("expected a number");
}
/* An operand rounded to single precision, for the `@f32*` intrinsics. */
static float as_f32(Value *v) { return (float)as_f64(v); }
static size_t as_index(Value *v) {
  if (v->tag != T_INT) thrax_fault("expected an integer index");
  if (v->u.i < 0) thrax_fault("negative index");
  return (size_t)v->u.i;
}
static uint8_t as_byte(Value *v) {
  if (v->tag != T_INT || v->u.i < 0 || v->u.i > 255)
    thrax_fault("expected a byte value (0..255)");
  return (uint8_t)v->u.i;
}
static Value *as_str(Value *v) {
  if (v->tag != T_STR) thrax_fault("expected a byte vector");
  return v;
}

/* -- structural equality (==) -------------------------------------------- */

static bool value_eq(Value *x, Value *y) {
  bool xnum = x->tag == T_INT || x->tag == T_REAL || x->tag == T_REAL32;
  bool ynum = y->tag == T_INT || y->tag == T_REAL || y->tag == T_REAL32;
  if (xnum && ynum) return as_f64(x) == as_f64(y);
  if (x->tag != y->tag) return false;
  switch (x->tag) {
    case T_STR:
      return x->u.str.len == y->u.str.len &&
             memcmp(x->u.str.data, y->u.str.data, x->u.str.len) == 0;
    case T_BOOL:
      return x->u.b == y->u.b;
    case T_UNIT:
      return true;
    case T_TUPLE:
    case T_VEC:
      if (x->u.seq.len != y->u.seq.len) return false;
      for (size_t i = 0; i < x->u.seq.len; i++)
        if (!value_eq(x->u.seq.items[i], y->u.seq.items[i])) return false;
      return true;
    case T_STRUCT: {
      if (x->u.strct.len != y->u.strct.len) return false;
      for (size_t i = 0; i < x->u.strct.len; i++) {
        Value *w = struct_field(y, x->u.strct.fields[i].name);
        if (!w || !value_eq(x->u.strct.fields[i].val, w)) return false;
      }
      return true;
    }
    case T_VARIANT:
      if (!same_name(x->u.variant.tag, y->u.variant.tag) ||
          x->u.variant.len != y->u.variant.len)
        return false;
      for (size_t i = 0; i < x->u.variant.len; i++)
        if (!value_eq(x->u.variant.fields[i], y->u.variant.fields[i]))
          return false;
      return true;
    default:
      return false;
  }
}

/* -- built-in operators -------------------------------------------------- */

/* A monomorphic arithmetic intrinsic (`@iadd`, `@fmul`, ...), the primitive the
 * operator instances are built on. `@i*` reads two integers, `@f*` two reals;
 * the type-directed instances guarantee the tags. */
static Value *arith_intrinsic(int id, Value *x, Value *y) {
  switch (id) {
  case THxB_IADD: return THxRT_int(x->u.i + y->u.i);
  case THxB_ISUB: return THxRT_int(x->u.i - y->u.i);
  case THxB_IMUL: return THxRT_int(x->u.i * y->u.i);
  case THxB_IDIV:
    if (y->u.i == 0) thrax_fault("division by zero");
    return THxRT_int(x->u.i / y->u.i);
  case THxB_IMOD:
    if (y->u.i == 0) thrax_fault("division by zero");
    return THxRT_int(x->u.i % y->u.i);
  /* `@u*` read the same bits as unsigned: a `Nat` past i64::MAX would divide
   * wrong under signed division. */
  case THxB_UDIV:
    if (y->u.i == 0) thrax_fault("division by zero");
    return THxRT_int((int64_t)((uint64_t)x->u.i / (uint64_t)y->u.i));
  case THxB_UMOD:
    if (y->u.i == 0) thrax_fault("division by zero");
    return THxRT_int((int64_t)((uint64_t)x->u.i % (uint64_t)y->u.i));
  /* `@f32*` rounds each operand and the result to single precision. */
  case THxB_F32ADD: return THxRT_real32(as_f32(x) + as_f32(y));
  case THxB_F32SUB: return THxRT_real32(as_f32(x) - as_f32(y));
  case THxB_F32MUL: return THxRT_real32(as_f32(x) * as_f32(y));
  case THxB_F32DIV: return THxRT_real32(as_f32(x) / as_f32(y));
  /* Rust f32 `%` is truncated toward zero, like C fmodf. */
  case THxB_F32MOD: {
    float a = as_f32(x), b = as_f32(y);
    return THxRT_real32(a - b * (float)(long long)(a / b));
  }
  case THxB_F32POW: return THxRT_real32(powf(as_f32(x), as_f32(y)));
  case THxB_FADD: return THxRT_real(as_f64(x) + as_f64(y));
  case THxB_FSUB: return THxRT_real(as_f64(x) - as_f64(y));
  case THxB_FMUL: return THxRT_real(as_f64(x) * as_f64(y));
  case THxB_FDIV: return THxRT_real(as_f64(x) / as_f64(y));
  /* Rust f64 `%` is C fmod (truncated toward zero). */
  case THxB_FMOD: {
    double a = as_f64(x), b = as_f64(y);
    return THxRT_real(a - b * (double)(long long)(a / b));
  }
  case THxB_FPOW: return THxRT_real(pow(as_f64(x), as_f64(y)));
  default: thrax_fault("not an arithmetic intrinsic");
  }
}

/* A bitwise intrinsic on the 64-bit value representation. A shift count is read
 * unsigned, and one of 64 or more (a negative count included) shifts every bit
 * out: `@ishl`/`@ushr` give 0, `@ishr` the sign. The bare C shifts would be
 * undefined there, and `>>` on a negative signed value is
 * implementation-defined, so the arithmetic shift goes through the complement. */
static Value *bit_intrinsic(int id, int64_t x, int64_t y) {
  uint64_t n = (uint64_t)y;
  switch (id) {
  case THxB_IAND: return THxRT_int(x & y);
  case THxB_IOR: return THxRT_int(x | y);
  case THxB_IXOR: return THxRT_int(x ^ y);
  case THxB_ISHL: return THxRT_int(n >= 64 ? 0 : (int64_t)((uint64_t)x << n));
  case THxB_USHR: return THxRT_int(n >= 64 ? 0 : (int64_t)((uint64_t)x >> n));
  case THxB_ISHR: {
    if (n > 63) n = 63;
    return THxRT_int(x < 0 ? ~(int64_t)((uint64_t)~x >> n) : (int64_t)((uint64_t)x >> n));
  }
  default: thrax_fault("not a bitwise intrinsic");
  }
}

static int cmp_bytes(Value *a, Value *b) {
  size_t n = a->u.str.len < b->u.str.len ? a->u.str.len : b->u.str.len;
  int c = memcmp(a->u.str.data, b->u.str.data, n);
  if (c != 0) return c < 0 ? -1 : 1;
  if (a->u.str.len == b->u.str.len) return 0;
  return a->u.str.len < b->u.str.len ? -1 : 1;
}

/* A monomorphic comparison intrinsic (`@ieq`, `@slt`, ...), the primitive the
 * comparison instances are built on. `@u*` reads the same bits as unsigned, so a
 * `Nat` past i64::MAX orders correctly. */
static Value *compare_intrinsic(int id, Value *x, Value *y) {
  switch (id) {
  case THxB_IEQ: return THxRT_bool(x->u.i == y->u.i);
  case THxB_ILT: return THxRT_bool(x->u.i < y->u.i);
  case THxB_ULT: return THxRT_bool((uint64_t)x->u.i < (uint64_t)y->u.i);
  case THxB_FEQ: return THxRT_bool(as_f64(x) == as_f64(y));
  case THxB_FLT: return THxRT_bool(as_f64(x) < as_f64(y));
  case THxB_SEQ:
    return THxRT_bool(x->u.str.len == y->u.str.len &&
                      memcmp(x->u.str.data, y->u.str.data, x->u.str.len) == 0);
  case THxB_SLT: return THxRT_bool(cmp_bytes(x, y) < 0);
  default: thrax_fault("not a comparison intrinsic");
  }
}

static Value *compare(int op, Value *x, Value *y) {
  int ord;
  if (x->tag == T_STR && y->tag == T_STR) {
    ord = cmp_bytes(x, y);
  } else {
    double a = as_f64(x), b = as_f64(y);
    ord = a < b ? -1 : (a > b ? 1 : 0);
  }
  switch (op) {
  case THxB_LT: return THxRT_bool(ord < 0);
  case THxB_GT: return THxRT_bool(ord > 0);
  case THxB_LE: return THxRT_bool(ord <= 0);
  default: return THxRT_bool(ord >= 0);
  }
}


static Value *concat(Value *x, Value *y) {
  if (x->tag == T_STR && y->tag == T_STR) {
    size_t len = x->u.str.len + y->u.str.len;
    uint8_t *data = THxMEM_alloc(len + 1);
    memcpy(data, x->u.str.data, x->u.str.len);
    memcpy(data + x->u.str.len, y->u.str.data, y->u.str.len);
    data[len] = 0;
    return mk_str_owned(data, len);
  }
  thrax_fault("`++` on unsupported operands");
}

/* -- strided tensors (mirrors the interpreter's data.rs) ----------------- */
/* A `[n]T` is a `@tensor`-named struct { buf, off, shape, strides } over a flat
 * buffer; a view (row / transpose / slice) shares `buf` and only changes
 * off/shape/strides. The rep reuses T_STRUCT, so alloc/retain/destroy are free. */

static Value *int_vec(size_t *xs, size_t n) {
  Value **items = xmalloc((n ? n : 1) * sizeof(Value *));
  for (size_t i = 0; i < n; i++) items[i] = THxRT_int((int64_t)xs[i]);
  Value *v = mk_vec(items, n);
  free(items);
  return v;
}

static size_t *read_dims(Value *v, size_t *rank_out) {
  if (v->tag != T_VEC) thrax_fault("tensor dims are not a vector");
  size_t n = v->u.seq.len;
  size_t *xs = xmalloc((n ? n : 1) * sizeof(size_t));
  for (size_t i = 0; i < n; i++)
    xs[i] = (size_t)THxVALUE_as_int(v->u.seq.items[i]);
  *rank_out = n;
  return xs;
}

static size_t *row_major(size_t *shape, size_t rank) {
  size_t *s = xmalloc((rank ? rank : 1) * sizeof(size_t));
  if (rank > 0) s[rank - 1] = 1;
  for (size_t k = rank - 1; k > 0; k--) s[k - 1] = s[k] * shape[k];
  return s;
}

static Value *mk_tensor(Value *buf, size_t off, size_t *shape, size_t *strides,
                        size_t rank) {
  const char *fnames[4] = {"buf", "off", "shape", "strides"};
  Value *vals[4] = {buf, THxRT_int((int64_t)off), int_vec(shape, rank),
                    int_vec(strides, rank)};
  return THxRT_struct("@tensor", 4, fnames, vals);
}

/* A tensor is a struct NAMED `@tensor`; the tag alone does not say so, because
 * every user struct shares it. */
static int is_tensor(Value *v) {
  return v->tag == T_STRUCT && v->u.strct.name &&
         strcmp(v->u.strct.name, "@tensor") == 0;
}

static void tensor_fields(Value *t, Value **buf, size_t *off, size_t **shape,
                          size_t **strides, size_t *rank) {
  if (t->tag != T_STRUCT) thrax_fault("expected a tensor");
  Value *b = struct_field(t, "buf"), *o = struct_field(t, "off"),
        *sh = struct_field(t, "shape"), *st = struct_field(t, "strides");
  if (!b || !o || !sh || !st) thrax_fault("malformed tensor");
  *buf = b;
  *off = (size_t)THxVALUE_as_int(o);
  size_t r2;
  *shape = read_dims(sh, rank);
  *strides = read_dims(st, &r2);
}

/* The tensor's scalars in row-major logical order (following a view). */
static Value *materialize(Value *t) {
  Value *buf;
  size_t off, *shape, *strides, rank;
  tensor_fields(t, &buf, &off, &shape, &strides, &rank);
  size_t total = 1;
  for (size_t k = 0; k < rank; k++) total *= shape[k];
  Value **items = xmalloc((total ? total : 1) * sizeof(Value *));
  size_t *idx = xmalloc((rank ? rank : 1) * sizeof(size_t));
  for (size_t k = 0; k < rank; k++) idx[k] = 0;
  for (size_t m = 0; m < total; m++) {
    size_t flat = off;
    for (size_t k = 0; k < rank; k++) flat += idx[k] * strides[k];
    if (flat >= buf->u.seq.len) thrax_fault("tensor buffer out of range");
    items[m] = buf->u.seq.items[flat];
    for (size_t k = rank; k-- > 0;) {
      if (++idx[k] < shape[k]) break;
      idx[k] = 0;
    }
  }
  Value *v = mk_vec(items, total);
  free(items);
  free(idx);
  free(shape);
  free(strides);
  return v;
}

/* Stack `elems` on a new leading axis (scalars -> rank 1; tensors flattened). */
static Value *tensor_stack(Value **elems, size_t n) {
  if (n == 0) {
    size_t sh[1] = {0}, st[1] = {1};
    return mk_tensor(mk_vec(NULL, 0), 0, sh, st, 1);
  }
  if (is_tensor(elems[0])) {
    Value *b0;
    size_t o0, *sub_shape, *sub_strides, sub_rank;
    tensor_fields(elems[0], &b0, &o0, &sub_shape, &sub_strides, &sub_rank);
    free(sub_strides);
    size_t sub_total = 1;
    for (size_t k = 0; k < sub_rank; k++) sub_total *= sub_shape[k];
    size_t total = n * sub_total;
    Value **items = xmalloc((total ? total : 1) * sizeof(Value *));
    size_t pos = 0;
    for (size_t i = 0; i < n; i++) {
      Value *mat = materialize(elems[i]);
      if (mat->u.seq.len != sub_total)
        thrax_fault("stacking tensors of unequal shape");
      for (size_t j = 0; j < sub_total; j++) items[pos++] = mat->u.seq.items[j];
    }
    Value *buf = mk_vec(items, total);
    free(items);
    size_t rank = 1 + sub_rank;
    size_t *shape = xmalloc(rank * sizeof(size_t));
    shape[0] = n;
    for (size_t k = 0; k < sub_rank; k++) shape[k + 1] = sub_shape[k];
    free(sub_shape);
    size_t *strides = row_major(shape, rank);
    Value *t = mk_tensor(buf, 0, shape, strides, rank);
    free(shape);
    free(strides);
    return t;
  }
  Value *buf = mk_vec(elems, n);
  size_t sh[1] = {n}, st[1] = {1};
  return mk_tensor(buf, 0, sh, st, 1);
}

#ifdef THRAX_WITH_COMPILER
/*------------------------------------------------------------------------------
 *\THE COMPILER BRIDGE: `@lex`, `@parse*` and `@eval` call into libthrax
 * (thrax.h, emitted above). Defined only for a program that uses them, so every
 * other program links without the library.
 *-----------------------------------------------------------------------------*/

/* Struct, variant and field names are borrowed `const char*` for a value's whole
 * life, so names arriving from the library are interned for the process. */
static const char **g_names = NULL;
static size_t g_nn = 0, g_ncap = 0;

#ifdef THRAX_THREADS
static pthread_mutex_t g_names_mu = PTHREAD_MUTEX_INITIALIZER;
static const char *intern_name_locked(const char *s);
static const char *intern_name(const char *s) {
  pthread_mutex_lock(&g_names_mu);
  const char *r = intern_name_locked(s);
  pthread_mutex_unlock(&g_names_mu);
  return r;
}
static const char *intern_name_locked(const char *s) {
#else
static const char *intern_name(const char *s) {
#endif
  for (size_t i = 0; i < g_nn; i++)
    if (strcmp(g_names[i], s) == 0) return g_names[i];
  if (g_nn == g_ncap) {
    g_ncap = g_ncap ? g_ncap * 2 : 16;
    g_names = realloc(g_names, g_ncap * sizeof(char *));
    if (!g_names) thrax_fault("name table allocation failed");
  }
  size_t len = strlen(s);
  char *copy = xmalloc(len + 1);
  memcpy(copy, s, len + 1);
  return g_names[g_nn++] = copy;
}

static Value *from_thrax(const thrax_value *v) {
  switch (v->kind) {
    case THRAX_UNIT: return THxRT_unit();
    case THRAX_INT: return THxRT_int(v->int_);
    case THRAX_FLOAT64: return THxRT_real(v->real);
    case THRAX_FLOAT32: return THxRT_real32((float)v->real);
    case THRAX_BOOL: return THxRT_bool((int)v->int_);
    case THRAX_STR: return THxRT_str((const char *)v->bytes, v->len);
    case THRAX_TUPLE:
    case THRAX_VEC:
    case THRAX_STRUCT:
    case THRAX_VARIANT: break;
  }
  Value **items = xmalloc(v->len * sizeof(Value *));
  for (size_t i = 0; i < v->len; i++) items[i] = from_thrax(&v->items[i]);
  Value *r;
  if (v->kind == THRAX_TUPLE) {
    r = THxRT_tuple(items, v->len);
  } else if (v->kind == THRAX_VEC) {
    r = mk_vec(items, v->len);
  } else if (v->kind == THRAX_VARIANT) {
    r = THxRT_variant(intern_name(v->name), intern_name(v->tag), v->len, items);
  } else {
    const char **keys = xmalloc(v->len * sizeof(char *));
    for (size_t i = 0; i < v->len; i++) keys[i] = intern_name(v->keys[i]);
    r = THxRT_struct(intern_name(v->name), v->len, keys, items);
    free(keys);
  }
  free(items);
  return r;
}

_Noreturn static void compiler_fault(const char *builtin, char *err) {
  fprintf(stderr, "thrax: runtime fault in %s:\n%s\n", builtin,
          err ? err : "(no message)");
  thrax_string_free(err);
  exit(1);
}

static Value *code_value(const char *src, size_t len) {
  const char *key = "src";
  Value *text = THxRT_str(src, len);
  return THxRT_struct("@Code", 1, &key, &text);
}

static Value *token_field(Value *tok, const char *field) {
  tok = THxVALUE_force(tok);
  if (tok->tag != T_STRUCT || strcmp(tok->u.strct.name, "@Token") != 0)
    thrax_fault("expected an @Token");
  Value *f = struct_field(tok, field);
  if (!f || f->tag != T_STR) thrax_fault("malformed @Token");
  return f;
}

/* Run a compiler builtin, or return NULL when `name` is not one. */
static Value *compiler_builtin(int id, Value **a) {
  const char *name = THxB_names[id];
  char *err = NULL;
  switch (id) {
  case THxB_TOKEN_KIND: return token_field(a[0], "kind");
  case THxB_TOKEN_TEXT: return token_field(a[0], "text");
  case THxB_LEX: case THxB_FORMAT: case THxB_EVAL: {
    int eval = id == THxB_EVAL;
    Value *arg = THxVALUE_force(a[0]);
    if (eval) {
      if (arg->tag != T_STRUCT || strcmp(arg->u.strct.name, "@Code") != 0)
        thrax_fault("@eval expects an @Code");
      arg = struct_field(arg, "src");
    }
    if (!arg || arg->tag != T_STR) thrax_fault("expected a string");
    thrax_value *v = id == THxB_LEX ? thrax_lex(arg->u.str.data, arg->u.str.len, &err)
                   : id == THxB_FORMAT ? thrax_format(arg->u.str.data, arg->u.str.len, &err)
                   : thrax_eval(arg->u.str.data, arg->u.str.len, &err);
    if (!v) compiler_fault(name, err);
    Value *r = from_thrax(v);
    thrax_value_free(v);
    return r;
  }
  case THxB_PARSE_STR: case THxB_PARSE_ITEMS: {
    Value *s = THxVALUE_force(a[0]);
    if (s->tag != T_STR) thrax_fault("expected a string");
    if (thrax_parse(s->u.str.data, s->u.str.len, id == THxB_PARSE_ITEMS, &err) != 0)
      compiler_fault(name, err);
    return code_value((const char *)s->u.str.data, s->u.str.len);
  }
  case THxB_PARSE: {
    /* `@Code` is text-backed: the lexemes rejoined with a space, as the
     * interpreter's `@parse` does. */
    Value *toks = THxVALUE_force(a[0]);
    if (toks->tag != T_VEC) thrax_fault("expected a vector");
    size_t len = 0;
    for (size_t i = 0; i < toks->u.seq.len; i++)
      len += token_field(toks->u.seq.items[i], "text")->u.str.len + 1;
    char *buf = xmalloc(len);
    size_t at = 0;
    for (size_t i = 0; i < toks->u.seq.len; i++) {
      Value *t = token_field(toks->u.seq.items[i], "text");
      if (i > 0) buf[at++] = ' ';
      memcpy(buf + at, t->u.str.data, t->u.str.len);
      at += t->u.str.len;
    }
    if (thrax_parse((const uint8_t *)buf, at, 0, &err) != 0)
      compiler_fault(name, err);
    Value *r = code_value(buf, at);
    free(buf);
    return r;
  }
  default: return NULL;
  }
}
#endif

/* `@par f` (and `@spawn f`, whose task may also perform `@IO`) starts the task
 * `f {}` and returns its future; `@await` blocks until the result is in and
 * returns it (borrowed: the future owns it). Without threads a task waits in a
 * queue until something needs it, as in the interpreter (see `run_next_task`). */
static Value *THxPAR_spawn(Value *f);
static Value *THxPAR_await(Value *fut);
static Value *THxPAR_spawn_io(Value *f);

static void chan_release(Chan *c);
static void tvar_release(TVar *tv);
static void stm_free_log(void);
static Value *THxCHAN_new(void);
static Value *THxCHAN_send(Value *ch, Value *v);
static Value *THxCHAN_recv(Value *ch);
static Value *THxTVAR_new(Value *v);
static Value *THxTVAR_read(Value *tv);
static Value *THxSTM_begin(void);
static Value *THxSTM_read(Value *tv);
static Value *THxSTM_write(Value *tv, Value *v);
static Value *THxSTM_commit(void);
static Value *THxSTM_wait(void);
static Value *THxSTM_save(void);
static Value *THxSTM_rollback(Value *mark);
#ifdef THRAX_THREADS
static void mark_shared(Value *v);
#endif

/* `@vec_push v x` (a[0], a[1]) or `@vec_set v i x` (a[0], a[1], a[2]). With
 * `owned`, the caller's reference on a[0] is one it gave up (a moved local, or
 * the operand row of a partial application nothing else holds), so when that
 * is a[0]'s only reference nobody can observe the update and it happens in
 * place, returning a[0] itself. Otherwise the result is a fresh vector. A push
 * in place grows the block geometrically, so building by pushes is linear. */
static Value *vec_update(int id, Value **a, int owned) {
  Value *v = a[0];
  if (v->tag != T_VEC) thrax_fault("expected a vector");
  size_t len = v->u.seq.len;
  int push = id == THxB_VEC_PUSH;
  size_t at = push ? len : as_index(a[1]);
  if (at >= len && !push) thrax_fault("vec index out of bounds");
  Value *x = a[push ? 1 : 2];
  if (owned && THxMEM_unique(v)) {
#ifdef THRAX_THREADS
    if (sh_load(v) != SH_LOCAL) mark_shared(x);
#endif
    THxMEM_retain(x);
    if (push) {
      if (len == v->u.seq.cap) {
        size_t cap = len < 4 ? 4 : len * 2;
        v->u.seq.items = THxMEM_realloc(v->u.seq.items, cap * sizeof(Value *));
        v->u.seq.cap = cap;
      }
      v->u.seq.items[len] = x;
      v->u.seq.len = len + 1;
    } else {
      Value *old = v->u.seq.items[at];
      v->u.seq.items[at] = x;
      THxMEM_release(old);
    }
    return v;
  }
  size_t n = push ? len + 1 : len;
  Value *r = alloc_value(T_VEC);
  r->u.seq.items = n ? THxMEM_alloc(n * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) r->u.seq.items[i] = v->u.seq.items[i];
  r->u.seq.items[at] = x;
  for (size_t i = 0; i < n; i++) THxMEM_retain(r->u.seq.items[i]);
  r->u.seq.len = n;
  r->u.seq.cap = n;
  return r;
}

static int is_vec_update(int id) { return id == THxB_VEC_PUSH || id == THxB_VEC_SET; }

static Value *run_builtin(int id, Value **a) {
  switch (id) {
  case THxB_IADD: case THxB_ISUB: case THxB_IMUL: case THxB_IDIV:
  case THxB_IMOD: case THxB_UDIV: case THxB_UMOD:
  case THxB_FADD: case THxB_FSUB: case THxB_FMUL: case THxB_FDIV:
  case THxB_FMOD: case THxB_FPOW:
  case THxB_F32ADD: case THxB_F32SUB: case THxB_F32MUL: case THxB_F32DIV:
  case THxB_F32MOD: case THxB_F32POW:
    return arith_intrinsic(id, a[0], a[1]);
  case THxB_IEQ: case THxB_ILT: case THxB_ULT: case THxB_FEQ:
  case THxB_FLT: case THxB_SEQ: case THxB_SLT:
    return compare_intrinsic(id, a[0], a[1]);
  case THxB_IAND: case THxB_IOR: case THxB_IXOR:
  case THxB_ISHL: case THxB_ISHR: case THxB_USHR:
    return bit_intrinsic(id, as_i64(a[0]), as_i64(a[1]));
  case THxB_INOT: return THxRT_int(~as_i64(a[0]));
  case THxB_F64_BITS: {
    double d = as_f64(a[0]);
    uint64_t u;
    memcpy(&u, &d, sizeof u);
    return THxRT_int((int64_t)u);
  }
  case THxB_BITS_F64: {
    uint64_t u = (uint64_t)as_i64(a[0]);
    double d;
    memcpy(&d, &u, sizeof d);
    return THxRT_real(d);
  }
  case THxB_F32_BITS: {
    float f = as_f32(a[0]);
    uint32_t u;
    memcpy(&u, &f, sizeof u);
    return THxRT_int((int64_t)u);
  }
  case THxB_BITS_F32: {
    uint32_t u = (uint32_t)as_i64(a[0]);
    float f;
    memcpy(&f, &u, sizeof f);
    return THxRT_real32(f);
  }
  case THxB_SWRAP: case THxB_UWRAP: {
    int64_t bits = a[0]->u.i, x = a[1]->u.i;
    if (bits < 1 || bits >= 64) return THxRT_int(x);
    unsigned shift = (unsigned)(64 - bits);
    if (id == THxB_SWRAP) return THxRT_int((int64_t)((uint64_t)x << shift) >> shift);
    return THxRT_int((int64_t)(((uint64_t)x << shift) >> shift));
  }
  case THxB_NEG: {
    if (a[0]->tag == T_INT) return THxRT_int(-a[0]->u.i);
    if (a[0]->tag == T_REAL) return THxRT_real(-a[0]->u.r);
    if (a[0]->tag == T_REAL32) return THxRT_real32(-(float)a[0]->u.r);
    thrax_fault("`neg` on a non-number");
  }
  case THxB_NOT: {
    if (a[0]->tag == T_BOOL) return THxRT_bool(!a[0]->u.b);
    thrax_fault("`not` on a non-boolean");
  }
  case THxB_EQ: return THxRT_bool(value_eq(a[0], a[1]));
  case THxB_LT: case THxB_GT: case THxB_LE: case THxB_GE:
    return compare(id, a[0], a[1]);
  case THxB_CONCAT: return concat(a[0], a[1]);
  case THxB_ACAT: return concat(a[0], a[1]);

  case THxB_ARRAY_ALLOC: {
    size_t len = as_index(a[0]);
    uint8_t *data = THxMEM_alloc(len + 1);
    memset(data, 0, len + 1);
    return mk_str_owned(data, len);
  }
  case THxB_ARRAY_LEN:
    return THxRT_int((int64_t)as_str(a[0])->u.str.len);
  case THxB_ARRAY_GET: {
    Value *s = as_str(a[0]);
    size_t i = as_index(a[1]);
    if (i >= s->u.str.len) thrax_fault("array index out of bounds");
    return THxRT_int(s->u.str.data[i]);
  }
  case THxB_ARRAY_PUSH: {
    Value *s = as_str(a[0]);
    size_t len = s->u.str.len + 1;
    uint8_t *data = THxMEM_alloc(len + 1);
    memcpy(data, s->u.str.data, s->u.str.len);
    data[s->u.str.len] = as_byte(a[1]);
    data[len] = 0;
    return mk_str_owned(data, len);
  }
  case THxB_ARRAY_SET: {
    Value *s = as_str(a[0]);
    size_t i = as_index(a[1]);
    if (i >= s->u.str.len) thrax_fault("array index out of bounds");
    uint8_t *data = THxMEM_alloc(s->u.str.len + 1);
    memcpy(data, s->u.str.data, s->u.str.len);
    data[i] = as_byte(a[2]);
    data[s->u.str.len] = 0;
    return mk_str_owned(data, s->u.str.len);
  }
  case THxB_ARRAY_SLICE: {
    Value *s = as_str(a[0]);
    size_t beg = as_index(a[1]);
    size_t end = as_index(a[2]);
    if (beg > s->u.str.len) beg = s->u.str.len;
    if (end < beg) end = beg;
    if (end > s->u.str.len) end = s->u.str.len;
    return THxRT_str((const char *)s->u.str.data + beg, end - beg);
  }

  case THxB_RECORD_WITHOUT: {
    Value *rec = a[0];
    if (rec->tag != T_STRUCT) thrax_fault("`record_without` on a non-record");
    Value *lbl = as_str(a[1]);
    size_t llen = lbl->u.str.len;
    const char *ldata = (const char *)lbl->u.str.data;
    size_t n0 = rec->u.strct.len;
    Field *fields = THxMEM_alloc((n0 ? n0 : 1) * sizeof(Field));
    size_t len = 0;
    int dropped = 0;
    for (size_t i = 0; i < n0; i++) {
      const char *fn = rec->u.strct.fields[i].name;
      if (!dropped && strlen(fn) == llen && memcmp(fn, ldata, llen) == 0) {
        dropped = 1; /* drop only the head occurrence */
        continue;
      }
      fields[len++] = rec->u.strct.fields[i];
    }
    Value *v = alloc_value(T_STRUCT);
    v->u.strct.name = rec->u.strct.name;
    v->u.strct.fields = fields;
    v->u.strct.len = len;
    for (size_t i = 0; i < len; i++) {
      THxMEM_retain(fields[i].val); /* the fresh struct owns every field */
    }
    return v;
  }
  case THxB_VEC_NEW: return mk_vec(NULL, 0);
  case THxB_VEC_FILL: {
    size_t len = as_index(a[0]);
    Value **items = len ? xmalloc(len * sizeof(Value *)) : NULL;
    for (size_t i = 0; i < len; i++) items[i] = a[1];
    Value *v = mk_vec(items, len);
    free(items);
    return v;
  }
  case THxB_VEC_LEN: {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    return THxRT_int((int64_t)a[0]->u.seq.len);
  }
  case THxB_TENSOR_LENGTH: {
    Value *buf;
    size_t off, *shape, *strides, rank;
    tensor_fields(a[0], &buf, &off, &shape, &strides, &rank);
    int64_t r = rank > 0 ? (int64_t)shape[0] : 0;
    free(shape);
    free(strides);
    return THxRT_int(r);
  }
  case THxB_TENSOR_STACK: {
    if (a[0]->tag != T_VEC) thrax_fault("@tensor_stack expects a vector");
    return tensor_stack(a[0]->u.seq.items, a[0]->u.seq.len);
  }
  case THxB_TENSOR_TRANSPOSE: {
    Value *buf;
    size_t off, *shape, *strides, rank;
    tensor_fields(a[0], &buf, &off, &shape, &strides, &rank);
    for (size_t i = 0; i < rank / 2; i++) { /* reverse both in place */
      size_t j = rank - 1 - i;
      size_t ts = shape[i]; shape[i] = shape[j]; shape[j] = ts;
      size_t td = strides[i]; strides[i] = strides[j]; strides[j] = td;
    }
    Value *t = mk_tensor(buf, off, shape, strides, rank);
    free(shape);
    free(strides);
    return t;
  }
  case THxB_TENSOR_SLICE: {
    Value *buf;
    size_t off, *shape, *strides, rank;
    tensor_fields(a[0], &buf, &off, &shape, &strides, &rank);
    if (rank == 0) thrax_fault("slice of a rank-0 tensor");
    size_t lo = as_index(a[1]);
    size_t hi = as_index(a[2]);
    if (lo > shape[0]) lo = shape[0];
    if (hi < lo) hi = lo;
    if (hi > shape[0]) hi = shape[0];
    size_t base = off + lo * strides[0];
    shape[0] = hi - lo;
    Value *t = mk_tensor(buf, base, shape, strides, rank);
    free(shape);
    free(strides);
    return t;
  }
  case THxB_TENSOR_INDEX_AXIS: {
    Value *buf;
    size_t off, *shape, *strides, rank;
    tensor_fields(a[0], &buf, &off, &shape, &strides, &rank);
    size_t axis = as_index(a[1]);
    if (axis >= rank) { free(shape); free(strides); thrax_fault("index axis out of range"); }
    if (shape[axis] == 0) { free(shape); free(strides); thrax_fault("index into an empty axis"); }
    int64_t iw = THxVALUE_as_int(a[2]);
    int64_t s = (int64_t)shape[axis];
    size_t i = (size_t)(((iw % s) + s) % s);
    size_t base = off + i * strides[axis];
    for (size_t k = axis; k + 1 < rank; k++) { /* drop `axis` */
      shape[k] = shape[k + 1];
      strides[k] = strides[k + 1];
    }
    rank -= 1;
    Value *r;
    if (rank == 0) {
      if (base >= buf->u.seq.len) thrax_fault("tensor index out of bounds");
      r = buf->u.seq.items[base]; /* scalar; borrowed */
    } else {
      r = mk_tensor(buf, base, shape, strides, rank);
    }
    free(shape);
    free(strides);
    return r;
  }
  case THxB_TENSOR_SLICE_AXIS: {
    Value *buf;
    size_t off, *shape, *strides, rank;
    tensor_fields(a[0], &buf, &off, &shape, &strides, &rank);
    size_t axis = as_index(a[1]);
    if (axis >= rank) { free(shape); free(strides); thrax_fault("slice axis out of range"); }
    size_t lo = as_index(a[2]);
    size_t hi = as_index(a[3]);
    if (lo > shape[axis]) lo = shape[axis];
    if (hi < lo) hi = lo;
    if (hi > shape[axis]) hi = shape[axis];
    size_t base = off + lo * strides[axis];
    shape[axis] = hi - lo;
    Value *t = mk_tensor(buf, base, shape, strides, rank);
    free(shape);
    free(strides);
    return t;
  }
  case THxB_DELAY: return THxRT_thunk(a[0]);
  case THxB_PAR: return THxPAR_spawn(a[0]);
  case THxB_SPAWN: return THxPAR_spawn_io(a[0]);
  case THxB_AWAIT: return THxPAR_await(a[0]);
  case THxB_CHAN_NEW: return THxCHAN_new();
  case THxB_CHAN_SEND: return THxCHAN_send(a[0], a[1]);
  case THxB_CHAN_RECV: return THxCHAN_recv(a[0]);
  case THxB_TVAR_NEW: return THxTVAR_new(a[0]);
  case THxB_TVAR_READ: return THxTVAR_read(a[0]);
  case THxB_STM_BEGIN: return THxSTM_begin();
  case THxB_STM_READ: return THxSTM_read(a[0]);
  case THxB_STM_WRITE: return THxSTM_write(a[0], a[1]);
  case THxB_STM_COMMIT: return THxSTM_commit();
  case THxB_STM_WAIT: return THxSTM_wait();
  case THxB_STM_SAVE: return THxSTM_save();
  case THxB_STM_ROLLBACK: return THxSTM_rollback(a[0]);
  case THxB_VEC_GET: {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t i = as_index(a[1]);
    if (i >= a[0]->u.seq.len) thrax_fault("vec index out of bounds");
    return a[0]->u.seq.items[i]; /* borrowed; the caller (do_ret) retains it */
  }
  case THxB_VEC_PUSH: return vec_update(id, a, 0);
  case THxB_VEC_SLICE: {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t n = a[0]->u.seq.len;
    size_t lo = as_index(a[1]);
    size_t hi = as_index(a[2]);
    if (lo > n) lo = n;
    if (hi < lo) hi = lo;
    if (hi > n) hi = n;
    return mk_vec(a[0]->u.seq.items + lo, hi - lo); /* mk_vec retains each */
  }
  case THxB_TENSOR_CONCAT: {
    Value *bx, *by;
    size_t ox, oy, *shx, *stx, rx, *shy, *sty, ry;
    tensor_fields(a[0], &bx, &ox, &shx, &stx, &rx);
    tensor_fields(a[1], &by, &oy, &shy, &sty, &ry);
    if (rx != ry) thrax_fault("concat: rank mismatch");
    for (size_t k = 1; k < rx; k++)
      if (shx[k] != shy[k]) thrax_fault("concat: trailing shape mismatch");
    Value *mx = materialize(a[0]), *my = materialize(a[1]);
    size_t total = mx->u.seq.len + my->u.seq.len;
    Value **items = xmalloc((total ? total : 1) * sizeof(Value *));
    memcpy(items, mx->u.seq.items, mx->u.seq.len * sizeof(Value *));
    memcpy(items + mx->u.seq.len, my->u.seq.items, my->u.seq.len * sizeof(Value *));
    Value *buf = mk_vec(items, total);
    free(items);
    size_t *shape = xmalloc((rx ? rx : 1) * sizeof(size_t));
    for (size_t k = 0; k < rx; k++) shape[k] = shx[k];
    if (rx > 0) shape[0] = shx[0] + shy[0];
    size_t *strides = row_major(shape, rx);
    Value *t = mk_tensor(buf, 0, shape, strides, rx);
    free(shx);
    free(stx);
    free(shy);
    free(sty);
    free(shape);
    free(strides);
    return t;
  }
  case THxB_TENSOR_INDEX: {
    Value *buf;
    size_t off, *shape, *strides, rank;
    tensor_fields(a[0], &buf, &off, &shape, &strides, &rank);
    if (rank == 0 || shape[0] == 0) {
      free(shape);
      free(strides);
      thrax_fault("index into an empty tensor");
    }
    int64_t iw = THxVALUE_as_int(a[1]);
    int64_t s0 = (int64_t)shape[0];
    size_t i = (size_t)(((iw % s0) + s0) % s0);
    size_t base = off + i * strides[0];
    Value *r;
    if (rank == 1) {
      if (base >= buf->u.seq.len) thrax_fault("tensor index out of bounds");
      r = buf->u.seq.items[base]; /* borrowed; do_ret retains it */
    } else {
      r = mk_tensor(buf, base, shape + 1, strides + 1, rank - 1); /* view */
    }
    free(shape);
    free(strides);
    return r;
  }
  case THxB_VEC_SET: return vec_update(id, a, 0);

  default: break;
  }
#ifdef THRAX_WITH_COMPILER
  Value *r = compiler_builtin(id, a);
  if (r) return r;
#endif
  thrax_fault("unknown built-in");
}

/* -- TARGET reflection --------------------------------------------------- */

Value *THxRT_target(const char *name) {
  if (strcmp(name, "int_bits") == 0 || strcmp(name, "ptr_bits") == 0)
    return THxRT_int((int64_t)THRAX_INT_BITS);
  if (strcmp(name, "int_max") == 0) return THxRT_int(THRAX_INT_MAX);
  if (strcmp(name, "int_min") == 0) return THxRT_int(THRAX_INT_MIN);
  if (strcmp(name, "arch") == 0) return THxRT_str(THRAX_ARCH, strlen(THRAX_ARCH));
  if (strcmp(name, "os") == 0) return THxRT_str(THRAX_OS, strlen(THRAX_OS));
  if (strcmp(name, "name") == 0)
    return THxRT_str(THRAX_ARCH "-" THRAX_OS, strlen(THRAX_ARCH "-" THRAX_OS));
  thrax_fault("unknown TARGET field");
}

/* An effect operation value (canonical effect + op names). */
Value *THxK_op(const char *effect, const char *op) {
  Value *v = alloc_value(T_OP);
  v->u.op.effect = effect;
  v->u.op.op = op;
  return v;
}

/*------------------------------------------------------------------------------
 *\VALUE LIFETIME (port of THxVALUE.c: destroy / patch_box)
 *-----------------------------------------------------------------------------*/

static void THxK_resump_addref(Resump *seg);
static void THxK_resump_release(Resump *seg);
static void task_destroy(Task *t);
#ifdef THRAX_THREADS
static void mark_shared(Value *v);
static void mark_immortal(Value *v);
#endif

/* Release the pointer children in `f[0..n)` (skipping the weak self edge,
 * child == v) and free the array itself. */
static void release_children(Value *v, Value **f, size_t n) {
  if (!f) return;
  for (size_t i = 0; i < n; i++)
    if (f[i] != v) THxMEM_release(f[i]);
  THxMEM_cell_free(f, n * sizeof(Value *));
}

/* Free v's owned payload: release children, free raw blocks. Shared by
 * THxVALUE_destroy (rc reached zero) and THxVALUE_patch_box (overwriting a
 * box's previous contents). */
static void payload_destroy(Value *v) {
  switch (v->tag) {
    case T_INT:
    case T_REAL:
    case T_REAL32:
    case T_BOOL:
    case T_UNIT:
    case T_OP: return;
    case T_STR: THxMEM_free(v->u.str.data); return;
    case T_TUPLE: release_children(v, v->u.seq.items, v->u.seq.len); return;
    case T_VEC:
      for (size_t i = 0; i < v->u.seq.len; i++)
        if (v->u.seq.items[i] != v) THxMEM_release(v->u.seq.items[i]);
      THxMEM_free(v->u.seq.items);
      return;
    case T_STRUCT: {
      Field *fs = v->u.strct.fields;
      if (fs) {
        for (size_t i = 0; i < v->u.strct.len; i++)
          if (fs[i].val != v) THxMEM_release(fs[i].val);
        THxMEM_cell_free(fs, v->u.strct.len * sizeof(Field));
      }
      return;
    }
    case T_VARIANT: release_children(v, v->u.variant.fields, v->u.variant.len); return;
    case T_CLOS: release_children(v, v->u.clos.env, v->u.clos.nenv); return;
    case T_BUILTIN: release_children(v, v->u.builtin.args, v->u.builtin.nargs); return;
    case T_EXTERN: release_children(v, v->u.ext.args, v->u.ext.nargs); return;
    case T_RESUMP: THxK_resump_release(v->u.resump); return;
    case T_THUNK:
    case T_BLACKHOLE: THxMEM_release(v->u.thunk); return;
    case T_FUTURE: task_destroy(v->u.future); return;
    case T_CHAN: chan_release(v->u.chan); return;
    case T_TVAR: tvar_release(v->u.tvar); return;
  }
  thrax_fault("payload_destroy: unhandled tag");
}

static void THxVALUE_destroy(Value *v) { payload_destroy(v); }

/* A fresh array holding f[0..n), each child retained except the weak self edge
 * (child == self). NULL when n == 0. */
static Value **copy_children(Value *self, Value **f, size_t n) {
  if (n == 0) return NULL;
  Value **c = THxMEM_cell(n * sizeof(Value *));
  for (size_t i = 0; i < n; i++) {
    c[i] = f[i];
    if (c[i] != self) THxMEM_retain(c[i]);
  }
  return c;
}

/* Make `dst` a copy of `v`'s payload (tag and union; never the count or the
 * shared mark). Pointer-array payloads and string bytes are DEEP-copied into
 * fresh blocks and the children retained, so `dst` and `v` afterwards have
 * independent lifetimes; a resumption's segment is shared via its own count. A
 * child equal to `self` (the cell that will hold the payload) is left
 * unretained: the weak self edge of a recursive-let closure. */
static void copy_payload(Value *dst, Value *self, Value *v) {
  dst->tag = v->tag;
  dst->u = v->u;
  switch (dst->tag) {
    case T_INT:
    case T_REAL:
    case T_REAL32:
    case T_BOOL:
    case T_UNIT:
    case T_OP: break;
    case T_STR: {
      size_t len = dst->u.str.len;
      uint8_t *b = THxMEM_alloc(len + 1);
      if (len) memcpy(b, v->u.str.data, len);
      b[len] = 0;
      dst->u.str.data = b;
      dst->u.str.cap = len;
      break;
    }
    case T_TUPLE:
    case T_VEC:
      dst->u.seq.items = copy_children(self, v->u.seq.items, dst->u.seq.len);
      dst->u.seq.cap = dst->u.seq.len;
      break;
    case T_STRUCT: {
      size_t n = dst->u.strct.len;
      if (n == 0) {
        dst->u.strct.fields = NULL;
        break;
      }
      Field *fs = THxMEM_cell(n * sizeof(Field));
      for (size_t i = 0; i < n; i++) {
        fs[i].name = v->u.strct.fields[i].name;
        fs[i].val = v->u.strct.fields[i].val;
        if (fs[i].val != self) THxMEM_retain(fs[i].val);
      }
      dst->u.strct.fields = fs;
      break;
    }
    case T_VARIANT:
      dst->u.variant.fields =
          copy_children(self, v->u.variant.fields, dst->u.variant.len);
      break;
    case T_CLOS:
      dst->u.clos.env = copy_children(self, v->u.clos.env, dst->u.clos.nenv);
      break;
    case T_BUILTIN:
      dst->u.builtin.args =
          copy_children(self, v->u.builtin.args, dst->u.builtin.nargs);
      break;
    case T_EXTERN:
      dst->u.ext.args = copy_children(self, v->u.ext.args, dst->u.ext.nargs);
      break;
    case T_RESUMP: THxK_resump_addref(dst->u.resump); break;
    case T_THUNK:
    case T_BLACKHOLE:
      if (dst->u.thunk != self) THxMEM_retain(dst->u.thunk);
      break;
    case T_FUTURE:
      /* The handle is one task; a copy is another owner of the same task, so
       * both cells observe one result. */
#ifdef THRAX_THREADS
      __atomic_fetch_add(&dst->u.future->rc, 1, __ATOMIC_RELAXED);
#else
      dst->u.future->rc++;
#endif
      break;
    case T_CHAN: obj_retain(&dst->u.chan->rc); break;
    case T_TVAR: obj_retain(&dst->u.tvar->rc); break;
  }
}

static int is_constant(Value *v) {
  return v == &g_unit || v == &g_true || v == &g_false ||
         (v >= g_small_ints && v < g_small_ints + (THX_SMALL_MAX - THX_SMALL_MIN + 1));
}

/* Whether `c` is one of `v`'s children. */
static int has_child(Value *v, Value *c) {
  Value **f = NULL;
  size_t n = 0;
  switch (v->tag) {
    case T_TUPLE:
    case T_VEC: f = v->u.seq.items, n = v->u.seq.len; break;
    case T_VARIANT: f = v->u.variant.fields, n = v->u.variant.len; break;
    case T_CLOS: f = v->u.clos.env, n = v->u.clos.nenv; break;
    case T_BUILTIN: f = v->u.builtin.args, n = v->u.builtin.nargs; break;
    case T_EXTERN: f = v->u.ext.args, n = v->u.ext.nargs; break;
    case T_STRUCT:
      for (size_t i = 0; i < v->u.strct.len; i++)
        if (v->u.strct.fields[i].val == c) return 1;
      return 0;
    case T_THUNK:
    case T_BLACKHOLE: return v->u.thunk == c;
    default: return 0;
  }
  for (size_t i = 0; i < n; i++)
    if (f[i] == c) return 1;
  return 0;
}

/* Patch `box` with `v` by moving `v`'s payload in, when the caller's is the
 * only reference to `v` (which it gives up) and neither cell is a child of
 * `v`: a self edge would change meaning in the move. Returns 0, changing
 * nothing, otherwise. Saves the copy `THxVALUE_patch_box` makes. */
static int steal_payload(Value *box, Value *v) {
  if (box == v || is_constant(box) || !unique_local(v)) return 0;
#ifdef THRAX_THREADS
  if (sh_load(box) != SH_LOCAL) return 0;
#endif
  if (v->tag == T_RESUMP || has_child(v, v) || has_child(v, box)) return 0;
  payload_destroy(box);
  box->tag = v->tag;
  box->u = v->u;
  v->tag = T_UNIT;
  THxMEM_release(v);
  return 1;
}

/* Back-patch a let box: make `box` a copy of `v` (see `copy_payload`),
 * preserving box's own count and shared mark. A box other threads can reach
 * must not take in a thread-local value, so `v` is marked first. */
static void THxVALUE_patch_box(Value *box, Value *v) {
  if (box == v) return;
  if (is_constant(box)) thrax_fault("patch_box: patching a shared constant");
  payload_destroy(box); /* a box is normally T_UNIT: a no-op */
#ifdef THRAX_THREADS
  uint8_t sh = sh_load(box);
  if (sh == SH_IMMORTAL)
    mark_immortal(v);
  else if (sh)
    mark_shared(v);
#endif
  copy_payload(box, box, v);
}

/*------------------------------------------------------------------------------
 *\THE REIFIED-K (CEK) DRIVER (port of THxK.c)
 *-----------------------------------------------------------------------------*/

/* The single, shared continuation stack; re-entrant runs share it via a base
 * marker (THxK_run_code / THxK_call record the current height). */
static THX_TLS KFrame *g_kont = NULL;
static THX_TLS size_t g_kn = 0, g_kcap = 0;

static void kont_push(KFrame f) {
  if (g_kn == g_kcap) {
    g_kcap = g_kcap ? g_kcap * 2 : 64;
    g_kont = realloc(g_kont, g_kcap * sizeof(KFrame));
    if (!g_kont) thrax_fault("continuation stack allocation failed");
  }
  g_kont[g_kn++] = f;
}

/* -- frames (refcounted activations) ------------------------------------- */

/* A frame and its slot array are one block, recycled by size. */
static inline size_t frame_size(size_t nlocals) {
  return sizeof(Frame) + nlocals * sizeof(Value *);
}

static Frame *frame_new(size_t nlocals, Value *clos) {
  Frame *f = THxMEM_cell(frame_size(nlocals));
  f->rc = 1;
  f->lent = 0;
  f->clos = clos;
  if (clos) {
    THxMEM_retain(clos);
    f->env = clos->u.clos.env;
    f->nenv = clos->u.clos.nenv;
  } else {
    f->env = NULL;
    f->nenv = 0;
  }
  f->locals = nlocals ? (Value **)(f + 1) : NULL;
  f->nlocals = nlocals;
  return f;
}

static void frame_retain(Frame *f) {
  if (f) ++f->rc;
}

static void frame_release(Frame *f) {
  if (!f) return;
  if (f->rc == 0) thrax_fault("frame_release: releasing a freed frame");
  if (--f->rc > 0) return;
  for (size_t i = 0; i < f->nlocals; i++)
    if (!frame_lent(f, i)) THxMEM_release(f->locals[i]);
  THxMEM_release(f->clos);
  THxMEM_cell_free(f, frame_size(f->nlocals));
}

/* -- kont-frame + segment lifetime --------------------------------------- */

/* Release everything a popped (or discarded) kont frame owns. */
static void kframe_release(KFrame *kf) {
  switch (kf->tag) {
    case K_RET: frame_release(kf->u.ret.frame); return;
    case K_PROMPT:
      for (size_t j = 0; j < kf->u.prompt.n; j++)
        THxMEM_release(kf->u.prompt.clauses[j]);
      THxMEM_free((void *)kf->u.prompt.effs);
      THxMEM_free((void *)kf->u.prompt.ops);
      THxMEM_free((void *)kf->u.prompt.uses);
      THxMEM_free(kf->u.prompt.clauses);
      THxMEM_release(kf->u.prompt.els);
      return;
    case K_DEFER: THxMEM_release(kf->u.defer.cleanup); return;
    case K_THUNKRET: THxMEM_release(kf->u.thunkret.saved); return;
    case K_AFTERCLAUSE:
      RES_DEC(&kf->u.afterclause.res->nbound);
      THxK_resump_release(kf->u.afterclause.res);
      return;
    case K_CLEANUPS:
      for (size_t j = 0; j < kf->u.cleanups.n; j++)
        THxMEM_release(kf->u.cleanups.cleanups[j]);
      THxMEM_free(kf->u.cleanups.cleanups);
      return;
  }
  thrax_fault("kframe_release: unhandled continuation frame");
}

static void THxK_resump_addref(Resump *seg) {
  if (seg) RES_INC(&seg->rc);
}

static void THxK_resump_release(Resump *seg) {
  if (!seg) return;
  unsigned left = RES_DEC(&seg->rc);
  if (left == (unsigned)-1) thrax_fault("resump_release: releasing a freed slice");
  if (left > 0) return;
  /* The frames are still here unless a resume moved them out (which leaves
   * `seg` NULL), so a slice resumed by copying releases them however often it
   * ran. */
  if (seg->seg)
    for (size_t i = 0; i < seg->n; i++) kframe_release(&seg->seg[i]);
  THxMEM_free(seg->seg);
  THxMEM_free(seg);
}

/* -- slice copying (a resume that may not be the last) ------------------- */

/* A fresh activation holding the same values as `f`. The locals are retained,
 * not copied: a value is immutable, so only the slot array has to be private. */
static Frame *frame_clone(Frame *f) {
  Frame *c = THxMEM_cell(frame_size(f->nlocals));
  c->rc = 1;
  c->clos = f->clos;
  THxMEM_retain(c->clos);
  c->env = f->env;
  c->nenv = f->nenv;
  c->nlocals = f->nlocals;
  c->lent = f->lent;
  c->locals = f->nlocals ? (Value **)(c + 1) : NULL;
  for (size_t i = 0; i < f->nlocals; i++) {
    c->locals[i] = f->locals[i];
    if (!frame_lent(f, i)) THxMEM_retain(c->locals[i]);
  }
  return c;
}

/* One activation of the slice being copied, so a frame two continuation frames
 * share stays ONE activation in the copy. */
typedef struct {
  Frame **orig;
  Frame **copy;
  size_t n;
} FrameMap;

static Frame *frame_copy_of(FrameMap *m, Frame *f) {
  for (size_t i = 0; i < m->n; i++)
    if (m->orig[i] == f) {
      frame_retain(m->copy[i]);
      return m->copy[i];
    }
  Frame *c = frame_clone(f);
  m->orig[m->n] = f;
  m->copy[m->n] = c;
  ++m->n;
  frame_retain(c); /* the map's own reference, dropped by frame_map_free */
  return c;
}

static void frame_map_free(FrameMap *m) {
  for (size_t i = 0; i < m->n; i++) frame_release(m->copy[i]);
  free(m->orig);
  free(m->copy);
}

/* A private copy of a captured slice, for a resume that may not be the last.
 * Values are immutable, so they are shared; what is copied is the machinery a
 * second resume would otherwise write through twice: the activations, and the
 * let box each K_RET patches in place.
 *
 * Two things stay shared, as in Koka and OCaml: a resumption stored in a copied
 * frame's locals, and the clauses a nested prompt holds. Sharing is what keeps
 * them correct: the copy holds another reference to a nested resumption, so
 * that one is no longer uniquely owned and its own resumes copy too. */
static void segment_copy(Resump *src, KFrame *out) {
  FrameMap map;
  map.orig = xmalloc((src->n ? src->n : 1) * sizeof(Frame *));
  map.copy = xmalloc((src->n ? src->n : 1) * sizeof(Frame *));
  map.n = 0;
  for (size_t i = 0; i < src->n; i++) {
    KFrame kf = src->seg[i];
    switch (kf.tag) {
      case K_RET: {
        Frame *c = frame_copy_of(&map, kf.u.ret.frame);
        /* A slot holding a box (a recursive `let`'s) is patched in place on
         * return, so the copy needs its own; a closure built by the right-hand
         * side still reads the original's (it captured that very value). */
        if (c->locals[kf.u.ret.slot]) {
          Value *box = fresh_box();
          THxMEM_retain(box);
          THxMEM_release(c->locals[kf.u.ret.slot]);
          c->locals[kf.u.ret.slot] = box;
        }
        kf.u.ret.frame = c;
        break;
      }
      case K_PROMPT: {
        size_t n = kf.u.prompt.n;
        const char **effs = THxMEM_alloc((n ? n : 1) * sizeof(char *));
        const char **ops = THxMEM_alloc((n ? n : 1) * sizeof(char *));
        int *uses = THxMEM_alloc((n ? n : 1) * sizeof(int));
        Value **cls = THxMEM_alloc((n ? n : 1) * sizeof(Value *));
        for (size_t j = 0; j < n; j++) {
          effs[j] = kf.u.prompt.effs[j];
          ops[j] = kf.u.prompt.ops[j];
          uses[j] = kf.u.prompt.uses[j];
          cls[j] = kf.u.prompt.clauses[j];
          THxMEM_retain(cls[j]);
        }
        kf.u.prompt.effs = effs;
        kf.u.prompt.ops = ops;
        kf.u.prompt.uses = uses;
        kf.u.prompt.clauses = cls;
        THxMEM_retain(kf.u.prompt.els);
        break;
      }
      case K_DEFER: THxMEM_retain(kf.u.defer.cleanup); break;
      case K_THUNKRET: THxMEM_retain(kf.u.thunkret.saved); break;
      case K_AFTERCLAUSE:
        THxK_resump_addref(kf.u.afterclause.res);
        RES_INC(&kf.u.afterclause.res->nbound);
        break;
      case K_CLEANUPS: {
        size_t n = kf.u.cleanups.n;
        Value **cl = THxMEM_alloc((n ? n : 1) * sizeof(Value *));
        for (size_t j = 0; j < n; j++) {
          cl[j] = kf.u.cleanups.cleanups[j];
          THxMEM_retain(cl[j]);
        }
        kf.u.cleanups.cleanups = cl;
        break;
      }
    }
    out[i] = kf;
  }
  frame_map_free(&map);
}

/* The `defer` cleanups a slice holds, in slice order, as a K_CLEANUPS frame:
 * what a clause that cannot resume keeps of its abandoned slice. */
static KFrame segment_cleanups(KFrame *seg, size_t n) {
  KFrame kf;
  kf.tag = K_CLEANUPS;
  kf.u.cleanups.cleanups = THxMEM_alloc((n ? n : 1) * sizeof(Value *));
  kf.u.cleanups.n = 0;
  for (size_t i = 0; i < n; i++)
    if (seg[i].tag == K_DEFER) {
      Value *c = seg[i].u.defer.cleanup;
      THxMEM_retain(c);
      kf.u.cleanups.cleanups[kf.u.cleanups.n++] = c;
    }
  return kf;
}

/* -- next-action registers ----------------------------------------------- */

typedef enum { ACT_NONE, ACT_RET, ACT_APPLY, ACT_JUMP } Act;
static THX_TLS Act g_act;
static THX_TLS Value *g_v, *g_fn, *g_arg;
/* Whether the pending application lends `g_arg` (see `THxK_apply_lent`). */
static THX_TLS int g_lend;
static THX_TLS BlockFn g_blk;
static THX_TLS Value *g_result;

/* -- block terminators --------------------------------------------------- */

void THxK_ret(Value *v) {
  g_act = ACT_RET;
  g_v = v;
}
void THxK_tailcall(Value *fn, Value *arg) {
  g_act = ACT_APPLY;
  g_fn = fn;
  g_arg = arg;
  g_lend = 0;
}
/* A tail call lending `arg`, which outlives the callee (a global's cached
 * value). */
void THxK_tailcall_lent(Value *fn, Value *arg) {
  THxK_tailcall(fn, arg);
  g_lend = 1;
}
/* A tail call passing on local `slot`: lent again when this activation was lent
 * it (its lender outlives the callee too), else its reference moves over. */
void THxK_tailcall_pass(Frame *fr, Value *fn, size_t slot) {
  if (frame_lent(fr, slot))
    THxK_tailcall_lent(fn, THxVALUE_local(fr->locals, fr->nlocals, slot));
  else
    THxK_tailcall(fn, THxVALUE_take(fr, slot));
}
void THxK_apply(Frame *fr, Value *fn, Value *arg, BlockFn cont, size_t slot) {
  KFrame kf;
  kf.tag = K_RET;
  kf.u.ret.frame = fr;
  kf.u.ret.cont = cont;
  kf.u.ret.slot = slot;
  frame_retain(fr); /* the kont keeps the suspended activation alive */
  kont_push(kf);
  g_act = ACT_APPLY;
  g_fn = fn;
  g_arg = arg;
  g_lend = 0;
}
/* A non-tail call lending `arg`: the waiting activation (or a global's cache)
 * keeps it alive until the callee returns, so a callee that borrows its
 * parameter binds it without counting. See the frontend's Note [Borrowed
 * parameters]. */
void THxK_apply_lent(Frame *fr, Value *fn, Value *arg, BlockFn cont, size_t slot) {
  THxK_apply(fr, fn, arg, cont, slot);
  g_lend = 1;
}
void THxK_jump(BlockFn cont) {
  g_act = ACT_JUMP;
  g_blk = cont;
}
void THxK_handle(Frame *fr, BlockFn cont, size_t slot, const char **effs,
                 const char **ops, const int *uses, Value **clauses,
                 size_t nclauses, Value *els, BlockFn body) {
  if (cont) {
    KFrame kr;
    kr.tag = K_RET;
    kr.u.ret.frame = fr;
    kr.u.ret.cont = cont;
    kr.u.ret.slot = slot;
    frame_retain(fr);
    kont_push(kr);
  }
  /* The generated clause tables are C compound literals with block lifetime;
   * copy them into runtime-lifetime memory the prompt owns. */
  const char **effs2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(char *));
  const char **ops2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(char *));
  int *uses2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(int));
  Value **claus2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(Value *));
  for (size_t i = 0; i < nclauses; i++) {
    effs2[i] = effs[i];
    ops2[i] = ops[i];
    uses2[i] = uses[i];
    claus2[i] = clauses[i];
    THxMEM_retain(claus2[i]);
  }
  KFrame kp;
  kp.tag = K_PROMPT;
  kp.u.prompt.effs = effs2;
  kp.u.prompt.ops = ops2;
  kp.u.prompt.uses = uses2;
  kp.u.prompt.clauses = claus2;
  kp.u.prompt.n = nclauses;
  kp.u.prompt.els = els;
  THxMEM_retain(els);
  kont_push(kp);
  g_act = ACT_JUMP;
  g_blk = body;
}
/* `defer cleanup do body`: push a KRet for the eventual value (unless in tail
 * position), a K_DEFER marker holding the 0-arg cleanup thunk, then run body. */
void THxK_defer_run(Frame *fr, BlockFn cont, size_t slot, Value *cleanup,
                    BlockFn body) {
  if (cont) {
    KFrame kr;
    kr.tag = K_RET;
    kr.u.ret.frame = fr;
    kr.u.ret.cont = cont;
    kr.u.ret.slot = slot;
    frame_retain(fr);
    kont_push(kr);
  }
  KFrame kd;
  kd.tag = K_DEFER;
  kd.u.defer.cleanup = cleanup;
  THxMEM_retain(cleanup); /* the kont stores it */
  kont_push(kd);
  g_act = ACT_JUMP;
  g_blk = body;
}

/* -- let-box helpers ----------------------------------------------------- */

void THxK_setlocal(Frame *fr, size_t slot, Value *v) {
  if (slot >= fr->nlocals) thrax_fault("setlocal: slot out of range");
  THxMEM_retain(v); /* retain-before-release: v may be the slot's old value */
  if (frame_lent(fr, slot))
    fr->lent &= ~(1u << slot);
  else
    THxMEM_release(fr->locals[slot]);
  fr->locals[slot] = v;
}
void THxK_drop(Frame *fr, size_t slot) {
  if (slot >= fr->nlocals) thrax_fault("drop: slot out of range");
  if (frame_lent(fr, slot))
    fr->lent &= ~(1u << slot);
  else
    THxMEM_release(fr->locals[slot]);
  fr->locals[slot] = NULL;
}
void THxK_setbox(Frame *fr, size_t slot) {
  THxK_setlocal(fr, slot, fresh_box());
}
/* A slot with no box is a non-recursive `let`'s: the value moves in. */
void THxK_backpatch(Frame *fr, size_t slot, Value *v) {
  if (slot >= fr->nlocals) thrax_fault("backpatch: slot out of range");
  Value *box = fr->locals[slot];
  if (!box) {
    THxK_setlocal(fr, slot, v);
    return;
  }
  THxVALUE_patch_box(box, v);
}

/* `let slot = from` at `from`'s last use: the reference moves between the
 * slots. Into a recursive let's box it is patched as any value is. */
void THxK_move(Frame *fr, size_t slot, size_t from) {
  if (slot >= fr->nlocals) thrax_fault("move: slot out of range");
  Value *v = THxVALUE_local(fr->locals, fr->nlocals, from);
  if (fr->locals[slot]) {
    THxVALUE_patch_box(fr->locals[slot], v);
    THxK_drop(fr, from);
    return;
  }
  fr->locals[slot] = v;
  fr->locals[from] = NULL;
}

/* -- constructor reuse ---------------------------------------------------
 * See Note [Constructor reuse]. */

/* Release the reuse token in local `token` (if any) and store `v` there. */
static void set_token(Frame *fr, long token, Value *v) {
  if (token < 0) {
    THxMEM_release(v);
    return;
  }
  if ((size_t)token >= fr->nlocals) thrax_fault("reuse token out of range");
  THxMEM_release(fr->locals[token]);
  fr->locals[token] = v;
}

/* Enter a case alternative where the scrutinee `v`, held by local `slot`,
 * dies. Held once, its fields move into the bound binders (`bind[i]`) without
 * a count changing, the rest are released, and the emptied cell is freed or
 * parked in local `token` (-1: none). Otherwise the binders share the fields,
 * the slot's reference is released, and the token is left empty. */
void THxK_unpack(Frame *fr, Value *v, size_t slot, long token, size_t base,
                 const unsigned char *bind, size_t n) {
  if (slot >= fr->nlocals || base + n > fr->nlocals) thrax_fault("unpack: slot out of range");
  if (v->tag != T_VARIANT || v->u.variant.len != n) thrax_fault("unpack: shape mismatch");
  Value **f = v->u.variant.fields;
  int own = fr->locals[slot] == v && unique_local(v);
  for (size_t i = 0; own && i < n; i++)
    if (f[i] == v) own = 0; /* a weak self edge is no reference to hand on */
  if (own) {
    fr->locals[slot] = NULL;
    for (size_t i = 0; i < n; i++) {
      if (bind[i]) {
        THxMEM_release(fr->locals[base + i]);
        fr->locals[base + i] = f[i];
      } else {
        THxMEM_release(f[i]);
      }
      f[i] = NULL;
    }
    set_token(fr, token, v);
    return;
  }
  for (size_t i = 0; i < n; i++)
    if (bind[i]) THxK_setlocal(fr, base + i, f[i]);
  THxK_drop(fr, slot);
  set_token(fr, token, NULL);
}

/* A variant built into the cell reuse token `token` holds, when it holds one
 * nobody else does (a copied activation of a resumption shares it); a fresh
 * one otherwise. Either way the token is used up, and the pool holds the
 * result's reference as for any fresh value. */
Value *THxRT_variant_at(Frame *fr, size_t token, const char *ty, const char *tag,
                        size_t len, Value **fields) {
  if (token >= fr->nlocals) thrax_fault("reuse token out of range");
  Value *v = fr->locals[token];
  fr->locals[token] = NULL;
  if (!v || v->tag != T_VARIANT || v->u.variant.len != len || !unique_local(v)) {
    THxMEM_release(v);
    return THxRT_variant(ty, tag, len, fields);
  }
#ifdef THRAX_ALLOC_STATS
  __atomic_fetch_add(&g_reuses_total, 1, __ATOMIC_RELAXED);
#endif
  v->u.variant.ty = ty;
  v->u.variant.tag = tag;
  for (size_t i = 0; i < len; i++) {
    if (v->u.variant.fields[i]) thrax_fault("reuse of a cell with live fields");
    v->u.variant.fields[i] = fields[i];
    THxMEM_retain(fields[i]);
  }
  pool_adopt(v);
  return v;
}

/* -- direct functions ---------------------------------------------------- */

/* A value a direct function returns is owned; the pool takes that reference,
 * so to its caller it is a temporary like any fresh value. */
static inline Value *THxK_own(Value *v) {
  pool_adopt(v);
  return v;
}

/* The integer intrinsics on C scalars, for generated code that keeps integers
 * unboxed; the boxed versions are in `arith_intrinsic`. */
static inline int64_t THxI(Value *v) { return v->u.i; }
static inline int64_t THxI_div(int64_t a, int64_t b) {
  if (b == 0) thrax_fault("division by zero");
  return a / b;
}
static inline int64_t THxI_mod(int64_t a, int64_t b) {
  if (b == 0) thrax_fault("division by zero");
  return a % b;
}
static inline int64_t THxI_udiv(int64_t a, int64_t b) {
  if (b == 0) thrax_fault("division by zero");
  return (int64_t)((uint64_t)a / (uint64_t)b);
}
static inline int64_t THxI_umod(int64_t a, int64_t b) {
  if (b == 0) thrax_fault("division by zero");
  return (int64_t)((uint64_t)a % (uint64_t)b);
}

/* Whether a direct call may still use the C stack: within THX_DIRECT_STACK
 * bytes of where this thread first asked. Past it, calls go through the
 * machine, whose continuation lives on the heap. */
static THX_TLS uintptr_t g_dbase = 0;
static inline int THxD_room(void) {
  uintptr_t sp = (uintptr_t)__builtin_frame_address(0);
  if (!g_dbase) g_dbase = sp;
  return sp >= g_dbase || g_dbase - sp < THX_DIRECT_STACK;
}

/* A direct function's activation: a Frame on the C stack over the slot array
 * `locals` and the environment `env`, both C arrays of the function. */
static inline void THxD_frame(Frame *fr, Value **locals, size_t nlocals, Value **env, size_t nenv) {
  fr->rc = 1;
  fr->lent = 0;
  fr->clos = NULL;
  fr->locals = locals;
  fr->nlocals = nlocals;
  fr->env = env;
  fr->nenv = nenv;
  memset(locals, 0, (nlocals ? nlocals : 1) * sizeof(Value *));
}

/* Release a direct activation: its slots, its arguments `params`, and the
 * temporaries it made since `mark`. */
static void THxD_reset(Frame *fr, Value **params, size_t nparams, size_t mark) {
  for (size_t i = 0; i < fr->nlocals; i++) THxMEM_release(fr->locals[i]);
  for (size_t i = 0; i < nparams; i++) THxMEM_release(params[i]);
  THxMEM_pool_drain(mark);
}

/* Return `v` from a direct activation, as an owned reference. */
static Value *THxD_leave(Frame *fr, Value **params, size_t nparams, size_t mark, Value *v) {
  THxMEM_retain(v);
  THxD_reset(fr, params, nparams, mark);
  return v;
}

/* Return the integer `r` from a direct activation with an unboxed result. */
static int64_t THxD_leave_i(Frame *fr, Value **params, size_t nparams, size_t mark, int64_t r) {
  THxD_reset(fr, params, nparams, mark);
  return r;
}

/* The integer an owned boxed result holds, releasing the box. */
static inline int64_t THxI_own(Value *v) {
  int64_t r = v->u.i;
  THxMEM_release(v);
  return r;
}

/* -- the machine --------------------------------------------------------- */

/* Enter a closure `clo`, placing `args[0..nargs)` in the leading local slots
 * (the rest left as boxes on demand). The new frame retains `clo` and its slots
 * retain the args; the outgoing activation is released. nargs: 0 (defer
 * cleanup), 1 (value clause / a call), 2 (handler clause). */
static void enter_clo(BlockFn *cur, Frame **fr, Value **in, Value *clo,
                      Value **args, size_t nargs, int lent) {
  if (clo->tag != T_CLOS) thrax_fault("entering a non-closure");
  size_t code = (size_t)clo->u.clos.code;
  if (code >= THxRT_code_count) thrax_fault("code index out of range");
  Frame *nf = frame_new(THxRT_code_nlocals[code], clo);
  for (size_t i = 0; i < nargs; i++) {
    if (lent && i == 0) {
      nf->locals[0] = args[0];
      nf->lent = 1;
    } else {
      THxK_setlocal(nf, i, args[i]);
    }
  }
  frame_release(*fr);
  *cur = THxRT_code_table[code];
  *fr = nf;
  *in = nargs ? args[0] : NULL;
}

static Value *unit(void) { return THxRT_unit(); }

/* Hand a finished value to the continuation stack (the interpreter's `ret`).
 * Returns 1 to continue the driver loop, 0 when the run's base is reached.
 * Maintains exactly one owned in-flight reference on the value being delivered
 * until a store (box, slot, kont) has taken its own.
 *
 * The returning activation is finished, so it is released first: whatever it
 * still held must not look like a live owner to the clause boundary below. An
 * activation that continues afterwards is held by its own K_RET. */
static int do_ret(BlockFn *cur, Frame **fr, Value **in, Value *v, size_t base) {
  THxMEM_retain(v); /* the in-flight reference */
  frame_release(*fr);
  *fr = NULL;
  for (;;) {
    if (g_kn == base) {
      g_result = v; /* transfer the in-flight reference to the caller */
      return 0;
    }
    KFrame kf = g_kont[--g_kn];
    switch (kf.tag) {
      case K_RET: {
        Value **slot = &kf.u.ret.frame->locals[kf.u.ret.slot];
        *cur = kf.u.ret.cont;
        *fr = kf.u.ret.frame; /* adopt the KRet's frame reference */
        *in = v;
        if (!*slot) {
          *slot = v; /* no box: the slot adopts the in-flight reference */
          return 1;
        }
        THxVALUE_patch_box(*slot, v);
        THxMEM_release(v); /* delivered: the box owns copies of v's children */
        return 1;
      }
      case K_PROMPT: {
        /* body finished normally -> run the value clause on the result */
        enter_clo(cur, fr, in, kf.u.prompt.els, &v, 1, 0);
        kframe_release(&kf); /* els survives via the new frame's clos ref */
        THxMEM_release(v);   /* delivered: retained in the clause's slot 0 */
        return 1;
      }
      case K_DEFER: {
        /* normal completion through a defer: run cleanup, then deliver v */
        KFrame tr;
        tr.tag = K_THUNKRET;
        tr.u.thunkret.saved = v; /* park the in-flight reference in the kont */
        kont_push(tr);
        Value *u = unit();
        enter_clo(cur, fr, in, kf.u.defer.cleanup, &u, 0, 0);
        kframe_release(&kf); /* cleanup survives via the new frame's clos ref */
        return 1;
      }
      case K_THUNKRET:
        THxMEM_release(v);
        v = kf.u.thunkret.saved; /* adopt the kont's reference as in-flight */
        continue;
      case K_AFTERCLAUSE: {
        /* Resumed, or still owned by something other than its boundaries (the
         * clause stored it, captured it, or is returning it): its cleanups run
         * wherever it ends up running. */
        Resump *res = kf.u.afterclause.res;
        if (RES_GET(&res->used) || RES_GET(&res->rc) > RES_GET(&res->nbound)) {
          kframe_release(&kf); /* drop the boundary edge; deliver v unchanged */
          continue;
        }
        /* discarded (abort): run the captured defer cleanups now, on the live
         * stack with enclosing handlers installed, then re-deliver v. */
        KFrame tr;
        tr.tag = K_THUNKRET;
        tr.u.thunkret.saved = v; /* park the in-flight reference in the kont */
        kont_push(tr);
        for (size_t i = 0; i < res->n; i++)
          if (res->seg[i].tag == K_DEFER) {
            KFrame kd;
            kd.tag = K_DEFER;
            kd.u.defer.cleanup = res->seg[i].u.defer.cleanup;
            THxMEM_retain(kd.u.defer.cleanup); /* the kont stores it */
            kont_push(kd);
          }
        kframe_release(&kf); /* the last owner: frees the discarded segment */
        v = unit();          /* start unwinding the freshly pushed KDefers */
        THxMEM_retain(v);    /* the new in-flight reference */
        continue;
      }
      case K_CLEANUPS: {
        /* The clause could not resume, so its slice went at the perform point;
         * its cleanups run here, after the clause body, exactly as the
         * discarded-continuation path above runs them. */
        KFrame tr;
        tr.tag = K_THUNKRET;
        tr.u.thunkret.saved = v;
        kont_push(tr);
        for (size_t i = 0; i < kf.u.cleanups.n; i++) {
          KFrame kd;
          kd.tag = K_DEFER;
          kd.u.defer.cleanup = kf.u.cleanups.cleanups[i];
          THxMEM_retain(kd.u.defer.cleanup);
          kont_push(kd);
        }
        kframe_release(&kf);
        v = unit();
        THxMEM_retain(v);
        continue;
      }
    }
    thrax_fault("do_ret: unhandled continuation frame");
  }
}

/* A fresh builtin value with `arg` appended to `f`'s accumulated operands. */
static Value *builtin_push(Value *f, Value *arg) {
  size_t n = f->u.builtin.nargs;
  Value *nb = alloc_value(T_BUILTIN);
  nb->u.builtin.id = f->u.builtin.id;
  nb->u.builtin.arity = f->u.builtin.arity;
  nb->u.builtin.nargs = n + 1;
  Value **args = THxMEM_cell((n + 1) * sizeof(Value *));
  for (size_t i = 0; i < n; i++) args[i] = f->u.builtin.args[i];
  args[n] = arg;
  for (size_t i = 0; i <= n; i++)
    THxMEM_retain(args[i]); /* the new callee owns its operand row */
  nb->u.builtin.args = args;
  return nb;
}

/* A fresh foreign value with `arg` appended to `f`'s accumulated operands. */
static Value *extern_push(Value *f, Value *arg) {
  size_t n = f->u.ext.nargs;
  Value *ne = alloc_value(T_EXTERN);
  ne->u.ext.idx = f->u.ext.idx;
  ne->u.ext.arity = f->u.ext.arity;
  ne->u.ext.nargs = n + 1;
  Value **args = THxMEM_cell((n + 1) * sizeof(Value *));
  for (size_t i = 0; i < n; i++) args[i] = f->u.ext.args[i];
  args[n] = arg;
  for (size_t i = 0; i <= n; i++) THxMEM_retain(args[i]);
  ne->u.ext.args = args;
  return ne;
}

/* Apply `fn` to `arg` (the interpreter's App dispatch). Returns 1 to continue,
 * 0 when the run completed. Holds owned in-flight references on `fn` and `arg`
 * across the frame switch (either may live in the dying activation): `owned`
 * means the caller handed its own references over, otherwise they are taken
 * here. Only an owned callee can be judged uniquely held. */
Value *THxK_call(Value *f, Value *arg); /* synchronous closure application */

/* `generate template f`: build a tensor the same size as `template`, element i =
 * f(i). Higher-order, so it lives here (not in leaf `run_builtin`) and applies the
 * closure via THxK_call. Mirrors the interpreter's generate. */
static Value *rt_generate(Value *tmpl, Value *f) {
  Value *buf;
  size_t off, *shape, *strides, rank;
  tensor_fields(tmpl, &buf, &off, &shape, &strides, &rank);
  size_t len = rank > 0 ? shape[0] : 0;
  free(shape);
  free(strides);
  Value **items = xmalloc((len ? len : 1) * sizeof(Value *));
  for (size_t i = 0; i < len; i++) {
    Value *iv = THxRT_int((int64_t)i);
    items[i] = THxK_call(f, iv); /* owned */
  }
  Value *v = tensor_stack(items, len); /* retains each item's contents */
  for (size_t i = 0; i < len; i++) THxMEM_release(items[i]); /* drop generate's ref */
  free(items);
  return v;
}

/* `@array_push` on a byte vector the caller moved in (`s` came from a local's
 * last use, so the temp pool holds the reference the slot gave up). When that is
 * the only reference the push grows `s` in place, doubling its capacity, and
 * returns `s` itself, which the consumer retains like any prim result. A buffer
 * built by pushing is then linear instead of quadratic. */
Value *THxK_array_push_owned(Value *s, Value *b) {
  if (s->tag != T_STR || !THxMEM_unique(s)) return run_builtin(THxB_ARRAY_PUSH, (Value *[]){s, b});
  uint8_t byte = as_byte(b);
  size_t len = s->u.str.len;
  if (len == s->u.str.cap) {
    size_t cap = len < 8 ? 16 : 2 * len;
    uint8_t *data = realloc(s->u.str.data, cap + 1);
    if (!data) thrax_fault("out of memory");
    s->u.str.data = data;
    s->u.str.cap = cap;
  }
  s->u.str.data[len] = byte;
  s->u.str.data[len + 1] = 0;
  s->u.str.len = len + 1;
  return s;
}

/* A built-in applied to all `n` of its operands at once, which are borrowed.
 * The result is fresh or one of the operands' children, as through do_apply. */
Value *THxK_prim(int id, Value **a) {
  if (id == THxB_TENSOR_CREATE) return rt_generate(a[0], a[1]);
  return run_builtin(id, a);
}
/* THxK_prim for `@vec_push` / `@vec_set`. `owned` says the caller moved its
 * first operand out of a slot or captured field (the temp pool now holds that
 * reference), rather than borrowing one that may still be read. */
Value *THxK_vec_update(int id, Value **a, int owned) {
  if (!is_vec_update(id)) thrax_fault("vec_update: not a vector update");
  return vec_update(id, a, owned);
}

static int do_apply(BlockFn *cur, Frame **fr, Value **in, Value *fn, Value *arg,
                    size_t base, int owned, int lent) {
  if (!fn) thrax_fault("apply: null callee");
  if (!owned) {
    THxMEM_retain(fn);
    THxMEM_retain(arg);
  }
  int r = 0;
  switch (fn->tag) {
    case T_CLOS:
      enter_clo(cur, fr, in, fn, &arg, 1, lent);
      r = 1;
      break;
    case T_BUILTIN: {
      size_t nn = fn->u.builtin.nargs;
      if (nn + 1 == fn->u.builtin.arity) {
        /* borrow the operand row: fn (retained) keeps args[0..nn) alive, and
         * arg is retained; run_builtin only reads them and returns fresh (or a
         * child do_ret then retains). A partial application this call solely
         * owns gives up its row, so its vector may be updated in place; one
         * still held elsewhere (`let f = @vec_push v in (f 1, f 2)`) may not. */
        Value **args = THxMEM_cell((nn + 1) * sizeof(Value *));
        for (size_t i = 0; i < nn; i++) args[i] = fn->u.builtin.args[i];
        args[nn] = arg;
        int id = fn->u.builtin.id;
        Value *res = nn > 0 && owned && is_vec_update(id) && THxMEM_unique(fn)
                         ? vec_update(id, args, 1)
                         : THxK_prim(id, args);
        THxMEM_cell_free(args, (nn + 1) * sizeof(Value *));
        r = do_ret(cur, fr, in, res, base);
      } else {
        r = do_ret(cur, fr, in, builtin_push(fn, arg), base);
      }
      break;
    }
    case T_EXTERN: {
      size_t nn = fn->u.ext.nargs;
      if (nn + 1 == fn->u.ext.arity) {
        if ((size_t)fn->u.ext.idx >= THxRT_extern_count)
          thrax_fault("extern index out of range");
        Value **args = THxMEM_cell((nn + 1) * sizeof(Value *));
        for (size_t i = 0; i < nn; i++) args[i] = fn->u.ext.args[i];
        args[nn] = arg;
        Value *res = THxRT_extern_table[fn->u.ext.idx](args);
        THxMEM_cell_free(args, (nn + 1) * sizeof(Value *));
        r = do_ret(cur, fr, in, res, base);
      } else {
        r = do_ret(cur, fr, in, extern_push(fn, arg), base);
      }
      break;
    }
    case T_OP: {
      /* perform: find the nearest prompt (above `base`) with a clause for this
       * op, capture the slice [prompt, here] (INCLUDING the prompt -> deep
       * handler), truncate to below the prompt, and run the clause there. */
      const char *eff = fn->u.op.effect;
      const char *op = fn->u.op.op;
      size_t p = g_kn;
      Value *clause = NULL;
      int uses = RESUME_CAPTURED;
      for (size_t i = g_kn; i-- > base;) {
        if (g_kont[i].tag != K_PROMPT) continue;
        for (size_t j = 0; j < g_kont[i].u.prompt.n; j++) {
          const char *ceff = g_kont[i].u.prompt.effs[j];
          const char *cop = g_kont[i].u.prompt.ops[j];
          bool m = same_name(cop, op) && (!(eff && ceff) || same_name(eff, ceff));
          if (m) {
            clause = g_kont[i].u.prompt.clauses[j];
            uses = g_kont[i].u.prompt.uses[j];
            p = i;
            break;
          }
        }
        if (clause) break;
      }
      if (!clause) thrax_fault("unhandled effect operation");

      size_t n = g_kn - p;
      Value *kval;
      if (uses == RESUME_NEVER) {
        /* The clause cannot resume: keep the slice's cleanups (they still run
         * after the clause body) and release the frames here, rather than
         * capturing a slice only to discover at the boundary that it was
         * abandoned. The prompt being released owns the clause closure, so hold
         * a reference across it. */
        THxMEM_retain(clause);
        KFrame cl = segment_cleanups(&g_kont[p], n);
        for (size_t i = 0; i < n; i++) kframe_release(&g_kont[p + i]);
        g_kn = p;
        if (cl.u.cleanups.n)
          kont_push(cl);
        else
          kframe_release(&cl); /* nothing deferred: just the empty array */
        kval = unit();
      } else {
        /* The slice's kont frames MOVE into the segment (their owned references
         * move with them; no counts change). */
        Resump *seg = THxMEM_alloc(sizeof(Resump));
        seg->rc = 2; /* the kval below, and the clause boundary */
        seg->nbound = 1;
        seg->seg = THxMEM_alloc((n ? n : 1) * sizeof(KFrame));
        memcpy(seg->seg, &g_kont[p], n * sizeof(KFrame));
        seg->n = n;
        seg->used = 0;
        seg->shared = 0;
        g_kn = p; /* the clause runs below the prompt (outside it) */

        kval = alloc_value(T_RESUMP);
        kval->u.resump = seg;
        KFrame ac;
        ac.tag = K_AFTERCLAUSE;
        ac.u.afterclause.res = seg;
        kont_push(ac);
      }

      Value *args[2];
      args[0] = arg;
      args[1] = kval;
      enter_clo(cur, fr, in, clause, args, 2, 0);
      if (uses == RESUME_NEVER) THxMEM_release(clause); /* the frame holds it now */
      r = 1;
      break;
    }
    case T_RESUMP: {
      /* resume: splice the captured slice back on and deliver arg. When this
       * call holds the only reference to the resumption, nothing can resume
       * it again, so the frames MOVE back and the emptied segment stays behind
       * as a husk for its boundary; otherwise splice a private copy, leaving
       * the slice resumable by its other owners. */
      Resump *seg = fn->u.resump;
      if (!seg->seg) thrax_fault("resumed a continuation that was moved out");
      RES_SET(&seg->used, 1);
      if (owned && !RES_GET(&seg->shared) && fn->rc == 1 &&
          RES_GET(&seg->rc) == RES_GET(&seg->nbound) + 1) {
        for (size_t i = 0; i < seg->n; i++) kont_push(seg->seg[i]);
        THxMEM_free(seg->seg);
        seg->seg = NULL;
        seg->n = 0;
      } else {
        KFrame *copy = xmalloc((seg->n ? seg->n : 1) * sizeof(KFrame));
        segment_copy(seg, copy);
        for (size_t i = 0; i < seg->n; i++) kont_push(copy[i]);
        free(copy);
      }
      r = do_ret(cur, fr, in, arg, base);
      break;
    }
    default:
      thrax_fault("apply: callee is not a function");
  }
  THxMEM_release(fn);
  if (!lent) THxMEM_release(arg);
  return r;
}

static Value *run_loop(BlockFn cur, Frame *fr, Value *in, size_t base) {
  for (;;) {
    size_t pm = THxMEM_pool_mark(); /* this bounce's temporaries */
    g_act = ACT_NONE;
    cur(fr, in);
    int cont = 1;
    /* The block is done, so its temporaries die before the terminator acts:
     * the counts the terminator reads (a resumption's uniqueness, a clause
     * boundary's liveness test) then see only real owners. */
    switch (g_act) {
      case ACT_JUMP: cur = g_blk; break;
      case ACT_RET:
        THxMEM_retain(g_v);
        THxMEM_pool_drain(pm);
        cont = do_ret(&cur, &fr, &in, g_v, base);
        THxMEM_release(g_v);
        break;
      case ACT_APPLY: {
        /* A lent argument outlives the callee, so it needs no in-flight
         * reference across the switch; one the callee does not borrow is
         * counted as usual. */
        int lent = g_lend && g_fn && g_fn->tag == T_CLOS &&
                   (size_t)g_fn->u.clos.code < THxRT_code_count &&
                   (THxRT_code_borrowed[g_fn->u.clos.code] & 1);
        THxMEM_retain(g_fn);
        if (!lent) THxMEM_retain(g_arg);
        THxMEM_pool_drain(pm);
        cont = do_apply(&cur, &fr, &in, g_fn, g_arg, base, 1, lent);
        break;
      }
      case ACT_NONE: thrax_fault("block set no terminator");
    }
    /* Everything this bounce constructed and did not store dies here; what was
     * stored (slot, kont, box children, g_result) took its own reference. */
    THxMEM_pool_drain(pm);
    if (!cont) return g_result;
  }
}

Value *THxK_run_code(size_t code) {
  if (code >= THxRT_code_count) thrax_fault("run_code: code out of range");
  size_t base = g_kn;
  Frame *fr = frame_new(THxRT_code_nlocals[code], NULL);
  return run_loop(THxRT_code_table[code], fr, NULL, base);
}

Value *THxK_call(Value *f, Value *arg) {
  size_t base = g_kn;
  BlockFn cur = NULL;
  Frame *fr = NULL;
  Value *in = NULL;
  if (!do_apply(&cur, &fr, &in, f, arg, base, 0, 0)) return g_result;
  return run_loop(cur, fr, in, base);
}

/* As THxK_call, but consuming the caller's references to `f` and `arg`, so a
 * callee nobody else holds is entered uniquely owned. */
static Value *THxK_call_owned(Value *f, Value *arg) {
  size_t base = g_kn;
  BlockFn cur = NULL;
  Frame *fr = NULL;
  Value *in = NULL;
  if (!do_apply(&cur, &fr, &in, f, arg, base, 1, 0)) return g_result;
  return run_loop(cur, fr, in, base);
}

/* Free the runtime's static scaffolding (the kont stack, temp pool, and dead
 * worklist) at exit. Not value lifetime, so untracked by the leak check; call
 * it after the check for pristine teardown. */
void THxRT_shutdown(void) {
  free(g_kont);
  g_kont = NULL;
  g_kcap = g_kn = 0;
  free(g_pool);
  g_pool = NULL;
  g_pcap = g_pn = 0;
  free(g_dead);
  g_dead = NULL;
  g_dcap = g_dn = 0;
  THxMEM_cells_release();
  stm_free_log();
}

#ifdef THRAX_THREADS
/*------------------------------------------------------------------------------
 *\PARALLEL TASKS: shared marking, blackholed thunks, a work-stealing pool
 *-----------------------------------------------------------------------------*/

/* Every immortal value, for the sweep at exit. */
static Value **g_immortal = NULL;
static size_t g_imn = 0, g_imcap = 0;
static pthread_mutex_t g_immortal_mu = PTHREAD_MUTEX_INITIALIZER;

/* Raise `root` and everything it reaches to `level` (Koka / Lean 4 style),
 * before it becomes reachable from another thread. A local value is held by
 * this thread alone until then, so the plain writes here are published by
 * whatever hands it over (a deque lock, a release store). A value already at
 * `level` ends the walk: everything below it is at least there too. */
static void mark_level(Value *root, uint8_t level) {
  if (!root || sh_load(root) >= level) return;
  size_t n = 0, cap = 64;
  Value **work = xmalloc(cap * sizeof(Value *));
  work[n++] = root;
#define THX_MARK_PUSH(c)                                         \
  do {                                                           \
    Value *c_ = (c);                                             \
    if (c_ && sh_load(c_) < level) {                             \
      if (n == cap) {                                            \
        cap *= 2;                                                \
        work = realloc(work, cap * sizeof(Value *));             \
        if (!work) thrax_fault("mark worklist allocation failed"); \
      }                                                          \
      work[n++] = c_;                                            \
    }                                                            \
  } while (0)
  if (level == SH_IMMORTAL) pthread_mutex_lock(&g_immortal_mu);
  while (n > 0) {
    Value *v = work[--n];
    if (sh_load(v) >= level) continue;
    __atomic_store_n(&v->shared, level, __ATOMIC_RELAXED);
    /* A shared value turned immortal keeps its negative count: other threads
     * may be counting it right now. They reach the slow path either way, and
     * the reference the global holds keeps the count from reaching zero. */
    int rc = rc_load(v);
    if (rc > 0) __atomic_store_n(&v->rc, level == SH_IMMORTAL ? 0 : -rc, __ATOMIC_RELAXED);
    if (level == SH_IMMORTAL) {
      if (g_imn == g_imcap) {
        g_imcap = g_imcap ? g_imcap * 2 : 256;
        g_immortal = realloc(g_immortal, g_imcap * sizeof(Value *));
        if (!g_immortal) thrax_fault("immortal table allocation failed");
      }
      g_immortal[g_imn++] = v;
    }
    switch (v->tag) {
      case T_TUPLE:
      case T_VEC:
        for (size_t i = 0; i < v->u.seq.len; i++) THX_MARK_PUSH(v->u.seq.items[i]);
        break;
      case T_STRUCT:
        for (size_t i = 0; i < v->u.strct.len; i++)
          THX_MARK_PUSH(v->u.strct.fields[i].val);
        break;
      case T_VARIANT:
        for (size_t i = 0; i < v->u.variant.len; i++)
          THX_MARK_PUSH(v->u.variant.fields[i]);
        break;
      case T_CLOS:
        for (size_t i = 0; i < v->u.clos.nenv; i++) THX_MARK_PUSH(v->u.clos.env[i]);
        break;
      case T_BUILTIN:
        for (size_t i = 0; i < v->u.builtin.nargs; i++)
          THX_MARK_PUSH(v->u.builtin.args[i]);
        break;
      case T_EXTERN:
        for (size_t i = 0; i < v->u.ext.nargs; i++) THX_MARK_PUSH(v->u.ext.args[i]);
        break;
      case T_THUNK:
      case T_BLACKHOLE: THX_MARK_PUSH(v->u.thunk); break;
      /* A continuation crosses threads by value: everything its frames hold
       * is marked, and the slice is flagged so a resume copies it rather than
       * moving frames another thread may be reading. A slice that a boundary
       * frame of it names (a continuation captured inside another) is marked
       * the same way. */
      case T_RESUMP: {
        Resump *stack[64];
        size_t top = 0;
        stack[top++] = v->u.resump;
        while (top > 0) {
          Resump *r = stack[--top];
          if (RES_GET(&r->shared)) continue;
          RES_SET(&r->shared, 1);
          for (size_t i = 0; r->seg && i < r->n; i++) {
            KFrame *kf = &r->seg[i];
            switch (kf->tag) {
              case K_RET:
                THX_MARK_PUSH(kf->u.ret.frame->clos);
                for (size_t j = 0; j < kf->u.ret.frame->nlocals; j++)
                  THX_MARK_PUSH(kf->u.ret.frame->locals[j]);
                break;
              case K_PROMPT:
                for (size_t j = 0; j < kf->u.prompt.n; j++)
                  THX_MARK_PUSH(kf->u.prompt.clauses[j]);
                THX_MARK_PUSH(kf->u.prompt.els);
                break;
              case K_DEFER: THX_MARK_PUSH(kf->u.defer.cleanup); break;
              case K_THUNKRET: THX_MARK_PUSH(kf->u.thunkret.saved); break;
              case K_CLEANUPS:
                for (size_t j = 0; j < kf->u.cleanups.n; j++)
                  THX_MARK_PUSH(kf->u.cleanups.cleanups[j]);
                break;
              case K_AFTERCLAUSE:
                if (top == 64) thrax_fault("continuations nested too deeply to share");
                stack[top++] = kf->u.afterclause.res;
                break;
            }
          }
        }
        break;
      }
      /* A future's task is counted on its own and its result is shared when
       * it lands, so the walk stops at the handle. */
      default: break;
    }
  }
  if (level == SH_IMMORTAL) pthread_mutex_unlock(&g_immortal_mu);
#undef THX_MARK_PUSH
  free(work);
}

static void mark_shared(Value *v) { mark_level(v, SH_SHARED); }
static void mark_immortal(Value *v) { mark_level(v, SH_IMMORTAL); }

/* Free every immortal value, once nothing else can reach them: after the
 * workers are joined and every counted value is gone. All payloads are
 * released before any cell is freed, since immortal values point at each
 * other (releasing an immortal child is a no-op, but it reads the child). */
static void THxPAR_free_immortals(void) {
  for (size_t i = 0; i < g_imn; i++) payload_destroy(g_immortal[i]);
  for (size_t i = 0; i < g_imn; i++) THxMEM_free(g_immortal[i]);
  free(g_immortal);
  g_immortal = NULL;
  g_imn = g_imcap = 0;
}

/* Claims a shared thunk for forcing. A compare-and-swap would do, but a failed
 * one counts as a write to the sanitizer, racing the winner's plain reads of
 * the tag it then sets; under the lock a losing claim only loads. */
static pthread_mutex_t g_claim_mu = PTHREAD_MUTEX_INITIALIZER;

static int claim_thunk(Value *v) {
  pthread_mutex_lock(&g_claim_mu);
  int won = __atomic_load_n(&v->tag, __ATOMIC_ACQUIRE) == T_THUNK;
  if (won) __atomic_store_n(&v->tag, (uint8_t)T_BLACKHOLE, __ATOMIC_RELAXED);
  pthread_mutex_unlock(&g_claim_mu);
  return won;
}

/* Force a shared lazy slot. The first thread to see T_THUNK swaps in
 * T_BLACKHOLE and evaluates; the others wait for the tag to change. The payload
 * is written before the tag (release), so a reader that sees the new tag (its
 * acquire load) sees the value. */
static Value *force_shared(Value *v) {
  for (;;) {
    uint8_t tag = __atomic_load_n(&v->tag, __ATOMIC_ACQUIRE);
    if (tag == T_BLACKHOLE) {
      sched_yield();
      continue;
    }
    if (tag != T_THUNK) return v;
    if (!claim_thunk(v)) continue;
    Value *f = v->u.thunk; /* the cell's reference, dropped once replaced */
    Value *forced = THxK_call(f, THxRT_unit()); /* owned */
    if (forced == v) {
      __atomic_store_n(&v->tag, (uint8_t)T_THUNK, __ATOMIC_RELEASE);
      THxMEM_release(forced);
      return v;
    }
    /* An immortal cell's old closure stays in the immortal table, which
     * frees it at exit. */
    if (sh_load(v) == SH_IMMORTAL)
      mark_immortal(forced);
    else
      mark_shared(forced);
    Value tmp;
    copy_payload(&tmp, v, forced);
    v->u = tmp.u;
    __atomic_store_n(&v->tag, tmp.tag, __ATOMIC_RELEASE);
    THxMEM_release(f);
    THxMEM_release(forced);
  }
}

static void task_destroy(Task *t) {
  if (__atomic_sub_fetch(&t->rc, 1, __ATOMIC_ACQ_REL) > 0) return;
  THxMEM_release(t->fn);
  THxMEM_release(t->result);
  THxMEM_free(t);
}

/* One deque per thread, `[0]` for the main thread (and any thread the pool did
 * not start). The owner pushes and pops at `bot` (newest first); thieves take
 * from `top` (oldest first). Entries are futures, each holding a reference. */
typedef struct {
  pthread_mutex_t mu;
  Value **buf;
  size_t top, bot, cap; /* live entries buf[i % cap] for top <= i < bot */
} Deque;

static Deque *g_deques = NULL;
static int g_nthreads = 0;
static pthread_t *g_workers = NULL;
static int g_mt_started = 0;
static int g_mt_stop = 0;
/* Entries across all deques, possibly stale (already claimed). Sleepers test it
 * under `g_mt_mu`, and every push or completion broadcasts under it, so no
 * wakeup is lost. */
static size_t g_queued = 0;
static pthread_mutex_t g_mt_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_mt_cv = PTHREAD_COND_INITIALIZER;
static THX_TLS int t_self = 0;

static void mt_wake_all(void) {
  pthread_mutex_lock(&g_mt_mu);
  pthread_cond_broadcast(&g_mt_cv);
  pthread_mutex_unlock(&g_mt_mu);
}

static void deque_push(Deque *d, Value *fut) {
  pthread_mutex_lock(&d->mu);
  if (d->bot - d->top == d->cap) {
    size_t ncap = d->cap ? d->cap * 2 : 64;
    Value **nb = xmalloc(ncap * sizeof(Value *));
    for (size_t i = d->top; i < d->bot; i++) nb[i - d->top] = d->buf[i % d->cap];
    free(d->buf);
    d->bot -= d->top;
    d->top = 0;
    d->buf = nb;
    d->cap = ncap;
  }
  d->buf[d->bot++ % d->cap] = fut;
  pthread_mutex_unlock(&d->mu);
}

static Value *deque_take(Deque *d, int steal) {
  Value *w = NULL;
  pthread_mutex_lock(&d->mu);
  if (d->bot > d->top) w = steal ? d->buf[d->top++ % d->cap] : d->buf[--d->bot % d->cap];
  pthread_mutex_unlock(&d->mu);
  return w;
}

static Value *mt_find_work(void) {
  Value *w = deque_take(&g_deques[t_self], 0);
  for (int i = 1; !w && i < g_nthreads; i++)
    w = deque_take(&g_deques[(t_self + i) % g_nthreads], 1);
  if (w) __atomic_fetch_sub(&g_queued, 1, __ATOMIC_ACQ_REL);
  return w;
}

static int task_claim(Task *t) {
  int expect = TASK_PENDING;
  return __atomic_compare_exchange_n(&t->state, &expect, TASK_RUNNING, 0,
                                     __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE);
}

/* Run a claimed task. The result is marked before it is published: every
 * thread awaiting the future takes references on it. */
/* `@spawn` tasks not finished yet, which the program waits for at exit. */
static long long g_io_pending = 0;

static void task_exec(Task *t) {
  Value *fn = t->fn;
  Value *r = THxK_call(fn, THxRT_unit()); /* owned, now by the task */
  mark_shared(r);
  t->result = r;
  t->fn = NULL;
  __atomic_store_n(&t->state, TASK_DONE, __ATOMIC_RELEASE);
  if (t->io) __atomic_fetch_sub(&g_io_pending, 1, __ATOMIC_ACQ_REL);
  mt_wake_all();
  THxMEM_release(fn);
}

/* A deque entry: run it unless its awaiter already claimed it, then drop the
 * deque's reference. */
static void mt_run_entry(Value *fut) {
  Task *t = fut->u.future;
  if (task_claim(t)) task_exec(t);
  THxMEM_release(fut);
}

static void *worker_main(void *arg) {
  t_self = (int)(intptr_t)arg;
  for (;;) {
    Value *w = mt_find_work();
    if (w) {
      mt_run_entry(w);
      THxMEM_pool_drain(0);
      continue;
    }
    pthread_mutex_lock(&g_mt_mu);
    while (!g_mt_stop && __atomic_load_n(&g_queued, __ATOMIC_ACQUIRE) == 0)
      pthread_cond_wait(&g_mt_cv, &g_mt_mu);
    int stop = g_mt_stop;
    pthread_mutex_unlock(&g_mt_mu);
    if (stop) break;
  }
  THxRT_shutdown();
  __atomic_fetch_add(&g_live_folded, g_live, __ATOMIC_ACQ_REL);
  return NULL;
}

/* Provided by the generated program: marks every forced global immortal. */
static void THxRT_share_globals(void);

/* Start the pool on the first `@par`: `THRAX_THREADS` threads in all (default:
 * one per online core), the calling thread being one of them. Until then no
 * other thread exists, so marking the globals here cannot race. */
static void mt_start(void) {
  if (__atomic_load_n(&g_mt_started, __ATOMIC_ACQUIRE)) return;
  pthread_mutex_lock(&g_mt_mu);
  if (!g_mt_started) {
    const char *env = getenv("THRAX_THREADS");
    int n = env ? atoi(env) : 0;
    if (n <= 0) n = thx_nprocs();
    if (n <= 0) n = 1;
    g_nthreads = n;
    g_deques = xmalloc((size_t)n * sizeof(Deque));
    for (int i = 0; i < n; i++) {
      pthread_mutex_init(&g_deques[i].mu, NULL);
      g_deques[i].buf = NULL;
      g_deques[i].top = g_deques[i].bot = g_deques[i].cap = 0;
    }
    THxRT_share_globals();
    g_workers = xmalloc((size_t)n * sizeof(pthread_t));
    for (int i = 1; i < n; i++)
      if (pthread_create(&g_workers[i], NULL, worker_main, (void *)(intptr_t)i) != 0)
        thrax_fault("cannot start a worker thread");
    __atomic_store_n(&g_mt_started, 1, __ATOMIC_RELEASE);
  }
  pthread_mutex_unlock(&g_mt_mu);
}

static Value *spawn_task(Value *f, int io) {
  mt_start();
  Task *t = THxMEM_alloc(sizeof(Task));
  t->rc = 1;
  t->state = TASK_PENDING;
  t->io = io;
  if (io) __atomic_fetch_add(&g_io_pending, 1, __ATOMIC_ACQ_REL);
  mark_shared(f);
  THxMEM_retain(f);
  t->fn = f;
  Value *fut = alloc_value(T_FUTURE);
  fut->u.future = t;
  fut->shared = SH_SHARED;
  fut->rc = -fut->rc;
  THxMEM_retain(fut); /* the deque's reference */
  deque_push(&g_deques[t_self], fut);
  __atomic_fetch_add(&g_queued, 1, __ATOMIC_ACQ_REL);
  mt_wake_all();
  return fut;
}

static Value *THxPAR_spawn(Value *f) { return spawn_task(f, 0); }
static Value *THxPAR_spawn_io(Value *f) { return spawn_task(f, 1); }

/* Wait for a future. A task nobody has started yet runs right here; otherwise
 * the waiter helps with queued work until its task is done, so a pool whose
 * every thread awaits cannot deadlock. */
static Value *THxPAR_await(Value *fut) {
  fut = THxVALUE_force(fut);
  if (fut->tag != T_FUTURE) thrax_fault("@await expects a future");
  Task *t = fut->u.future;
  for (;;) {
    int st = __atomic_load_n(&t->state, __ATOMIC_ACQUIRE);
    if (st == TASK_DONE) return t->result;
    if (st == TASK_PENDING && task_claim(t)) {
      task_exec(t);
      continue;
    }
    Value *w = mt_find_work();
    if (w) {
      mt_run_entry(w);
      continue;
    }
    pthread_mutex_lock(&g_mt_mu);
    while (__atomic_load_n(&t->state, __ATOMIC_ACQUIRE) != TASK_DONE &&
           __atomic_load_n(&g_queued, __ATOMIC_ACQUIRE) == 0)
      pthread_cond_wait(&g_mt_cv, &g_mt_mu);
    pthread_mutex_unlock(&g_mt_mu);
  }
}

/* A token unique to the calling thread. */
static void *THxPAR_self(void) { return (void *)&t_self; }

/* One step of waiting on another thread: run a queued task, or yield. */
static void THxPAR_help(void) {
  Value *w = __atomic_load_n(&g_mt_started, __ATOMIC_ACQUIRE) ? mt_find_work() : NULL;
  if (w)
    mt_run_entry(w);
  else
    sched_yield();
}

/* Stop and join the workers, then drop the tasks nobody ran (pure, so their
 * results were unobservable). Runs on the main thread before the leak check. */
static void THxPAR_shutdown(void) {
  if (!g_mt_started) return;
  pthread_mutex_lock(&g_mt_mu);
  g_mt_stop = 1;
  pthread_cond_broadcast(&g_mt_cv);
  pthread_mutex_unlock(&g_mt_mu);
  for (int i = 1; i < g_nthreads; i++) pthread_join(g_workers[i], NULL);
  for (int i = 0; i < g_nthreads; i++) {
    Value *w;
    while ((w = deque_take(&g_deques[i], 1))) THxMEM_release(w);
    free(g_deques[i].buf);
    pthread_mutex_destroy(&g_deques[i].mu);
  }
  free(g_deques);
  free(g_workers);
  g_deques = NULL;
  g_workers = NULL;
  g_queued = 0;
  g_mt_started = 0;
  g_mt_stop = 0;
}
/* Wait for every `@spawn` task to finish, helping with queued work: a program
 * ends after the tasks it spawned. */
static void THxPAR_join_io(void) {
  while (__atomic_load_n(&g_io_pending, __ATOMIC_ACQUIRE) > 0) {
    Value *w = mt_find_work();
    if (w) {
      mt_run_entry(w);
      continue;
    }
    pthread_mutex_lock(&g_mt_mu);
    while (__atomic_load_n(&g_io_pending, __ATOMIC_ACQUIRE) > 0 &&
           __atomic_load_n(&g_queued, __ATOMIC_ACQUIRE) == 0)
      pthread_cond_wait(&g_mt_cv, &g_mt_mu);
    pthread_mutex_unlock(&g_mt_mu);
  }
}
#else
/* Without threads a task waits here, oldest first, until something needs it:
 * an `@await` of its future, a receive or a retry that cannot go on, or the end
 * of the program. That is the interpreter's order, so both engines agree. Each
 * entry is a future holding a reference. */
static Value **g_tasks = NULL;
static size_t g_task_head = 0, g_task_len = 0, g_task_cap = 0;

static void task_destroy(Task *t) {
  if (--t->rc > 0) return;
  THxMEM_release(t->fn);
  THxMEM_release(t->result);
  THxMEM_free(t);
}

static void task_exec(Task *t) {
  Value *fn = t->fn;
  t->state = TASK_RUNNING;
  t->fn = NULL;
  t->result = THxK_call(fn, THxRT_unit()); /* owned, now by the task */
  t->state = TASK_DONE;
  THxMEM_release(fn);
}

static Value *spawn_task(Value *f) {
  Task *t = THxMEM_alloc(sizeof(Task));
  t->rc = 1;
  t->state = TASK_PENDING;
  t->io = 0;
  THxMEM_retain(f);
  t->fn = f;
  t->result = NULL;
  Value *fut = alloc_value(T_FUTURE);
  fut->u.future = t;
  if (g_task_len == g_task_cap) {
    size_t ncap = g_task_cap ? g_task_cap * 2 : 8;
    Value **nb = xmalloc(ncap * sizeof(Value *));
    for (size_t i = 0; i < g_task_len; i++) nb[i] = g_tasks[(g_task_head + i) % g_task_cap];
    free(g_tasks);
    g_tasks = nb;
    g_task_head = 0;
    g_task_cap = ncap;
  }
  THxMEM_retain(fut); /* the queue's reference */
  g_tasks[(g_task_head + g_task_len++) % g_task_cap] = fut;
  return fut;
}

static Value *THxPAR_spawn(Value *f) { return spawn_task(f); }
static Value *THxPAR_spawn_io(Value *f) { return spawn_task(f); }

/* Run the oldest task nobody has started; 0 when there is none. */
static int run_next_task(void) {
  while (g_task_len > 0) {
    Value *fut = g_tasks[g_task_head];
    g_task_head = (g_task_head + 1) % g_task_cap;
    g_task_len--;
    Task *t = fut->u.future;
    int ran = t->state == TASK_PENDING;
    if (ran) task_exec(t);
    THxMEM_release(fut);
    if (ran) return 1;
  }
  return 0;
}

static Value *THxPAR_await(Value *fut) {
  fut = THxVALUE_force(fut);
  if (fut->tag != T_FUTURE) return fut;
  Task *t = fut->u.future;
  if (t->state == TASK_RUNNING) thrax_fault("a task awaits its own result");
  if (t->state == TASK_PENDING) task_exec(t);
  return t->result;
}

/* Drop the tasks nobody ran (the `Show` entry evaluates one global and stops). */
static void THxPAR_shutdown(void) {
  while (g_task_len > 0) {
    THxMEM_release(g_tasks[g_task_head]);
    g_task_head = (g_task_head + 1) % g_task_cap;
    g_task_len--;
  }
  free(g_tasks);
  g_tasks = NULL;
  g_task_cap = 0;
}
static void THxPAR_free_immortals(void) {}
/* A program ends after the tasks it spawned. */
static void THxPAR_join_io(void) {
  while (run_next_task()) {
  }
}
#endif

/*------------------------------------------------------------------------------
 *\CHANNELS AND TRANSACTIONAL VARIABLES (#283)
 *-----------------------------------------------------------------------------*/

/* Whether waiting on another thread can make progress: some other task exists
 * that might send, or commit. Without one, the waiter is deadlocked. */
static int others_may_run(void) {
#ifdef THRAX_THREADS
  return __atomic_load_n(&g_mt_started, __ATOMIC_ACQUIRE);
#else
  return 0;
#endif
}

static void chan_release(Chan *c) {
  if (!obj_release(&c->rc)) return;
  for (size_t i = 0; i < c->len; i++) THxMEM_release(c->buf[(c->head + i) % c->cap]);
  free(c->buf);
#ifdef THRAX_THREADS
  pthread_mutex_destroy(&c->mu);
#endif
  THxMEM_free(c);
}

static Chan *as_chan(Value *v) {
  v = THxVALUE_force(v);
  if (v->tag != T_CHAN) thrax_fault("expected a channel");
  return v->u.chan;
}

static Value *THxCHAN_new(void) {
  Chan *c = THxMEM_alloc(sizeof(Chan));
  c->rc = 1;
#ifdef THRAX_THREADS
  pthread_mutex_init(&c->mu, NULL);
#endif
  Value *v = alloc_value(T_CHAN);
  v->u.chan = c;
  return v;
}

/* A sent value may be received on any thread, so it is marked shared first. */
static Value *THxCHAN_send(Value *ch, Value *v) {
  Chan *c = as_chan(ch);
#ifdef THRAX_THREADS
  mark_shared(v);
#endif
  THxMEM_retain(v);
#ifdef THRAX_THREADS
  pthread_mutex_lock(&c->mu);
#endif
  if (c->len == c->cap) {
    size_t ncap = c->cap ? c->cap * 2 : 8;
    Value **nb = xmalloc(ncap * sizeof(Value *));
    for (size_t i = 0; i < c->len; i++) nb[i] = c->buf[(c->head + i) % c->cap];
    free(c->buf);
    c->buf = nb;
    c->head = 0;
    c->cap = ncap;
  }
  c->buf[(c->head + c->len) % c->cap] = v;
#ifdef THRAX_THREADS
  __atomic_store_n(&c->len, c->len + 1, __ATOMIC_RELEASE);
  pthread_mutex_unlock(&c->mu);
  if (others_may_run()) mt_wake_all();
#else
  c->len++;
#endif
  return THxRT_unit();
}

/* Take the oldest message; while there is none, help with queued tasks (one
 * of them may be the sender), and sleep when there is nothing to help with. */
static Value *THxCHAN_recv(Value *ch) {
  Chan *c = as_chan(ch);
  for (;;) {
#ifdef THRAX_THREADS
    pthread_mutex_lock(&c->mu);
#endif
    if (c->len > 0) {
      Value *v = c->buf[c->head];
      c->head = (c->head + 1) % c->cap;
#ifdef THRAX_THREADS
      __atomic_store_n(&c->len, c->len - 1, __ATOMIC_RELEASE);
      pthread_mutex_unlock(&c->mu);
#else
      c->len--;
#endif
      pool_adopt(v); /* the channel's reference */
      return v;
    }
#ifdef THRAX_THREADS
    pthread_mutex_unlock(&c->mu);
#endif
#ifndef THRAX_THREADS
    if (run_next_task()) continue;
#endif
    if (!others_may_run())
      thrax_fault("deadlock: receiving from a channel that no task will send to");
#ifdef THRAX_THREADS
    Value *w = mt_find_work();
    if (w) {
      mt_run_entry(w);
      continue;
    }
    pthread_mutex_lock(&g_mt_mu);
    while (__atomic_load_n(&c->len, __ATOMIC_ACQUIRE) == 0 &&
           __atomic_load_n(&g_queued, __ATOMIC_ACQUIRE) == 0)
      pthread_cond_wait(&g_mt_cv, &g_mt_mu);
    pthread_mutex_unlock(&g_mt_mu);
#endif
  }
}

#ifdef THRAX_THREADS
static pthread_mutex_t g_stm_mu = PTHREAD_MUTEX_INITIALIZER;
#define STM_LOCK() pthread_mutex_lock(&g_stm_mu)
#define STM_UNLOCK() pthread_mutex_unlock(&g_stm_mu)
#else
#define STM_LOCK() ((void)0)
#define STM_UNLOCK() ((void)0)
#endif

static void tvar_release(TVar *tv) {
  if (!obj_release(&tv->rc)) return;
  THxMEM_release(tv->val);
  THxMEM_free(tv);
}

static TVar *as_tvar(Value *v) {
  v = THxVALUE_force(v);
  if (v->tag != T_TVAR) thrax_fault("expected a transactional variable");
  return v->u.tvar;
}

/* A transactional variable's value may be read on any thread, so whatever
 * goes into one is marked shared first. */
static Value *THxTVAR_new(Value *v) {
#ifdef THRAX_THREADS
  mark_shared(v);
#endif
  THxMEM_retain(v);
  TVar *tv = THxMEM_alloc(sizeof(TVar));
  tv->rc = 1;
  tv->val = v;
  Value *cell = alloc_value(T_TVAR);
  cell->u.tvar = tv;
  return cell;
}

/* The current value, and the version it belongs to, read together. */
static Value *tvar_snapshot(TVar *tv, uint64_t *version) {
  STM_LOCK();
  Value *v = tv->val;
  THxMEM_retain(v);
  *version = tv->version;
  STM_UNLOCK();
  pool_adopt(v);
  return v;
}

static Value *THxTVAR_read(Value *cell) {
  uint64_t version;
  return tvar_snapshot(as_tvar(cell), &version);
}

/* The calling thread's transaction log: each variable read with the version it
 * saw, and each write in order, the newest last. Entries hold references. */
typedef struct {
  TVar *tv;
  uint64_t version;
} StmRead;
typedef struct {
  TVar *tv;
  Value *val;
} StmWrite;
static THX_TLS StmRead *g_stm_reads = NULL;
static THX_TLS size_t g_stm_nr = 0, g_stm_rcap = 0;
static THX_TLS StmWrite *g_stm_writes = NULL;
static THX_TLS size_t g_stm_nw = 0, g_stm_wcap = 0;

static void stm_truncate_writes(size_t n) {
  while (g_stm_nw > n) {
    StmWrite *w = &g_stm_writes[--g_stm_nw];
    THxMEM_release(w->val);
    tvar_release(w->tv);
  }
}

static void stm_clear(void) {
  stm_truncate_writes(0);
  while (g_stm_nr > 0) tvar_release(g_stm_reads[--g_stm_nr].tv);
}

static void stm_free_log(void) {
  stm_clear();
  free(g_stm_reads);
  free(g_stm_writes);
  g_stm_reads = NULL;
  g_stm_writes = NULL;
  g_stm_rcap = g_stm_wcap = 0;
}

static Value *THxSTM_begin(void) {
  stm_clear();
  return THxRT_unit();
}

/* The transaction's own latest write, or else the committed value, whose
 * version is logged for validation. */
static Value *THxSTM_read(Value *cell) {
  TVar *tv = as_tvar(cell);
  for (size_t i = g_stm_nw; i > 0; i--)
    if (g_stm_writes[i - 1].tv == tv) return g_stm_writes[i - 1].val; /* borrowed */
  uint64_t version;
  Value *v = tvar_snapshot(tv, &version);
  if (g_stm_nr == g_stm_rcap) {
    g_stm_rcap = g_stm_rcap ? g_stm_rcap * 2 : 16;
    g_stm_reads = realloc(g_stm_reads, g_stm_rcap * sizeof(StmRead));
    if (!g_stm_reads) thrax_fault("transaction log allocation failed");
  }
  obj_retain(&tv->rc);
  g_stm_reads[g_stm_nr++] = (StmRead){tv, version};
  return v;
}

static Value *THxSTM_write(Value *cell, Value *v) {
  TVar *tv = as_tvar(cell);
#ifdef THRAX_THREADS
  mark_shared(v);
#endif
  if (g_stm_nw == g_stm_wcap) {
    g_stm_wcap = g_stm_wcap ? g_stm_wcap * 2 : 16;
    g_stm_writes = realloc(g_stm_writes, g_stm_wcap * sizeof(StmWrite));
    if (!g_stm_writes) thrax_fault("transaction log allocation failed");
  }
  obj_retain(&tv->rc);
  THxMEM_retain(v);
  g_stm_writes[g_stm_nw++] = (StmWrite){tv, v};
  return THxRT_unit();
}

static int stm_reads_unchanged(void) {
  for (size_t i = 0; i < g_stm_nr; i++)
    if (g_stm_reads[i].tv->version != g_stm_reads[i].version) return 0;
  return 1;
}

/* Publish the writes, all at once, if nothing the transaction read has been
 * written since; the replaced values are released outside the lock. */
static Value *THxSTM_commit(void) {
  STM_LOCK();
  int ok = stm_reads_unchanged();
  if (ok) {
    for (size_t i = 0; i < g_stm_nw; i++) {
      StmWrite *w = &g_stm_writes[i];
      Value *old = w->tv->val;
      w->tv->val = w->val;
      w->tv->version++;
      w->val = old;
    }
  }
  STM_UNLOCK();
  stm_clear();
  return THxRT_bool(ok);
}

/* `retry`: wait until another thread commits a write to something the
 * transaction read, helping with queued tasks meanwhile. A task run while
 * helping uses this thread's log for its own transactions, so the reads to
 * watch are moved out of it first. */
static Value *THxSTM_wait(void) {
  stm_truncate_writes(0);
  StmRead *reads = g_stm_reads;
  size_t nr = g_stm_nr;
  g_stm_reads = NULL;
  g_stm_nr = g_stm_rcap = 0;
  for (;;) {
    STM_LOCK();
    int same = 1;
    for (size_t i = 0; i < nr && same; i++)
      if (reads[i].tv->version != reads[i].version) same = 0;
    STM_UNLOCK();
    if (!same) break;
#ifndef THRAX_THREADS
    if (run_next_task()) continue;
#endif
    if (!others_may_run())
      thrax_fault("deadlock: a transaction retries, and no task will change what it read");
#ifdef THRAX_THREADS
    Value *w = mt_find_work();
    if (w)
      mt_run_entry(w);
    else
      sched_yield();
#endif
  }
  for (size_t i = 0; i < nr; i++) tvar_release(reads[i].tv);
  free(reads);
  return THxRT_unit();
}

static Value *THxSTM_save(void) { return THxRT_int((long long)g_stm_nw); }

static Value *THxSTM_rollback(Value *mark) {
  Value *m = THxVALUE_force(mark);
  if (m->tag != T_INT || m->u.i < 0 || (size_t)m->u.i > g_stm_nw)
    thrax_fault("expected a transaction mark");
  stm_truncate_writes((size_t)m->u.i);
  return THxRT_unit();
}

/* -- show (matches interpreter Value::show) ------------------------------ */

typedef struct {
  char *buf;
  size_t len, cap;
} Str;

static void str_push(Str *s, const char *bytes, size_t n) {
  if (s->len + n + 1 > s->cap) {
    while (s->len + n + 1 > s->cap) s->cap = s->cap ? s->cap * 2 : 64;
    s->buf = realloc(s->buf, s->cap);
    if (!s->buf) thrax_fault("out of memory");
  }
  memcpy(s->buf + s->len, bytes, n);
  s->len += n;
  s->buf[s->len] = '\0';
}
static void str_puts(Str *s, const char *z) { str_push(s, z, strlen(z)); }

static void fmt_real(Str *s, double r) {
  char tmp[64];
  for (int prec = 1; prec <= 17; prec++) {
    snprintf(tmp, sizeof(tmp), "%.*g", prec, r);
    if (strtod(tmp, NULL) == r) break;
  }
  str_puts(s, tmp);
}

/* The shortest decimal that round-trips through a float, matching Rust's
 * `f32::to_string` (and so the interpreter's `Value::Real32` display). */
static void fmt_real32(Str *s, float r) {
  char tmp[64];
  for (int prec = 1; prec <= 9; prec++) {
    snprintf(tmp, sizeof(tmp), "%.*g", prec, (double)r);
    if (strtof(tmp, NULL) == r) break;
  }
  str_puts(s, tmp);
}

static void show_into(Str *s, Value *v);

static void show_bytes_debug(Str *s, const uint8_t *data, size_t len) {
  str_puts(s, "\"");
  for (size_t i = 0; i < len; i++) {
    uint8_t c = data[i];
    switch (c) {
      case '"': str_puts(s, "\\\""); break;
      case '\\': str_puts(s, "\\\\"); break;
      case '\n': str_puts(s, "\\n"); break;
      case '\t': str_puts(s, "\\t"); break;
      case '\r': str_puts(s, "\\r"); break;
      default:
        if (c >= 0x20 && c < 0x7f) {
          char ch = (char)c;
          str_push(s, &ch, 1);
        } else {
          char esc[16];
          snprintf(esc, sizeof(esc), "\\u{%x}", c);
          str_puts(s, esc);
        }
    }
  }
  str_puts(s, "\"");
}

static void show_into(Str *s, Value *v) {
  char num[32];
  switch (v->tag) {
    case T_INT:
      snprintf(num, sizeof(num), "%lld", (long long)v->u.i);
      str_puts(s, num);
      break;
    case T_REAL: fmt_real(s, v->u.r); break;
    case T_REAL32: fmt_real32(s, (float)v->u.r); break;
    case T_STR: show_bytes_debug(s, v->u.str.data, v->u.str.len); break;
    case T_BOOL: str_puts(s, v->u.b ? "true" : "false"); break;
    case T_UNIT: str_puts(s, "{}"); break;
    case T_TUPLE:
      str_puts(s, "{");
      for (size_t i = 0; i < v->u.seq.len; i++) {
        if (i) str_puts(s, ", ");
        show_into(s, v->u.seq.items[i]);
      }
      str_puts(s, "}");
      break;
    case T_STRUCT:
      str_puts(s, v->u.strct.name);
      str_puts(s, ".{ ");
      for (size_t i = 0; i < v->u.strct.len; i++) {
        if (i) str_puts(s, ", ");
        str_puts(s, ".");
        str_puts(s, v->u.strct.fields[i].name);
        str_puts(s, " = ");
        show_into(s, v->u.strct.fields[i].val);
      }
      str_puts(s, " }");
      break;
    case T_VARIANT:
      str_puts(s, ".");
      str_puts(s, v->u.variant.tag);
      if (v->u.variant.len) {
        str_puts(s, ".{ ");
        for (size_t i = 0; i < v->u.variant.len; i++) {
          if (i) str_puts(s, ", ");
          show_into(s, v->u.variant.fields[i]);
        }
        str_puts(s, " }");
      }
      break;
    case T_VEC:
      str_puts(s, "vec[");
      for (size_t i = 0; i < v->u.seq.len; i++) {
        if (i) str_puts(s, ", ");
        show_into(s, v->u.seq.items[i]);
      }
      str_puts(s, "]");
      break;
    case T_CLOS:
    case T_BUILTIN:
    case T_EXTERN:
    case T_OP: str_puts(s, "<function>"); break;
    case T_RESUMP: str_puts(s, "<continuation>"); break;
    case T_THUNK:
    case T_BLACKHOLE: str_puts(s, "<unforced>"); break;
#ifdef THRAX_THREADS
    case T_FUTURE: show_into(s, THxPAR_await(v)); break;
#endif
    case T_CHAN: str_puts(s, "<channel>"); break;
    case T_TVAR: str_puts(s, "<tvar>"); break;
  }
}

static char *thrax_show(Value *v) {
  Str s = {NULL, 0, 0};
  show_into(&s, v);
  if (!s.buf) {
    s.buf = xmalloc(1);
    s.buf[0] = '\0';
  }
  return s.buf;
}

/* The standard library's scalar conversions (`thx_i2d`, `thx_real_to_str`, ...)
   live in the single-source `thraxstd.c`, appended after this runtime by the
   emitter (crates/ccg/src/lib.rs) and linked into the interpreter in-process. */
