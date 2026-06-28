# The Thrax native (C) backend

Thrax has two ways to run a program from the same closure-converted IR
(`inc/IR.hpp`):

- the **interpreter** — the reified-K abstract machine `IT` (`src/IT.cpp`),
  reached by `DR::run_program`; and
- the **native backend** `CC` (`src/CC.cpp`), reached by `DR::emit_c` /
  `DR::build_project`, which lowers the IR to a **self-contained** C translation
  unit (the hand-written runtime is baked into the `thrax` binary and emitted
  inline) and compiles it to a native executable.

```
LX → EX → TC → CR → CRxANF → IR.lower ─┬─► IT  (interpret)
                                        └─► CC  (emit C, runtime inlined) ─► cc ─► native binary
```

The runtime sources live in `src/THx*.c` + `inc/THx*.h`; at build time the
makefile's `$(GEN_HDR)` rule amalgamates them (stripping inter-runtime includes)
into the generated header `inc/THxRTxAMALG.hpp` as `CC::RUNTIME_C`, which `CC`
prepends to every emitted program. So a generated `.c` needs no runtime on disk:
`cc prog.c -o prog`.

The IR was designed for this: closure conversion splits every variable into
`Local` (activation slot), `Env` (closure-record field) and `Glob` (top level),
and the program is in A-normal form — so each IR `Code` becomes one C function
and the ANF maps almost 1:1 onto C statements.

## Usage

```sh
thrax --emit-c FILE.thx > prog.c   # emit a self-contained C file (runtime baked in)
cc -O2 prog.c -o prog              # compile -- no extra sources, no -I needed
./prog                             # run
```

Or build a whole project in one step (writes `<dir>/bin/<name>.{ir,c}` + exe):

```sh
thrax --build dat/foo_project && ./dat/foo_project/bin/foo_project
```

`make native-test` emits, compiles and runs every effect-free example in `dat/`
and checks each exits 0 (the same oracle as the interpreter smoke test — see
*Verification*).

## Value representation

A runtime value is a pointer-boxed tagged union, `Value` (`inc/THxVALUE.h`),
mirroring the interpreter's `IT::Value`. Evaluation is strict (no thunks). Tags:
`T_INT`, `T_REAL`, `T_STR` (also Array bytes), `T_STRUCT`, `T_VARIANT`,
`T_CLOS` (an IR code index + captured env), `T_BUILTIN` (a curried operator),
`T_UNK` (placeholder / unit).

**Generated code never reads a union arm directly.** Every access goes through a
*checked accessor* (`THxVALUE_as_int`, `THxVALUE_local`, `THxVALUE_env`, `THxVALUE_field`,
`THxVALUE_variant_field`, …) that asserts the tag and any bounds first, via the
`THxCHECK_ASSERT` / `THxCHECK_FAIL` family (`inc/THxCHECK.h`). A codegen or runtime
bug therefore aborts with a `file:line (fn): message` report instead of
corrupting memory.

## How the lowering works (`src/CC.cpp`)

- **One C function per IR `Code`**: `static Value* THx_code_<i>(Value** env,
  Value* arg)`. Locals live in a zero-initialized `Value* locals[nlocals]`.
- **Atoms** become single C expressions: `Local i`→`THxVALUE_local(...)`, `Env
  i`→`THxVALUE_env(...)`, `Glob n`→`THxRT_glob("n")`, literals→`THxRT_int/THxRT_real/THxRT_str`,
  `MkClosure`→`THxRT_closure(code, captures, n)`.
- **`Ret`/`MkStruct`/`Field`/`MkVariant`/`Unk`** deliver one expression to the
  destination (a `return`, or an assignment to a let/case result variable).
- **`Let`** binds the slot to a placeholder `THxRT_unk()` box, builds the rhs (whose
  closures may capture the box), then copies the value into the box — the same
  back-patch the interpreter does (`IT.cpp` `KRet`), so recursive `let` works.
- **`App`** in tail position emits `return THxRT_tailcall(fn, arg)`; otherwise
  `THxRT_apply(fn, arg)`.
- **`Case`** evaluates the scrutinee once, then an if/else chain on
  `THxVALUE_as_int` / `THxVALUE_as_num` / `strcmp(THxVALUE_ctor(...))`; a `Con` arm binds its
  payload into the local slots via `THxVALUE_variant_field`.
- **Globals** are lazy, memoized CAFs: `THxRT_glob` runs the nullary code on
  first demand and caches it (cyclic value globals abort), exactly as
  `IT::glob`. Unknown names fall through to `THxRT_builtin` (a built-in operator
  key like `+@Int`).
- **Entry**: `main` forces every global (mirroring the smoke test) and then, if
  the program has an entry point, runs it and returns its `Int` as the exit
  code.

### Tail calls

The runtime trampoline (`THxRT_apply` / `THxRT_tailcall` / `THxRT_force_code` in
`src/THxRT.c`) makes all tail calls — including mutual recursion — run in
**constant stack** (verified at 50M iterations). A tail call parks `(fn, arg)`
in bounce registers and returns a sentinel; the enclosing `THxRT_apply` loop picks
it up.

## The "exploreable" / domino-failure discipline

The backend is built so that adding or removing an IR node, a value tag or a
built-in breaks **every** dependent path loudly:

- C++ walkers (`CC.cpp`, the `EnvScan` / `Reject` / `Emitter` visitors) switch
  over `IR::EKind` / `AKind` / `AltKind` with **no `default`**, so a new IR
  variant fails `-Wswitch`/`-Werror` until handled; impossible arms use
  `UT_FAIL_MSG`.
