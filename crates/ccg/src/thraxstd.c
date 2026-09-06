/* The standard library's native support functions: the scalar conversions the
   language runtime needs but that are not plain C library calls. This is the
   SINGLE source of truth, shared by both engines: the native backend appends it
   to every emitted program (crates/ccg/src/lib.rs), and the interpreter compiles
   and links it into the `thrax` binary, resolving these symbols in-process
   through the normal `@extern "C" "..." ""` seam (see library/C.thx, CORE.thx).

   Everything here depends on libc only (snprintf/strtod/strtof), so it compiles
   standalone on native and, later, to a companion wasm module. */

#include <stdio.h>
#include <stdlib.h>

/* Integer <-> real conversions. `@cast` crosses only integer widths, so these
   bridge the integer/real boundary. */
double thx_i2d(long n) { return (double)n; }
long thx_d2i(double x) { return (long)(x < 0 ? x - 0.5 : x + 0.5); }
float thx_i2f(long n) { return (float)n; }
long thx_f2i(float x) { return (long)(x < 0 ? x - 0.5f : x + 0.5f); }

/* Float width conversions. `@float32` and `@float64` have distinct runtime
   representations, so widening/narrowing is real work (unlike the erased `@cast`
   between integer widths). */
double thx_f2d(float f) { return (double)f; }
float thx_d2f(double d) { return (float)d; }

/* Format a real as its shortest decimal that round-trips (the same rule as the
   runtime's `fmt_real`/`fmt_real32` display path, and Rust's `f64::to_string`,
   so both engines print a float identically). The result lives in a static
   buffer that the FFI copies into a Thrax `@str` immediately on return, so it
   need not outlive the call. */
const char *thx_real_to_str(double r) {
  static char buf[64];
  for (int prec = 1; prec <= 17; prec++) {
    snprintf(buf, sizeof(buf), "%.*g", prec, r);
    if (strtod(buf, NULL) == r) break;
  }
  return buf;
}
const char *thx_f32_to_str(float r) {
  static char buf[64];
  for (int prec = 1; prec <= 9; prec++) {
    snprintf(buf, sizeof(buf), "%.*g", prec, (double)r);
    if (strtof(buf, NULL) == r) break;
  }
  return buf;
}

/* Reinterpret a machine word as a raw pointer (`@ptr` travels as an Int), so
   the standard library can spell a null pointer as `C.null` (= i2p 0). */
void *thx_i2p(long n) { return (void *)n; }
