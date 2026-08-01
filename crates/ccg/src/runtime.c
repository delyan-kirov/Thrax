/* Thrax C-backend runtime. Emitted verbatim ahead of the generated program by
 * the `ccg` crate. It is the C port of the reified-K abstract machine
 * (crates/interpreter/src/machine.rs, itself the port of engines/IT.cpp): the
 * continuation is an EXPLICIT heap stack of frames, so a handler can capture and
 * splice the delimited continuation between a prompt and a `perform`, and deep
 * non-tail recursion grows the heap rather than the C stack.
 *
 * Generated code is compiled to BLOCK FUNCTIONS. A block runs straight-line C
 * (atoms, pure lets, case branching) and ends by calling exactly one TERMINATOR
 * (THxK_ret / THxK_tailcall / THxK_apply / THxK_jump / THxK_handle /
 * THxK_defer_run); the driver acts on it. An activation's locals/env live in a
 * heap Frame, so a block can be re-entered after a suspension.
 *
 * Memory is precise reference counting, the port of platforms/THxMEMRC.c +
 * THxVALUE.c + THxK.c. Values, frames and resumption segments carry a count;
 * every heap store retains and every un-store releases, freeing at zero and
 * releasing children iteratively (a dead-value worklist, never the C stack).
 * Fresh values are registered in a temp pool that the driver drains after each
 * block bounce, reclaiming per-iteration garbage. A live-allocation counter lets
 * the generated main assert it exits clean. The one cycle, a recursive-let
 * closure that captured its own box, is a weak self edge (a child equal to its
 * container is neither retained nor released). */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct Value Value;
typedef struct Frame Frame;
typedef struct Resump Resump;

/* A compiled basic block. `in` is the value delivered to a resumed block; blocks
 * re-read their live variables from `fr`, so `in` is usually unused. */
typedef void (*BlockFn)(Frame *fr, Value *in);

/* Provided by the generated program. */
extern BlockFn THxRT_code_table[];
extern const size_t THxRT_code_nlocals[];
extern const size_t THxRT_code_count;

typedef enum {
  T_INT,
  T_REAL,
  T_STR, /* byte vector: both Str and Array */
  T_BOOL,
  T_UNIT,
  T_TUPLE,
  T_STRUCT,
  T_VARIANT,
  T_VEC,       /* Vec `T (boxed elements) */
  T_CLOS,      /* closure: an IR code index + captured environment */
  T_BUILTIN,   /* a (possibly partially applied) built-in operator */
  T_OP,        /* an effect operation, first-class; performs when applied */
  T_RESUMP     /* a captured continuation; affine -- resumes once */
} Tag;

typedef struct {
  const char *name;
  Value *val;
} Field;

struct Value {
  Tag tag;
  unsigned rc;
  union {
    int64_t i;
    double r;
    bool b;
    struct {
      uint8_t *data;
      size_t len;
    } str;
    struct {
      Value **items;
      size_t len;
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
      const char *name;
      size_t arity;
      Value **args;
      size_t nargs;
    } builtin;
    struct {
      const char *effect; /* NULL for an ambient (unqualified) operation */
      const char *op;
    } op;
    Resump *resump;
  } u;
};

/* One activation: the local slot array (params + let/case binders) and the
 * current closure's captured record. Refcounted and heap-allocated so it
 * survives a suspension captured in the continuation stack. */
struct Frame {
  unsigned rc;
  Value *clos; /* retained; NULL for a nullary CAF */
  Value **locals;
  size_t nlocals;
  Value **env; /* borrowed from `clos` */
  size_t nenv;
};

/* Continuation frames (the reified-K stack) and captured resumptions. Defined
 * up here so a resumption's `escaped` flag is reachable from the value
 * constructors (which mark a stored resumption). */
typedef enum {
  K_RET,        /* resume a suspended activation with the returned value */
  K_PROMPT,     /* a handler delimiter */
  K_DEFER,      /* a deferred cleanup marker */
  K_THUNKRET,   /* deliver `saved`, discarding the incoming value */
  K_AFTERCLAUSE /* clause boundary: decides a captured k's defer fate */
} KTag;

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
      Value *kval;
    } afterclause;
  } u;
} KFrame;

/* A captured continuation slice. Affine (`used`); `escaped` records a stored
 * resumption (the C analog of the interpreter's shared_ptr use-count test).
 * `rc` counts the T_RESUMP values that own the segment (a resumption
 * content-copied into a let box aliases it). */
struct Resump {
  unsigned rc;
  KFrame *seg;
  size_t n;
  int used;
  int escaped;
};

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

static size_t g_live = 0; /* live allocations: values + raw blocks */

/* Allocate `size` zeroed bytes with value lifetime (counted). */
static void *THxMEM_alloc(size_t size) {
  void *p = calloc(1, size ? size : 1);
  if (!p) thrax_fault("out of memory");
  ++g_live;
  return p;
}

static void THxMEM_free(void *p) {
  if (!p) return;
  if (g_live == 0) thrax_fault("THxMEM_free: more frees than allocations");
  --g_live;
  free(p);
}

/* The temp pool: every fresh value is registered here holding the pool's
 * initial reference; the driver drains to a mark after each block bounce. */
static Value **g_pool = NULL;
static size_t g_pn = 0, g_pcap = 0;

static void THxMEM_release(Value *v);

