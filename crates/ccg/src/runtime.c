/* Thrax C-backend runtime. Emitted verbatim ahead of the generated program by
 * the `ccg` crate; it is a hand port of the interpreter's value model and
 * built-ins (see crates/interpreter/src/eval.rs). Memory is bump-style: values
 * are malloc'd and never freed, which is correct for a program that runs to
 * completion and exits. */

#include <pthread.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <ucontext.h>

typedef struct Value Value;
typedef struct Env Env;
typedef struct Resump Resump;
typedef Value *(*Fn)(Env *env, Value *arg);

typedef enum {
  T_INT,
  T_REAL,
  T_STR, /* byte vector: both Str and Array */
  T_BOOL,
  T_UNIT,
  T_TUPLE,
  T_STRUCT,
  T_VARIANT,
  T_VEC, /* Vec `T (boxed elements) */
  T_CLOSURE,
  T_BUILTIN,
  T_OPERATION, /* an effect operation; applying it performs */
  T_RESUMPTION /* a captured one-shot continuation; applying it resumes */
} Tag;

typedef struct {
  const char *name;
  Value *val;
} Field;

struct Value {
  Tag tag;
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
      Fn fn;
      Env *env;
    } clo;
    struct {
      const char *name;
      size_t arity;
      Value **args;
      size_t nargs;
    } builtin;
    struct {
      const char *effect; /* NULL for an ambient (unqualified) operation */
      const char *op;
    } operation;
    Resump *resump;
  };
};

struct Env {
  const char *name;
  Value *val;
  Env *parent;
};

static void *xmalloc(size_t n) {
  void *p = malloc(n);
  if (!p) {
    fprintf(stderr, "thrax: out of memory\n");
    exit(70);
  }
  return p;
}

_Noreturn static Value *thrax_fault(const char *msg) {
  fprintf(stderr, "thrax: runtime fault: %s\n", msg);
  exit(1);
}

/* -- constructors -------------------------------------------------------- */

static Value *mk(Tag t) {
  Value *v = xmalloc(sizeof(Value));
  v->tag = t;
  return v;
}
static Value *mk_int(int64_t n) {
  Value *v = mk(T_INT);
  v->i = n;
  return v;
}
static Value *mk_real(double r) {
  Value *v = mk(T_REAL);
  v->r = r;
  return v;
}
static Value *mk_bool(bool b) {
  Value *v = mk(T_BOOL);
  v->b = b;
  return v;
}
static Value *mk_unit(void) { return mk(T_UNIT); }

static Value *mk_str(const uint8_t *data, size_t len) {
  Value *v = mk(T_STR);
  v->str.data = xmalloc(len ? len : 1);
  memcpy(v->str.data, data, len);
  v->str.len = len;
  return v;
}
/* Take ownership of an already-allocated buffer. */
static Value *mk_str_owned(uint8_t *data, size_t len) {
  Value *v = mk(T_STR);
  v->str.data = data;
  v->str.len = len;
  return v;
}

static Value *mk_tuple(Value **items, size_t len) {
  Value *v = mk(T_TUPLE);
  v->seq.items = items;
  v->seq.len = len;
  return v;
}
static Value *mk_vec(Value **items, size_t len) {
  Value *v = mk(T_VEC);
  v->seq.items = items;
  v->seq.len = len;
  return v;
}
static Value *mk_struct(const char *name, Field *fields, size_t len) {
  Value *v = mk(T_STRUCT);
  v->strct.name = name;
  v->strct.fields = fields;
  v->strct.len = len;
  return v;
}
static Value *mk_variant(const char *ty, const char *tag, Value **fields,
                         size_t len) {
  Value *v = mk(T_VARIANT);
  v->variant.ty = ty;
  v->variant.tag = tag;
  v->variant.fields = fields;
  v->variant.len = len;
  return v;
}
static Value *mk_closure(Fn fn, Env *env) {
  Value *v = mk(T_CLOSURE);
  v->clo.fn = fn;
  v->clo.env = env;
  return v;
}
static Value *mk_builtin(const char *name, size_t arity) {
  Value *v = mk(T_BUILTIN);
  v->builtin.name = name;
  v->builtin.arity = arity;
  v->builtin.args = NULL;
  v->builtin.nargs = 0;
  return v;
}
static Value *mk_operation(const char *effect, const char *op) {
  Value *v = mk(T_OPERATION);
  v->operation.effect = effect;
  v->operation.op = op;
  return v;
}
static Value *mk_resumption(Resump *r) {
  Value *v = mk(T_RESUMPTION);
  v->resump = r;
  return v;
}

/* -- environment --------------------------------------------------------- */

