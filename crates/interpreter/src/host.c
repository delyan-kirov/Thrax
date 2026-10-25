/* The interpreter's host layer: loading a shared library and finding a symbol
 * in it (dlopen/dlsym, or LoadLibrary/GetProcAddress), and the stdio mode, behind
 * calls that are the same on every platform. The C headers carry each platform's
 * `RTLD_*` values, which differ (macOS's RTLD_DEFAULT is not NULL). */

#ifndef _WIN32
#define _GNU_SOURCE /* glibc declares RTLD_DEFAULT only under it */
#endif

#include <stdio.h>
#include <string.h>

#ifdef _WIN32

#include <fcntl.h>
#include <io.h>
#include <windows.h>
#include <psapi.h>

/* LoadLibrary takes only `\` as a path separator, while `@extern` paths are
 * written with `/` to read the same on every platform. */
void *thx_host_dl_open(const char *name) {
  char path[MAX_PATH];
  size_t n = strlen(name);
  if (n >= sizeof path) return NULL;
  for (size_t i = 0; i <= n; i++) path[i] = name[i] == '/' ? '\\' : name[i];
  return (void *)LoadLibraryA(path);
}

static void *find(HMODULE m, const char *sym) { return (void *)GetProcAddress(m, sym); }

/* The Universal CRT exports some POSIX functions only under their ISO-reserved
 * names (`_popen`, `_write`), and `time` not at all: the headers inline it over
 * `_time64`. */
static const char *crt_alias(const char *sym) {
  static char buf[128];
  if (strcmp(sym, "time") == 0) return "_time64";
  if (sym[0] == '_' || strlen(sym) + 2 > sizeof buf) return NULL;
  buf[0] = '_';
  strcpy(buf + 1, sym);
  return buf;
}

/* The process' loaded set, the counterpart of `dlsym(RTLD_DEFAULT, ...)`: the
 * executable (whose `dllexport`ed functions are the `thx_*` helpers), then every
 * module loaded so far. */
static void *find_loaded(const char *sym) {
  void *p = find(GetModuleHandleA(NULL), sym);
  if (p) return p;
  HMODULE mods[1024];
  DWORD bytes = 0;
  if (!K32EnumProcessModules(GetCurrentProcess(), mods, sizeof mods, &bytes)) return NULL;
  size_t n = bytes / sizeof(HMODULE);
  if (n > sizeof mods / sizeof mods[0]) n = sizeof mods / sizeof mods[0];
  for (size_t i = 0; i < n; i++)
    if ((p = find(mods[i], sym)) != NULL) return p;
  return NULL;
}

void *thx_host_dl_sym(void *handle, const char *sym) {
  void *p = handle ? find((HMODULE)handle, sym) : find_loaded(sym);
  if (p) return p;
  const char *alias = crt_alias(sym);
  if (!alias) return NULL;
  return handle ? find((HMODULE)handle, alias) : find_loaded(alias);
}

/* Without this the C runtime writes "\r\n" for every "\n" a foreign call prints,
 * while Rust's own output does not. */
void thx_host_stdio_binary(void) {
  _setmode(_fileno(stdout), _O_BINARY);
  _setmode(_fileno(stderr), _O_BINARY);
}

#else

#include <dlfcn.h>

void *thx_host_dl_open(const char *name) { return dlopen(name, RTLD_NOW | RTLD_GLOBAL); }

void *thx_host_dl_sym(void *handle, const char *sym) {
  return dlsym(handle ? handle : RTLD_DEFAULT, sym);
}

void thx_host_stdio_binary(void) {}

#endif
