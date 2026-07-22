# Platform Abstraction

**Status:** Phases 0, 1 and 2 DONE. 0+1 (2026-07-18): the latent FFI/CC bugs
are fixed, `TG::Target` exists and is threaded through DR/TC/CC (`--target=`
flag, host-only), `OP.hpp`'s `#if` and the `THxRT.c` re-derivation are
deleted, the emitted C carries the `#define THX_INT` + `_Static_assert`
domino. Phase 2 (2026-07-21, revised form -- see section 3.3): `@extern` is
now `@extern "C" "symbol" "lib"` with SYMBOLIC library names; the
interpreter resolves them to host sonames at dlopen time
(`TG::Target::soname`), the native backend emits DIRECT linker-resolved
calls via asm-label declarations plus link flags (`CC::link_flags`) --
generated programs never dlopen. Phase 2.5 DONE (2026-07-21): `$ @run
<expr>` compile-time execution + the `BUILD` module (library/BUILD.thx) --
`@run BUILD.lib`/`BUILD.lib_path` directives feed the link set and library
search paths from Thrax source, serving both engines. Phase 3 DONE
(2026-07-21): `platforms/THxPLAT.h` (THX_OS_* / THX_PTR_BITS / THX_INT_T,
first in the runtime bake), the Value `T_INT` payload is the TARGET word
(wraps at 32 bits on 32-bit targets), and `build check-platform` enforces
the no-raw-platform-macros boundary by mechanism (also in pre-push). THxDL
was DROPPED as obsolete -- phase 2's direct-call design means generated
programs never dlopen (see section 4). Phase 4 DONE (2026-07-22):
`wasm32-wasi` is the first cross target -- `TG::Toolchain` as data (wasi-sdk
clang via `$WASI_CC`, stack placed first + enlarged), 32-bit `Int` end to end
(`@int32` dispatch keys, literal range check, wrap-on-store), a generated
qualified `TARGET` reflection module, and `build wasm-test` (combined suite
under wasmtime, in CI). Phase 5 (compiler-in-browser + website via Emscripten)
is next.
**Date:** 2026-07-22 (originally 2026-07-18).
**Scope:** how the compiler, the two engines, and the runtime learn what
platform they are compiling *for* (target) versus running *on* (host), and
where platform-specific knowledge is allowed to live. Companion to
`doc/architecture.md` (physical layout) and `doc/stdcore` vision (prelude/`C`
namespace).

**Goal:** if a platform has libc, Thrax programs run on it. Concretely:
Linux/macOS/Windows x wasm x x86-64/arm64/32-bit, without the compiler binary's
own build platform leaking into the programs it produces.

---

## 0. The core defect: host and target are conflated

Today "the platform" is decided in four independent places, all of them at the
wrong time:

1. **When the `thrax` binary is compiled** -- `OP.hpp` picks `TY_INT =
   "@int32"/"@int64"` from `__SIZEOF_POINTER__` of the machine building the
   compiler. The *language's* `Int` is an accident of where the compiler was
   built.
2. **When the frontend runs** -- `DR.cpp` bakes `"libc.so.6"` (or
   `"msvcrt.dll"`) into every `C.*` extern in the IR. The IR -- and therefore
   the emitted C -- carries a Linux soname; wrong on macOS
   (`libSystem.B.dylib`), musl, wasm.
3. **When the emitted C is compiled** -- `THxRT.c` re-derives `THX_INT` from
   *its* compiler's pointer size. This must agree with what (1) chose, but
   nothing checks it: a cross-compile silently disagrees on the `mono` keys
   (`"+@int64"` vs `"+@int32"`) and dies at runtime dispatch.
4. **When the program runs** -- `FF.cpp` dlopens whatever string the IR baked
   in.

Rust's framing dissolves this: **host** (where a stage runs) and **target**
(what the program is for) are different values, and the target is *data* -- an
explicit input to compilation -- never a preprocessor conditional evaluated at
some earlier stage's build.

