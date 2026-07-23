/*-------------------------------------------------------------------------------
 *\file UTxIO.hpp
 *\info Cross-platform I/O module -- declarations. The one place OS-specific
 *      file and process handling lives, so nothing else in the project touches
 *      fopen, getenv, or -- especially -- std::system. Two consumers share it:
 *
 *        - the compiler amalgamation (UTxIO.cpp is globbed into it), which
 *          reads source files and shells out to the target C compiler;
 *        - the build tool (UTxBUILD.hpp includes this header and forwards its
 *          BLD helpers here), which reads/writes generated files and spawns cc.
 *          Because the bootstrap compiles the build tool from source, its
 *          command line lists utilities/UTxIO.cpp (+ AR.cpp for the arena
 *          overload) alongside build.cpp.
 *
 *      Process launching is argv-based, never a shell: posix_spawn on POSIX
 *      (Linux, macOS, the BSDs) and CreateProcess on Windows. There is no
 *      /bin/sh vs cmd.exe divergence and no quoting/injection surface. This is
 *      also the durable answer to "what replaces std::system?" -- C++26 does
 *      not add a portable no-shell spawn, so posix_spawn/CreateProcess is it.
 *
 *      Names follow the Jai/Rust/Koka lineage (read_entire_file, write_to_file,
 *      append_to_file, run_command).
 *-----------------------------------------------------------------------------*/

#ifndef UTxIO_HEADER_
#define UTxIO_HEADER_

#include "UT.hpp" // UT::Vu, AR::Arena

#include <optional>
#include <string>
#include <vector>

namespace IO
{
using std::string;
using std::vector;

// --- Files ------------------------------------------------------------------

// Read a whole file as bytes. std::nullopt when it can't be opened or read --
// the caller decides whether that is an error and what to say about it.
std::optional<string> read_entire_file(const string &path);

// Arena-backed overload for the compiler front-end: the returned view (and its
// trailing NUL) live in `arena`, so the lexer can hold it for the whole
// compile with no extra copy. On failure it prints why and returns a view
// whose data() is null (the callers test exactly that).
UT::Vu read_entire_file(const string &path, AR::Arena &arena);

// Truncate (or create) `path` and write `bytes`. False if it can't be written.
bool write_to_file(const string &path, UT::Vu bytes);

// Append `bytes` to `path`, creating it if absent. False on failure.
bool append_to_file(const string &path, UT::Vu bytes);

// --- Environment ------------------------------------------------------------

// Look up an environment variable. std::nullopt when it is unset (distinct
// from set-but-empty, which is a present empty string).
std::optional<string> get_env(const string &name);

// --- Process ----------------------------------------------------------------

// Spawn a program directly -- NO shell. argv[0] is the program (searched on
// PATH when it contains no slash). `out_file`/`err_file`, if given, redirect
// the child's stdout/stderr to those files; otherwise each stream is inherited
// (live on the terminal). Nothing is ever discarded. Returns the child's exit
// code, or -1 if it could not be spawned.
int spawn(const vector<string> &argv,
          const string         &out_file = "",
          const string         &err_file = "");

// Run a program to completion (no shell), inheriting stdout and stderr so its
// output is live on the terminal. This is the std::system replacement: same
// "run cc and inherit its diagnostics" behavior, minus the shell. Returns the
// child's exit code (-1 if it could not be spawned).
int run_command(const vector<string> &argv);

// The full result of running a program: exit code plus BOTH captured streams.
// Nothing is discarded -- the caller decides what to show.
struct Captured
{
  int    code;
  string out;
  string err;
};

// Run a program (no shell) and capture stdout AND stderr, via temp files so
// large output never deadlocks a pipe. Returns {code, out, err}.
Captured capture_command(const vector<string> &argv);

} // namespace IO

#endif // UTxIO_HEADER_
