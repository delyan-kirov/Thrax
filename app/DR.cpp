#include "DR.hpp"
#include "CC.hpp"
#include "ER.hpp"
#include "STDLIBxAMALG.hpp"

#include "EX.hpp"
#include "FF.hpp"
#include "IR.hpp"
#include "IT.hpp"
#include "MR.hpp"
#include "OP.hpp"
#include "TC.hpp"
#include "UT.hpp"
#include "UTxIO.hpp"

namespace DR
{

namespace
{
UT::Vu
read_entire_file(
  UT::Vu file_name, AR::Arena &arena)
{
  return IO::read_entire_file(std::string(file_name), arena);
}

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

bool
check_ctime_asserts(
  const IR::Program           &prog,
  const MR::Result            &mr,
  const std::vector<MR::Unit> &units,
  AR::Arena                   &arena)
{
  if (mr.ctime_asserts.empty()) return true;

  IT::Machine                 m{ prog };
  std::vector<ER::Diagnostic> ds;
  for (const auto &[name, anchor] : mr.ctime_asserts)
  {
    if (IT::as_int(m.glob(name)) != 0) continue; // held

    size_t line = 0; // count newlines up to the anchor in its own unit
    for (const MR::Unit &u : units)
    {
      const char *b = u.content.data();
      const char *a = anchor.data();
      if (a && b && a >= b && a < b + u.content.size())
      {
        line = 1;
        for (const char *p = b; p < a; ++p)
          if (*p == '\n') ++line;
        break;
      }
    }
    ds.push_back(
      ER::mk_root(arena,
                  ER::Code::ASSERT_FAILED,
                  anchor,
                  line,
                  UT::strdup(arena, "compile-time assertion failed")));
  }

  if (ds.empty()) return true;
  print_diags(units, ds);
  return false;
}

// What `@run` directives asked of this compilation (see library/BUILD.thx):
// symbolic libraries to link/preload, and library search paths.
struct BuildDirectives
{
  std::vector<std::string> libs;
  std::vector<std::string> lib_paths;
};

// Force every `@run` global through the interpreter -- compile-time
// execution, Jai's #run. Values are discarded, except BUILD.Directive
// results, which are collected into `out` when the caller consumes them
// (the interpret and --build paths; the others force for effect only).
static void
force_ctime_runs(
  const IR::Program &prog, const MR::Result &mr, BuildDirectives *out)
{
  if (mr.ctime_runs.empty()) return;

  IT::Machine m{ prog };
  for (const auto &[name, anchor] : mr.ctime_runs)
  {
    (void)anchor;
    IT::pVal v = m.glob(name);
    if (!out || !v) continue;
    if (IT::VKind::Variant != IT::kind(v)) continue;
    const IT::VVariant &var = std::get<IT::VVariant>(v->as);
    if (var.type_name != "BUILD/Directive" || var.fields.size() != 1) continue;
    if (IT::VKind::Str != IT::kind(var.fields[0])) continue;
    const std::string &s = std::get<IT::VStr>(var.fields[0]->as).val;
    if (var.tag == "Lib")
      out->libs.push_back(s);
    else if (var.tag == "LibPath")
      out->lib_paths.push_back(s);
  }
}

static const char PRELUDE_FILE[] = "PRELUDE_implTarget.thx";

static UT::Vu
prelude_source(
  AR::Arena &arena, const TG::Target &tg)
{
  std::string s     = "@mod PRELUDE\n";
  auto        alias = [&](const char *name, const char *target) {
    s += "$ ";
    s += name;
    s += " : @alias = ";
    s += target;
    s += "\n";
  };
  // `Int`/`Nat` alias the TARGET's word type
  alias("Int", tg.int_ty());
  alias("Nat", tg.nat_ty());
  for (const OP::BaseAlias &a : OP::base_aliases) alias(a.name, a.target);
  return UT::strdup(arena, s.c_str());
}

static const char C_FILE[] = "C.thx";

static UT::Vu
core_c_source(
  AR::Arena &arena, const TG::Target &tg)
{
  const std::string I = tg.int_ty(), S = OP::TY_STR, P = OP::TY_PTR, U = "{}";
  std::string       s      = "@mod C\n";
  auto              ext_in = [&](const char        *lib,
                    const char        *name,
                    const std::string &sig,
                    const char        *sym) {
    s += "$ ";
    s += name;
    s += " : ";
    s += sig;
    s += " = @extern \"C\" \"";
    s += sym;
    s += "\" \"";
    s += lib;
    s += "\"\n";
  };
  auto ext = [&](const char *name, const std::string &sig, const char *sym) {
    ext_in("libc", name, sig, sym);
  };

  ext("abort", U + " -> " + U, "abort");
  ext("exit", I + " -> " + U, "exit");
  ext("puts", S + " -> " + I, "puts");
  ext("putchar", I + " -> " + I, "putchar");
  ext("getchar", U + " -> " + I, "getchar");
  ext("malloc", I + " -> " + P, "malloc");
  ext("free", P + " -> " + U, "free");
  ext("strlen", S + " -> " + I, "strlen");
  ext("fopen", S + " -> " + S + " -> " + I, "fopen");
  ext("fclose", I + " -> " + I, "fclose");
  ext("fgetc", I + " -> " + I, "fgetc");
  ext("fputs", S + " -> " + I + " -> " + I, "fputs");
  ext("fflush", I + " -> " + I, "fflush");
  ext("fseek", I + " -> " + I + " -> " + I + " -> " + I, "fseek");
  ext("ftell", I + " -> " + I, "ftell");
  ext("write", I + " -> " + S + " -> " + I + " -> " + I, "write");
  ext("remove", S + " -> " + I, "remove");
  ext("getenv", S + " -> " + S, "getenv");
  ext("time", I + " -> " + I, "time"); // time(NULL): call as `C.time 0`

  // libm: symbolic "libm"; where the math symbols actually live (a separate
  // soname on Linux, folded into libc elsewhere) is TG::soname's business.
  const std::string R  = OP::TY_REAL64;
  const char       *lm = "libm";
  ext_in(lm, "sqrt", R + " -> " + R, "sqrt");
  ext_in(lm, "sin", R + " -> " + R, "sin");
  ext_in(lm, "cos", R + " -> " + R, "cos");
  ext_in(lm, "tan", R + " -> " + R, "tan");
  ext_in(lm, "exp", R + " -> " + R, "exp");
  ext_in(lm, "log", R + " -> " + R, "log");
  ext_in(lm, "floor", R + " -> " + R, "floor");
  ext_in(lm, "ceil", R + " -> " + R, "ceil");
  ext_in(lm, "round", R + " -> " + R, "round");
  ext_in(lm, "pow", R + " -> " + R + " -> " + R, "pow");
  ext_in(lm, "fmod", R + " -> " + R + " -> " + R, "fmod");
  ext_in(lm, "atan2", R + " -> " + R + " -> " + R, "atan2");
  return UT::strdup(arena, s.c_str());
}

static const char TARGET_FILE[] = "TARGET.thx";

// The `@mod TARGET` compilation-target reflection module: ordinary Thrax
// globals whose values are the target chosen for THIS build, accessed
// qualified (`TARGET.int_bits`, `TARGET.os`, ...). Generated (not a static
// stdlib file) because the values are only known once the target is fixed --
// the same reason the prelude's `Int`/`Nat` aliases are generated. A program
// reads these to adapt to the target's word size instead of hardcoding a
// portable constant (e.g. `when TARGET.int_bits is 64 then ... else ...`).
static UT::Vu
target_source(
  AR::Arena &arena, const TG::Target &tg)
{
  const std::string I = tg.int_ty(), S = OP::TY_STR;
  std::string       s       = "@mod TARGET\n";
  auto              def_int = [&](const char *name, long long v) {
    // A negative value's magnitude may be 2^(w-1) (int_min), which exceeds
    // the positive-literal ceiling; write it as `0 - (|v|-1) - 1` so every
    // literal part is in range (`-(v+1)` avoids overflow at v == INT_MIN).
    std::string rhs
      = v < 0 ? "0 - " + std::to_string(-(v + 1)) + " - 1" : std::to_string(v);
    s += "$ " + std::string(name) + " : " + I + " = " + rhs + "\n";
  };
  auto def_str = [&](const char *name, const char *v) {
    s += "$ " + std::string(name) + " : " + S + " = \"" + v + "\"\n";
  };
  def_int("int_bits", tg.ptr_bits()); // Int is the target word
  def_int("ptr_bits", tg.ptr_bits());
  def_int("int_max", tg.int_max());
  def_int("int_min", tg.int_min());
  def_str("os", tg.os_name());
  def_str("arch", tg.arch_name());
  def_str("name", tg.name().c_str());
  return UT::strdup(arena, s.c_str());
}

// Lex + parse one source (content/file) into `units`, forwarding diagnostics.
static void
parse_one(
  UT::Vu                       content,
  UT::Vu                       file,
  AR::Arena                   &arena,
  std::vector<MR::Unit>       &units,
  std::vector<ER::Diagnostic> &parse_diags)
{
  LX::Lexer  lexer{ content, file, arena };
  EX::Parser parser{ lexer };
  parser();
  units.push_back(MR::Unit{ file, content, parser.m_exprs });
  for (const ER::Diagnostic &d : parser.m_diags) parse_diags.push_back(d);
}

bool
parse_units(
  const std::vector<UT::Vu> &files,
  AR::Arena                 &arena,
  std::vector<MR::Unit>     &units,
  const TG::Target          &tg)
{
  bool                        ok = true;
  std::vector<ER::Diagnostic> parse_diags;

  // The prelude and the `C` libc namespace first, so their declarations are in
  // scope for every file.
  parse_one(prelude_source(arena, tg),
            UT::Vu{ PRELUDE_FILE, sizeof(PRELUDE_FILE) - 1 },
            arena,
            units,
            parse_diags);
  parse_one(core_c_source(arena, tg),
            UT::Vu{ C_FILE, sizeof(C_FILE) - 1 },
            arena,
            units,
            parse_diags);
  parse_one(target_source(arena, tg),
            UT::Vu{ TARGET_FILE, sizeof(TARGET_FILE) - 1 },
            arena,
            units,
            parse_diags);

  for (const StdlibUnit &u : STDLIB_UNITS)
    parse_one(UT::Vu{ u.src, std::strlen(u.src) },
              UT::Vu{ u.name, std::strlen(u.name) },
              arena,
              units,
              parse_diags);

  for (UT::Vu file : files)
  {
    UT::Vu content = read_entire_file(file, arena);
    if (!content.data())
    {
      ok = false;
      continue;
    }
    parse_one(content, file, arena, units, parse_diags);
  }

  if (!parse_diags.empty())
  {
    print_diags(units, parse_diags);
    ok = false;
  }
  return ok;
}

// Link modules and type-check (which now lowers pattern sugar internally, after
// its struct/union tables are built). On success `out` holds the flattened,
// mangled program and entry point. Prints and returns false on the first
// failing stage.
bool
compile_units(
  std::vector<MR::Unit> &units,
  AR::Arena             &arena,
  MR::Result            &out,
  const TG::Target      &tg)
{
  out = MR::link(units, arena);
  if (print_diags(units, out.diags)) return false;

  std::vector<ER::Diagnostic> type_diags
    = TC::check(out.program, arena, {}, tg);
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
    const EX::ExEffectDecl &ed = std::get<EX::ExEffectDecl>(e.as);
    for (const EX::FieldDecl &op : ed.ops)
      // The canonical `Effect.op` identity -- matches what MR rewrites uses and
      // clause heads to, and what TC keys its operation schemes by.
      prog.operations.insert(std::string(ed.name) + "." + std::string(op.name));
  }
}

Interp
interpret_file(
  UT::Vu file, const TG::Target &tg)
{
  // The front-end arena lives only for lexing/parsing/type-checking; the Core
  // arena (owned by the returned Interp) outlives it and holds the Core the env
  // points into, copied out of the front end by CR::build.
  AR::Arena front{};
  Interp    ip;
  ip.arena = std::make_unique<AR::Arena>();

  std::vector<MR::Unit> units;
  if (!parse_units({ file }, front, units, tg)) return ip;

  MR::Result mr;
  if (!compile_units(units, front, mr, tg)) return ip;

  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, *ip.arena);