## 1. Inventory of platform leaks

Marked FIXMEs:

| Site | Leak |
|---|---|
| `compiler/OP.hpp:49` | `TY_INT`/`TY_NAT` chosen by the compiler binary's own pointer width (`#if __SIZEOF_POINTER__`). |
| `app/DR.cpp:170` | `THX_LIBC` soname (`libc.so.6`/`msvcrt.dll`) hardcoded, baked into IR extern strings at frontend time. FIXED by phase 2: libs are symbolic, `THX_LIBC` deleted. |
| `platforms/THxRT.c:153` | Runtime independently re-derives `THX_INT` from *its* build; must agree with (1) but nothing enforces it. |

Unmarked, found by audit:

| Site | Leak |
|---|---|
| `engines/CC.cpp` `c_type` | Maps `@int64 -> "long"` via `TY_INT`. On Windows LLP64 `long` is 32-bit -- **wrong-width bug** the moment emitted C meets MSVC/mingw. Must use `<stdint.h>` names. |
| `engines/CC.cpp` extern glue | Emits `#include <dlfcn.h>` + `dlopen/dlsym` -- POSIX-only generated C; no Win32/wasm story. |
| `app/DR.cpp:578` | `std::system("cc -O2 ...")` -- host C compiler hardcoded; no cross toolchain, no MSVC, no wasi-sdk/emcc. |
| `engines/FF.cpp` `desc_of` | Keys on **stale pre-Phase-0 names** (`"Int"`, `"Str"`, ...). Canonical `@`-names all fall through to the word-sized `slong` default -- including `@str`/`@ptr`. Works today only because on x86-64 pointer and slong have identical ABI class. Breaks on 32-bit and wasm. Latent bug. |
| `engines/ITxDATA.cpp:79` | Stale `"Str"`/`"Real"` return-type string compares (same vintage as above). |
| `engines/ITxDATA.cpp:66` | `Real` argument marshalled by `memcpy(&w,&d,sizeof(w))` into an `ssize_t` word -- on a 32-bit host this copies 4 of the double's 8 bytes. The whole "everything is one machine word" FFI marshalling is a 64-bit-host assumption. |
| `platforms/THxVALUE.h:59` | `T_INT` payload is `long long` unconditionally -- a 32-bit target's `Int` (`@int32`) would still compute at 64-bit width in the runtime; semantics undefined per target. |

Contained / already correct (keep as models):

- `utilities/UTxBUILD.hpp` -- build-tool process shim: POSIX/Win32 isolated
  behind one function, everything above is platform-free. This is the shape we
  want everywhere.
- `engines/FF.cpp` -- "the only module that touches libffi/dlopen" is the right
  *placement*; only its type table is stale.
- `external/libffi` vendored + layered resolution -- already the Windows/no-nix
  lifeline per `doc/architecture.md` section 5.
- `examples/io_example/MAIN.thx:23` FIXME is about effects, not platforms.

## 2. Prior art, and what we take from each

- **Rust**: the *target spec as data* -- a triple/struct (`arch`, `os`, `env`,
  pointer width, endianness) that every stage reads; `cfg` conditionals over
  it; and the `core` / `alloc` / `std` layering where `core` has zero OS
  dependency and `std` is the portable facade over per-OS `sys` modules that
  all expose one identical internal interface.
- **Koka**: compile to *portable* C, and confine every platform conditional to
  the runtime library (kklib). The generated C is the same everywhere; the
  runtime is the porting layer. We already have this shape -- `CC` bakes
  `platforms/THx*` in as text -- we just have to keep the *generated* part
  clean of `#ifdef`s that belong to the runtime.
- **Jai**: the platform is an ordinary *compile-time value* in the language
  (`#if OS == .WINDOWS` over normal modules), and the build is a program, not
  a description. Our CTFE `@assert`, dead-global elimination, and `build.cpp`
  are exactly the ingredients: conditional platform code can be ordinary
  Thrax modules pruned by DCE -- no preprocessor in the language.
