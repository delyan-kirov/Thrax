#include "DR.hpp"
#include "ER.hpp"
#include "EX.hpp"
#include "IR.hpp"
#include "IT.hpp"
#include "LL.hpp"
#include "MR.hpp"
#include "TC.hpp"
#include "UT.hpp"

namespace DR
{

namespace
{
// TODO: Better print messages
// TODO: Better error handling
UT::Vu
read_entire_file(
  UT::Vu file_name, AR::Arena &arena)
{
  const char *file_str = file_name.data();
  size_t      file_len = 0;
  char       *buffer   = nullptr;
  size_t      result   = 0;

  FILE *file_stream = std::fopen(file_str, "rb");
  if (!file_stream)
  {
    std::fprintf(stderr, "ERROR: could not open file: %s\n", file_str);
    goto DEFER_RETURN;
  }

  std::fseek(file_stream, 0, SEEK_END);
  file_len = ftell(file_stream);

  std::rewind(file_stream);
  buffer           = (char *)arena.alloc(sizeof(char) * (file_len + 1));
  buffer[file_len] = 0;

  result = std::fread(buffer, 1, file_len, file_stream);
  if (result != file_len)
  {
    std::fprintf(
      stderr, "ERROR: could not map file %s to memory buffer\n", file_str);

    goto DEFER_RETURN;
  }

DEFER_RETURN:
  if (file_stream) std::fclose(file_stream);
  return UT::Vu{ buffer, file_len };
}
// Render each diagnostic against whichever unit's source owns its anchor, so a
// multi-file build points the caret at the right file. Identical diagnostics
// (same code, location, and message -- e.g. a global inferred in both phase B
// and phase C) are printed once. Returns true if any diagnostic was given.
bool
print_diags(
  const std::vector<MR::Unit> &units, const std::vector<ER::Diagnostic> &diags)
{
  std::unordered_set<std::string> seen;

  for (const ER::Diagnostic &d : diags)
  {
    UT::Vu content{}, file{};
    if (d.root)
    {
      // Skip an exact duplicate of one already shown.
      std::string key = std::to_string((int)d.root->code) + "@"
                        + std::to_string((size_t)d.root->anchor.data()) + ":"
                        + std::string(d.root->msg);
      if (!seen.insert(key).second) continue;

      for (const MR::Unit &u : units)
      {
        const char *b = u.content.data();
        const char *a = d.root->anchor.data();
        if (a && b && a >= b && a < b + u.content.size())
        {
          content = u.content;
          file    = u.filename;
          break;
        }
      }
    }
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  }
  return !diags.empty();
}

// Read, lex, and parse every file into a unit. Returns false (after printing)
// if any file failed to read or parse. Units are collected first so diagnostics
// can be rendered against the right source.
bool
parse_units(
  const std::vector<UT::Vu> &files,
  AR::Arena                 &arena,
  std::vector<MR::Unit>     &units)
{
  bool                        ok = true;
  std::vector<ER::Diagnostic> parse_diags;

  for (UT::Vu file : files)
  {
    UT::Vu content = read_entire_file(file, arena);
    if (!content.data())
    {
      ok = false;
      continue;
    }
    LX::Lexer  lexer{ content, file, arena };
    EX::Parser parser{ lexer };
    parser();
    units.push_back(MR::Unit{ file, content, parser.m_exprs });
    for (const ER::Diagnostic &d : parser.m_diags) parse_diags.push_back(d);
  }

  if (!parse_diags.empty())
  {
    print_diags(units, parse_diags);
    ok = false;
  }
  return ok;
}

// Lower patterns (per file), link modules, and type-check. On success `out`
// holds the flattened, mangled program and entry point. Prints and returns
// false on the first failing stage.
bool
compile_units(
  std::vector<MR::Unit> &units, AR::Arena &arena, MR::Result &out)
{
  std::vector<ER::Diagnostic> lower_diags;
  for (MR::Unit &u : units)
  {
    std::vector<ER::Diagnostic> ds = LL::lower(u.exprs, arena);
    for (const ER::Diagnostic &d : ds) lower_diags.push_back(d);
  }
  if (print_diags(units, lower_diags)) return false;

  out = MR::link(units, arena);
  if (print_diags(units, out.diags)) return false;

  std::vector<ER::Diagnostic> type_diags = TC::check(out.program, arena, {});
  if (print_diags(units, type_diags)) return false;

  return true;
}

} // namespace

std::vector<std::string>
expand_sources(
  const std::vector<UT::Vu> &paths)
{
  namespace fs = std::filesystem;
  std::vector<std::string> out;

  for (UT::Vu p : paths)
  {
    std::string     ps(p);
    std::error_code ec;
    if (!fs::is_directory(ps, ec))
    {
      out.push_back(ps); // a file (or a non-existent path): take it verbatim
      continue;
    }

    // A directory: every `.thx` file directly inside it, sorted, no recursion,
    // skipping `_`-prefixed names.
    std::vector<std::string> here;
    for (const fs::directory_entry &e : fs::directory_iterator(ps, ec))
    {
      if (ec) break;
      if (!e.is_regular_file(ec)) continue; // skips sub-directories
      const fs::path &path = e.path();
      if (path.extension() != ".thx") continue;
      std::string name = path.filename().string();
      if (!name.empty() && name[0] == '_') continue;
      here.push_back(path.string());
    }
    std::sort(here.begin(), here.end());
    for (std::string &f : here) out.push_back(std::move(f));
  }

  return out;
}

// Record the names of all effect operations declared in `program` into the
// lowered IR, so the machine resolves a use of one to an operation value.
static void
collect_operations(
  const EX::Exprs &program, IR::Program &prog)
{
  for (size_t i = 0; i < program.size(); ++i)
  {
    const EX::Expr &e = program[i];
    if (e.tag != EX::ExprTag::EffectDecl) continue;
    for (const EX::FieldDecl &op : std::get<EX::ExEffectDecl>(e.as).ops)
      prog.operations.insert(std::string(op.name));
  }
}

Interp
interpret_file(
  UT::Vu file)
{
  // The front-end arena lives only for lexing/parsing/type-checking; the Core
  // arena (owned by the returned Interp) outlives it and holds the Core the env
  // points into, copied out of the front end by CR::build.
  AR::Arena front{};
  Interp    ip;
  ip.arena = std::make_unique<AR::Arena>();

  std::vector<MR::Unit> units;
  if (!parse_units({ file }, front, units)) return ip;

  MR::Result mr;
  if (!compile_units(units, front, mr)) return ip;

  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, *ip.arena);

