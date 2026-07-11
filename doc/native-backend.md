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
and the program is in A-normal form — so an IR `Code` maps almost 1:1 onto C
statements. Execution is driven by a **reified-K (CEK) machine in the runtime**
(`src/THxK.c`), a C port of the interpreter's abstract machine: the continuation
is an explicit heap stack, so algebraic-effect handlers can capture and splice
the delimited continuation between a prompt and a `perform` — something compiled
code on the native C stack cannot do. See *Algebraic effects* below.

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

`build native-test` emits, compiles and runs every example in `examples/` and
checks each exits 0 (the same oracle as the interpreter smoke test — see
*Verification* — and, since the generated main leak-checks on exit, also a leak
regression). All 24 examples — including the ones that use algebraic effects —
compile and run natively.

## Value representation

A runtime value is a pointer-boxed tagged union, `Value` (`inc/THxVALUE.h`),
mirroring the interpreter's `IT::Value`. Evaluation is strict (no thunks). Tags:
`T_INT`, `T_REAL`, `T_STR` (also Array bytes), `T_STRUCT`, `T_VARIANT`,
`T_CLOS` (an IR code index + captured env), `T_BUILTIN` (a curried operator),
`T_EXTERN` (a curried foreign call), `T_OP` (an effect operation),
`T_RESUMP` (a captured continuation / resumption), `T_DEFER` (the `defer`
intrinsic, curried over its two thunks), `T_UNK` (placeholder / unit).

**Generated code never reads a union arm directly.** Every access goes through a
*checked accessor* (`THxVALUE_as_int`, `THxVALUE_local`, `THxVALUE_env`, `THxVALUE_field`,
`THxVALUE_variant_field`, …) that asserts the tag and any bounds first, via the
`THxCHECK_ASSERT` / `THxCHECK_FAIL` family (`inc/THxCHECK.h`). A codegen or runtime
bug therefore aborts with a `file:line (fn): message` report instead of
corrupting memory.

## How the lowering works (`src/CC.cpp`)

The IR is lowered to **block functions** driven by the CEK machine. A block runs
straight-line C and ends by calling exactly one **terminator**; the driver
(`src/THxK.c`) acts on it. An activation's locals/env live in a heap `Frame`
(not C autos), so a block can be re-entered after a suspension.

- **Block**: `static void blk_<n>(Frame* fr, Value* in)`. A `Code`'s entry block
  is its first; **suspension points spawn continuation blocks** (see `Let`).
- **Atoms** become single C expressions: `Local i`→`THxVALUE_local(fr->locals,
  …)`, `Env i`→`THxVALUE_env(fr->env, …)`, `Glob n`→`THxRT_glob("n")` (or
  `THxK_op("Eff.op")` for an effect operation, `THxK_defer()` for `defer`),
  literals→`THxRT_int/…`, `MkClosure`→`THxRT_closure(code, captures, n)`.
- **Terminators**: `THxK_ret(v)` delivers a value up the continuation;
  `THxK_tailcall(fn,arg)` tail-applies (constant stack); `THxK_apply(fr,fn,arg,
  cont,slot)` non-tail-applies, pushing a return continuation to block `cont`;
  `THxK_jump(cont)` transfers locally (a case join); `THxK_handle(…)` installs a
  handler.
- **`Ret`/`MkStruct`/`Field`/`MkVariant`/`Unk`/`Extern`** compute one value
  expression and `deliver` it to the block's sink (a `THxK_ret`, or a slot
  back-patch + `THxK_jump`).
- **`Let`** places a placeholder box in the slot (`THxK_setbox`) so recursive
  closures can capture it, then: if the rhs **suspends** (a non-tail `App` or a
  `Handle`), the body becomes a fresh continuation block and the rhs delivers
  into the slot (the driver back-patches the box on return, exactly as `IT.cpp`
  `KRet`); otherwise the pure rhs is computed into the slot and the body is
  emitted **inline** in the same block (the pure path stays flat).
- **`App`** tail → `THxK_tailcall`; non-tail (always a let rhs, by ANF) →
  `THxK_apply` with the let body as its continuation block.
- **`Case`** evaluates the scrutinee once, then an if/else chain on
  `THxVALUE_as_int` / `THxVALUE_as_num` / `strcmp(THxVALUE_ctor(...))`; a `Con`
  arm binds its payload into the frame slots via `THxVALUE_variant_field`; each
  branch delivers to the same sink.
- **Globals** are lazy, memoized CAFs: `THxRT_glob` runs the nullary code
  (`THxK_run_code`) on first demand and caches it (cyclic value globals abort),
  exactly as `IT::glob`. Unknown names fall through to `THxRT_builtin`.