- **Zig** (honorable mention): cross-compilation as the *default posture* --
  every design question below is answered by asking "does this still work
  when host != target?".

## 3. The abstraction

### 3.1 `Target` is data, threaded once (`compiler/TG.hpp`)

One small value object, the single source of platform truth for a compilation:

```cpp
namespace TG {
struct Target {
  enum class Os   { Linux, Macos, Windows, Wasi };
  enum class Arch { X86_64, Aarch64, X86, Arm, Wasm32 };
  Os    os;
  Arch  arch;
  int   ptr_bits;      // 32 or 64, derived from arch
  // derived views, so no caller re-implements policy:
  const char *int_ty()  const;  // "@int64" / "@int32"
  const char *nat_ty()  const;
  const char *libc_soname() const;  // "libc.so.6" / "msvcrt.dll" / ...
  bool  has_dlopen() const;         // false on Wasi
};
Target host();                        // detected once, at runtime
std::optional<Target> parse(std::string_view); // "--target=x86_64-linux" ...
}
```

Rules:

- Constructed **at runtime** in `app/` (default `TG::host()`, overridable by
  `--target=`), passed down. Never a preprocessor decision inside
  `compiler/` or `engines/`.
- `OP.hpp` loses its `#if`: it declares *all* the canonical width names
  (`TY_INT8..64`, ...) -- which it already does -- and nothing else. `TY_INT` /
  `TY_NAT` as global constants are **deleted**; every current reader
  (`TC` literal typing, `DR` prelude aliasing, `CC`, pattern lowering)
  takes the name from the `Target`.
- The interpreter engine (`IT`/`FF`) is *defined* to run programs **for the
  host**: `--target` != host with the interpreter is a diagnosed error, not a
  silent lie. Cross is the native backend's job.

### 3.2 The emitted C is portable; the runtime is the porting layer

Koka's split, enforced:

- **`platforms/` is the only directory allowed to contain OS/arch
  conditionals** (`_WIN32`, `__linux__`, `__wasm__`, ...). New file
  `platforms/THxPLAT.h` centralizes detection into `THX_OS_*` / `THX_PTR_BITS`
  macros; the other `THx*` files consume those, never raw predefined macros.
  (`utilities/UTxBUILD.hpp` keeps its process shim -- it *is* a platform layer
  for the build tool -- but switches to the same detection header.)
- **`build.cpp` enforces this as a mechanism**, like the dependency DAG: a
  check target greps for raw platform macros outside `platforms/` and fails
  the build. Boundaries by mechanism, not discipline (architecture.md section 3).
- **The domino check replaces trust.** `CC` knows the target, so it emits into
  every program: `#define THX_INT "@int32|@int64"` (the runtime's `mono`-key
  width -- deleting the `THxRT.c` re-derivation) **and**
  `_Static_assert(sizeof(void*) == N, "compiled for <target>")`. A generated
  `.c` fed to the wrong C compiler now fails at *compile* time with a named
  error instead of dispatching garbage at runtime.
- **dlopen goes behind a runtime seam**: `platforms/THxDL.{h,c}` exposing
  `THxDL_sym(lib, sym)`; POSIX `dlopen`, Win32 `LoadLibraryA/GetProcAddress`,
  wasi = static-link-only stub that traps with a clear message. `CC` stops
  emitting raw `dlopen` calls and `#include <dlfcn.h>`; it calls the seam,
  which is already baked in with the rest of the runtime.

### 3.3 Library names are symbolic; resolution is the consumer's job (DONE)

The declaration form is `@extern "C" "symbol" "lib"` -- plain application
shape: the ABI first (`"C"` is the only one today; `"js"` is reserved for
wasm imports), the symbol, then the SYMBOLIC library name (`"libc"`,
`"libm"`, `"raylib"`). No path or soname ever appears in source or IR, and
each consumer resolves as late as possible -- dlopen-vs-link IS the
interpreter/native split:

