/* The C portability layer: what the runtime and the standard library's native
 * support functions need that differs between the platforms a Ares program is
 * built for (Linux, macOS, Windows through MinGW-w64, wasm through emcc). The
 * emitter puts it ahead of the runtime in every generated program; aresstd.c
 * includes it when compiled on its own. Platform conditionals belong here, not
 * scattered through the runtime. */
#ifndef ARX_PORTABLE_H
#define ARX_PORTABLE_H

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

#define ARX_STR_(x) #x
#define ARX_STR(x) ARX_STR_(x)

/* The assembler name of the C symbol `name`, for an `__asm__` label. Mach-O
 * prefixes every C symbol with an underscore; ELF, PE/COFF on x86_64 and wasm
 * do not. */
#define ARX_ASM_NAME(name) ARX_STR(__USER_LABEL_PREFIX__) name

/* A PE executable has no dynamic symbol table to `-rdynamic` into, so the
 * functions the interpreter looks up in its own binary are exported by name. */
#ifdef _WIN32
#define ARX_EXPORT __declspec(dllexport)
#else
#define ARX_EXPORT
#endif

#ifdef ARES_THREADS
#include <pthread.h>
#include <sched.h>
#include <unistd.h>
/* local-exec: the program is an executable, and under a PIC default the
 * general-dynamic model would call __tls_get_addr on every machine step. The
 * model is an ELF notion; elsewhere the default is already the cheap one. */
#if defined(__ELF__)
#define ARX_TLS _Thread_local __attribute__((tls_model("local-exec")))
#else
#define ARX_TLS _Thread_local
#endif

static inline int arx_nprocs(void) {
#ifdef _WIN32
  return pthread_num_processors_np();
#else
  return (int)sysconf(_SC_NPROCESSORS_ONLN);
#endif
}
#else
#define ARX_TLS
#endif

/* How much C stack the direct calls of compiled functions may use, per
 * thread, before calls fall back to the machine and its heap continuation.
 * A wasm stack is small, and macOS and Windows give a secondary thread less
 * than Linux does. */
#ifndef ARX_DIRECT_STACK
#if defined(__wasm__)
#define ARX_DIRECT_STACK (24u * 1024u)
#elif defined(_WIN32) || defined(__APPLE__)
#define ARX_DIRECT_STACK (256u * 1024u)
#else
#define ARX_DIRECT_STACK (1024u * 1024u)
#endif
#endif

/* The Windows C runtime turns every "\n" written to a text-mode stream into
 * "\r\n", so a compiled program's output would not match the interpreter's. */
static inline void arx_stdio_binary(void) {
#ifdef _WIN32
  _setmode(_fileno(stdout), _O_BINARY);
  _setmode(_fileno(stderr), _O_BINARY);
#endif
}

#endif
