# Thrax Project Architecture

**Status:** design agreed; migration not yet started. Records the target
structure, the reasoning behind it, and the phased plan to get there.
**Date:** 2026-07-10.
**Scope:** physical project layout, module boundaries, the build program, the
dependency model, and third-party vendoring. Not about language semantics.

---

## 0. Goals

The reorganization is driven by concrete needs, not tidiness:

1. **Clear compiler/platform split** platforms must be developable
   independently, for simplicity, stability, and robustness.
2. **Vendoring of core 3rd-party libs** (currently only libffi) as **git
   subtrees**, while **nix** provides *developer* tooling (clang, bear, tokei)
   and libs for *showcase* mini-projects (raylib).
3. **A better, more advanced testing strategy** e.g. eventually differential
   testing of the parser against a reference (bison) grammar.
4. **Replace make with a real build program** (`build.cpp`) make is already
   awkward and the planned work (per-module unity, test harnesses, mini-project
   compilation, env injection) needs an actual programming language.
5. **More mini-project examples**, with a better way to compile and test them.

Because the project will grow, modules are **isolated** and each gets its own
**amalgamation (unity) build** extending the conventions already in use (the
`UT`/`AR`/`LX`... prefix scheme, the amalgamated unity TU, the PCH header, the
baked runtime).

### Guiding principle: nix is an accelerator, never a substrate

The project **always builds on any platform**. nix only makes bootstrapping and
optional tools/libs *easy*. This is a hard rule, and it is enforced by a single
mechanism: **every value nix injects must have a non-nix fallback.** If
`build.cpp` ever hard-requires a `/nix/store` path or a nix-only env var, nix has
silently become mandatory. Windows (no native nix) is the forcing function that
keeps this honest.

---

## 1. Target directory layout

```
Thrax/
+-- build.cpp            # the build program (replaces makefile)
+-- flake.nix            # nix devShell: tooling + optional lib paths + bootstrap
|
+-- utilities/           # reusable infrastructure, depends on nothing
+-- compiler/            # source -> IR, depends only on utilities
+-- engines/             # consume IR: interpreter, C backend, FFI seam
+-- platforms/           # runtime + OS services
+-- external/            # vendored 3rd-party (subtrees): libffi
+-- app/                 # the `thrax` CLI (links compiler + engines)
+-- tools/               # future: lsp, formatter, syntax, package-manager
+-- tests/               # unit + integration (was tst/)
+-- examples/            # mini-projects / showcases (was dat/)
\-- doc/
```

---

## 2. Module map (current file -> target module)

The two-letter prefixes already act as modules; the reorg gives them real
directories and per-module builds.

| Target        | Files (current names)                                             | Role |
|---------------|------------------------------------------------------------------|------|
| `utilities/`  | `UT`, `AR`                                                        | core utils, arena |
| `compiler/`   | `LX`+`LXxDATA`, `EX`+`EXxDATA`, `ER`, `OP`, `TC`+`TCxDATA`, `CR`+`CRxANF`, `LL`, `IR`, `MR` | lexer, parser, diagnostics, operator vocab, typecheck, Core+ANF, lowering, IR, module-resolution |
| `engines/`    | `IT`+`ITxDATA`, `CC`, `FF`                                        | interpreter (reified-K), C backend, FFI |
| `platforms/`  | `THxCHECK`, `THxVALUE`, `THxMEM`, `THxMEMBUMP`, `THxRT`, `THxK`   | the C runtime |
| `external/`   | `libffi`                                                          | vendored subtree (pruned fork) |
| `app/`        | `main.cpp` + the run/emit half of `DR`                           | the `thrax` CLI |
| `tests/`      | `tst/TS`, `tst/tst_all`                                           | test harness |
| `examples/`   | `dat/foo_project`, `dat/io_example`, `dat/raylib_demo`           | showcase `.thx` projects |

### Notes / decisions embedded in the map

- **`DR` (Driver) is split.** It currently orchestrates the *whole* pipeline
  including running engines (`DR::run_program`, `DR::emit_c`,
  `DR::build_project`), so it cannot live in `compiler/` the compiler must not
  know about engines. The source->IR orchestration stays compiler-internal; the
  IR->run/emit orchestration moves to `app/`. **This boundary is the one genuine
  design call in the migration; the exact seam is still to be drawn from
  `DR.cpp`.**
- **`FF` is the sole external+platform seam** "the only module that touches
  libffi/dlopen." Containing the libffi dependency to one file is deliberate and
  makes the layered dependency resolution (section 5) local.
- **`CC` embeds the platform runtime as text.** The C backend bakes the
  `THx*` runtime into itself as a `RUNTIME_C` string so emitted programs are
  self-contained. This is an *engines->platforms coupling as build-time data*,
  not a link edge, and any reorg must preserve the generated-header seam.
- **`examples/` are runtime inputs, not a code module** `.thx` programs used
  as regression fixtures. They do not participate in the C++ dependency graph.

---

## 3. Dependency rules

```
                 utilities
                /    |    \
        compiler   platforms  external
             \        |        /
              \       |       /
               \      v      /
                  engines
                     |
                    app  (and tools)

tests   depend on everything
examples depend on nothing (they are inputs, not code)
```

- `utilities` depends on nothing.
- `compiler` depends only on `utilities`.
- `platforms` depends only on `utilities`.
- `engines` may depend on `compiler`, `platforms`, `utilities`, `external`.
- `app`/`tools` may depend on all project modules.
- `tests` may depend on everything.
- No module depends on `examples`.

**Enforcement is a mechanism, not a convention.** The build program (section 4) holds
the DAG as data and **refuses to build on a cycle or an illegal edge**. This
replaces today's situation where the amalgamated header + PCH expose every
interface to every TU and only discipline keeps the boundaries.