- `FF` (interpreter, host): `TG::Target::soname` maps the symbolic name to a
  host loadable name at `dlopen` time ("libc" -> `libc.so.6` /
  `libSystem.B.dylib` / `msvcrt.dll`, "libm" -> where the math symbols live,
  "raylib" -> `libraylib.so` with the platform's decoration; a name
  containing '.' or '/' passes verbatim, user's responsibility).
- `CC` (native, target): every foreign symbol is a DIRECT call the system
  linker resolves, statically or dynamically per what it finds. The
  declaration uses an asm label (`extern int64_t THx_sym_0(char*)
  __asm__("puts");`) so the C identifier cannot collide with the libc
  headers the inlined runtime includes. Generated programs contain no
  dlopen and need no `-ldl`; `CC::link_flags` derives one flag per library
  ("libc" implicit, "libm" -> `-lm`, "raylib" -> `-lraylib`, paths
  verbatim), `--build` applies them, and the generated header comment
  prints the exact cc line. This is also what wasm needs (no dlopen).
- `DR`'s generated `C.thx` writes `"libc"`/`"libm"`.

SEARCH PATHS and the link set are project data fed from Thrax itself (phase
2.5, DONE -- see section 4): `$ @run BUILD.lib "m"` /
`$ @run BUILD.lib_path "./vendor"` -- the Jai "build is a program" door,
opened a crack. The ABI slot is where `"js"` externs for wasm imports will
land (native wasm32: emitted import attributes; browser interpreter: the
host-table trampolines).

### 3.4 Word size: `Int` is the target word, decided in one place

Keep the Jai/Rust-isize policy already chosen in the stdcore vision -- `Int`
aliases `@int64` or `@int32` by **target** (not compiler-host) word size, via
`Target::int_ty()` in exactly one call site (`DR`'s prelude generation; TC and
friends read the resolved alias). Consequences owned deliberately:

- Interpreter values stay 64-bit internally (`long long` / `ssize_t` on the
  host) -- fine, because the interpreter only runs host programs (section 3.1).
- The native runtime's `Value` `T_INT` payload becomes `THX_INT_T`
  (`int64_t`/`int32_t` from `THxPLAT.h` per `THX_PTR_BITS`) so a 32-bit
  target's arithmetic actually wraps at 32 bits -- semantics match the type.
- `CC::c_type` maps every `@intN`/`@natN` to `<stdint.h>` names
  (`int64_t`, not `long`) -- fixes the LLP64 bug outright.

### 3.5 Language level: target constants + ordinary modules (Jai)

No `#ifdef` enters the language. Instead:

- `DR` generates a `TARGET` reflection module from `Target` (like the prelude
  `Int`/`Nat` aliases and the `C` libc namespace, it is emitted per build
  because the values are only known once the target is fixed). Accessed
  qualified, no import: `TARGET.int_bits`/`.ptr_bits : Int` (the target word),
  `TARGET.int_max`/`.int_min : Int` (the `Int` value range), `TARGET.os`/
  `.arch`/`.name : Str`. A qualified module (not bare `TARGET_*` constants)
  keeps the global namespace clean. **DONE** (phase 4).
- Platform-conditional code is ordinary Thrax (`when TARGET.int_bits is 64
  then ... else ...`); the existing **dead-global elimination** prunes an
  unreferenced side, so a Linux build never even references `Windows.*`
  externs. (A CTFE `@if` that folds a branch on a constant condition -- so a
  64-bit-only literal in the dead branch never reaches TC's range check -- is
  the eventual strengthening, but DCE + `@assert` already carry the model.)
- Stdlib layering, Rust-shaped, matching the stdcore vision:
  - **core/prelude** -- platform-free (`List`, aliases, `assert`);
  - **sys** -- per-platform extern namespaces: today's auto-injected `C`
    (libc, generated per *target*), later `Win32`, `Wasi` siblings;
  - **std** -- portable modules (`IO`, `File`) written *in Thrax* against the
    target constants, choosing their sys backend internally. Users import
    `std`; only `std` (and the daring) import `sys`.

### 3.6 Toolchain is data too

`DR::build_project`'s `"cc -O2"` becomes a `TG::Toolchain{ cc, hint, cflags,
runner, exe_suffix, rpath }` looked up from `Target` (host default: `cc`;
`wasm32-wasi`: `$WASI_CC`, a wasi-sdk clang; `x86_64-windows-gnu` later:
`x86_64-w64-mingw32-gcc` or `zig cc -target ...`). **DONE** for host +
wasm32-wasi (phase 4); the tool's *location* stays out of the compiler via an
env var, the same discipline `@extern` library names follow. The invocation
still goes through `std::system` for now; moving it to the argv-vector spawn
in `UTxBUILD` is a later cleanup.

## 4. Implementation plan

House rule applies: one variable per phase, suites green at every step,
`--ir` rebaselined only where the phase says so.

- **Phase 0 (DONE) -- Pay the latent-bug debt (no design change).** Fix `FF::desc_of`
  + `ITxDATA` to key on canonical `@`-names with a true pointer descriptor for
  `@str`/`@ptr`/`@array`; fix `CC::c_type` to `<stdint.h>` names; fix the
  `Real`-argument word marshalling. Host behavior identical, suites green.
  These fixes are prerequisites for *every* later phase and are correct on
  their own.
- **Phase 1 (DONE) -- `TG::Target`, host-only.** Add `compiler/TG.{hpp}`; construct in
  `app/main.cpp` (default host; parse `--target` but reject != host for now);
  thread through `DR` (prelude + `C.thx` generation), `TC` (literal/base-type
  spelling), `CC`. Delete the `OP.hpp` `#if` and `TY_INT`/`TY_NAT` constants.
  `CC` emits `#define THX_INT` + the `_Static_assert` domino; delete the
  `THxRT.c` sniff. IR byte-identical on the host.