static Env *env_extend(Env *parent, const char *name, Value *val) {
  Env *e = xmalloc(sizeof(Env));
  e->name = name;
  e->val = val;
  e->parent = parent;
  return e;
}
static Value **env_slot(Env *env, const char *name) {
  for (; env; env = env->parent) {
    if (strcmp(env->name, name) == 0)
      return &env->val;
  }
  return NULL;
}

static Value *struct_field(Value *v, const char *name) {
  if (v->tag != T_STRUCT)
    return NULL;
  for (size_t i = 0; i < v->strct.len; i++)
    if (strcmp(v->strct.fields[i].name, name) == 0)
      return v->strct.fields[i].val;
  return NULL;
}

/* `record.field`: a struct field by name, or a tuple element by index. */
static Value *thrax_field(Value *v, const char *name) {
  if (v->tag == T_STRUCT) {
    Value *f = struct_field(v, name);
    if (f) return f;
    thrax_fault("no such field");
  }
  if (v->tag == T_TUPLE) {
    char *end;
    long idx = strtol(name, &end, 10);
    if (*end == '\0' && idx >= 0 && (size_t)idx < v->seq.len)
      return v->seq.items[idx];
    thrax_fault("no such tuple index");
  }
  thrax_fault("field access on a non-record");
}

/* Record update: the base struct's fields seeded, then `extra` overriding
 * existing names or extending. An empty `name` keeps the base's type name. */
static Value *mk_struct_update(Value *base, const char *name, Field *extra,
                              size_t nextra) {
  if (base->tag != T_STRUCT)
    thrax_fault("record update of a non-struct value");
  size_t cap = base->strct.len + nextra;
  Field *fields = xmalloc((cap ? cap : 1) * sizeof(Field));
  size_t len = base->strct.len;
  memcpy(fields, base->strct.fields, len * sizeof(Field));
  for (size_t i = 0; i < nextra; i++) {
    bool found = false;
    for (size_t j = 0; j < len; j++)
      if (strcmp(fields[j].name, extra[i].name) == 0) {
        fields[j].val = extra[i].val;
        found = true;
        break;
      }
    if (!found)
      fields[len++] = extra[i];
  }
  const char *nm = (name && name[0]) ? name : base->strct.name;
  return mk_struct(nm, fields, len);
}

/* -- coercions (faults on the wrong kind, like the interpreter) ---------- */

static int64_t as_i64(Value *v) {
  if (v->tag != T_INT)
    thrax_fault("expected an integer");
  return v->i;
}
static double as_f64(Value *v) {
  if (v->tag == T_INT)
    return (double)v->i;
  if (v->tag == T_REAL)
    return v->r;
  thrax_fault("expected a number");
}
static size_t as_index(Value *v) {
  if (v->tag != T_INT)
    thrax_fault("expected an integer index");
  if (v->i < 0)
    thrax_fault("negative index");
  return (size_t)v->i;
}
static uint8_t as_byte(Value *v) {
  if (v->tag != T_INT || v->i < 0 || v->i > 255)
    thrax_fault("expected a byte value (0..255)");
  return (uint8_t)v->i;
}
static Value *as_str(Value *v) {
  if (v->tag != T_STR)
    thrax_fault("expected a byte vector");
  return v;
}

/* -- structural equality (?=) -------------------------------------------- */

static bool value_eq(Value *x, Value *y) {
  bool xnum = x->tag == T_INT || x->tag == T_REAL;
  bool ynum = y->tag == T_INT || y->tag == T_REAL;
  if (xnum && ynum)
    return as_f64(x) == as_f64(y);
  if (x->tag != y->tag)
    return false;
  switch (x->tag) {
    case T_STR:
      return x->str.len == y->str.len &&
             memcmp(x->str.data, y->str.data, x->str.len) == 0;
    case T_BOOL:
      return x->b == y->b;
    case T_UNIT:
      return true;
    case T_TUPLE:
    case T_VEC:
      if (x->seq.len != y->seq.len)
        return false;
      for (size_t i = 0; i < x->seq.len; i++)
        if (!value_eq(x->seq.items[i], y->seq.items[i]))
          return false;
      return true;
    case T_STRUCT: {
      if (x->strct.len != y->strct.len)
        return false;
      for (size_t i = 0; i < x->strct.len; i++) {
        Value *w = struct_field(y, x->strct.fields[i].name);
        if (!w || !value_eq(x->strct.fields[i].val, w))
          return false;
      }
      return true;
    }
    case T_VARIANT:
      if (strcmp(x->variant.tag, y->variant.tag) != 0 ||
          x->variant.len != y->variant.len)
        return false;
      for (size_t i = 0; i < x->variant.len; i++)
        if (!value_eq(x->variant.fields[i], y->variant.fields[i]))
          return false;
      return true;
    default:
      return false;
  }
}

/* -- built-in operators -------------------------------------------------- */

