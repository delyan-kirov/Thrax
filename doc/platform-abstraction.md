# Platform Abstraction

**Status:** Phases 0 and 1 DONE (2026-07-18): the latent FFI/CC bugs are
fixed, `TG::Target` exists and is threaded through DR/TC/CC (`--target=` flag,
host-only), `OP.hpp`'s `#if` and the `THxRT.c` re-derivation are deleted, the
emitted C carries the `#define THX_INT` + `_Static_assert` domino. Phases 2-5
(symbolic sonames, THxPLAT/THxDL, cross toolchains, wasm/32-bit) not started.
**Date:** 2026-07-18.
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
| `app/DR.cpp:170` | `THX_LIBC` soname (`libc.so.6`/`msvcrt.dll`) hardcoded, baked into IR extern strings at frontend time. |
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

### 3.3 Library names are symbolic; sonames resolve at the edge

The IR must never contain `"libc.so.6"`. `@extern` grows the convention that a
lib name without a dot/slash is **symbolic** (`"c"`, `"m"`, `"raylib"`), and
each consumer resolves it as late as possible:

- `FF` (interpreter, host): symbolic -> host soname table at `dlopen` time.
- `CC` (native, target): symbolic -> target soname (from `Target`) at emit
  time -- or, where the platform links libc implicitly (everywhere, really),
  the "c" library lowers to a *direct call* to the named symbol instead of a
  `THxDL_sym` lookup, which is also what wasm needs (no dlopen) and removes
  `-ldl` from programs that only use libc.
- `DR`'s generated `C.thx` writes `"c"`, and `THX_LIBC` is deleted.

An explicit path (`"./libfoo.so"`, `"foo.dll"`) stays verbatim -- user's
responsibility, unchanged.

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

- `DR` injects compile-time constants into the prelude from `Target`:
  `TARGET_OS : Str`, `TARGET_ARCH : Str`, `TARGET_PTR_BITS : Int`.
- Platform-conditional code is ordinary Thrax (`if TARGET_OS ?= "windows"
  then ... else ...`); the existing **dead-global elimination** prunes the
  untaken side, so a Linux build never even references `Windows.*` externs.
  (A CTFE `@if` that folds branches at compile time is the eventual
  strengthening, but DCE + `@assert` already carry the model.)
- Stdlib layering, Rust-shaped, matching the stdcore vision:
  - **core/prelude** -- platform-free (`List`, aliases, `assert`);
  - **sys** -- per-platform extern namespaces: today's auto-injected `C`
    (libc, generated per *target*), later `Win32`, `Wasi` siblings;
  - **std** -- portable modules (`IO`, `File`) written *in Thrax* against the
    target constants, choosing their sys backend internally. Users import
    `std`; only `std` (and the daring) import `sys`.

### 3.6 Toolchain is data too

`DR::build_project`'s `"cc -O2"` becomes a `Toolchain{ cc, cflags, ldflags }`
looked up from `Target` (host default: `cc`; `x86_64-windows-gnu`:
`x86_64-w64-mingw32-gcc` or `zig cc -target ...`; `wasm32-wasi`: wasi-sdk
clang), overridable by `--cc=`/env. The invocation moves from `std::system`
to the argv-vector spawn we already trust in `UTxBUILD`.

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
- **Phase 2 -- Symbolic libs.** `C.thx` says `"c"`; `FF` gets the host soname
  table; `CC` lowers `"c"` externs to direct calls (drops `-ldl` for
  libc-only programs). Delete `THX_LIBC`. Rebaseline `--ir` (extern lib
  strings change).
- **Phase 3 -- Runtime porting layer.** Add `platforms/THxPLAT.h` +
  `platforms/THxDL.{h,c}` (POSIX + Win32 + wasi-stub); `CC` emits calls to the
  seam only; `Value` int payload becomes `THX_INT_T`. Add the `build.cpp`
  platform-macro gate over `compiler/`, `engines/`, `app/`, `utilities/`
  (UTxBUILD's shim migrates onto `THxPLAT` detection).
- **Phase 4 -- First real cross target.** `TARGET_*` prelude constants +
  `Toolchain` table; enable `--target=x86_64-windows-gnu` (via mingw or
  `zig cc`) for `--emit-c`/`--build`; CI job cross-compiles every example and
  runs it under wine (or just compiles, first). Windows is the forcing
  function for section 3.2/3.3 exactly as it is for the nix rule.
- **Phase 5 -- 32-bit + wasm.** `wasm32-wasi` target: 32-bit `Int` end to end
  (literal range checks in TC, `THX_INT_T` at work, no-dlopen path), the most
  hostile target and therefore the best oracle that the abstraction holds.
  CI matrix: linux-x64 (host suites) + windows-cross + wasm32-wasi.

### Explicitly out of scope (doors left open)

Cross-*interpreting* (IT is host-only by definition); MSVC as a first-class
toolchain (generated projects remain the Windows-native story per
architecture.md section 6); endianness (all initial targets little-endian; the
`Target` field exists so byte-order-sensitive code has something to ask);
per-target `std` beyond `IO` basics.
