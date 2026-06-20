/*-------------------------------------------------------------------------------
 *\file UT.cpp
 *\info Out-of-line definitions for the non-template helpers declared in UT.hpp.
 *      The templates (Vec, the lookup helpers) necessarily stay in the header.
 *-----------------------------------------------------------------------------*/

#include "UT.hpp"

namespace UT
{

void
abort()
{
#if defined(_MSC_VER)
  std::abort();
#elif defined(__x86_64__) || defined(_M_X64) || defined(__i386__)              \
  || defined(_M_IX86)
  asm("int3");
#else
  std::abort();
#endif
}

void
fail_if(
  const char *file,    //
  const char *fn_name, //
  const int   line,    //
  const char *prefix,  //
  const char *msg)
{
  if (msg)
  {
    std::printf("[%s] %s:%d: %s\n", prefix, file, line, fn_name);
    std::printf("  %d | \033[1;37m%s\033[0m\n", line, msg);
    UT::abort();
  }
}

Vu
strdup(
  AR::Arena &arena, Vu s)
{
  char *mem = (char *)arena.alloc(s.size() + 1);
  (void)std::memcpy(mem, s.data(), s.size());
  mem[s.size()] = 0;
  return Vu{ mem, s.size() };
}

Vu
strdup(
  AR::Arena &arena, const char *s)
{
  return strdup(arena, Vu{ s });
}

} // namespace UT