static Value *alloc_value(Tag t) {
  Value *v = THxMEM_alloc(sizeof(Value));
  v->tag = t;
  v->rc = 1; /* the pool's reference */
  if (g_pn == g_pcap) {
    g_pcap = g_pcap ? g_pcap * 2 : 256;
    g_pool = realloc(g_pool, g_pcap * sizeof(Value *));
    if (!g_pool) thrax_fault("temp pool allocation failed");
  }
  g_pool[g_pn++] = v;
  return v;
}

static size_t THxMEM_pool_mark(void) { return g_pn; }

static void THxMEM_pool_drain(size_t mark) {
  if (mark > g_pn) thrax_fault("pool_drain: mark above pool top");
  while (g_pn > mark) THxMEM_release(g_pool[--g_pn]);
}

static void THxMEM_retain(Value *v) {
  if (!v) return;
  if (v->rc == 0) thrax_fault("retain: retaining a freed value");
  ++v->rc;
}

static int THxMEM_unique(Value *v) { return v && v->rc == 1; }

/* Dead-value worklist: destruction never recurses on the C stack. */
static Value **g_dead = NULL;
static size_t g_dn = 0, g_dcap = 0;
static int g_draining = 0;

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
  if (v->rc == 0) thrax_fault("release: releasing a freed value");
  if (--v->rc > 0) return;
  dead_push(v);
  if (g_draining) return; /* an outer drain loop will get to it */
  g_draining = 1;
  while (g_dn > 0) {
    Value *d = g_dead[--g_dn];
    THxVALUE_destroy(d); /* re-enters THxMEM_release: enqueues only */
    THxMEM_free(d);
  }
  g_draining = 0;
}

static size_t THxMEM_live(void) { return g_live; }

/* -- constructors -------------------------------------------------------- */

static void mark_escape(Value *v);

Value *THxRT_int(long long n) {
  Value *v = alloc_value(T_INT);
  v->u.i = (int64_t)n;
  return v;
}
Value *THxRT_real(double r) {
  Value *v = alloc_value(T_REAL);
  v->u.r = r;
  return v;
}
Value *THxRT_bool(int b) {
  Value *v = alloc_value(T_BOOL);
  v->u.b = b ? true : false;
  return v;
}
Value *THxRT_unit(void) { return alloc_value(T_UNIT); }

Value *THxRT_str(const char *data, size_t len) {
  Value *v = alloc_value(T_STR);
  v->u.str.data = THxMEM_alloc(len ? len : 1);
  memcpy(v->u.str.data, data, len);
  v->u.str.len = len;
  return v;
}
/* Take ownership of an already-THxMEM_alloc'd buffer. */
static Value *mk_str_owned(uint8_t *data, size_t len) {
  Value *v = alloc_value(T_STR);
  v->u.str.data = data;
  v->u.str.len = len;
  return v;
}

Value *THxRT_tuple(Value **items, size_t len) {
  Value *v = alloc_value(T_TUPLE);
  v->u.seq.items = len ? THxMEM_alloc(len * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.seq.items[i] = items[i];
    THxMEM_retain(items[i]);
    mark_escape(items[i]);
  }
  v->u.seq.len = len;
  return v;
}
static Value *mk_vec(Value **items, size_t len) {
  Value *v = alloc_value(T_VEC);
  v->u.seq.items = len ? THxMEM_alloc(len * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.seq.items[i] = items[i];
    THxMEM_retain(items[i]);
    mark_escape(items[i]);
  }
  v->u.seq.len = len;
  return v;
}
Value *THxRT_struct(const char *name, size_t len, const char **fnames,
                    Value **vals) {
  Value *v = alloc_value(T_STRUCT);
  v->u.strct.name = name;
  v->u.strct.fields = len ? THxMEM_alloc(len * sizeof(Field)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.strct.fields[i].name = fnames[i];
    v->u.strct.fields[i].val = vals[i];
    THxMEM_retain(vals[i]);
    mark_escape(vals[i]);
  }
  v->u.strct.len = len;
  return v;
}
Value *THxRT_variant(const char *ty, const char *tag, size_t len,
                     Value **fields) {
  Value *v = alloc_value(T_VARIANT);
  v->u.variant.ty = ty;
  v->u.variant.tag = tag;
  v->u.variant.fields = len ? THxMEM_alloc(len * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < len; i++) {
    v->u.variant.fields[i] = fields[i];
    THxMEM_retain(fields[i]);
    mark_escape(fields[i]);
  }
  v->u.variant.len = len;
  return v;
}
Value *THxRT_closure(int code, Value **captures, size_t n) {
  Value *v = alloc_value(T_CLOS);
  v->u.clos.code = code;
  v->u.clos.env = n ? THxMEM_alloc(n * sizeof(Value *)) : NULL;
  for (size_t i = 0; i < n; i++) {
    v->u.clos.env[i] = captures[i];
    THxMEM_retain(captures[i]);
    mark_escape(captures[i]);
  }
  v->u.clos.nenv = n;
  return v;
}
Value *THxRT_builtin(const char *name, size_t arity) {
  Value *v = alloc_value(T_BUILTIN);
  v->u.builtin.name = name;
  v->u.builtin.arity = arity;
  v->u.builtin.args = NULL;
  v->u.builtin.nargs = 0;
  return v;
}

/* Note a stored resumption: reachable from a heap structure means the clause
 * stashed it (to resume later) rather than abandoning it, so its `defer`
 * cleanups must not be finalized at the clause boundary. */