  ip.prog = IR::lower(env, *ip.arena);
  collect_operations(mr.program, ip.prog);

  if (!check_ctime_asserts(ip.prog, mr, units, *ip.arena))
    ip.prog = IR::Program{};
  else
    force_ctime_runs(ip.prog, mr, nullptr);
  return ip;
}

int
run_program(
  const std::vector<UT::Vu> &files, const TG::Target &tg)
{
  AR::Arena             front{};
  std::vector<MR::Unit> units;

  if (!parse_units(files, front, units, tg)) return 1;

  MR::Result mr;
  if (!compile_units(units, front, mr, tg)) return 1;

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

  if (!check_ctime_asserts(prog, mr, units, ir_arena)) return 1;

  // Apply the `@run` BUILD directives to this run: search paths and
  // preloads for the FFI's dlopen.
  BuildDirectives bd;
  force_ctime_runs(prog, mr, &bd);
  for (const std::string &p : bd.lib_paths) FF::add_lib_path(p);
  for (const std::string &l : bd.libs) FF::add_preload(l);

  // Run the entry via the reified-K machine: `main` for `Int`, or `main ""` for
  // `Str -> Int` (the CLI argument is empty until an `Args` type exists).
  return IT::machine_main(prog, mr.entry, mr.entry_takes_arg);
}