- **Entry**: `main` forces every global (mirroring the smoke test) and then, if
  the program has an entry point, applies it (`THxK_call`) and returns its `Int`
  as the exit code.

### Tail calls and deep recursion

The driver applies closures by allocating a heap `Frame` and jumping to the
callee's entry block, never recursing on the C stack. A `THxK_tailcall` reuses no
KRet, so tail calls — including mutual recursion — run in **constant stack**; a
non-tail `THxK_apply` pushes one `KRet` on the heap continuation stack, so **deep
non-tail recursion grows the heap, not the C stack** (this lifts the old v1
stack-overflow limit).

## The "exploreable" / domino-failure discipline

The backend is built so that adding or removing an IR node, a value tag or a
built-in breaks **every** dependent path loudly:

- The `CC.cpp` `Emitter` visitors (`atom` / `value_expr` / `has_suspension` /
  `emit_expr` / `has_extern`) switch over `IR::EKind` / `AKind` / `AltKind` with
  **no `default`**, so a new IR variant fails `-Wswitch`/`-Werror` until handled;
  impossible arms use `UT_FAIL_MSG`.
- The C runtime switches over `Tag` (`THxK.c` `do_apply`, `THxVALUE_tag_name`)
  and over the continuation-frame kind with no `default` and ends in a
  `THxCHECK_FAIL`; the built-in *arity table* and *dispatch chain*
  (`src/THxRT.c`) list the same keys, and a mismatch trips a `THxCHECK_FAIL`.

## Supported surface (v1)

Atoms `Local`/`Env`/`Glob`/`LitI`/`LitR`/`LitS`/`MkClosure`; expressions `Ret`,
`Let` (incl. recursive), `App` (with TCO), `Case` (Int/Real/Con + payload bind +
default), `MkStruct`, `Field`, `MkVariant`, `Unk`; lazy-memoized global CAFs;
built-in Int/Real operators and the `%array` primitive; **C FFI** (`@extern`,
below); and **algebraic effects** — handlers (`do`/`ctl`), operations,
first-class resumptions, and `defer` (see below).

The backend now lowers the whole IR: **all 24 `examples/` programs compile and
run natively**, matching the interpreter. `CC::unsupported` no longer rejects
anything (it is retained as a fallback seam).

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

## Algebraic effects (the CEK driver)

Handlers/`perform`/`resume`/`defer` work by capturing and splicing slices of a
**reified continuation stack** — the same design as the interpreter (`IT.cpp`),
ported to C in `src/THxK.c` (option 1 of the two routes originally sketched
here). The driver owns a single heap continuation stack `kont` of `KFrame`s
(`K_RET` / `K_PROMPT` / `K_DEFER` / `K_THUNKRET` / `K_AFTERCLAUSE`) and drives
the generated block functions:

- **`do body ctl k …`** lowers to `THxK_handle`, which pushes a `K_PROMPT`
  holding the (evaluated) clause closures and the value clause `els`, then runs
  the body block under it. A handler clause is the 2-parameter `Code`
  (`IR::conv_clause`): op argument in slot 0, resumption `k` in slot 1.
- **Performing** an operation (`do_apply` on a `T_OP`) searches down for the
  nearest prompt with a clause, captures the `kont` slice from that prompt to
  here (**including** the prompt → deep handler) into a `THxK_Resump`, truncates
  `kont`, pushes a `K_AFTERCLAUSE`, and runs the clause below the prompt.
- **Resuming** (`do_apply` on a `T_RESUMP`) splices the captured slice back and
  delivers the value. Resumptions are **affine**: a second use aborts
  (`seg->used`).
- **`defer cleanup do body`** lowers to the `%finally` intrinsic (`T_DEFER`,
  curried over two thunks); it installs a `K_DEFER` marker that runs the cleanup
  on normal completion, on abort (the captured continuation is discarded — the
  `K_AFTERCLAUSE` runs the pending cleanups with enclosing handlers still
  installed), or when a stored continuation later completes.

The interpreter distinguishes a **stored** resumption (a coroutine that stashed
`k`) from a **discarded** one via a `shared_ptr` use-count; the C runtime has no
refcount, so instead `THxK_mark_escape` flags a `THxK_Resump` when it is copied
into a closure capture or a struct/variant field (the value constructors in
`src/THxRT.c` call it). A stored/escaped `k`'s `defer` cleanups are not finalized
at the clause boundary — they run on later completion. (Limitation, inherited
from the interpreter: a stored continuation dropped without ever resuming never
runs its `defer`.)

## Memory: reference counting (the default engine)