- **Phase 2 (DONE, 2026-07-21, revised) -- Symbolic libs, direct native
  calls.** Landed stronger than originally sketched: `@extern` became
  `@extern "C" "symbol" "lib"` (ABI slot for future `"js"`), ALL native
  externs are direct linker-resolved calls via asm labels (not just libc),
  generated programs never dlopen, `CC::link_flags` + `--build` own the
  link line, `TG::Target::soname` owns interpreter resolution. `THX_LIBC`
  is gone. `--ir` rebaselined (extern lib strings changed).

The goal driving the rest of the sequence (decided 2026-07-21): **wasm as
the first cross target**, en route to a static showcase WEBSITE with the
interpreter embedded in the page. Wasm forces the same discipline Windows
would have (and more: no dlopen at all, 32-bit), and wasi-sdk/wasmtime are
easier to hold in nix than mingw/wine; Windows moves after wasm.

- **Phase 2.5 (DONE, 2026-07-21) -- `@run` + the `BUILD` compiler API.**
  Declaration-level `$ @run <expr>` (generalizing `@assert`'s CTFE: a
  hidden `%run$N` global, a DCE root, forced through the interpreter after
  linking in every mode; the value is discarded). The compiler API landed
  as DATA rather than intrinsics: `library/BUILD.thx` is an ordinary stdlib
  module whose `lib : Str -> Directive` / `lib_path : Str -> Directive`
  build `BUILD.Directive` values; DR inspects each forced `@run` result and
  applies directives (`force_ctime_runs`). `lib` joins the link line
  natively (`CC::lib_flag`) and the interpreter's preload set (dlopen
  RTLD_GLOBAL); `lib_path` becomes -L + rpath natively and a dlopen prefix
  in the interpreter (`FF::add_lib_path`/`add_preload`). No new effect or
  intrinsic machinery -- a `Directive` built at run time is just a value.
  examples/CT_RUN.thx exercises it in the suite. (Expression-position
  `@run` splicing literal-serializable values comes later.)