bool
dump_ir(
  const std::vector<UT::Vu> &files, const TG::Target &tg)
{
  AR::Arena             front{};
  std::vector<MR::Unit> units;
  if (!parse_units(files, front, units, tg)) return false;

  MR::Result mr;
  if (!compile_units(units, front, mr, tg)) return false;

  AR::Arena   core_arena{};
  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, core_arena);

  IR::Program prog = IR::lower(env, core_arena);
  collect_operations(mr.program, prog);
  if (!check_ctime_asserts(prog, mr, units, core_arena)) return false;
  force_ctime_runs(prog, mr, nullptr);
  std::printf("%s", IR::pprint(prog).c_str());
  return true;
}

bool
emit_c(
  const std::vector<UT::Vu> &files, const TG::Target &tg)
{
  AR::Arena             front{};
  std::vector<MR::Unit> units;
  if (!parse_units(files, front, units, tg)) return false;

  MR::Result mr;
  if (!compile_units(units, front, mr, tg)) return false;

  // The IR arena must outlive emission: CC reads names/literals that view it.
  AR::Arena   core_arena{};
  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, core_arena);

  IR::Program prog = IR::lower(env, core_arena);
  collect_operations(mr.program, prog);

  if (!check_ctime_asserts(prog, mr, units, core_arena)) return false;
  force_ctime_runs(prog, mr, nullptr);

  if (std::optional<std::string> why = CC::unsupported(prog))
  {
    std::fprintf(stderr,
                 "thrax: the native backend does not yet support %s; run with "
                 "the interpreter instead\n",
                 why->c_str());
    return false;
  }

  std::string c = CC::emit(prog, mr.entry, mr.entry_takes_arg, tg);
  std::fwrite(c.data(), 1, c.size(), stdout);
  return true;
}

