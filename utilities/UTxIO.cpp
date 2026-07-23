/*-------------------------------------------------------------------------------
 *\file UTxIO.cpp
 *\info Cross-platform I/O module -- definitions. See UTxIO.hpp for the surface
 *      and the platform notes. All OS-specific includes and syscalls are
 *      confined to this file.
 *-----------------------------------------------------------------------------*/

#include "UTxIO.hpp"

#include <cstdio> // fopen/fread/fwrite -- file I/O without iostreams
#include <cstdlib>
#include <filesystem>
#include <system_error>

// Platform shim: every OS-specific include and syscall is confined below. The
// wasm sandboxes (Emscripten/WASI) have no process model -- and this file joins
// the compiler amalgamation, which the browser-playground build compiles with
// emcc -- so spawning is a stub there rather than pulling in <spawn.h>.
#if defined(_WIN32)
#include <windows.h>
#elif defined(__EMSCRIPTEN__) || defined(__wasi__)
// no process headers: spawn is unsupported in the wasm sandbox
#else
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
extern char **environ;
#endif

namespace IO
{

// --- Files ------------------------------------------------------------------

std::optional<string>
read_entire_file(
  const string &path)
{
  FILE *f = std::fopen(path.c_str(), "rb");
  if (!f) return std::nullopt;
  string data;
  char   buf[4096];
  size_t n;
  while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) data.append(buf, n);
  bool ok = std::ferror(f) == 0;
  std::fclose(f);
  if (!ok) return std::nullopt;
  return data;
}

UT::Vu
read_entire_file(
  const string &path, AR::Arena &arena)
{
  FILE *f = std::fopen(path.c_str(), "rb");
  if (!f)
  {
    std::fprintf(stderr, "ERROR: could not open file: %s\n", path.c_str());
    return UT::Vu{};
  }
  std::fseek(f, 0, SEEK_END);
  long len = std::ftell(f);
  std::rewind(f);
  if (len < 0)
  {
    std::fclose(f);
    std::fprintf(stderr, "ERROR: could not size file: %s\n", path.c_str());
    return UT::Vu{};
  }
  char *buf  = (char *)arena.alloc(sizeof(char) * ((size_t)len + 1));
  buf[len]   = 0;
  size_t got = std::fread(buf, 1, (size_t)len, f);
  std::fclose(f);
  if (got != (size_t)len)
  {
    std::fprintf(
      stderr, "ERROR: could not map file %s to memory buffer\n", path.c_str());
    return UT::Vu{};
  }
  return UT::Vu{ buf, (size_t)len };
}

bool
write_to_file(
  const string &path, UT::Vu bytes)
{
  FILE *f = std::fopen(path.c_str(), "wb");
  if (!f) return false;
  bool ok = std::fwrite(bytes.data(), 1, bytes.size(), f) == bytes.size();
  return std::fclose(f) == 0 && ok;
}

bool
append_to_file(
  const string &path, UT::Vu bytes)
{
  FILE *f = std::fopen(path.c_str(), "ab");
  if (!f) return false;
  bool ok = std::fwrite(bytes.data(), 1, bytes.size(), f) == bytes.size();
  return std::fclose(f) == 0 && ok;
}

// --- Environment ------------------------------------------------------------

std::optional<string>
get_env(
  const string &name)
{
  const char *v = std::getenv(name.c_str());
  if (!v) return std::nullopt;
  return string(v);
}

// --- Process ----------------------------------------------------------------

int
spawn(
  const vector<string> &argv, const string &out_file, const string &err_file)
{
#if defined(_WIN32)
  string cl; // Windows wants one command line, not an argv array.
  for (size_t i = 0; i < argv.size(); ++i)
    cl += (i ? " \"" : "\"") + argv[i] + "\"";
  SECURITY_ATTRIBUTES sa{};
  sa.nLength        = sizeof sa;
  sa.bInheritHandle = TRUE;
  HANDLE       fo = INVALID_HANDLE_VALUE, fe = INVALID_HANDLE_VALUE;
  STARTUPINFOA si{};
  si.cb         = sizeof si;
  si.dwFlags    = STARTF_USESTDHANDLES;
  si.hStdInput  = GetStdHandle(STD_INPUT_HANDLE);
  si.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
  si.hStdError  = GetStdHandle(STD_ERROR_HANDLE);
  if (!out_file.empty())
  {
    fo            = CreateFileA(out_file.c_str(),
                     GENERIC_WRITE,
                     0,
                     &sa,
                     CREATE_ALWAYS,
                     FILE_ATTRIBUTE_NORMAL,
                     nullptr);
    si.hStdOutput = fo;
  }
  if (!err_file.empty())
  {
    fe           = CreateFileA(err_file.c_str(),
                     GENERIC_WRITE,
                     0,
                     &sa,
                     CREATE_ALWAYS,
                     FILE_ATTRIBUTE_NORMAL,
                     nullptr);
    si.hStdError = fe;
  }
  vector<char> cmd(cl.begin(), cl.end());
  cmd.push_back('\0');
  PROCESS_INFORMATION pi{};
  DWORD               code = (DWORD)-1;
  if (CreateProcessA(nullptr,
                     cmd.data(),
                     nullptr,
                     nullptr,
                     TRUE,
                     0,
                     nullptr,
                     nullptr,
                     &si,
                     &pi))
  {
    WaitForSingleObject(pi.hProcess, INFINITE);
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
  }
  if (fo != INVALID_HANDLE_VALUE) CloseHandle(fo);
  if (fe != INVALID_HANDLE_VALUE) CloseHandle(fe);
  return (int)code;
#elif defined(__EMSCRIPTEN__) || defined(__wasi__)
  // No process model in the wasm sandbox; the browser playground evaluates
  // in-process and never reaches here.
  (void)argv;
  (void)out_file;
  (void)err_file;
  return -1;
#else
  vector<char *> a;
  a.reserve(argv.size() + 1);
  for (const string &s : argv) a.push_back(const_cast<char *>(s.c_str()));
  a.push_back(nullptr);
  posix_spawn_file_actions_t fa;
  posix_spawn_file_actions_init(&fa);
  if (!out_file.empty())
    posix_spawn_file_actions_addopen(
      &fa, 1, out_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (!err_file.empty())
    posix_spawn_file_actions_addopen(
      &fa, 2, err_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
  pid_t pid;
  int   rc = posix_spawnp(&pid, a[0], &fa, nullptr, a.data(), environ);
  posix_spawn_file_actions_destroy(&fa);
  if (rc != 0) return -1;
  int st;
  waitpid(pid, &st, 0);
  return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
#endif
}

int
run_command(
  const vector<string> &argv)
{
  return spawn(argv);
}

Captured
capture_command(
  const vector<string> &argv)
{
  namespace fs             = std::filesystem;
  static unsigned long seq = 0;
  fs::path             dir = fs::temp_directory_path();
  const string of = (dir / ("thrax-io-o" + std::to_string(seq))).string();
  const string ef = (dir / ("thrax-io-e" + std::to_string(seq++))).string();
  Captured     r;
  r.code = spawn(argv, of, ef);
  r.out  = read_entire_file(of).value_or("");
  r.err  = read_entire_file(ef).value_or("");
  std::error_code ec;
  fs::remove(of, ec);
  fs::remove(ef, ec);
  return r;
}

} // namespace IO
