/* The standard library's native support functions: the scalar conversions the
   language runtime needs but that are not plain C library calls. This is the
   SINGLE source of truth, shared by both engines: the native backend appends it
   to every emitted program (crates/ccg/src/lib.rs), and the interpreter compiles
   and links it into the `thrax` binary, resolving these symbols in-process
   through the normal `@extern "C" "..." ""` seam (see library/C.thx, CORE.thx).

   Everything here depends on libc only (snprintf/strtod/strtof, dirent), so it
   compiles standalone on native and, later, to a companion wasm module. A
   generated program already carries portable.h ahead of this file, so the
   include below is skipped there. */

#ifndef THX_PORTABLE_H
#include "portable.h"
#endif

#include <dirent.h>

/* Integer <-> real conversions. `@cast` crosses only integer widths, so these
   bridge the integer/real boundary. `long long`, not `long`: an `@int` is 64
   bits everywhere, and Windows' `long` is 32. */
THX_EXPORT double thx_i2d(long long n) { return (double)n; }
THX_EXPORT long long thx_d2i(double x) { return (long long)(x < 0 ? x - 0.5 : x + 0.5); }
THX_EXPORT float thx_i2f(long long n) { return (float)n; }
THX_EXPORT long long thx_f2i(float x) { return (long long)(x < 0 ? x - 0.5f : x + 0.5f); }

/* Float width conversions. `@float32` and `@float64` have distinct runtime
   representations, so widening/narrowing is real work (unlike the erased `@cast`
   between integer widths). */
THX_EXPORT double thx_f2d(float f) { return (double)f; }
THX_EXPORT float thx_d2f(double d) { return (float)d; }

/* Format a real as its shortest decimal that round-trips (the same rule as the
   runtime's `fmt_real`/`fmt_real32` display path, and Rust's `f64::to_string`,
   so both engines print a float identically). The result lives in a static
   buffer that the FFI copies into a Thrax `@str` immediately on return, so it
   need not outlive the call. */
THX_EXPORT const char *thx_real_to_str(double r) {
  static char buf[64];
  for (int prec = 1; prec <= 17; prec++) {
    snprintf(buf, sizeof(buf), "%.*g", prec, r);
    if (strtod(buf, NULL) == r) break;
  }
  return buf;
}
THX_EXPORT const char *thx_f32_to_str(float r) {
  static char buf[64];
  for (int prec = 1; prec <= 9; prec++) {
    snprintf(buf, sizeof(buf), "%.*g", prec, (double)r);
    if (strtof(buf, NULL) == r) break;
  }
  return buf;
}

/* Reinterpret a machine word as a raw pointer (`@ptr` travels as an Int), so
   the standard library can spell a null pointer as `C.null` (= i2p 0). */
THX_EXPORT void *thx_i2p(long long n) { return (void *)(intptr_t)n; }

/* A directory's entry names, `.` and `..` excluded, each followed by a newline,
   in whatever order the system lists them (`IO.read_dir` sorts). NULL when the
   directory cannot be opened. The FFI cannot walk a `struct dirent *`, whose
   layout differs per platform, so the walk happens here. Like the formatters
   above, the result lives in a static buffer the FFI copies on return. */
THX_EXPORT const char *thx_read_dir(const char *path) {
  static char *buf = NULL;
  static size_t cap = 0;
  DIR *dir = opendir(path);
  if (!dir) return NULL;
  size_t len = 0;
  struct dirent *e;
  while ((e = readdir(dir)) != NULL) {
    const char *name = e->d_name;
    if (strcmp(name, ".") == 0 || strcmp(name, "..") == 0) continue;
    size_t n = strlen(name);
    if (len + n + 2 > cap) {
      size_t grown = cap ? cap * 2 : 256;
      while (grown < len + n + 2) grown *= 2;
      char *p = realloc(buf, grown);
      if (!p) break;
      buf = p;
      cap = grown;
    }
    memcpy(buf + len, name, n);
    len += n;
    buf[len++] = '\n';
  }
  closedir(dir);
  if (!buf) return "";
  buf[len] = '\0';
  return buf;
}