bool
build_project(
  UT::Vu dir, const TG::Target &tg)
{
  namespace fs = std::filesystem;
  std::string     dpath(dir);
  std::error_code ec;

  if (!fs::is_directory(dpath, ec))
  {
    std::fprintf(stderr,
                 "thrax: '%s' is not a directory (a project is a directory "
                 "containing a MAIN module)\n",
                 dpath.c_str());
    return false;
  }

  // The project's source files: every `.thx` directly in the directory (the
  // same rule the CLI uses for a directory path).
  std::vector<std::string> names = expand_sources({ dir });
  if (names.empty())
  {
    std::fprintf(
      stderr, "thrax: no .thx source files in '%s'\n", dpath.c_str());
    return false;
  }
  std::vector<UT::Vu> files;
  files.reserve(names.size());
  for (const std::string &n : names)
    files.push_back(UT::Vu{ n.data(), n.size() });

  AR::Arena             front{};
  std::vector<MR::Unit> units;
  if (!parse_units(files, front, units, tg)) return false;

  MR::Result mr;
  if (!compile_units(units, front, mr, tg)) return false;

  if (mr.entry.empty())
  {
    std::fprintf(stderr,
                 "thrax: project '%s' has no entry point -- define a module "
                 "'MAIN' with a 'main : Int' (or 'main : Str -> Int')\n",
                 dpath.c_str());
    return false;
  }

  AR::Arena   core_arena{};
  CR::StatEnv env;
  for (size_t i = 0; i < mr.program.size(); ++i)
    CR::build(&mr.program[i], env, core_arena);

  IR::Program prog = IR::lower(env, core_arena);
  collect_operations(mr.program, prog);

  if (!check_ctime_asserts(prog, mr, units, core_arena)) return false;

  // `@run` BUILD directives join the link line below.
  BuildDirectives bd;
  force_ctime_runs(prog, mr, &bd);

  if (std::optional<std::string> why = CC::unsupported(prog))
  {
    std::fprintf(stderr,
                 "thrax: the native backend does not yet support %s; cannot "
                 "build project '%s' (run it with the interpreter instead)\n",
                 why->c_str(),
                 dpath.c_str());
    return false;
  }

  // Output goes in <project>/bin/<project>.{ir,c} and the executable
  // <project>/bin/<project>. `stem` is the project directory's own name.
  fs::path    pdir = fs::path(dpath);
  std::string stem = pdir.filename().string();
  if (stem.empty())
  {
    pdir = pdir.parent_path();
    stem = pdir.filename().string();
  }
  fs::path outdir = pdir / "bin";
  fs::create_directories(outdir, ec);
  if (ec)
  {
    std::fprintf(stderr,
                 "thrax: cannot create '%s': %s\n",
                 outdir.string().c_str(),
                 ec.message().c_str());
    return false;
  }

  TG::Toolchain tc = TG::toolchain(tg);
  if (tc.cc.empty())
  {
    std::fprintf(stderr, "thrax: %s\n", tc.hint.c_str());
    return false;
  }

  fs::path ir_path  = outdir / (stem + ".ir");
  fs::path c_path   = outdir / (stem + ".c");
  fs::path exe_path = outdir / (stem + tc.exe_suffix);

  if (!IO::write_to_file(ir_path.string(), IR::pprint(prog)))
  {
    std::fprintf(
      stderr, "thrax: cannot write '%s'\n", ir_path.string().c_str());
    return false;
  }
  if (!IO::write_to_file(c_path.string(),
                         CC::emit(prog, mr.entry, mr.entry_takes_arg, tg)))
  {
    std::fprintf(stderr, "thrax: cannot write '%s'\n", c_path.string().c_str());
    return false;
  }

  // The generated unit is self-contained (the runtime is baked in), so the
  // compile needs nothing but the target's C compiler (TG::toolchain) --
  // plus a link flag per library the program's externs name (CC::link_flags;
  // libc is implicit, generated programs never dlopen -- the system linker
  // resolves, statically or dynamically per what it finds), plus whatever
  // the `@run` BUILD directives asked for: -L/rpath per lib_path, one
  // lib_flag per lib. Built as an argv vector and spawned with no shell
  // (IO::run_command) -- no quoting, no injection surface, no std::system.
  std::vector<std::string> argv = { tc.cc };
  for (const std::string &f : tc.cflags) argv.push_back(f);
  argv.push_back(c_path.string());
  argv.push_back("-o");
  argv.push_back(exe_path.string());
  for (const std::string &p : bd.lib_paths)
  {
    argv.push_back("-L" + p);
    if (tc.rpath) argv.push_back("-Wl,-rpath," + p);
  }
  for (const std::string &f : CC::link_flags(prog)) argv.push_back(f);
  for (const std::string &l : bd.libs)
    if (std::string f = CC::lib_flag(l); !f.empty()) argv.push_back(f);

  int rc = IO::run_command(argv);
  if (rc != 0)
  {
    std::fprintf(stderr, "thrax: C compilation failed (cc exit %d)\n", rc);
    return false;
  }

  std::printf("thrax: built project '%s'\n  ir:  %s\n  c:   %s\n  exe: %s\n",
              stem.c_str(),
              ir_path.string().c_str(),
              c_path.string().c_str(),
              exe_path.string().c_str());
  if (!tc.runner.empty())
    std::printf("  run: %s %s\n", tc.runner.c_str(), exe_path.string().c_str());
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
