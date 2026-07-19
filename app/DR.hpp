/*-------------------------------------------------------------------------------
 *\file DR.hpp
 *\info Header file for the Driver (pipeline orchestration)
 * *----------------------------------------------------------------------------*/

#ifndef DR_HEADER_
#define DR_HEADER_

#include "CR.hpp"
#include "IT.hpp"
#include "TG.hpp"

namespace DR
{

struct Interp
{
  std::unique_ptr<AR::Arena> arena;
  IR::Program                prog;
};

// Expand command-line paths into the list of source files to compile. A path
// that names a directory contributes every `.thx` file directly inside it
// (sorted, NOT recursing into sub-directories, skipping names that start with
// '_'); any other path is taken verbatim (so an explicitly named file is always
// included, even a `_`-prefixed one). The returned strings own the names.
std::vector<std::string> expand_sources(const std::vector<UT::Vu> &paths);

// Full single-file pipeline (LX -> EX -> LL -> MR -> TC -> Core), returning the
// module-resolved global environment together with the arena that owns it. Used
// by the test harness; does not require or invoke an entry point. On a pipeline
// failure the returned env is empty (diagnostics already printed).
Interp interpret_file(UT::Vu file, const TG::Target &tg = TG::host());

int run_program(const std::vector<UT::Vu> &files,
                const TG::Target          &tg = TG::host());

bool dump_ast(UT::Vu file);

bool dump_ir(const std::vector<UT::Vu> &files,
             const TG::Target          &tg = TG::host());

bool emit_c(const std::vector<UT::Vu> &files,
            const TG::Target          &tg = TG::host());

bool build_project(UT::Vu dir, const TG::Target &tg = TG::host());

} // namespace DR

#endif // DR_HEADER_
