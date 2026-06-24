#include "DR.hpp"
#include "ER.hpp"
#include "EX.hpp"
#include "LL.hpp"
#include "MR.hpp"
#include "TC.hpp"
#include "UT.hpp"

#include <string>
#include <unordered_set>
#include <vector>

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

IT::StatEnv
interpret_file(
  UT::Vu file)
{
  AR::Arena             arena{};
  IT::StatEnv           env;
  std::vector<MR::Unit> units;

  if (!parse_units({ file }, arena, units)) return env;

  MR::Result mr;
  if (!compile_units(units, arena, mr)) return env;

  for (size_t i = 0; i < mr.program.size(); ++i)
    IT::exprs2pLm(&mr.program[i], env);

  return env;
}

int
run_program(
  const std::vector<UT::Vu> &files)
{
  AR::Arena             arena{};
  std::vector<MR::Unit> units;

  if (!parse_units(files, arena, units)) return 1;

  MR::Result mr;
  if (!compile_units(units, arena, mr)) return 1;

  if (mr.entry.empty())
  {
    std::fprintf(stderr,
                 "thrax: no entry point -- define a module 'MAIN' with a "
                 "'main : Int' (or 'main : Str -> Int')\n");
    return 1;
  }

  IT::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    IT::exprs2pLm(&mr.program[i], env);

  // Invoke the entry: `main` for `Int`, or `main ""` for `Str -> Int` (the CLI
  // argument is empty until an `Args` type exists).
  EX::Expr var{ EX::ExprTag::Var };
  var.as         = EX::ExVar{ mr.entry };
  EX::Expr *node = &var;
  EX::Expr  strn{ EX::ExprTag::Str };
  EX::Expr  app{ EX::ExprTag::App };
  if (mr.entry_takes_arg)
  {
    strn.as = EX::ExStr{ UT::Vu{ "", 0 } };
    app.as  = EX::ExApp{ &var, &strn };
    node    = &app;
  }

  IT::pLm res = IT::eval(IT::exprs2pLm(node, env), {}, env);
  if (res && res->tag == IT::LTag::INT)
    return (int)std::get<IT::Int>(res->as).unwrap;
  return 0;
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
