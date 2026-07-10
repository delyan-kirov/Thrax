/*-------------------------------------------------------------------------------
 *\file THxCHECK.c
 *\info Implementation of the runtime abort path. A failure prints a single
 *      red diagnostic line to stderr and exits non-zero, mirroring the
 *      compiler's UT::fail_if so the two halves of Thrax report faults alike.
 *-----------------------------------------------------------------------------*/

#include "THxCHECK.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

void
THxCHECK_fail(
  const char *file, int line, const char *fn, const char *msg)
{
  fprintf(stderr,
          "\033[31mthrax-rt FAIL\033[0m %s:%d (%s): %s\n",
          file,
          line,
          fn,
          msg);
  abort();
}

void
THxCHECK_failf(
  const char *file, int line, const char *fn, const char *fmt, ...)
{
  fprintf(stderr, "\033[31mthrax-rt FAIL\033[0m %s:%d (%s): ", file, line, fn);
  va_list args;
  va_start(args, fmt);
  vfprintf(stderr, fmt, args);
  va_end(args);
  fputc('\n', stderr);
  abort();
}