  ip.prog = IR::lower(env, *ip.arena);
  collect_operations(mr.program, ip.prog);
  return ip;
}

int
run_program(
  const std::vector<UT::Vu> &files)
{
  AR::Arena             front{};
  std::vector<MR::Unit> units;

  if (!parse_units(files, front, units)) return 1;

  MR::Result mr;
  if (!compile_units(units, front, mr)) return 1;

  if (mr.entry.empty())
  {
    std::fprintf(stderr,
                 "thrax: no entry point -- define a module 'MAIN' with a "
                 "'main : Int' (or 'main : Str -> Int')\n");
    return 1;
  }

  // The IR arena outlives the front-end arena and holds every IR node the
  // machine runs (lowered from the Core, which is copied out of the front end).
  AR::Arena   ir_arena{};
  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, ir_arena);

  IR::Program prog = IR::lower(env, ir_arena);
  collect_operations(mr.program, prog);

  // Run the entry via the reified-K machine: `main` for `Int`, or `main ""` for
  // `Str -> Int` (the CLI argument is empty until an `Args` type exists).
  return IT::machine_main(prog, mr.entry, mr.entry_takes_arg);
}

bool
dump_ir(
  const std::vector<UT::Vu> &files)
{
  AR::Arena             front{};
  std::vector<MR::Unit> units;
  if (!parse_units(files, front, units)) return false;

  MR::Result mr;
  if (!compile_units(units, front, mr)) return false;

  AR::Arena   core_arena{};
  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, core_arena);

  IR::Program prog = IR::lower(env, core_arena);
  collect_operations(mr.program, prog);
  std::printf("%s", IR::pprint(prog).c_str());
  return true;
}

bool
dump_ast(
  UT::Vu file)
{
  AR::Arena arena{};

  UT::Vu content = read_entire_file(file, arena);
  if (!content.data()) return false; // unreadable file; error already printed

  LX::Lexer  lexer{ content, file, arena };
  EX::Parser parser{ lexer };
  parser();

  for (const ER::Diagnostic &d : parser.m_diags)
  {
    std::fprintf(stderr, "%s\n", ER::pprint(d, content, file).c_str());
  }
  if (!parser.m_diags.empty()) return false;

  for (size_t i = 0; i < parser.m_exprs.size(); ++i)
  {
    std::printf("%s\n", EX::pprint(&parser.m_exprs[i]).c_str());
  }

  return true;
}

} // namespace DR
