#ifndef TS_HEADER
#define TS_HEADER

#include "DR.hpp"

inline void
tst_file(
  UT::String file)
{
  printf("NEW [%s] %0*d>>\n", file.m_mem, 100, 0);

  IT::StatEnv env = DR::interpret_file(file);

  for (auto &[name, def] : env)
  {
    IT::pLm result = IT::eval(def, {}, env);
    printf("%s = %s\n", name.c_str(), IT::pprint(result).c_str());
  }

  printf("END [%s] %0*d<<\n", file.m_mem, 100, 0);
}

#endif // TST_HEADER
