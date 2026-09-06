# TODO

The project backlog: open items in one place. Larger subsystems keep their own
design notes under `documentation/`, linked below.

## Engine / type system / FFI

- **Standard-library native support lib. DONE (native, 2026-09-06).** The
  `thx_*` scalar conversions (`i2d`/`d2i`/`i2f`/`f2i`/`f2d`/`d2f`/`i2p`,
  `real_to_str`/`f32_to_str`) now live in ONE source, `crates/ccg/src/thraxstd.c`.
  The native backend appends it to every emitted program (removed the duplicate
  defs from `runtime.c`); the interpreter compiles and links it in-process
  (`crates/interpreter/build.rs`, `static:+whole-archive`, plus `-rdynamic`
  emitted as a `rustc-link-arg` from the `thrax`/`ccg`/`interpreter` build scripts,
  NOT `RUSTFLAGS`/config, which a CI-set `RUSTFLAGS` overrides) and resolves the
  symbols through the normal
  `@extern "C" "..." ""` / `dlsym(RTLD_DEFAULT)` seam. The whole per-symbol host
  table in `machine/data.rs` (86 arms of libm/libc/`thx_*` + its `extern "C"`
  block and marshalling helpers) is GONE: native `run_extern` now just guards the
  `"WASM"` abi, calls `crate::machine::ffi::call_extern` (libffi), and `fflush`es
  C stdio. Every foreign call, libc/libm and `thx_*` alike, takes the one libffi
  path, reaching the same real symbols the compiled backend links. Verified: ccg's
  cross-check suite runs both engines and diffs their output, so they agree
  byte-for-byte (float formatting included).

  Follow-ups:
  - **`dlsym` address caching (perf, optional).** Every libc call now does a
    `dlsym` (the resolver caches `dlopen` handles but not resolved symbol
    addresses). Cache the address per `(lib, symbol)` in `machine::ffi::resolve`
    if it ever matters. Pure optimization; correctness and output are unchanged.
  - **wasm (blocked, task #11).** Bringing `thx_*` to the wasm playground the same
    way (compile `thraxstd.c` for wasm against nix's wasi-libc, link into the
    interpreter cdylib, call via `extern "C"`) is blocked in the current dev shell:
    there is no wasm linker (`wasm-ld`/`ld.lld`/`lld` all missing, and rustc's
    self-contained wasm dir is absent), so the playground cdylib cannot be linked
    or tested here at all. Unblock by adding `lld` (`pkgs.lld` /
    `llvmPackages.bintools`) to `flake.nix`, which also un-breaks the playground
    build in general. Note: the playground already stubs every `@extern` on wasm,
    so floats never worked there; this is net-new, not a regression.

- **Building funcitons** In jai, you can use the
  programming language like a library and even interact with it with an event loop.
  I think that's useful.

- **lsp, syntax highlighting, tags** There should be more language tooling. 

- **Overloading of `@extern` with record params.** Same-name overloads of a C
  function do not resolve: a positional call fails overload resolution (tuple to
  record promotion does not run during overload trial-unify), and a named-record
  call type-checks but the extern record-flatten misfires. Distinct-name
  fixed-arity bindings work. This is the mechanism a `printf` family would want.

- **Operator overloading is parsed but not implemented.** `$ @operator.{ op } :
  ty = expr` produces an `Item::OperatorDef` that nothing in typing or lowering
  consumes, so a user operator never resolves. Wire it up or drop the surface.
  The syntax should be updated to `@opr "<operator>"`.

## Tooling / testing

- **Negative test suite.** Every suite today is positive (asserts good programs
  pass), which structurally cannot catch a soundness hole (a bad program that
  wrongly passes, which is how the struct-literal arity bug shipped). Need
  programs each paired with the diagnostic (or error code) they must produce; a
  run fails if such a program compiles, or fails with the wrong error. Seed cases:
  positional struct literal with wrong arity/field count; bare `.{..}` with no
  inferable struct; over/under-applied type constructors; scalar/record promotion
  that should not fire. Mechanism (file format, stability against message churn,
  corpus vs beside the Rust tests) still to design.

## Larger subsystems (own design docs)

- **Effects M3.2:** evidence passing (O(1) dispatch) and the tail-resumptive
  optimization; the handler is still a dynamic search up the stack. See
  `documentation/effect-system-design.md`.
- **Project reorg:** largely done already (the Rust rewrite delivered the modular
  `crates/` layout and retired make for cargo + nix; the only makefiles left are
  in the vendored `external/libffi`). Residual ideas: differential parser testing
  against the bison grammar (`documentation/thrax.y`), and a fuller platform
  split.

## Surface syntax (open decisions)

- **Interpolation:** require interpolants to already be `Str`, or add a
  `show`/stringify overload first? Brace-escape spelling? Perhaps new syntax?
- **Ranges:** desugar to a library `range` function (recommended) or a core
  primitive? Descending `..>` now or later?
- **More intrinsics** `@if` `@else` `@inc` `@tokens` `@char`
- **Effects to std lib functions** why does printing not have an effect?
- **More operators** for example the map operator `<!>`, exponential `^`
  and potentially more.


## Static linking / self-contained binaries (musl)

**Goal.** For a "basic" (non-GUI, no glibc-specific FFI) Thrax program, `thrax
build` should be able to produce a **fully static, dependency-free executable**,
no `.so` lookups, no runtime library-path problems at all. Just a file you can
copy anywhere and run.

**What already landed this session (verify these are committed):**
- `thrax build` writes artifacts into a `thrax-out/` directory instead of spewing
  `<stem>.c` + the executable next to the source. (`cmd_build` in
  `crates/thrax/src/driver.rs`.)
- For a path-named `@extern` **shared** lib (e.g. `bin/libraylib.so`), the build
  now bakes an **rpath** so the runtime loader finds the versioned soname (it
  canonicalizes the symlink to the real dir). Skips `.a` archives. Same file.
- Static linking of a library **already works today**: point an `@extern` at a
  static archive, `@extern "C" "sym" "path/libfoo.a"`, and its code is baked in
  (verified: `nm` shows the symbol `T`, `ldd` is clean, no rpath needed). So the
  FFI supports both `.so` (dynamic + rpath) and `.a` (static).

**What we found (the investigation):**
- You **cannot statically link a `.so`**, a `.so` is inherently dynamic; static
  linking needs the `.a` build of the library. (nixpkgs raylib ships only `.so`.)
- A **GUI app cannot be fully static**: raylib pulls in `libOpenGL`/`libGLX`/
  `libX11`, which MUST be dynamic (the real GL implementation is the GPU driver,
  loaded at run time by the system). This is true on every Linux, not a Thrax
  limitation. So static only makes sense for basic/compute programs.
- **glibc is hostile to static linking** (it `dlopen`s NSS/locale even when
  "static"). The right tool is **musl**, a libc designed for clean static links.
- **Proven it works:** `thrax emit-c examples/FIB.thx > fib.c`, then compiled with
  a musl toolchain (`x86_64-unknown-linux-musl-gcc -O2 -static`), gave a **138 KB
  fully static** binary (`file`: "statically linked"; `ldd`: "not a dynamic
  executable"; it ran, `test = 0`). So our generated C is already musl-clean.

**The one subtlety (what "link with musl" actually means):**
It is a **toolchain/sysroot swap**, not "append `libc.a`". libc is two halves
that must match: the **headers** (the compiler reads glibc's vs musl's struct
layouts) and the **library** (`libc.a`) + its **crt startup objects**
(`crt1.o`/`crti.o`/`crtn.o`, the code that runs before `main`). You must compile
against musl's headers AND link musl's crt + `libc.a`, all consistent; mixing
glibc headers with musl's lib corrupts. A "sysroot" is just a prefix holding
`include/` + `lib/` (headers + libc.a + crt) for a target.

**Building musl gives you everything** (this is why the plan is fine): `./configure
&& make && make install prefix=P` produces `P/include`, `P/lib/libc.a`, the crt
objects, AND a `musl-gcc` wrapper, all under one prefix (a clean sysroot).

**Compiler note (we use clang, not gcc):**
- `musl-gcc` is a **gcc**-only wrapper (specs file), it does not drive clang.
- clang does not need a wrapper: it is natively a cross-compiler. Use
  `clang --target=x86_64-linux-musl --sysroot=P -static ...`. (clang also needs
  its own runtime, `compiler-rt`/`libgcc`, plus `crtbegin`/`crtend` from the
  compiler, in addition to musl's `crt1`/`crti`/`crtn`; a normal clang has these.)
- So building musl gives the sysroot (useful to BOTH compilers) + a gcc wrapper
  (only if you use gcc). On clang, point at the sysroot directly.

**Where we wanted to go (the plan, staged):**
1. **Add a musl build TARGET**, exactly like the existing `wasm32-wasi` one:
   `thrax build --target=x86_64-linux-musl`, where `utilities::toolchain()`
   returns the musl cc + `-static`. Honor a `THRAX_MUSL_CC` env (mirroring the
   existing `THRAX_WASM_CC`), defaulting to `clang --target=... --sysroot=...`.
   The wasm branch in `crates/utilities/src/target.rs` `toolchain()` is the
   template. This gets static basic binaries working with ~the wasm target's
   effort, using an external musl toolchain (nix's `pkgsCross.musl64` cc), no
   vendoring yet.
2. **Vendor musl as a git subtree** (like `external/libffi`) and build it locally
   in a build step, so the sysroot is produced from source and the whole thing
   works **without nix** (the "nix is an accelerator, not a substrate" ethos).
   Building it ourselves yields a single-prefix sysroot, which sidesteps the nix
   friction below. Model: how `crates/interpreter/build.rs` vendors + builds
   libffi.

**Gotchas discovered (for when you return cold):**
- nixpkgs **splits musl** into separate outputs, headers in `musl.dev`, libs in
  `musl.out`. A single `--sysroot` needs both under one prefix, our own build
  gives that; the split is why a quick clang+nix-musl demo failed with "cannot
  find -lc".
- nixpkgs' `clang` is **wrapped for the host target** and warns/misbehaves with a
  non-host `--target`. Use an unwrapped or musl-targeted clang, or our own sysroot.
- Scope: static-musl only for basic programs (no GUI/GL; any `@extern` must
  resolve in musl or be a static `.a`).

**Key files:** `crates/utilities/src/target.rs` (`toolchain()`, the target/cc
seam; wasm branch is the template), `crates/thrax/src/driver.rs` (`cmd_build`,
link flags / rpath / output dir), `crates/interpreter/build.rs` (the vendored-
libffi build, the model for vendoring+building musl


**Why is it so difficult to define a stupid ass operator?**, there are no asserts,
everything is in different files for no reason. It's just impossible to find what
you want. How is this code good? It's not good, it's trash. 

---

## Str vs C strings (`CStr`) -- revisit

Considered making `C.thx`'s libc bindings take a distinct `CStr` (a NUL-terminated
`char*`) instead of the prelude `Str`, so a non-NUL-terminated Thrax string can't be
typed as a C string. Backed out for now: the FFI ALREADY copies a `Str`/`@array` and
appends a NUL for the duration of a call (`crates/interpreter/src/machine/ffi.rs`, the
`c.push(0)` path + native `char*` marshalling), so `Str -> char*` is already safe, and
introducing `CStr` (= a distinct type, e.g. `@alias = @array`) breaks `library/IO.thx`
(which passes `Str` to `C.write`/`fopen`/`getenv`/...) and needs a `Str`<->`CStr`
conversion primitive that doesn't exist (`Str` and `@array` don't interconvert). So it's
type-level clarity, not a correctness fix, at the cost of a new primitive + an IO.thx
rewrite + conversions at every seam.

It still doesn't feel quite right (a Thrax `Str` isn't guaranteed NUL-terminated, and the
"append a NUL on marshal" trick hides that from the type system; the return direction --
C hands you a `char*` you then treat as a `Str` -- is also unmodeled). Revisit: decide
whether C interop should go through an explicit `CStr` + `to_c_string`/`from_c_string`,
and how the conversion primitive should work (identity at runtime, but a real type cast).

---

## Sequences: `@vec` default, codata `for`, `List` optional (design direction)

Three-way sequence story (being moved toward):
- `@vec` -- materialized, contiguous, random-access, finite. THE default for `[...]`.
- `Stream` (`@codata`) -- lazy / possibly infinite / streaming. Already exists in CORE
  (`Stream : @codata t = head : t, tail : Stream t`; `count_from`; `[lo ...]`).
- `List` (cons `@union`) -- optional, persistent, O(1) cons. Kept in CORE, no longer the
  default; keeps `h :: t` patterns via its `@compiler_interface_sequence_view` hook.

Iteration DONE (2026-09-06, in CORE): two combinators, kept deliberately minimal.
`for` runs a body FOR ITS EFFECTS (returns `{}`); `formap` TRANSFORMS (returns the
mapped structure). Both understand the `Loop` effect (`break` stops, `continue`
skips; value-less). `for` is overloaded for `Stream`, giving an INDEFINITE effectful
loop ended by `break` (or run forever), with `forever : Stream {}` as a source:
`for (count_from 0) (\i = if <cond> => break {} else ...)`. `formap` is finite-only
(collecting an infinite stream doesn't terminate). The effect row `<e>` passes
through uncaught. See the `Loop`/`for`/`formap` block in CORE.thx. (No dedicated
example/test module for now, verified on both engines manually.)

---

## DONE (2026-09-06): `@vec` is the default sequence; `List` removed from the compiler

`[...]` / `[lo...hi]` build a `@vec`; `::` prepends to one; `[]`/`[a,b,..r]`/`h::t`
patterns match a `@vec` via its `@compiler_interface_sequence_view` hook (in CORE). The
compiler no longer hardcodes `List`/`Cons`/`Nil` anywhere in literal/pattern lowering --
sequence patterns all route through the Stage 2 `sequence_view` machinery. `VEC.thx` is
now the comprehensive sequence module (index/push-based, O(n)); `LIST.thx` was deleted;
the `List` cons union stays OPTIONAL in CORE (constructed/matched via `List.Cons`/`Nil`,
bridged with `VEC.from_list`/`to_list`). Added a `@vec_slice` primitive.

Follow-ups worth doing: `x :: xs` (prepend) is O(n) on a vector, so `::`-recursion is
quadratic -- callers should build with push / index iteration (documented in VEC.thx).
The codata iteration direction above is now DONE (`foreach`/`loop` over `Stream`).

---

## DONE (2026-09-06): compiler forgets `Str`/`Real`/`Ptr`; strict numeric literals

The compiler no longer knows any friendly scalar name. The type constants are the
`@`-forms (`@int`/`@nat`/`@float64`/`@str`/`@ptr`/`@bool`/`@array`/`@vec`); `canonical_con`
and `display_con` are gone, so a type both parses and prints as its source spelling.
`@str` is the string type and `@ptr` the opaque pointer. `Real` survives only as a CORE
alias (`$ Real : @alias = @float64`) for readability; `Str`/`Ptr` were migrated to
`@str`/`@ptr` across the corpus, and `library/C.thx` (CORE-less) uses `@float64` directly.
The libc-marshalling arms in `machine/ffi.rs` and `ccg/src/gen.rs` dropped their dead
friendly spellings.

Numeric literals are now STRICT: an untyped integer literal is always an integer and
never adopts a float type from context. `100.0 % 7`, `1 + 2.0`, and `x + 1` (for
`x : @float64`) are type errors -- write `7.0` / `1.0` / `1.0`. This falls out of the
`@`-form change: making real literals `@float64` (which, unlike the old distinct `Real`,
has mixed-width `%`/`+`/... overloads with `@float32`) turned the old "a real anywhere
promotes the int literal" behavior into an ambiguity, and the decision was to forbid the
promotion rather than resurrect it. `propagate_result_to_operands` stays integer-only.