static Value *arith(const char *op, Value *x, Value *y) {
  if (x->tag == T_INT && y->tag == T_INT) {
    int64_t a = x->i, b = y->i, r;
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
    return mk_int(r);
  }
  double a = as_f64(x), b = as_f64(y), r;
  switch (op[0]) {
    case '+': r = a + b; break;
    case '-': r = a - b; break;
    case '*': r = a * b; break;
    case '/': r = a / b; break;
    default: {
      /* Rust f64 `%` is the C fmod. */
      r = a - b * (double)(long long)(a / b);
      break;
    }
  }
  return mk_real(r);
}

static int cmp_bytes(Value *a, Value *b) {
  size_t n = a->str.len < b->str.len ? a->str.len : b->str.len;
  int c = memcmp(a->str.data, b->str.data, n);
  if (c != 0)
    return c < 0 ? -1 : 1;
  if (a->str.len == b->str.len)
    return 0;
  return a->str.len < b->str.len ? -1 : 1;
}

static Value *compare(const char *op, Value *x, Value *y) {
  int ord; /* -1, 0, 1 */
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
  return mk_bool(r);
}

static Value *list_append(Value *xs, Value *ys) {
  if (xs->tag == T_VARIANT && strcmp(xs->variant.tag, "Cons") == 0) {
    Value **fields = xmalloc(2 * sizeof(Value *));
    fields[0] = xs->variant.fields[0];
    fields[1] = list_append(xs->variant.fields[1], ys);
    return mk_variant("List", "Cons", fields, 2);
  }
  return ys;
}

static Value *concat(Value *x, Value *y) {
  if (x->tag == T_STR && y->tag == T_STR) {
    size_t len = x->str.len + y->str.len;
    uint8_t *data = xmalloc(len ? len : 1);
    memcpy(data, x->str.data, x->str.len);
    memcpy(data + x->str.len, y->str.data, y->str.len);
    return mk_str_owned(data, len);
  }
  if (x->tag == T_VARIANT && strcmp(x->variant.ty, "List") == 0)
    return list_append(x, y);
  thrax_fault("`++` on unsupported operands");
}

/* Forward: the auto-injected `C` namespace. */
static Value *run_c(const char *name, Value **a, size_t n);

static Value *run_builtin(const char *name, Value **a, size_t n) {
  (void)n;
  if (strcmp(name, "+") == 0 || strcmp(name, "-") == 0 ||
      strcmp(name, "*") == 0 || strcmp(name, "/") == 0 ||
      strcmp(name, "%") == 0)
    return arith(name, a[0], a[1]);
  if (strcmp(name, "neg") == 0) {
    if (a[0]->tag == T_INT) return mk_int(-a[0]->i);
    if (a[0]->tag == T_REAL) return mk_real(-a[0]->r);
    thrax_fault("`neg` on a non-number");
  }
  if (strcmp(name, "not") == 0) {
    if (a[0]->tag == T_BOOL) return mk_bool(!a[0]->b);
    thrax_fault("`not` on a non-boolean");
  }
  if (strcmp(name, "?=") == 0) return mk_bool(value_eq(a[0], a[1]));
  if (strcmp(name, "?<") == 0 || strcmp(name, "?>") == 0 ||
      strcmp(name, "<=") == 0 || strcmp(name, ">=") == 0)
    return compare(name, a[0], a[1]);
  if (strcmp(name, "++") == 0) return concat(a[0], a[1]);

  if (strcmp(name, "array_alloc") == 0) {
    size_t len = as_index(a[0]);
    uint8_t *data = xmalloc(len ? len : 1);
    memset(data, 0, len);
    return mk_str_owned(data, len);
  }
  if (strcmp(name, "array_len") == 0) return mk_int((int64_t)as_str(a[0])->str.len);
  if (strcmp(name, "array_get") == 0) {
    Value *s = as_str(a[0]);
    size_t i = as_index(a[1]);
    if (i >= s->str.len) thrax_fault("array index out of bounds");
    return mk_int(s->str.data[i]);
  }
  if (strcmp(name, "array_push") == 0) {
    Value *s = as_str(a[0]);
    size_t len = s->str.len + 1;
    uint8_t *data = xmalloc(len);
    memcpy(data, s->str.data, s->str.len);
    data[s->str.len] = as_byte(a[1]);
    return mk_str_owned(data, len);
  }
  if (strcmp(name, "array_set") == 0) {
    Value *s = as_str(a[0]);
    size_t i = as_index(a[1]);
    if (i >= s->str.len) thrax_fault("array index out of bounds");
    uint8_t *data = xmalloc(s->str.len ? s->str.len : 1);
    memcpy(data, s->str.data, s->str.len);
    data[i] = as_byte(a[2]);
    return mk_str_owned(data, s->str.len);
  }
  if (strcmp(name, "array_slice") == 0) {
    Value *s = as_str(a[0]);
    size_t beg = as_index(a[1]);
    size_t end = as_index(a[2]);
    if (beg > s->str.len) beg = s->str.len;
    if (end < beg) end = beg;
    if (end > s->str.len) end = s->str.len;
    return mk_str(s->str.data + beg, end - beg);
  }

  if (strcmp(name, "vec_new") == 0) return mk_vec(NULL, 0);
  if (strcmp(name, "vec_fill") == 0) {
    size_t len = as_index(a[0]);
    Value **items = len ? xmalloc(len * sizeof(Value *)) : NULL;
    for (size_t i = 0; i < len; i++) items[i] = a[1];
    return mk_vec(items, len);
  }
  if (strcmp(name, "vec_len") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    return mk_int((int64_t)a[0]->seq.len);
  }
  if (strcmp(name, "vec_get") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t i = as_index(a[1]);
    if (i >= a[0]->seq.len) thrax_fault("vec index out of bounds");
    return a[0]->seq.items[i];
  }
  if (strcmp(name, "vec_push") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t len = a[0]->seq.len + 1;
    Value **items = xmalloc(len * sizeof(Value *));
    memcpy(items, a[0]->seq.items, a[0]->seq.len * sizeof(Value *));
    items[a[0]->seq.len] = a[1];
    return mk_vec(items, len);
  }
  if (strcmp(name, "vec_set") == 0) {
    if (a[0]->tag != T_VEC) thrax_fault("expected a vector");
    size_t i = as_index(a[1]);
    if (i >= a[0]->seq.len) thrax_fault("vec index out of bounds");
    Value **items = xmalloc((a[0]->seq.len ? a[0]->seq.len : 1) * sizeof(Value *));
    memcpy(items, a[0]->seq.items, a[0]->seq.len * sizeof(Value *));
    items[i] = a[2];
    return mk_vec(items, a[0]->seq.len);
  }

  if (strncmp(name, "C.", 2) == 0) return run_c(name, a, n);
  thrax_fault("unknown built-in");
}

