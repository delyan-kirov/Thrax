/*-------------------------------------------------------------------------------
 *\file UTxCOMPAT.hpp
 *\info Portable shims for libc functions the compiler uses
 *-----------------------------------------------------------------------------*/

#ifndef UTxCOMPAT_HEADER_
#define UTxCOMPAT_HEADER_

#if defined(_WIN32)

#include <cstdarg>
#include <cstdio>
#include <cstdlib>

inline int
asprintf(
  char **out, const char *fmt, ...)
{
  va_list ap;
  va_start(ap, fmt);
  va_list ap_len;
  va_copy(ap_len, ap);
  const int len = std::vsnprintf(nullptr, 0, fmt, ap_len);
  va_end(ap_len);
  if (len < 0)
  {
    va_end(ap);
    *out = nullptr;
    return -1;
  }
  char *buf = static_cast<char *>(std::malloc(static_cast<size_t>(len) + 1));
  if (!buf)
  {
    va_end(ap);
    *out = nullptr;
    return -1;
  }
  const int written
    = std::vsnprintf(buf, static_cast<size_t>(len) + 1, fmt, ap);
  va_end(ap);
  *out = buf;
  return written;
}

#endif // _WIN32

#endif // UTxCOMPAT_HEADER_
