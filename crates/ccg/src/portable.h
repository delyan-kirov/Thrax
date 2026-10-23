/* The C portability layer: what the runtime and the standard library's native
 * support functions need that differs between the platforms a Thrax program is
 * built for (Linux, macOS, Windows through MinGW-w64, wasm through emcc). The
 * emitter puts it ahead of the runtime in every generated program; thraxstd.c
 * includes it when compiled on its own. Platform conditionals belong here, not
 * scattered through the runtime. */
#ifndef THX_PORTABLE_H
#define THX_PORTABLE_H

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

#define THX_STR_(x) #x
#define THX_STR(x) THX_STR_(x)

/* The assembler name of the C symbol `name`, for an `__asm__` label. Mach-O
 * prefixes every C symbol with an underscore; ELF, PE/COFF on x86_64 and wasm
 * do not. */
#define THX_ASM_NAME(name) THX_STR(__USER_LABEL_PREFIX__) name

/* A PE executable has no dynamic symbol table to `-rdynamic` into, so the
 * functions the interpreter looks up in its own binary are exported by name. */
#ifdef _WIN32
#define THX_EXPORT __declspec(dllexport)
#else
#define THX_EXPORT
#endif

#ifdef THRAX_THREADS
#include <pthread.h>
#include <sched.h>
#include <unistd.h>
/* local-exec: the program is an executable, and under a PIC default the
 * general-dynamic model would call __tls_get_addr on every machine step. The
 * model is an ELF notion; elsewhere the default is already the cheap one. */
#if defined(__ELF__)
#define THX_TLS _Thread_local __attribute__((tls_model("local-exec")))
#else
#define THX_TLS _Thread_local
#endif

static inline int thx_nprocs(void) {
#ifdef _WIN32
  return pthread_num_processors_np();
#else
  return (int)sysconf(_SC_NPROCESSORS_ONLN);
#endif
}
#else
#define THX_TLS
#endif

/* The Windows C runtime turns every "\n" written to a text-mode stream into
 * "\r\n", so a compiled program's output would not match the interpreter's. */
static inline void thx_stdio_binary(void) {
#ifdef _WIN32
  _setmode(_fileno(stdout), _O_BINARY);
  _setmode(_fileno(stderr), _O_BINARY);
#endif
}

#endif
