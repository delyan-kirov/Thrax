#ifndef TS_HEADER_
#define TS_HEADER_

#include "DR.hpp"
#include "UT.hpp"

namespace TS
{

// Interpret a source file as a smoke test: parse, type-check and evaluate every
// top-level definition. interpret_file prints any parse/type diagnostics (to
// stderr); a clean run reports a single green OK on stdout, otherwise a red
// FAIL on stderr. No value dumps.
void tst_file(UT::Vu file);

// Collect every file in `dir`, sorted for stable output.
std::vector<std::string> scan_dir(const char *dir);

// Scan ./dat and interpret every file in it, one by one.
void run_all();

} // namespace TS

#endif // TS_HEADER_
