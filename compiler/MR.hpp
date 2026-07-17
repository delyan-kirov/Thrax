/*-------------------------------------------------------------------------------
 *\file MR.hpp
 *\info Module resolution ("link") layer.
 *
 * MR runs after the parser (EX) and pattern lowering (LL), across ALL source
 * files at once, and before the type checker (TC). It turns a set of per-file
 * module fragments into one flat program of uniquely-named globals:
 *
 *   - every file opens with `@mod NAME`; files sharing a name are one module;
 *   - each module-level term symbol `foo` is mangled to a unique `NAME/foo`
 *     (the '/' cannot occur in a source identifier), and every reference to it
 *     -- qualified `MOD.foo` or an unambiguous unqualified `foo` -- is
 * rewritten to that mangled name;
 *   - `$ with ...` imports and `$ @private` / `$ @public` toggles are consumed
 *     here and stripped from the output;
 *   - the program entry point is the `main` global of module `MAIN`.
 *
 * After MR the rest of the pipeline (TC, IT) sees an ordinary flat list of
 * globals and needs no knowledge of modules. Type/constructor names stay in one
 * global namespace for now (per-module types are a later increment).
 *-----------------------------------------------------------------------------*/

#ifndef MR_HEADER_
#define MR_HEADER_

#include "EX.hpp"

namespace MR
{

// One parsed (and pattern-lowered) source file. `exprs[0]` is its `@mod`
// header.
struct Unit
{
  UT::Vu    filename;
  UT::Vu    content;
  EX::Exprs exprs;
};

struct Result
{
  EX::Exprs                   program; // flattened, mangled globals
  UT::Vu                      entry;   // mangled MAIN/main, or empty
  bool                        entry_takes_arg = false; // true for `Str -> Int`
  std::vector<ER::Diagnostic> diags;                   // empty on success
  std::vector<std::pair<UT::Vu, UT::Vu>> ctime_asserts;
};

// Link `units` into one program. On any error the diagnostics are returned and
// `program` should not be used.
Result link(std::vector<Unit> &units, AR::Arena &arena);

} // namespace MR

#endif // MR_HEADER_