- **Phase 3 (DONE, 2026-07-21) -- Runtime porting layer.**
  `platforms/THxPLAT.h`: centralized THX_OS_* / THX_PTR_BITS detection plus
  `THX_INT_T` (the target word), baked FIRST into the runtime amalgam; the
  `Value` `T_INT` payload is now `THX_INT_T` -- APIs still traffic in
  `long long`, and the truncation on store IS the 32-bit wrap. The
  `build check-platform` gate greps for raw platform macros outside
  `platforms/` (wired into pre-push), with four justified exemptions:
  `compiler/TG.hpp` (the sanctioned host() detection), `utilities/
  UTxBUILD.hpp` and `utilities/UT.cpp` (build-tool/debug-trap shims --
  KEPT on their own detection rather than migrated onto THxPLAT, because
  utilities may not depend on platforms in the module DAG), and
  `build.cpp` (the gate's own macro list). Two planned items were already
  moot: `THxDL.{h,c}` is DROPPED (phase 2's direct calls mean generated
  programs never dlopen; the interpreter's dlopen is confined to FF.cpp,
  whose browser story is the phase-5 host table, and a Windows-native
  compiler build is out of scope), and the ITxDATA/FF `Real` marshalling
  debt was already paid in phase 0 (`FF::Slot` is fixed 64-bit).
- **Phase 4 -- First real cross target: `wasm32-wasi`. DONE.** `TG::Toolchain`
  as data (host: `cc`; wasm32-wasi: `$WASI_CC`, a wasi-sdk clang from the nix
  flake, with `--stack-first`/`-z stack-size=8MB` -- wasm has no stack guard
  page and wasm-ld defaults the shadow stack *above* the data segment, so the
  runtime's recursive global-forcing silently corrupted globals until the
  stack was placed first and enlarged) + `runner`/`exe_suffix`; `main.cpp`
  lets a cross target through for `--build`/`--emit-c`/`--ir` (interpreting
  stays host-only). 32-bit `Int` end to end: `@int32` mono keys registered in
  ITxDATA (both widths present, TC's target-folded key selects; wasm32 CTFE
  wraps on this host exactly like the emitted program), `THX_INT_T` payload
  wraps on store, and TC rejects integer literals that don't fit the target
  word (`Target::lit_max`, `INT_LITERAL_RANGE`). Target reflection is a
  generated qualified module `TARGET` (see 3.5). RANDOM/TAIL_CALLS/RC_LOOP
  made portable (Schrage's method; constants kept under 2^31). `build
  wasm-test` cross-compiles the combined suite and runs it under wasmtime;
  wired into CI. Exit criterion met: every example passes under wasmtime.
- **Phase 5 -- The compiler in the browser + the website.** `build wasm`
  compiles the compiler+interpreter with Emscripten (add emscripten to the
  nix flake). The browser host is wasm32: `TG::host()` learns it, and FF
  swaps libffi+dlopen for a compiled-in host table of the `C` namespace's
  functions (statically linked from Emscripten's musl; custom externs
  resolve against the table or fail clearly -- no libffi port needed).
  Then `examples/website/`: a static site (landing + tour generated from
  the CI-verified examples + rendered doc/*.md + the playground page
  loading thrax.wasm), deployed to GitHub Pages by CI. `"js"` externs
  (wasm imports via the ABI slot) follow for interactive demos.
- **Phase 6 -- Windows + 32-bit natives.** `x86_64-windows-gnu` via mingw or
  `zig cc`, wine-run CI; the remaining 32-bit native targets. CI matrix:
  linux-x64 (host suites) + wasm32-wasi + windows-cross.

### Explicitly out of scope (doors left open)

Cross-*interpreting* (IT is host-only by definition); MSVC as a first-class
toolchain (generated projects remain the Windows-native story per
architecture.md section 6); endianness (all initial targets little-endian; the
`Target` field exists so byte-order-sensitive code has something to ask);
per-target `std` beyond `IO` basics.