/* -- the auto-injected `C` libc namespace -------------------------------- */

#define MAX_FILES 256
static FILE *thrax_files[MAX_FILES];
static int64_t thrax_next_fd = 1;

static char *cstr_of(Value *v) {
  Value *s = as_str(v);
  char *out = xmalloc(s->str.len + 1);
  memcpy(out, s->str.data, s->str.len);
  out[s->str.len] = '\0';
  return out;
}

static Value *run_c(const char *name, Value **a, size_t n) {
  (void)n;
  const char *fn = name + 2; /* skip "C." */
  if (strcmp(fn, "getenv") == 0) {
    char *key = cstr_of(a[0]);
    const char *val = getenv(key);
    if (!val) val = "";
    return mk_str((const uint8_t *)val, strlen(val));
  }
  if (strcmp(fn, "fopen") == 0) {
    char *path = cstr_of(a[0]);
    char *mode = cstr_of(a[1]);
    FILE *f = fopen(path, mode);
    if (!f) return mk_int(0);
    if (thrax_next_fd >= MAX_FILES) {
      fclose(f);
      return mk_int(0);
    }
    int64_t id = thrax_next_fd++;
    thrax_files[id] = f;
    return mk_int(id);
  }
  if (strcmp(fn, "fclose") == 0) {
    int64_t id = as_i64(a[0]);
    if (id > 0 && id < MAX_FILES && thrax_files[id]) {
      fclose(thrax_files[id]);
      thrax_files[id] = NULL;
      return mk_int(0);
    }
    return mk_int(-1);
  }
  if (strcmp(fn, "fgetc") == 0) {
    int64_t id = as_i64(a[0]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return mk_int(-1);
    int c = fgetc(thrax_files[id]);
    return mk_int(c == EOF ? -1 : c);
  }
  if (strcmp(fn, "fseek") == 0) {
    int64_t id = as_i64(a[0]), off = as_i64(a[1]), whence = as_i64(a[2]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return mk_int(-1);
    int w = whence == 1 ? SEEK_CUR : (whence == 2 ? SEEK_END : SEEK_SET);
    return mk_int(fseek(thrax_files[id], (long)off, w) == 0 ? 0 : -1);
  }
  if (strcmp(fn, "ftell") == 0) {
    int64_t id = as_i64(a[0]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return mk_int(-1);
    long p = ftell(thrax_files[id]);
    return mk_int(p < 0 ? -1 : (int64_t)p);
  }
  if (strcmp(fn, "fputs") == 0) {
    Value *s = as_str(a[0]);
    int64_t id = as_i64(a[1]);
    if (id <= 0 || id >= MAX_FILES || !thrax_files[id]) return mk_int(-1);
    size_t wrote = fwrite(s->str.data, 1, s->str.len, thrax_files[id]);
    return mk_int(wrote == s->str.len ? 0 : -1);
  }
  if (strcmp(fn, "remove") == 0) {
    char *path = cstr_of(a[0]);
    return mk_int(remove(path) == 0 ? 0 : -1);
  }
  if (strcmp(fn, "write") == 0) {
    int64_t fd = as_i64(a[0]);
    Value *s = as_str(a[1]);
    size_t len = as_index(a[2]);
    if (len > s->str.len) len = s->str.len;
    FILE *out = fd == 2 ? stderr : stdout;
    size_t wrote = fwrite(s->str.data, 1, len, out);
    return mk_int(wrote == len ? (int64_t)len : -1);
  }
  if (strcmp(fn, "getchar") == 0) {
    int c = getchar();
    return mk_int(c == EOF ? -1 : c);
  }
  if (strcmp(fn, "time") == 0) {
    return mk_int((int64_t)time(NULL));
  }
  thrax_fault("unsupported C function");
}

/* -- TARGET reflection --------------------------------------------------- */

static Value *target_value(const char *name) {
  if (strcmp(name, "int_bits") == 0 || strcmp(name, "ptr_bits") == 0)
    return mk_int((int64_t)(sizeof(size_t) * 8));
  if (strcmp(name, "int_max") == 0) return mk_int(INT64_MAX);
  if (strcmp(name, "int_min") == 0) return mk_int(INT64_MIN);
  if (strcmp(name, "arch") == 0)
    return mk_str((const uint8_t *)THRAX_ARCH, strlen(THRAX_ARCH));
  if (strcmp(name, "os") == 0)
    return mk_str((const uint8_t *)THRAX_OS, strlen(THRAX_OS));
  if (strcmp(name, "name") == 0)
    return mk_str((const uint8_t *)THRAX_ARCH "-" THRAX_OS,
                  strlen(THRAX_ARCH "-" THRAX_OS));
  return NULL;
}

static size_t builtin_arity(const char *name) {
  if (!strcmp(name, "not") || !strcmp(name, "neg") || !strcmp(name, "array_len") ||
      !strcmp(name, "array_alloc") || !strcmp(name, "vec_len") ||
      !strcmp(name, "vec_new"))
    return 1;
  if (!strcmp(name, "+") || !strcmp(name, "-") || !strcmp(name, "*") ||
      !strcmp(name, "/") || !strcmp(name, "%") || !strcmp(name, "?=") ||
      !strcmp(name, "?<") || !strcmp(name, "?>") || !strcmp(name, "<=") ||
      !strcmp(name, ">=") || !strcmp(name, "++") || !strcmp(name, "array_get") ||
      !strcmp(name, "array_push") || !strcmp(name, "vec_get") ||
      !strcmp(name, "vec_push") || !strcmp(name, "vec_fill"))
    return 2;
  if (!strcmp(name, "array_set") || !strcmp(name, "array_slice") ||
      !strcmp(name, "vec_set"))
    return 3;
  return (size_t)-1;
}

static size_t c_arity(const char *name) {
  if (!strcmp(name, "getenv") || !strcmp(name, "fclose") ||
      !strcmp(name, "fgetc") || !strcmp(name, "ftell") ||
      !strcmp(name, "remove") || !strcmp(name, "getchar") ||
      !strcmp(name, "time"))
    return 1;
  if (!strcmp(name, "fopen") || !strcmp(name, "fputs")) return 2;
  if (!strcmp(name, "fseek") || !strcmp(name, "write")) return 3;
  return (size_t)-1;
}

/* -- application --------------------------------------------------------- */

/* The effect engine (defined below). Applying an operation performs it;
 * applying a resumption resumes the captured computation once. */
static Value *thrax_perform(const char *effect, const char *op, Value *arg);
static Value *thrax_resume(Resump *rk, Value *v);

static Value *apply(Value *f, Value *x) {
  if (f->tag == T_CLOSURE)
    return f->clo.fn(f->clo.env, x);
  if (f->tag == T_BUILTIN) {
    size_t n = f->builtin.nargs;
    Value **args = xmalloc((n + 1) * sizeof(Value *));
    for (size_t i = 0; i < n; i++) args[i] = f->builtin.args[i];
    args[n] = x;
    if (n + 1 == f->builtin.arity)
      return run_builtin(f->builtin.name, args, n + 1);
    Value *v = mk(T_BUILTIN);
    v->builtin.name = f->builtin.name;
    v->builtin.arity = f->builtin.arity;
    v->builtin.args = args;
    v->builtin.nargs = n + 1;
    return v;
  }
  if (f->tag == T_OPERATION)
    return thrax_perform(f->operation.effect, f->operation.op, x);
  if (f->tag == T_RESUMPTION)
    return thrax_resume(f->resump, x);
  thrax_fault("applied a non-function value");
}

/* -- algebraic effects: a fiber-based engine ----------------------------- */
/* Each `handle` runs its body on a fresh ucontext fiber. A performed operation
 * swaps back to the matching handler's driver, delivering the op; the suspended
 * fiber is the resumption. Resuming swaps back into the fiber (deep: the handler
 * is reinstalled around the continuation). This mirrors the interpreter's CPS
 * machine; the one-shot continuations may be stored and resumed from anywhere.
 *
 * `defer` cleanups live on the fiber that registered them and run on normal
 * completion; a handler that abandons a continuation runs the still-pending
 * cleanups of its fiber (found by a reachability scan of the clause result, in
 * place of the interpreter's Rc strong-count check). */

#define FIBER_STACK (1 << 20)

typedef struct Fiber {
  ucontext_t ctx;
  Value **cleanups; /* stack of `\_ = cleanup` closures (defer) */
  size_t ncleanups, capcleanups;
} Fiber;

typedef struct {
  const char *effect; /* NULL = ambient clause */
  const char *op;
  Value *clause; /* closure `\arg = \k = body` */
} HClause;

typedef struct HandlerV {
  HClause *clauses;
  size_t n;
  Value *deflt; /* closure `\x = body`, or NULL */
} HandlerV;

typedef struct Frame {
  HandlerV *h;
  ucontext_t drive; /* where to switch to reach this handle's driver */
  struct Frame *parent;
  Fiber *fiber;
} Frame;

struct Resump {
  Fiber *fiber;
  HandlerV *h; /* the handler, reinstalled on resume (deep) */
  bool used;
};

/* Single-threaded machine registers (the whole program runs on one pthread). */
static Fiber g_main_fiber;
static Fiber *g_cur_fiber = &g_main_fiber;
static Frame *g_frames = NULL;
static ucontext_t *g_park; /* where the current fiber returns control */
static Value *g_next_body; /* handoff: the body to start a fiber with */
enum { SIG_RETURN, SIG_PERFORM };
static int g_sig;
static Value *g_hv; /* handoff: return value / perform arg / resume value */
static const char *g_eff;
static const char *g_op_name;

static HandlerV *thrax_handler_new(size_t n) {
  HandlerV *h = xmalloc(sizeof(HandlerV));
  h->clauses = n ? xmalloc(n * sizeof(HClause)) : NULL;
  h->n = n;
  h->deflt = NULL;
  return h;
}
static void thrax_handler_set(HandlerV *h, size_t i, const char *effect,
                              const char *op, Value *clause) {
  h->clauses[i].effect = effect;
  h->clauses[i].op = op;
  h->clauses[i].clause = clause;
}
static void thrax_handler_default(HandlerV *h, Value *clo) { h->deflt = clo; }

static bool clause_matches(const char *c_eff, const char *c_op,
                           const char *p_eff, const char *p_op) {
  if (strcmp(c_op, p_op) != 0)
    return false;
  if (p_eff && c_eff)
    return strcmp(p_eff, c_eff) == 0;
  return true;
}
static HClause *find_clause(HandlerV *h, const char *eff, const char *op) {
  for (size_t i = 0; i < h->n; i++)
    if (clause_matches(h->clauses[i].effect, h->clauses[i].op, eff, op))
      return &h->clauses[i];
  return NULL;
}

static void fiber_trampoline(void) {
  Value *body = g_next_body;
  Value *v = apply(body, mk_unit());
  g_sig = SIG_RETURN;
  g_hv = v;
  swapcontext(&g_cur_fiber->ctx, g_park); /* body done: back to the driver */
}

static Fiber *fiber_new(void) {
  Fiber *f = xmalloc(sizeof(Fiber));
  f->cleanups = NULL;
  f->ncleanups = f->capcleanups = 0;
  getcontext(&f->ctx);
  f->ctx.uc_stack.ss_sp = xmalloc(FIBER_STACK);
  f->ctx.uc_stack.ss_size = FIBER_STACK;
  f->ctx.uc_link = NULL;
  makecontext(&f->ctx, fiber_trampoline, 0);
  return f;
}

/* Is `rk`'s continuation reachable from `v`? If so a clause stored it (to resume
 * later) rather than abandoning it. */
static bool reachable_resump(Value *v, Resump *rk, int depth) {
  if (!v || depth > 100000)
    return false;
  switch (v->tag) {
    case T_RESUMPTION:
      return v->resump == rk;
    case T_TUPLE:
    case T_VEC:
      for (size_t i = 0; i < v->seq.len; i++)
        if (reachable_resump(v->seq.items[i], rk, depth + 1))
          return true;
      return false;
    case T_STRUCT:
      for (size_t i = 0; i < v->strct.len; i++)
        if (reachable_resump(v->strct.fields[i].val, rk, depth + 1))
          return true;
      return false;
    case T_VARIANT:
      for (size_t i = 0; i < v->variant.len; i++)
        if (reachable_resump(v->variant.fields[i], rk, depth + 1))
          return true;
      return false;
    default:
      return false;
  }
}

static void run_finalizers_of(Resump *rk) {
  Fiber *f = rk->fiber;
  while (f->ncleanups > 0)
    apply(f->cleanups[--f->ncleanups], mk_unit());
}

/* Drive a fiber under handler frame `fr`, either starting its body or delivering
 * a resume value. Returns once the fiber returns (default applied) or performs an
 * operation this handler matches (its clause run). */
static Value *drive(Frame *fr, bool starting, Value *deliver) {
  Fiber *f = fr->fiber;
  Frame *saved_frames = g_frames; /* == fr->parent */
  Fiber *saved_cur = g_cur_fiber;
  ucontext_t *saved_park = g_park;

  g_frames = fr;
  g_cur_fiber = f;
  g_park = &fr->drive;
  if (starting)
    g_next_body = deliver;
  else
    g_hv = deliver;

  swapcontext(&fr->drive, &f->ctx);

  g_cur_fiber = saved_cur;
  g_park = saved_park;
  g_frames = fr->parent; /* this handler is popped while its clause runs */

  Value *out;
  if (g_sig == SIG_RETURN) {
    Value *v = g_hv;
    out = fr->h->deflt ? apply(fr->h->deflt, v) : v;
  } else {
    Value *arg = g_hv;
    Resump *rk = xmalloc(sizeof(Resump));
    rk->fiber = f;
    rk->h = fr->h;
    rk->used = false;
    Value *k = mk_resumption(rk);
    HClause *c = find_clause(fr->h, g_eff, g_op_name);
    if (!c)
      thrax_fault("no handler clause matched a performed operation");
    out = apply(apply(c->clause, arg), k);
    if (!rk->used && !reachable_resump(out, rk, 0))
      run_finalizers_of(rk);
  }
  g_frames = saved_frames;
  return out;
}

static Value *thrax_handle(Value *body, HandlerV *h) {
  Frame *fr = xmalloc(sizeof(Frame));
  fr->h = h;
  fr->parent = g_frames;
  fr->fiber = fiber_new();
  return drive(fr, true, body);
}

static Value *thrax_resume(Resump *rk, Value *v) {
  if (rk->used)
    thrax_fault("continuation resumed more than once");
  rk->used = true;
  Frame *fr = xmalloc(sizeof(Frame));
  fr->h = rk->h;
  fr->parent = g_frames;
  fr->fiber = rk->fiber;
  return drive(fr, false, v);
}

static Value *thrax_perform(const char *effect, const char *op, Value *arg) {
  Frame *fr = g_frames;
  while (fr && !find_clause(fr->h, effect, op))
    fr = fr->parent;
  if (!fr)
    thrax_fault("unhandled effect operation");
  g_sig = SIG_PERFORM;
  g_eff = effect;
  g_op_name = op;
  g_hv = arg;
  swapcontext(&g_cur_fiber->ctx, &fr->drive);
  return g_hv; /* resumed with this value */
}

static void thrax_defer_push(Value *cleanup) {
  Fiber *f = g_cur_fiber;
  if (f->ncleanups == f->capcleanups) {
    f->capcleanups = f->capcleanups ? f->capcleanups * 2 : 4;
    f->cleanups = realloc(f->cleanups, f->capcleanups * sizeof(Value *));
    if (!f->cleanups) {
      fprintf(stderr, "thrax: out of memory\n");
      exit(70);
    }
  }
  f->cleanups[f->ncleanups++] = cleanup;
}
static void thrax_defer_run_top(void) {
  Fiber *f = g_cur_fiber;
  if (f->ncleanups > 0)
    apply(f->cleanups[--f->ncleanups], mk_unit());
}

/* -- global resolution --------------------------------------------------- */

typedef struct {
  const char *key;  /* "Module.name" */
  const char *bare; /* "name" */
  Value *(*force)(void);
} Global;

typedef struct {
  const char *effect;
  const char *op;
} OpDecl;

/* Defined by the generated part of the program (after the globals table). */
static const Global *thrax_globals(void);
static size_t thrax_nglobals(void);
static const OpDecl *thrax_ops(void);
static size_t thrax_nops(void);

/* Resolve a name to an effect operation value, or NULL if it names none. A
 * qualified `Effect.op` picks that effect; a bare `op` picks the sole effect
 * declaring it, else stays ambient (NULL effect, resolved by the handler). */
static Value *resolve_operation(const char *module, const char *name) {
  const OpDecl *ops = thrax_ops();
  size_t n = thrax_nops();
  if (module) {
    for (size_t i = 0; i < n; i++)
      if (strcmp(ops[i].effect, module) == 0 && strcmp(ops[i].op, name) == 0)
        return mk_operation(ops[i].effect, name);
    return NULL;
  }
  const char *only = NULL;
  int count = 0;
  for (size_t i = 0; i < n; i++)
    if (strcmp(ops[i].op, name) == 0) {
      only = ops[i].effect;
      count++;
    }
  if (count == 0)
    return NULL;
  return mk_operation(count == 1 ? only : NULL, name);
}

static Value *force_key(const char *key) {
  const Global *g = thrax_globals();
  size_t n = thrax_nglobals();
  for (size_t i = 0; i < n; i++)
    if (strcmp(g[i].key, key) == 0)
      return g[i].force();
  return NULL;
}

static Value *resolve_var(Env *env, const char *module, const char *name) {
  if (!module) {
    Value **slot = env_slot(env, name);
    if (slot) return *slot;
  }
  if (module) {
    char key[256];
    snprintf(key, sizeof(key), "%s.%s", module, name);
    Value *g = force_key(key);
    if (g) return g;
  }
  const Global *gs = thrax_globals();
  size_t n = thrax_nglobals();
  for (size_t i = 0; i < n; i++)
    if (strcmp(gs[i].bare, name) == 0)
      return gs[i].force();
  size_t ar = builtin_arity(name);
  if (ar != (size_t)-1) return mk_builtin(name, ar);
  if (module && strcmp(module, "TARGET") == 0) {
    Value *v = target_value(name);
    if (v) return v;
  }
  if (module && strcmp(module, "C") == 0) {
    size_t ca = c_arity(name);
    if (ca != (size_t)-1) {
      char *full = xmalloc(strlen(name) + 3);
      memcpy(full, "C.", 2);
      strcpy(full + 2, name);
      return mk_builtin(full, ca);
    }
  }
  Value *op = resolve_operation(module, name);
  if (op)
    return op;
  thrax_fault("unbound name");
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
    if (!s->buf) {
      fprintf(stderr, "thrax: out of memory\n");
      exit(70);
    }
  }
  memcpy(s->buf + s->len, bytes, n);
  s->len += n;
  s->buf[s->len] = '\0';
}
static void str_puts(Str *s, const char *z) { str_push(s, z, strlen(z)); }

/* Rust's shortest round-tripping float formatting: the least precision whose
 * printout parses back to the same bits. Also mirror the `.0` on integral
 * reals that Rust's Display prints. */
static void fmt_real(Str *s, double r) {
  char tmp[64];
  for (int prec = 1; prec <= 17; prec++) {
    snprintf(tmp, sizeof(tmp), "%.*g", prec, r);
    if (strtod(tmp, NULL) == r) break;
  }
  str_puts(s, tmp);
  /* Rust prints e.g. `2` reals as `2`, but Display on f64 keeps them integral
   * without a trailing `.0`; %g already drops it, so nothing to add. */
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
      snprintf(num, sizeof(num), "%lld", (long long)v->i);
      str_puts(s, num);
      break;
    case T_REAL:
      fmt_real(s, v->r);
      break;
    case T_STR:
      show_bytes_debug(s, v->str.data, v->str.len);
      break;
    case T_BOOL:
      str_puts(s, v->b ? "true" : "false");
      break;
    case T_UNIT:
      str_puts(s, "{}");
      break;
    case T_TUPLE:
      str_puts(s, "{");
      for (size_t i = 0; i < v->seq.len; i++) {
        if (i) str_puts(s, ", ");
        show_into(s, v->seq.items[i]);
      }
      str_puts(s, "}");
      break;
    case T_STRUCT:
      str_puts(s, v->strct.name);
      str_puts(s, ".{ ");
      for (size_t i = 0; i < v->strct.len; i++) {
        if (i) str_puts(s, ", ");
        str_puts(s, ".");
        str_puts(s, v->strct.fields[i].name);
        str_puts(s, " = ");
        show_into(s, v->strct.fields[i].val);
      }
      str_puts(s, " }");
      break;
    case T_VARIANT:
      str_puts(s, ".");
      str_puts(s, v->variant.tag);
      if (v->variant.len) {
        str_puts(s, ".{ ");
        for (size_t i = 0; i < v->variant.len; i++) {
          if (i) str_puts(s, ", ");
          show_into(s, v->variant.fields[i]);
        }
        str_puts(s, " }");
      }
      break;
    case T_VEC:
      str_puts(s, "vec[");
      for (size_t i = 0; i < v->seq.len; i++) {
        if (i) str_puts(s, ", ");
        show_into(s, v->seq.items[i]);
      }
      str_puts(s, "]");
      break;
    case T_CLOSURE:
    case T_BUILTIN:
    case T_OPERATION:
      str_puts(s, "<function>");
      break;
    case T_RESUMPTION:
      str_puts(s, "<continuation>");
      break;
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
