#ifndef TS_HEADER_
#define TS_HEADER_

#include "DR.hpp"
#include "UT.hpp"

namespace TS
{

// Collect every file in `dir`, sorted for stable output.
std::vector<std::string> scan_dir(const char *dir);

// Compile all examples + tests/MAIN.thx into one program and run its MAIN
// entry. Returns the number of failing example modules (0 = all pass).
int run_all();

} // namespace TS

#endif // TS_HEADER_