static void mark_escape(Value *v) {
  if (v && v->tag == T_RESUMP) v->u.resump->escaped = 1;
}

/* A struct built from a base (record update): base's fields seeded, then the
 * listed overrides. An empty `name` keeps the base's type name. */
Value *THxRT_struct_update(Value *base, const char *name, size_t nextra,
                           const char **fnames, Value **vals) {
  if (base->tag != T_STRUCT) thrax_fault("record update of a non-struct value");
  size_t cap = base->u.strct.len + nextra;
  Field *fields = THxMEM_alloc((cap ? cap : 1) * sizeof(Field));
  size_t len = base->u.strct.len;
  memcpy(fields, base->u.strct.fields, len * sizeof(Field));
  for (size_t i = 0; i < nextra; i++) {
    bool found = false;
    for (size_t j = 0; j < len; j++)
      if (strcmp(fields[j].name, fnames[i]) == 0) {
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
    mark_escape(fields[i].val);
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
  if (v->tag == T_REAL) return v->u.r;
  thrax_fault("expected a number");
}
int THxVALUE_as_bool(Value *v) {
  if (v->tag != T_BOOL) thrax_fault("expected a boolean");
  return v->u.b ? 1 : 0;
}
Value *THxVALUE_local(Value **locals, size_t n, size_t i) {
  if (i >= n) thrax_fault("local slot out of range");
  return locals[i];
}
Value *THxVALUE_env(Value **env, size_t n, size_t i) {
  if (i >= n) thrax_fault("env field out of range");
  return env[i];
}
char *THxVALUE_str(Value *v) {
  if (v->tag != T_STR) thrax_fault("expected a byte vector");
  return (char *)v->u.str.data;
}
static Value *struct_field(Value *v, const char *name) {
  for (size_t i = 0; i < v->u.strct.len; i++)
    if (strcmp(v->u.strct.fields[i].name, name) == 0)
      return v->u.strct.fields[i].val;
  return NULL;
}
/* `record.field`: a struct field by name, or a tuple element by index. */
Value *THxVALUE_field(Value *v, const char *name) {
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
  if (v->tag == T_REAL) return v->u.r;
  thrax_fault("expected a number");
}
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

/* -- structural equality (?=) -------------------------------------------- */

static bool value_eq(Value *x, Value *y) {
  bool xnum = x->tag == T_INT || x->tag == T_REAL;
  bool ynum = y->tag == T_INT || y->tag == T_REAL;
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
      if (strcmp(x->u.variant.tag, y->u.variant.tag) != 0 ||
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

static Value *arith(const char *op, Value *x, Value *y) {
  if (x->tag == T_INT && y->tag == T_INT) {
    int64_t a = x->u.i, b = y->u.i, r;
    switch (op[0]) {
      case '+': r = a + b; break;
      case '-': r = a - b; break;
      case '*': r = a * b; break;
      case '/':
        if (b == 0) thrax_fault("division by zero");
        r = a / b;
        break;
      default:
        if (b == 0) thrax_fault("division by zero");
        r = a % b;
        break;
    }
    return THxRT_int(r);
  }
  double a = as_f64(x), b = as_f64(y), r;
  switch (op[0]) {
    case '+': r = a + b; break;
    case '-': r = a - b; break;
    case '*': r = a * b; break;
    case '/': r = a / b; break;
    default:
      /* Rust f64 `%` is C fmod. */
      r = a - b * (double)(long long)(a / b);
      break;
  }
  return THxRT_real(r);
}

static int cmp_bytes(Value *a, Value *b) {
  size_t n = a->u.str.len < b->u.str.len ? a->u.str.len : b->u.str.len;
  int c = memcmp(a->u.str.data, b->u.str.data, n);
  if (c != 0) return c < 0 ? -1 : 1;
  if (a->u.str.len == b->u.str.len) return 0;
  return a->u.str.len < b->u.str.len ? -1 : 1;
}

static Value *compare(const char *op, Value *x, Value *y) {
  int ord;
  if (x->tag == T_STR && y->tag == T_STR) {
    ord = cmp_bytes(x, y);
  } else {
    double a = as_f64(x), b = as_f64(y);
    ord = a < b ? -1 : (a > b ? 1 : 0);
  }
  bool r;
  if (strcmp(op, "?<") == 0)
    r = ord < 0;
  else if (strcmp(op, "?>") == 0)
    r = ord > 0;
  else if (strcmp(op, "<=") == 0)
    r = ord <= 0;
  else
    r = ord >= 0;
  return THxRT_bool(r);
}

static Value *list_append(Value *xs, Value *ys) {
  if (xs->tag == T_VARIANT && strcmp(xs->u.variant.tag, "Cons") == 0) {
    Value *fields[2];
    fields[0] = xs->u.variant.fields[0];
    fields[1] = list_append(xs->u.variant.fields[1], ys);
    return THxRT_variant("List", "Cons", 2, fields);
  }
  return ys;
}

static Value *concat(Value *x, Value *y) {
  if (x->tag == T_STR && y->tag == T_STR) {
    size_t len = x->u.str.len + y->u.str.len;
    uint8_t *data = THxMEM_alloc(len ? len : 1);
    memcpy(data, x->u.str.data, x->u.str.len);
    memcpy(data + x->u.str.len, y->u.str.data, y->u.str.len);
    return mk_str_owned(data, len);
  }
  if (x->tag == T_VARIANT && strcmp(x->u.variant.ty, "List") == 0)
    return list_append(x, y);
  thrax_fault("`++` on unsupported operands");
}

static Value *run_c(const char *name, Value **a, size_t n);

static Value *run_builtin(const char *name, Value **a, size_t n) {
  (void)n;
  if (strcmp(name, "+") == 0 || strcmp(name, "-") == 0 ||
      strcmp(name, "*") == 0 || strcmp(name, "/") == 0 ||
      strcmp(name, "%") == 0)
    return arith(name, a[0], a[1]);
  if (strcmp(name, "neg") == 0) {
    if (a[0]->tag == T_INT) return THxRT_int(-a[0]->u.i);
    if (a[0]->tag == T_REAL) return THxRT_real(-a[0]->u.r);
    thrax_fault("`neg` on a non-number");
  }
  if (strcmp(name, "not") == 0) {
    if (a[0]->tag == T_BOOL) return THxRT_bool(!a[0]->u.b);
    thrax_fault("`not` on a non-boolean");
  }
  if (strcmp(name, "?=") == 0) return THxRT_bool(value_eq(a[0], a[1]));
  if (strcmp(name, "?<") == 0 || strcmp(name, "?>") == 0 ||
      strcmp(name, "<=") == 0 || strcmp(name, ">=") == 0)
    return compare(name, a[0], a[1]);
  if (strcmp(name, "++") == 0) return concat(a[0], a[1]);

  if (strcmp(name, "array_alloc") == 0) {
    size_t len = as_index(a[0]);
    uint8_t *data = THxMEM_alloc(len ? len : 1);
    memset(data, 0, len);
    return mk_str_owned(data, len);
  }
  if (strcmp(name, "array_len") == 0)
    return THxRT_int((int64_t)as_str(a[0])->u.str.len);
  if (strcmp(name, "array_get") == 0) {
    Value *s = as_str(a[0]);
    size_t i = as_index(a[1]);
    if (i >= s->u.str.len) thrax_fault("array index out of bounds");
    return THxRT_int(s->u.str.data[i]);
  }
  if (strcmp(name, "array_push") == 0) {
    Value *s = as_str(a[0]);
    size_t len = s->u.str.len + 1;
    uint8_t *data = THxMEM_alloc(len);
    memcpy(data, s->u.str.data, s->u.str.len);
    data[s->u.str.len] = as_byte(a[1]);
    return mk_str_owned(data, len);
  }
  if (strcmp(name, "array_set") == 0) {
    Value *s = as_str(a[0]);
    size_t i = as_index(a[1]);
    if (i >= s->u.str.len) thrax_fault("array index out of bounds");
    uint8_t *data = THxMEM_alloc(s->u.str.len ? s->u.str.len : 1);
    memcpy(data, s->u.str.data, s->u.str.len);
    data[i] = as_byte(a[2]);
    return mk_str_owned(data, s->u.str.len);
  }
  if (strcmp(name, "array_slice") == 0) {
    Value *s = as_str(a[0]);
    size_t beg = as_index(a[1]);
    size_t end = as_index(a[2]);
    if (beg > s->u.str.len) beg = s->u.str.len;
    if (end < beg) end = beg;
    if (end > s->u.str.len) end = s->u.str.len;
    return THxRT_str((const char *)s->u.str.data + beg, end - beg);
  }

  if (strcmp(name, "vec_new") == 0) return mk_vec(NULL, 0);
  if (strcmp(name, "vec_fill") == 0) {
    size_t len = as_index(a[0]);
    Value **items = len ? xmalloc(len * sizeof(Value *)) : NULL;
    for (size_t i = 0; i < len; i++) items[i] = a[1];
    Value *v = mk_vec(items, len);
    free(items);
    return v;
  }
  if (strcmp(name, "vec_len") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    return THxRT_int((int64_t)a[0]->u.seq.len);
  }
  if (strcmp(name, "vec_get") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t i = as_index(a[1]);
    if (i >= a[0]->u.seq.len) thrax_fault("vec index out of bounds");
    return a[0]->u.seq.items[i]; /* borrowed; the caller (do_ret) retains it */
  }
  if (strcmp(name, "vec_push") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t len = a[0]->u.seq.len + 1;
    Value **items = xmalloc(len * sizeof(Value *));
    memcpy(items, a[0]->u.seq.items, a[0]->u.seq.len * sizeof(Value *));
    items[a[0]->u.seq.len] = a[1];
    Value *v = mk_vec(items, len);
    free(items);
    return v;
  }
  if (strcmp(name, "vec_set") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t i = as_index(a[1]);
    if (i >= a[0]->u.seq.len) thrax_fault("vec index out of bounds");
    Value **items = xmalloc((a[0]->u.seq.len ? a[0]->u.seq.len : 1) *
                            sizeof(Value *));
    memcpy(items, a[0]->u.seq.items, a[0]->u.seq.len * sizeof(Value *));
    items[i] = a[2];
    Value *v = mk_vec(items, a[0]->u.seq.len);
    free(items);
    return v;
  }

  if (strncmp(name, "C.", 2) == 0) return run_c(name, a, n);
  thrax_fault("unknown built-in");
}

/* -- the auto-injected `C` libc namespace -------------------------------- */

#define MAX_FILES 256
static FILE *thrax_files[MAX_FILES];
static int64_t thrax_next_fd = 1;

/* Uncounted NUL-terminated copy for a C-string argument; the caller frees it. */
static char *cstr_of(Value *v) {
  Value *s = as_str(v);
  char *out = xmalloc(s->u.str.len + 1);
  memcpy(out, s->u.str.data, s->u.str.len);
  out[s->u.str.len] = '\0';
  return out;
}

static Value *run_c(const char *name, Value **a, size_t n) {
  (void)n;
  const char *fn = name + 2; /* skip "C." */
  if (strcmp(fn, "getenv") == 0) {
    char *key = cstr_of(a[0]);
    const char *val = getenv(key);
    if (!val) val = "";
    free(key);
    return THxRT_str(val, strlen(val));
  }
  if (strcmp(fn, "fopen") == 0) {
    char *path = cstr_of(a[0]);
    char *mode = cstr_of(a[1]);
    FILE *f = fopen(path, mode);
    free(path);
    free(mode);
    if (!f) return THxRT_int(0);
    if (thrax_next_fd >= MAX_FILES) {
      fclose(f);
      return THxRT_int(0);
    }
    int64_t id = thrax_next_fd++;
    thrax_files[id] = f;
    return THxRT_int(id);
  }
  if (strcmp(fn, "fclose") == 0) {
    int64_t id = as_i64(a[0]);
    if (id > 0 && id < MAX_FILES && thrax_files[id]) {
      fclose(thrax_files[id]);
      thrax_files[id] = NULL;
      return THxRT_int(0);
    }
    return THxRT_int(-1);
  }
  if (strcmp(fn, "fgetc") == 0) {
    int64_t id = as_i64(a[0]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return THxRT_int(-1);
    int c = fgetc(thrax_files[id]);
    return THxRT_int(c == EOF ? -1 : c);
  }
  if (strcmp(fn, "fseek") == 0) {
    int64_t id = as_i64(a[0]), off = as_i64(a[1]), whence = as_i64(a[2]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return THxRT_int(-1);
    int w = whence == 1 ? SEEK_CUR : (whence == 2 ? SEEK_END : SEEK_SET);
    return THxRT_int(fseek(thrax_files[id], (long)off, w) == 0 ? 0 : -1);
  }
  if (strcmp(fn, "ftell") == 0) {
    int64_t id = as_i64(a[0]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return THxRT_int(-1);
    long p = ftell(thrax_files[id]);
    return THxRT_int(p < 0 ? -1 : (int64_t)p);
  }
  if (strcmp(fn, "fputs") == 0) {
    Value *s = as_str(a[0]);
    int64_t id = as_i64(a[1]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return THxRT_int(-1);
    size_t wrote = fwrite(s->u.str.data, 1, s->u.str.len, thrax_files[id]);
    return THxRT_int(wrote == s->u.str.len ? 0 : -1);
  }
  if (strcmp(fn, "remove") == 0) {
    char *path = cstr_of(a[0]);
    int rc = remove(path);
    free(path);
    return THxRT_int(rc == 0 ? 0 : -1);
  }
  if (strcmp(fn, "write") == 0) {
    int64_t fd = as_i64(a[0]);
    Value *s = as_str(a[1]);
    size_t len = as_index(a[2]);
    if (len > s->u.str.len) len = s->u.str.len;
    FILE *out = fd == 2 ? stderr : stdout;
    size_t wrote = fwrite(s->u.str.data, 1, len, out);
    return THxRT_int(wrote == len ? (int64_t)len : -1);
  }
  if (strcmp(fn, "getchar") == 0) {
    int c = getchar();
    return THxRT_int(c == EOF ? -1 : c);
  }
  if (strcmp(fn, "time") == 0) return THxRT_int((int64_t)time(NULL));
  thrax_fault("unsupported C function");
}

/* -- TARGET reflection --------------------------------------------------- */

Value *THxRT_target(const char *name) {
  if (strcmp(name, "int_bits") == 0 || strcmp(name, "ptr_bits") == 0)
    return THxRT_int((int64_t)(sizeof(size_t) * 8));
  if (strcmp(name, "int_max") == 0) return THxRT_int(INT64_MAX);
  if (strcmp(name, "int_min") == 0) return THxRT_int(INT64_MIN);
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

/* Release the pointer children in `f[0..n)` (skipping the weak self edge,
 * child == v) and free the array itself. */
static void release_children(Value *v, Value **f, size_t n) {
  if (!f) return;
  for (size_t i = 0; i < n; i++)
    if (f[i] != v) THxMEM_release(f[i]);
  THxMEM_free(f);
}

/* Free v's owned payload: release children, free raw blocks. Shared by
 * THxVALUE_destroy (rc reached zero) and THxVALUE_patch_box (overwriting a
 * box's previous contents). */
static void payload_destroy(Value *v) {
  switch (v->tag) {
    case T_INT:
    case T_REAL:
    case T_BOOL:
    case T_UNIT:
    case T_OP: return;
    case T_STR: THxMEM_free(v->u.str.data); return;
    case T_TUPLE:
    case T_VEC: release_children(v, v->u.seq.items, v->u.seq.len); return;
    case T_STRUCT: {
      Field *fs = v->u.strct.fields;
      if (fs) {
        for (size_t i = 0; i < v->u.strct.len; i++)
          if (fs[i].val != v) THxMEM_release(fs[i].val);
        THxMEM_free(fs);
      }
      return;
    }
    case T_VARIANT: release_children(v, v->u.variant.fields, v->u.variant.len); return;
    case T_CLOS: release_children(v, v->u.clos.env, v->u.clos.nenv); return;
    case T_BUILTIN: release_children(v, v->u.builtin.args, v->u.builtin.nargs); return;
    case T_RESUMP: THxK_resump_release(v->u.resump); return;
  }
  thrax_fault("payload_destroy: unhandled tag");
}

static void THxVALUE_destroy(Value *v) { payload_destroy(v); }

/* A fresh array holding f[0..n), each child retained except the weak self edge
 * (child == self). NULL when n == 0. */
static Value **copy_children(Value *self, Value **f, size_t n) {
  if (n == 0) return NULL;
  Value **c = THxMEM_alloc(n * sizeof(Value *));
  for (size_t i = 0; i < n; i++) {
    c[i] = f[i];
    if (c[i] != self) THxMEM_retain(c[i]);
  }
  return c;
}

/* Back-patch a let box: make `box` a copy of `v`, preserving box's own count.
 * Pointer-array payloads and string bytes are DEEP-copied into fresh blocks and
 * the children retained, so box and v afterwards have independent lifetimes; a
 * resumption's segment is shared via its own count. A child equal to `box` is
 * left unretained (the weak self edge of a recursive-let closure). */
static void THxVALUE_patch_box(Value *box, Value *v) {
  if (box == v) return;
  payload_destroy(box); /* a box is normally T_UNIT: a no-op */
  unsigned saved = box->rc;
  *box = *v;
  box->rc = saved;
  switch (box->tag) {
    case T_INT:
    case T_REAL:
    case T_BOOL:
    case T_UNIT:
    case T_OP: break;
    case T_STR: {
      size_t len = box->u.str.len;
      uint8_t *b = THxMEM_alloc(len ? len : 1);
      if (len) memcpy(b, v->u.str.data, len);
      box->u.str.data = b;
      break;
    }
    case T_TUPLE:
    case T_VEC:
      box->u.seq.items = copy_children(box, v->u.seq.items, box->u.seq.len);
      break;
    case T_STRUCT: {
      size_t n = box->u.strct.len;
      if (n == 0) {
        box->u.strct.fields = NULL;
        break;
      }
      Field *fs = THxMEM_alloc(n * sizeof(Field));
      for (size_t i = 0; i < n; i++) {
        fs[i].name = v->u.strct.fields[i].name;
        fs[i].val = v->u.strct.fields[i].val;
        if (fs[i].val != box) THxMEM_retain(fs[i].val);
      }
      box->u.strct.fields = fs;
      break;
    }
    case T_VARIANT:
      box->u.variant.fields =
          copy_children(box, v->u.variant.fields, box->u.variant.len);
      break;
    case T_CLOS:
      box->u.clos.env = copy_children(box, v->u.clos.env, box->u.clos.nenv);
      break;
    case T_BUILTIN:
      box->u.builtin.args =
          copy_children(box, v->u.builtin.args, box->u.builtin.nargs);
      break;
    case T_RESUMP: THxK_resump_addref(box->u.resump); break;
  }
}

/*------------------------------------------------------------------------------
 *\THE REIFIED-K (CEK) DRIVER (port of THxK.c)
 *-----------------------------------------------------------------------------*/

/* The single, shared continuation stack; re-entrant runs share it via a base
 * marker (THxK_run_code / THxK_call record the current height). */
static KFrame *g_kont = NULL;
static size_t g_kn = 0, g_kcap = 0;

static void kont_push(KFrame f) {
  if (g_kn == g_kcap) {
    g_kcap = g_kcap ? g_kcap * 2 : 64;
    g_kont = realloc(g_kont, g_kcap * sizeof(KFrame));
    if (!g_kont) thrax_fault("continuation stack allocation failed");
  }
  g_kont[g_kn++] = f;
}

/* -- frames (refcounted activations) ------------------------------------- */

static Frame *frame_new(size_t nlocals, Value *clos) {
  Frame *f = THxMEM_alloc(sizeof(Frame));
  f->rc = 1;
  f->clos = clos;
  if (clos) {
    THxMEM_retain(clos);
    f->env = clos->u.clos.env;
    f->nenv = clos->u.clos.nenv;
  } else {
    f->env = NULL;
    f->nenv = 0;
  }
  f->locals = nlocals ? THxMEM_alloc(nlocals * sizeof(Value *)) : NULL;
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
  for (size_t i = 0; i < f->nlocals; i++) THxMEM_release(f->locals[i]);
  THxMEM_free(f->locals);
  THxMEM_release(f->clos);
  THxMEM_free(f);
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
      THxMEM_free(kf->u.prompt.clauses);
      THxMEM_release(kf->u.prompt.els);
      return;
    case K_DEFER: THxMEM_release(kf->u.defer.cleanup); return;
    case K_THUNKRET: THxMEM_release(kf->u.thunkret.saved); return;
    case K_AFTERCLAUSE: THxMEM_release(kf->u.afterclause.kval); return;
  }
  thrax_fault("kframe_release: unhandled continuation frame");
}

static void THxK_resump_addref(Resump *seg) {
  if (!seg) return;
  ++seg->rc;
  seg->escaped = 1;
}

static void THxK_resump_release(Resump *seg) {
  if (!seg) return;
  if (seg->rc == 0) thrax_fault("resump_release: releasing a freed slice");
  if (--seg->rc > 0) return;
  if (!seg->used)
    for (size_t i = 0; i < seg->n; i++) kframe_release(&seg->seg[i]);
  THxMEM_free(seg->seg); /* NULL after a resume: a no-op */
  THxMEM_free(seg);
}

/* -- next-action registers ----------------------------------------------- */

typedef enum { ACT_NONE, ACT_RET, ACT_APPLY, ACT_JUMP } Act;
static Act g_act;
static Value *g_v, *g_fn, *g_arg;
static BlockFn g_blk;
static Value *g_result;

/* -- block terminators --------------------------------------------------- */

void THxK_ret(Value *v) {
  g_act = ACT_RET;
  g_v = v;
}
void THxK_tailcall(Value *fn, Value *arg) {
  g_act = ACT_APPLY;
  g_fn = fn;
  g_arg = arg;
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
}
void THxK_jump(BlockFn cont) {
  g_act = ACT_JUMP;
  g_blk = cont;
}
void THxK_handle(Frame *fr, BlockFn cont, size_t slot, const char **effs,
                 const char **ops, Value **clauses, size_t nclauses, Value *els,
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
  /* The generated clause tables are C compound literals with block lifetime;
   * copy them into runtime-lifetime memory the prompt owns. */
  const char **effs2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(char *));
  const char **ops2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(char *));
  Value **claus2 = THxMEM_alloc((nclauses ? nclauses : 1) * sizeof(Value *));
  for (size_t i = 0; i < nclauses; i++) {
    effs2[i] = effs[i];
    ops2[i] = ops[i];
    claus2[i] = clauses[i];
    THxMEM_retain(claus2[i]);
  }
  KFrame kp;
  kp.tag = K_PROMPT;
  kp.u.prompt.effs = effs2;
  kp.u.prompt.ops = ops2;
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
  THxMEM_release(fr->locals[slot]);
  fr->locals[slot] = v;
}
void THxK_setbox(Frame *fr, size_t slot) {
  THxK_setlocal(fr, slot, THxRT_unit());
}
void THxK_backpatch(Frame *fr, size_t slot, Value *v) {
  if (slot >= fr->nlocals) thrax_fault("backpatch: slot out of range");
  Value *box = fr->locals[slot];
  if (!box) thrax_fault("backpatch: no box in slot");
  THxVALUE_patch_box(box, v);
}

/* -- the machine --------------------------------------------------------- */

/* Enter a closure `clo`, placing `args[0..nargs)` in the leading local slots
 * (the rest left as boxes on demand). The new frame retains `clo` and its slots
 * retain the args; the outgoing activation is released. nargs: 0 (defer
 * cleanup), 1 (value clause / a call), 2 (handler clause). */
static void enter_clo(BlockFn *cur, Frame **fr, Value **in, Value *clo,
                      Value **args, size_t nargs) {
  if (clo->tag != T_CLOS) thrax_fault("entering a non-closure");
  size_t code = (size_t)clo->u.clos.code;
  if (code >= THxRT_code_count) thrax_fault("code index out of range");
  Frame *nf = frame_new(THxRT_code_nlocals[code], clo);
  for (size_t i = 0; i < nargs; i++) THxK_setlocal(nf, i, args[i]);
  frame_release(*fr);
  *cur = THxRT_code_table[code];
  *fr = nf;
  *in = nargs ? args[0] : NULL;
}

static Value *unit(void) { return THxRT_unit(); }

/* Hand a finished value to the continuation stack (the interpreter's `ret`).
 * Returns 1 to continue the driver loop, 0 when the run's base is reached.
 * Maintains exactly one owned in-flight reference on the value being delivered
 * until a store (box, slot, kont) has taken its own. */
static int do_ret(BlockFn *cur, Frame **fr, Value **in, Value *v, size_t base) {
  THxMEM_retain(v); /* the in-flight reference */
  for (;;) {
    if (g_kn == base) {
      frame_release(*fr);
      *fr = NULL;
      g_result = v; /* transfer the in-flight reference to the caller */
      return 0;
    }
    KFrame kf = g_kont[--g_kn];
    switch (kf.tag) {
      case K_RET: {
        Value *box = kf.u.ret.frame->locals[kf.u.ret.slot];
        if (!box) thrax_fault("ret: no box in resumed slot");
        THxVALUE_patch_box(box, v);
        *cur = kf.u.ret.cont;
        frame_release(*fr);
        *fr = kf.u.ret.frame; /* adopt the KRet's frame reference */
        *in = v;
        THxMEM_release(v); /* delivered: the box owns copies of v's children */
        return 1;
      }
      case K_PROMPT: {
        /* body finished normally -> run the value clause on the result */
        enter_clo(cur, fr, in, kf.u.prompt.els, &v, 1);
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
        enter_clo(cur, fr, in, kf.u.defer.cleanup, &u, 0);
        kframe_release(&kf); /* cleanup survives via the new frame's clos ref */
        return 1;
      }
      case K_THUNKRET:
        THxMEM_release(v);
        v = kf.u.thunkret.saved; /* adopt the kont's reference as in-flight */
        continue;
      case K_AFTERCLAUSE: {
        Resump *res = kf.u.afterclause.kval->u.resump;
        if (res->used || res->escaped) {
          kframe_release(&kf); /* drop the kval edge; deliver v unchanged */
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
        kframe_release(&kf); /* drop kval: may free the discarded segment */
        v = unit();          /* start unwinding the freshly pushed KDefers */
        THxMEM_retain(v);    /* the new in-flight reference */
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
  nb->u.builtin.name = f->u.builtin.name;
  nb->u.builtin.arity = f->u.builtin.arity;
  nb->u.builtin.nargs = n + 1;
  Value **args = THxMEM_alloc((n + 1) * sizeof(Value *));
  for (size_t i = 0; i < n; i++) args[i] = f->u.builtin.args[i];
  args[n] = arg;
  for (size_t i = 0; i <= n; i++)
    THxMEM_retain(args[i]); /* the new callee owns its operand row */
  nb->u.builtin.args = args;
  return nb;
}

/* Apply `fn` to `arg` (the interpreter's App dispatch). Returns 1 to continue,
 * 0 when the run completed. Holds owned in-flight references on `fn` and `arg`
 * across the frame switch (either may live in the dying activation). */
static int do_apply(BlockFn *cur, Frame **fr, Value **in, Value *fn, Value *arg,
                    size_t base) {
  if (!fn) thrax_fault("apply: null callee");
  THxMEM_retain(fn);
  THxMEM_retain(arg);
  int r = 0;
  switch (fn->tag) {
    case T_CLOS:
      enter_clo(cur, fr, in, fn, &arg, 1);
      r = 1;
      break;
    case T_BUILTIN: {
      size_t nn = fn->u.builtin.nargs;
      if (nn + 1 == fn->u.builtin.arity) {
        /* borrow the operand row: fn (retained) keeps args[0..nn) alive, and
         * arg is retained; run_builtin only reads them and returns fresh (or a
         * child do_ret then retains). */
        Value **args = THxMEM_alloc((nn + 1) * sizeof(Value *));
        for (size_t i = 0; i < nn; i++) args[i] = fn->u.builtin.args[i];
        args[nn] = arg;
        Value *res = run_builtin(fn->u.builtin.name, args, nn + 1);
        THxMEM_free(args);
        r = do_ret(cur, fr, in, res, base);
      } else {
        r = do_ret(cur, fr, in, builtin_push(fn, arg), base);
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
      for (size_t i = g_kn; i-- > base;) {
        if (g_kont[i].tag != K_PROMPT) continue;
        for (size_t j = 0; j < g_kont[i].u.prompt.n; j++) {
          const char *ceff = g_kont[i].u.prompt.effs[j];
          const char *cop = g_kont[i].u.prompt.ops[j];
          bool m = strcmp(cop, op) == 0 &&
                   (!(eff && ceff) || strcmp(eff, ceff) == 0);
          if (m) {
            clause = g_kont[i].u.prompt.clauses[j];
            p = i;
            break;
          }
        }
        if (clause) break;
      }
      if (!clause) thrax_fault("unhandled effect operation");

      /* The slice's kont frames MOVE into the segment (their owned references
       * move with them; no counts change). */
      size_t n = g_kn - p;
      Resump *seg = THxMEM_alloc(sizeof(Resump));
      seg->rc = 1; /* owned by the kval below */
      seg->seg = THxMEM_alloc((n ? n : 1) * sizeof(KFrame));
      memcpy(seg->seg, &g_kont[p], n * sizeof(KFrame));
      seg->n = n;
      seg->used = 0;
      seg->escaped = 0;
      g_kn = p; /* the clause runs below the prompt (outside it) */

      Value *kval = alloc_value(T_RESUMP);
      kval->u.resump = seg; /* the value owns the segment's single reference */
      KFrame ac;
      ac.tag = K_AFTERCLAUSE;
      ac.u.afterclause.kval = kval;
      THxMEM_retain(kval); /* the kont stores it */
      kont_push(ac);

      Value *args[2];
      args[0] = arg;
      args[1] = kval;
      enter_clo(cur, fr, in, clause, args, 2);
      r = 1;
      break;
    }
    case T_RESUMP: {
      /* resume: splice the captured slice back on, deliver arg. Affine. The
       * kont frames MOVE back; the emptied segment stays behind as a husk. */
      Resump *seg = fn->u.resump;
      if (seg->used) thrax_fault("resumption used more than once (affine)");
      seg->used = 1;
      for (size_t i = 0; i < seg->n; i++) kont_push(seg->seg[i]);
      THxMEM_free(seg->seg);
      seg->seg = NULL;
      seg->n = 0;
      r = do_ret(cur, fr, in, arg, base);
      break;
    }
    default:
      thrax_fault("apply: callee is not a function");
  }
  THxMEM_release(fn);
  THxMEM_release(arg);
  return r;
}

static Value *run_loop(BlockFn cur, Frame *fr, Value *in, size_t base) {
  for (;;) {
    size_t pm = THxMEM_pool_mark(); /* this bounce's temporaries */
    g_act = ACT_NONE;
    cur(fr, in);
    int cont = 1;
    switch (g_act) {
      case ACT_JUMP: cur = g_blk; break;
      case ACT_RET: cont = do_ret(&cur, &fr, &in, g_v, base); break;
      case ACT_APPLY:
        cont = do_apply(&cur, &fr, &in, g_fn, g_arg, base);
        break;
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
  if (!do_apply(&cur, &fr, &in, f, arg, base)) return g_result;
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
    case T_OP: str_puts(s, "<function>"); break;
    case T_RESUMP: str_puts(s, "<continuation>"); break;
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
