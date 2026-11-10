/* libares: the Ares compiler as a C library.
 *
 * Link `libares.a` or `libares.so`. A native Ares program links it on its own
 * when it uses `@lex`, `@parse`, `@parse_str`, `@format` or `@eval` at run
 * time; any other C program may call it directly.
 *
 * Values cross as an owned tree of `ares_value`. Every tree and error string
 * the library returns is the caller's to release with `ares_value_free` and
 * `ares_string_free`. Calls are not thread-safe: the compiler keeps per-thread
 * state, so use the library from one thread. */

#ifndef ARES_H
#define ARES_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
  ARES_UNIT = 0,
  ARES_INT = 1,     /* `int` */
  ARES_FLOAT64 = 2, /* `real` */
  ARES_FLOAT32 = 3, /* `real`, exactly representable as an f32 */
  ARES_BOOL = 4,    /* `int`: 0 or 1 */
  ARES_STR = 5,     /* `bytes`, `len` bytes, NUL-terminated past `len` */
  ARES_TUPLE = 6,   /* `items`, `len` of them */
  ARES_STRUCT = 7,  /* `name`; `keys[i]` names `items[i]`, `len` of each */
  ARES_VARIANT = 8, /* `name` is the union type, `tag` the variant; `items` */
  ARES_VEC = 9      /* `items`, `len` of them */
} ares_kind;

typedef struct ares_value ares_value;
struct ares_value {
  ares_kind kind;
  int64_t int_;
  double real;
  const uint8_t *bytes;
  const char *name;
  const char *tag;
  const char *const *keys;
  const ares_value *items;
  size_t len;
};

/* Tokenize `src`. Returns a ARES_VEC of `Token` variants, each tagged with its
 * kind and holding its lexeme as one ARES_STR item, or NULL with `*err` set on
 * a lex error. */
ares_value *ares_lex(const uint8_t *src, size_t len, char **err);

/* Check that `src` parses as `kind`: 0 an expression, 1 top-level items, 2 a
 * type, 3 effect-row entries. Returns 0 when it does, else 1 with `*err` set. */
int ares_parse(const uint8_t *src, size_t len, int kind, char **err);

/* Format the top-level items `src` as `ares fmt` does. Returns a ARES_STR,
 * or NULL with `*err` set when the text does not parse as items or the
 * formatter refuses it. */
ares_value *ares_format(const uint8_t *src, size_t len, char **err);

/* Compile and run the expression `src` against the standard library, returning
 * its value, or NULL with `*err` set. Only first-order values cross: a function
 * result is an error. Imports resolve against the working directory. */
ares_value *ares_eval(const uint8_t *src, size_t len, char **err);

void ares_value_free(ares_value *v);
void ares_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* ARES_H */