The runtime is memory-managed by **precise reference counting**
(`platforms/THxMEMRC.c`, selected by default; compile the emitted C with
`-DTHX_MEM_BUMP` to fall back to the old never-freeing bump arena — a memory
bug that disappears under bump is an RC-discipline bug). Strict + pure data is
acyclic by construction, so plain RC is complete for data; no cycle collector.

The discipline lives in the **runtime seam, not in emitted retain/release
pairs** — generated code was already forbidden from touching representation
(every construction/read/store goes through `THxRT_*`/`THxVALUE_*`/`THxK_*`),
so those functions carry the ownership rules:

- **Stores own**: a frame slot (`THxK_setlocal`), a constructor child
  (struct/variant fields, closure captures, curried builtin/extern/defer
  operand rows), every kont frame's payload (KRet's frame, KPrompt's
  clauses + `els`, KDefer's cleanup, KThunkRet's saved value, KAfterClause's
  kval), and the global CAF cache each hold `+1`; reads borrow.
- **Frames are refcounted** (driver's current activation + every KRet, live or
  captured) and retain the closure whose `env` they borrow.
- **The temp pool**: every fresh value is registered with the pool owning its
  initial reference; the driver drains it after each block bounce. Whatever a
  bounce stored survives via its own retain; the rest — the per-iteration
  garbage of a long-running loop — dies immediately. (Measured on
  `examples/RC_LOOP.thx`, 200k iterations of struct allocation: **1.3 MB peak
  RSS under RC vs 277 MB under bump**.)
- **Box back-patch** (`THxVALUE_patch_box`): the KRet delivery content-copies
  into the let box, deep-copying payload arrays (a shared array would be freed
  twice) and retaining children — *except a child pointing back at the box
  itself*, the **weak self edge** of a recursive-`let` closure that captured
  its own box. That is the one genuine RC cycle (the interpreter's `VRec`
  analog) and both `patch_box` and `THxVALUE_destroy` skip it symmetrically.
- **Continuation capture moves**: a performed effect moves the kont slice into
  the resumption segment (no count changes); resuming moves it back; a
  discarded, never-resumed segment releases its contents — so aborted
  computations (exceptions) free their captured frames. Segments carry a small
  count of their own since `patch_box` can alias a resumption value.
- **Release is iterative** (a dead-value worklist in `THxMEMRC.c`), so dropping
  a long list does not recurse on the C stack.

**The built-in leak check:** the generated `main` releases the CAF cache and
the temp pool on exit and then asserts `THxMEM_live() == 0`, exiting **97**
(with a stderr report) if any allocation survived. Every `build native-test`
example is thereby also a leak regression test. Under `-DTHX_MEM_BUMP`,
`THxMEM_live()` is 0 and the check passes trivially.

## Missing features and where they plug in

### Unboxing *(optimization)*

Values are pointer-boxed. A later representation pass could keep `Int`/`Real`
unboxed in registers for true low-level performance.

## Known limitations

- Resumptions are **affine** (resume at most once); a second use aborts at
  runtime. A stored continuation dropped without ever resuming never runs its
  `defer` (inherited from the interpreter) — though its memory *is* reclaimed.
- FFI marshals the base scalar/pointer types; aggregates (struct/variant) are
  not passed across the boundary, and only libc-resolvable libraries are
  exercised (others just need the right `-l`/path).
- RC handles the direct self-cycle of a recursive-`let` closure (the weak self
  edge). An *indirect* local value cycle (a recursive `let` whose box is
  reachable only through other fresh nodes, e.g. `let rec x = Cons 1 (Cons 2
  x)`) would leak — the same class the interpreter's weak-`VRec` assumption
  covers; such bindings do not occur in practice (recursive lets bind
  functions).
- Values are pointer-boxed, not register-unboxed; RC is runtime-seam-based, not
  a Perceus-style compile-time dup/drop pass (no in-place reuse optimization).

## Verification

The interpreter smoke test (`tst/TS.cpp`) forces every global, so any runtime
fault in any top-level definition surfaces; several examples self-check with a
`… else 1 / 0` guard. The compiled binary's generated `THx_force_all` mirrors
this exactly, so the equivalence is:

> interpreter smoke test **OK**  ⇔  compiled binary **exits 0**

`build native-test` checks this for every example (all 24, effects included),
and the generated main's leak check makes each one a leak regression test too;
`build test` runs the interpreter side. The effect examples (EFFECTS, FINALLY,
COROUTINES, PIPES, EFFECT_OVERLOAD) are validated to produce the **same values**
natively as under the interpreter (exceptions 42/-1, generator 42, state 21,
`defer` 103/9/6/2), and the emitted binaries are valgrind-clean of invalid
reads/writes.
