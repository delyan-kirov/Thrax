# Platforms

Linux is where Thrax is developed: the root `flake.nix` is the dev shell, and
everything is built and tested there first. macOS, Windows and wasm are the
platforms Thrax ships to. The compiler runs on each of them, builds programs
for each of them, and CI tests each of them.

| Platform | Compiler runs there | Programs built for it | How CI tests it |
|---|---|---|---|
| Linux x86-64 | yes | `thrax build` (musl static by default in a release) | `cargo test`, corpus on both engines |
| macOS arm64 | yes | `thrax build` (clang) | `cargo test`, corpus, in `tools/macos` |
| Windows x86-64 | yes (MinGW-w64) | `thrax build` (MinGW gcc) | `cargo test`, corpus, natively in MSYS2; and cross-built from Linux under Wine |
| wasm32 | in the browser (playground) | `thrax build --target=wasm32-wasi` (emcc, run by node) | corpus under node; playground smoke test |

## Dev shells

Each extra platform has its own flake, so the main dev shell stays as it is.

- `tools/macos/flake.nix`: the build-and-test subset of the root shell for a
  Mac (rustc, cargo, libffi, git, bison; the stdenv's clang is `cc`). No
  valgrind, emscripten or editor tooling. `nix develop ./tools/macos`.
- `tools/windows/flake.nix`: a Linux shell that cross-compiles to
  `x86_64-pc-windows-gnu` with MinGW-w64 and runs the result under Wine.
  `cargo build` and `cargo test` produce Windows binaries (in
  `target/windows/`) and cargo runs them under Wine. `thrax-wine-corpus` runs
  the corpus on both Windows engines. Under Wine a Windows process cannot wait
  on the Unix gcc, so the tests that compile C at run time fail there; Windows
  CI runs them natively.
- `applications/web/flake.nix`: the playground (`wasm32-unknown-unknown`).
- `external/flake.nix`: autotools for rebuilding the vendored libffi.

## The portability layer

Platform conditionals live in a few files, not throughout the code:

- `crates/utilities/src/target.rs`: the target as data (word size, library
  names, which C compiler, `.exe`, rpath). `Target::host()` is the one place
  that reads the build's `cfg!` flags.
- `crates/utilities/src/libc.rs`: the real prototypes of the C library
  functions `library/C.thx` binds. A binding says `@int` for every integer
  and handle; C says `int`, `long`, `size_t` or `FILE*`, whose widths differ
  by target (`long` is 32 bits on Windows, `size_t` on wasm32). Both engines
  marshal a libc call at the real widths, so a narrow return is
  sign-extended (EOF is -1) and wasm's type-checked calls link.
- `crates/ccg/src/portable.h`: the C layer emitted ahead of every generated
  program: the assembler name of a C symbol (Mach-O's leading `_`), thread
  headers and TLS, `dllexport`, binary stdout on Windows.
- `crates/ccg/src/thraxstd.c`: the native helpers both engines share,
  including `thx_read_dir`, which `IO.read_dir` uses instead of a shell.
- `crates/interpreter/src/host.c` + `host.rs`: loading a library and finding
  a symbol (`dlopen`/`dlsym`, or `LoadLibrary`/`GetProcAddress`, with the
  Universal CRT's `_`-prefixed names).
- `crates/thrax/src/term.c`: the REPL's terminal (termios, or the Windows
  console with virtual-terminal input).
- `crates/utilities/src/build_cc.rs`: which C compiler and archiver the build
  scripts use for the target (`CC_<target>`, `TARGET_CC`, `CC`), shared
  through `#[path]`.

Two Windows details: the compiler reserves an 8 MiB main stack, as Linux and
macOS give it, and `thraxstd.c` `dllexport`s its functions, since a PE
executable has no `-rdynamic`.

`TARGET` reflects the target being built, at compile time too: `thrax build
--target=wasm32-wasi` runs `@build` and `$ @e` code with `TARGET.os ==
"wasi"`, while their foreign calls still run on the host.

## What a platform cannot run

`tests/EXCEPTIONS.thx` lists the examples the corpus skips on a platform, with
the reason. `tests/MAIN.thx` leaves them out there and prints a `SKIP` line
for each; `thxdo corpus` drops skipped modules from both sides before it
compares. An entry is for something the platform cannot do, never for a bug.

| Module | Platform | Why |
|---|---|---|
| `STDLIB_ASYNC_IO` | windows | polls a pipe from a POSIX shell (`sh`, `poll(2)`) |
| `STDLIB_ASYNC_IO` | wasi | spawns a shell, and wasm has no processes |
| `META_RUNTIME` | wasi | runs the compiler at run time; libthrax is host-only |

Outside the corpus: MinGW has no ThreadSanitizer, so the two TSan tests in
`crates/ccg/tests/compile.rs` skip on Windows.

## Tasks

- `thxdo corpus`: both engines on the host.
- `thxdo corpus wasm`: the interpreter against the wasm32-wasi build under node.
- `thxdo build release [musl|gnu]`: the SDK for the host in `target/dist`.
- `thxdo build release windows`: the Windows SDK, cross-built from Linux.

## Releases

`.github/workflows/release.yml` runs on a `v*` tag and publishes one SDK
archive per platform: `bin/thrax`, `lib/` (the compiler library; on Linux also
the musl sysroot), `include/thrax.h` and `library/*.thx`. The Linux and macOS
SDKs are built on plain runners, because a binary linked in a Nix shell names
the glibc in `/nix/store`. Both link the vendored libffi statically. The
Windows SDK is cross-built from Linux and ships `libffi-8.dll` beside
`thrax.exe`. A manual run of the workflow is a dry run.

Running programs needs nothing else. Building them needs the platform's C
compiler: `cc` on Linux and macOS, MinGW-w64 `gcc` on Windows (MSYS2's UCRT64
is the tested one; `THRAX_CC` names another), `emcc` for wasm.