---

## 4. Build program (`build.cpp`)

Replaces the makefile. Style: **nob-like** a single self-bootstrapping,
self-rebuilding C++ TU, *not* a framework.

- **Model as data.** Modules, their file lists, the dependency DAG, and the
  per-module unity/PCH config live as data. Building is one *backend* over that
  model; generating artifacts is another.
- **Backends over one model:** *execute* (compile the primary path);
  *generate* `compile_commands.json` (clangd, near-term); later and only when
  needed, generate MSVC/CMake projects.
- **Per-module unity + PCH.** Each module compiles as one amalgamation TU with
  its own PCH. Incremental strategy is **coarse**: hash a module's inputs,
  rebuild that module's TU on change. Per-module granularity makes coarse
  acceptable and avoids reimplementing depfile bookkeeping.
- **Preserves the makefile's good parts:** runtime amalgamation -> baked
  `RUNTIME_C` header (now an explicit build step, not a `grep` incantation),
  PCH ordering, `test` / `native-test` targets.
- **Bootstrap seam is explicit.** Tier-0 (no nix): `c++ -std=c++20 build.cpp -o
  build && ./build`, documented at the top of the README. nix runs the same
  line in its `shellHook`. A future `build.bat` mirrors it for Windows.
- **Env ownership: nix owns environment, `build.cpp` owns compilation.** They do
  not both mutate shell state. For non-nix users, `build env` *prints* exports
  to `eval`; it never reaches into the live shell.

---

## 5. Third-party libs vendoring policy

**Vendor what the compiler's *output* depends on; let nix provide what the
*developer* depends on.**

- **libffi** and future *core* 3rd-party libs -> **git subtree** of a **personal
  fork**, added with `--squash` (keeps upstream history out of our log; the
  files live in-tree so `clone` just builds no submodule init footgun).
  - The fork is **pruned** (testsuite/docs/unused arches removed) and ships
    **pre-generated `fficonfig.h`/`ffitarget.h`**, so it builds without
    autotools.
- **Layered resolution** in `FF`'s build (nix users never touch the vendored
  copy): `$LIBFFI` (nix) -> `pkg-config` -> `external/libffi`. On failure, a
  clear, actionable message distinguishing a non-git tarball from an offline
  clone.
- **Developer tooling and showcase libs** (clang, bear, tokei; raylib for the
  demo) -> **nix devShell**, never vendored. The raylib *demo* may be a subtree;
  raylib the *library* is nix-provided.

---

## 6. Windows story

nix is Linux/macOS only, so Windows relies entirely on the always-builds Tier-0
path plus generated project files.

- **Preferred:** generate MSVC (`.sln`/`.vcxproj`) and/or CMake project files
  *from the build model, on Linux/nix*. The Windows developer opens the solution
  and builds with their own toolchain **no need for `build.cpp` to run
  natively on Windows**, no win32 process-spawn shim to maintain.
- Vendored libffi (pruned fork with committed headers) is the Windows lifeline 
  there is no reliable `pkg-config`/system libffi there.
- Native-on-Windows `build.cpp` (with a win32 shim) remains a *fallback option*,
  not the plan.

---

## 7. Testing strategy (direction)

- Keep unit + integration tests (`tests/`), and the example programs as
  regression fixtures (`examples/` with `expected_output.txt`).
- **Future:** differential testing of the parser against a **bison** reference
  grammar (acceptance), paired with **round-trip / property tests**
  (`parse -> print -> parse` fixed point) for AST *shape*, and fuzzing for
  crashes. The bison grammar is a second source of truth and must be kept in
  sync hence it is one tool among several, not the only one.

---

## 8. Migration plan

Guiding rule: **one variable per phase, green at every step, `make` stays alive
until `build.cpp` reaches parity.** Never move files *and* change the build in
the same phase.

- **Phase 0 Groundwork.** Branch; write this doc; decide the `DR` split
  (compiler-driver vs app-driver boundary) from `DR.cpp`.
- **Phase 1 `build.cpp` at parity, on the current flat layout.** Reproduce
  today's build exactly (single amalgamation, PCH, baked runtime, tests). No
  file moves. A/B against make; `native-test` still 22/22. Keep `makefile` as
  fallback.
- **Phase 2 libffi as subtree.** Create/prune fork, commit generated headers,
  `git subtree add --prefix=external/libffi <fork> <ref> --squash`. Add layered
  resolution to `FF`'s build. Validate the vendored path with **nix off**.
- **Phase 3 Physical reorg, still single amalgamation.** Move files into the
  target dirs; execute the `DR` split; fix include paths. Only variable is
  location. Validate identical binaries + tests.
- **Phase 4 Per-module unity + DAG enforcement.** Split into per-module
  `unity.cpp` + per-module PCH; encode the DAG as data and reject illegal edges;
  coarse per-module incremental. Validate boundary check catches a planted bad
  include; **measure** the incremental-build win.
- **Phase 5 nix bootstrap, env, tooling, retire make.** Extend `shellHook` to
  bootstrap `build` and inject `THRAX_ROOT`/`PATH` (nix stays optional). Emit
  `compile_commands.json`. Add `build env`. Delete the makefile once parity
  holds.
- **Phase 6 examples/tests polish.** `dat/` -> `examples/` with
  `expected_output.txt`; wire the runner into `build.cpp`.

### Explicitly out of scope (door left open, not built)

Bison differential harness, MSVC/CMake generators, native-Windows `build.cpp`.
The Phase 4 build-model-as-data makes each a *localized addition later*, so
there is no cost to deferring and real cost to building them speculatively.