- The C runtime switches over `Tag` with no `default` and ends in a
  `THxCHECK_FAIL`; the built-in *arity table* and *dispatch chain*
  (`src/THxRT.c`) list the same keys, and a mismatch trips a `THxCHECK_FAIL`.

## Supported surface (v1)

Atoms `Local`/`Env`/`Glob`/`LitI`/`LitR`/`LitS`/`MkClosure`; expressions `Ret`,
`Let` (incl. recursive), `App` (with TCO), `Case` (Int/Real/Con + payload bind +
default), `MkStruct`, `Field`, `MkVariant`, `Unk`; lazy-memoized global CAFs;
built-in Int/Real operators and the `%array` primitive; and **C FFI** (`@extern`,
below).

This is the strict subset plus FFI — the only thing the backend rejects today is
algebraic effects, so 17 of the 22 `dat/` examples compile and run (the other 5
use effects).

### Foreign calls (FFI)

`@extern.{ "symbol", "library" }` compiles to a **direct, typed C call** — no
libffi. For each call site `CC::emit_externs` emits a wrapper that resolves the
symbol once with `dlsym` (so there is no clash with the runtime's own
`<stdio.h>`/`<stdlib.h>` declarations, and any library works), calls it through a
function pointer whose type is built from the Thrax signature (`CC::c_type`
mirrors `FF::desc_of`), and marshals arguments/result (`Str`/`Array` pass their
byte pointer, `Ptr` is an integer address, `Real` a double, a `{}` arg is
dropped, a `{}` result yields unit). The wrappers fill the `THxRT_extern_table`
the runtime dispatches `T_EXTERN` through; the value side mirrors builtins
(curried, fires when saturated). A program that uses FFI links with `-ldl`
(`CC::uses_ffi`; `--build` adds it automatically):

```sh
thrax --emit-c FILE.thx > prog.c && cc -O2 prog.c -ldl -o prog
```

## Missing features and where they plug in

`CC::unsupported` rejects a program up front (naming the feature) when it uses
something below, so the user is pointed back to the interpreter.

### Memory: ref counting *(seam in place)*

v1 links `src/THxMEMBUMP.c`: a bump arena that never frees, with
`THxMEM_retain`/`THxMEM_release` as no-ops and `Value::rc` reserved but unused. Finite
programs run fine; long-running loops (e.g. a `while` UI loop) leak.

To add ref counting: provide `src/THxMEMRC.c` implementing `THxMEM_alloc`
(headered block), `THxMEM_retain`/`THxMEM_release` over `Value::rc` (freeing at zero and
recursively releasing children), and link it instead of the bump engine. Then
`CC` must emit `THxMEM_retain`/`THxMEM_release` at the binding/return/overwrite points
(every accessor result that is stored or returned is retained; every local that
goes out of scope is released). Recursive-`let` cycles need the back-patch box
plus a *weak* self-slot, mirroring the interpreter's `VRec` (`ITxDATA.hpp`).
This is why the lowering already routes all allocation through `THxMEM.h`.

### Algebraic effects *(documented; the big one)*

The interpreter implements handlers/`perform`/`resume`/`defer` by capturing and
splicing slices of a **reified continuation stack** (`IT.cpp`: `kont`,
`KPrompt`/`KDefer`/`KAfterClause`, `Resumption`). Compiled code on the native C
stack cannot capture a continuation, so this needs one of:

1. an explicit K-stack runtime in `src/` + `inc/` — a C port of the `IT` machine's `kont`
   and `Resumption` machinery, with `CC` lowering bodies into steppable frames; or
2. CPS-converting the `CC` output (every function takes a continuation; a
   trampoline drives it).

Work items either way: codegen for `Handle`, the effect-operation `Glob`
(`prog.operations`), `Resump`, and `%defer`; the 2-parameter handler-clause
`Code` (`IR::conv_clause`) the backend currently rejects. The bounce-register
trampoline (option 1's substrate) and the `Value`/accessor layer are already in
place. Note: effects also subsume the deep-non-tail-recursion limit below, since
an explicit stack removes the dependence on the C stack.

### Unboxing *(optimization)*

Values are pointer-boxed. A later representation pass could keep `Int`/`Real`
unboxed in registers for true low-level performance.

## Known v1 limitations

- No algebraic effects (clean diagnostic; `defer` counts as an effect, so
  `dat/io_example` is still rejected despite FFI now working).
- FFI marshals the base scalar/pointer types; aggregates (struct/variant) are
  not passed across the boundary, and only libc-resolvable libraries are
  exercised (others just need the right `-l`/path).
- Bump allocator never frees — long-running loops leak.
- Deep **non-tail** recursion uses the C stack and can overflow (tail recursion
  is constant-stack). Lifted by the effects/explicit-stack work.
- Values are pointer-boxed, not register-unboxed.

## Verification

The interpreter smoke test (`tst/TS.cpp`) forces every global, so any runtime
fault in any top-level definition surfaces; several examples self-check with a
`… else 1 / 0` guard. The compiled binary's generated `THx_force_all` mirrors
this exactly, so the equivalence is:

> interpreter smoke test **OK**  ⇔  compiled binary **exits 0**

`make native-test` checks this for every effect-free `dat/*.thx` (and confirms
the effectful ones are cleanly rejected).
