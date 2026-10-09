/* libthrax: the Thrax compiler as a C library.
 *
 * Link `libthrax.a` or `libthrax.so`. A native Thrax program links it on its own
 * when it uses `@lex`, `@parse`, `@parse_str`, `@parse_items` or `@eval` at run
 * time; any other C program may call it directly.
 *
 * Values cross as an owned tree of `thrax_value`. Every tree and error string
 * the library returns is the caller's to release with `thrax_value_free` and
 * `thrax_string_free`. Calls are not thread-safe: the compiler keeps per-thread
 * state, so use the library from one thread. */

#ifndef THRAX_H
#define THRAX_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
  THRAX_UNIT = 0,
  THRAX_INT = 1,     /* `int` */
  THRAX_FLOAT64 = 2, /* `real` */
  THRAX_FLOAT32 = 3, /* `real`, exactly representable as an f32 */
  THRAX_BOOL = 4,    /* `int`: 0 or 1 */
  THRAX_STR = 5,     /* `bytes`, `len` bytes, NUL-terminated past `len` */
  THRAX_TUPLE = 6,   /* `items`, `len` of them */
  THRAX_STRUCT = 7,  /* `name`; `keys[i]` names `items[i]`, `len` of each */
  THRAX_VARIANT = 8, /* `name` is the union type, `tag` the variant; `items` */
  THRAX_VEC = 9      /* `items`, `len` of them */
} thrax_kind;

typedef struct thrax_value thrax_value;
struct thrax_value {
  thrax_kind kind;
  int64_t int_;
  double real;
  const uint8_t *bytes;
  const char *name;
  const char *tag;
  const char *const *keys;
  const thrax_value *items;
  size_t len;
};

/* Tokenize `src`. Returns a THRAX_VEC of `@token` structs, each with the string
 * fields `kind` and `text`, or NULL with `*err` set on a lex error. */
thrax_value *thrax_lex(const uint8_t *src, size_t len, char **err);

/* Check that `src` parses: as an expression, or as top-level items when
 * `items` is nonzero. Returns 0 when it does, else 1 with `*err` set. */
int thrax_parse(const uint8_t *src, size_t len, int items, char **err);

/* Compile and run the expression `src` against the standard library, returning
 * its value, or NULL with `*err` set. Only first-order values cross: a function
 * result is an error. Imports resolve against the working directory. */
thrax_value *thrax_eval(const uint8_t *src, size_t len, char **err);

void thrax_value_free(thrax_value *v);
void thrax_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* THRAX_H */
