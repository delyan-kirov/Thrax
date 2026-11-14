# The Ares native (C) backend

Ares has two ways to run a program from the same closure-converted IR
(`inc/IR.hpp`):

- the **interpreter** the reified-K abstract machine `IT` (`src/IT.cpp`),
  reached by `DR::run_program`; and
- the **native backend** `CC` (`src/CC.cpp`), reached by `DR::emit_c` /
  `DR::build_project`, which lowers the IR to a **self-contained** C translation
  unit (the hand-written runtime is baked into the `ares` binary and emitted
  inline) and compiles it to a native executable.

```
LX -> EX -> TC -> CR -> CRxANF -> IR.lower -+-> IT  (interpret)
                                        \-> CC  (emit C, runtime inlined) -> cc -> native binary
```

The runtime sources live in `src/ARESx*.c` + `inc/ARESx*.h`; at build time the
makefile's `$(GEN_HDR)` rule amalgamates them (stripping inter-runtime includes)
into the generated header `inc/ARESxRTxAMALG.hpp` as `CC::RUNTIME_C`, which `CC`
prepends to every emitted program. So a generated `.c` needs no runtime on disk:
`cc prog.c -o prog`.

The IR was designed for this: closure conversion splits every variable into
`Local` (activation slot), `Env` (closure-record field) and `Glob` (top level),
and the program is in A-normal form so an IR `Code` maps almost 1:1 onto C
statements. Execution is driven by a **reified-K (CEK) machine in the runtime**
(`src/ARESxK.c`), a C port of the interpreter's abstract machine: the continuation
is an explicit heap stack, so algebraic-effect handlers can capture and splice
the delimited continuation between a prompt and a `perform` something compiled
code on the native C stack cannot do. See *Algebraic effects* below.

## Usage

```sh
ares --emit-c FILE.ares > prog.c   # emit a self-contained C file (runtime baked in)
cc -O2 prog.c -o prog              # compile -- no extra sources, no -I needed
./prog                             # run
```

Or build a whole project in one step (writes `<dir>/bin/<name>.{ir,c}` + exe):

```sh
ares --build dat/foo_project && ./dat/foo_project/bin/foo_project
```

`build native-test` emits, compiles and runs every example in `examples/` and
checks each exits 0 (the same oracle as the interpreter smoke test see
*Verification* and, since the generated main leak-checks on exit, also a leak
regression). All 24 examples including the ones that use algebraic effects 
compile and run natively.

## Value representation

A runtime value is a pointer-boxed tagged union, `Value` (`inc/ARESxVALUE.h`),
mirroring the interpreter's `IT::Value`. Evaluation is strict (no thunks). Tags:
`T_INT`, `T_REAL`, `T_STR` (also Array bytes), `T_STRUCT`, `T_VARIANT`,
`T_CLOS` (an IR code index + captured env), `T_BUILTIN` (a curried operator),
`T_EXTERN` (a curried foreign call), `T_OP` (an effect operation),
`T_RESUMP` (a captured continuation / resumption), `T_DEFER` (the `@finale`
intrinsic, curried over its two thunks), `T_UNK` (placeholder / unit).

**Generated code never reads a union arm directly.** Every access goes through a
*checked accessor* (`ARESxVALUE_as_int`, `ARESxVALUE_local`, `ARESxVALUE_env`, `ARESxVALUE_field`,
`ARESxVALUE_variant_field`, ...) that asserts the tag and any bounds first, via the
`ARESxCHECK_ASSERT` / `ARESxCHECK_FAIL` family (`inc/ARESxCHECK.h`). A codegen or runtime
bug therefore aborts with a `file:line (fn): message` report instead of
corrupting memory.

## How the lowering works (`src/CC.cpp`)

The IR is lowered to **block functions** driven by the CEK machine. A block runs
straight-line C and ends by calling exactly one **terminator**; the driver
(`src/ARESxK.c`) acts on it. An activation's locals/env live in a heap `Frame`
(not C autos), so a block can be re-entered after a suspension.

- **Block**: `static void blk_<n>(Frame* fr, Value* in)`. A `Code`'s entry block
  is its first; **suspension points spawn continuation blocks** (see `Let`).
- **Atoms** become single C expressions: `Local i`->`ARESxVALUE_local(fr->locals,
  ...)`, `Env i`->`ARESxVALUE_env(fr->env, ...)`, `Glob n`->`force_code(<code>)` (or
  `ARESxRT_builtin(ARESxB_<NAME>, arity)` for a built-in, `ARESxK_op("Eff.op")` for an
  effect operation, `ARESxK_defer()` for `@finale`),
  literals->`ARESxRT_int/...`, `MkClosure`->`ARESxRT_closure(code, captures, n)`.
- **Terminators**: `ARESxK_ret(v)` delivers a value up the continuation;
  `ARESxK_tailcall(fn,arg)` tail-applies (constant stack); `ARESxK_apply(fr,fn,arg,
  cont,slot)` non-tail-applies, pushing a return continuation to block `cont`;
  `ARESxK_jump(cont)` transfers locally (a case join); `ARESxK_handle(...)` installs a
  handler.
- **`Ret`/`MkStruct`/`Field`/`MkVariant`/`Unk`/`Extern`** compute one value
  expression and `deliver` it to the block's sink (a `ARESxK_ret`, or a slot
  back-patch + `ARESxK_jump`).
- **`Let`** empties its slot (`ARESxK_drop`); a recursive `let` (its rhs mentions
  the slot) instead places a placeholder box there (`ARESxK_setbox`) so recursive
  closures can capture it. Then: if the rhs **suspends** (a non-tail `App` or a
  `Handle`), the body becomes a fresh continuation block and the rhs delivers
  into the slot on return (an empty slot adopts the value, a box is
  back-patched); otherwise the pure rhs is computed into the slot and the body
  is emitted **inline** in the same block (the pure path stays flat).
- **`App`** tail -> `ARESxK_tailcall`; non-tail (always a let rhs, by ANF) ->
  `ARESxK_apply` with the let body as its continuation block.
- **`Case`** evaluates the scrutinee once, then an if/else chain on
  `ARESxVALUE_as_int` / `ARESxVALUE_as_num` / `ARESxVALUE_ctor_is(...)`; a `Con`
  arm binds its payload into the frame slots via `ARESxVALUE_variant_field`; each
  branch delivers to the same sink.
- **Globals** are lazy, memoized CAFs: `force_code` runs the nullary code
  (`ARESxK_run_code`) on first demand and caches it (cyclic value globals abort),
  exactly as `IT::glob`. The generator resolves every global name to its code
  index, so nothing is looked up by name at run time.
- **Built-ins** are numbered by `frontend::ir::data::BUILTINS`: the generator
  emits that table as the `ARESxB_*` enum ahead of the runtime, and a saturated
  call is `ARESxK_prim(ARESxB_<NAME>, args)`, which the runtime dispatches with a
  `switch`. `@vec_push` and `@vec_set` go through `ARESxK_vec_update`, which also
  says whether the vector operand was moved (a last use): a moved vector whose
  count is 1 is updated in place, its item block growing geometrically, and any
  other is copied once. A partial application of either updates in place only
  when the applying call owns it alone (`let f = @vec_push v in (f 1, f 2)`
  copies). `ARESxMEM_unique` reads a shared value's atomic count, so a vector
  another thread can reach is never mutated under it.
- **Entry**: the generated C `main` forces every global reachable from the entry,
  then applies `@main` (`ARESxK_call`) to the argument vector as a `@Vec @Str` and
  returns its `@Int` as the exit code. A harness mode (`ccg::Entry::Show`) instead
  forces a named global and prints `name = value`, which is how the differential
  tests compare the two engines.

### Tail calls and deep recursion

The driver applies closures by allocating a heap `Frame` and jumping to the
callee's entry block, never recursing on the C stack. A `ARESxK_tailcall` reuses no
KRet, so tail calls including mutual recursion run in **constant stack**; a
non-tail `ARESxK_apply` pushes one `KRet` on the heap continuation stack, so **deep
non-tail recursion grows the heap, not the C stack** (this lifts the old v1
stack-overflow limit).

### Direct functions and unboxed integers

Most calls cannot perform an effect, and those need none of the machine. A
known global function (a closure literal, seen through its currying) whose body
provably never suspends (no handler, no `@finale`, every call a built-in, a
partial application, or another such function) also gets a plain C function
`d_<code>`: its activation is a `Frame` on the C stack, a saturated call of it
from a block or another direct function is a C call, and a self tail call is a
`goto`. The analysis is a greatest fixpoint over the IR (`direct_safe` in
`gen.rs`), so mutual recursion qualifies.

The C stack is finite, so a direct call is made only while `ARESxD_room()` says
the thread is within `ARX_DIRECT_STACK` bytes (portable.h) of where it started;
past that a direct function runs its callee on a nested machine and blocks take
the ordinary heap path, so deep recursion still works.

Integer intrinsics (`@iadd`, `@ilt`, ...) compile to C arithmetic on
`int64_t`; a result used only as another intrinsic's operand or a literal
case's scrutinee stays a C scalar. A direct function's arguments and result
cross as `int64_t` when its body shows them to be integers. See
Note [Direct functions] and Note [Unboxed integers] in `crates/ccg/src/gen.rs`.

Two middle-end changes feed this: match compilation inlines a fallthrough
thunk that has a single call site (an `if` no longer allocates a closure), and
`sink_partials` also moves partial applications of curried global functions
down to the application that saturates them.

## The "exploreable" / domino-failure discipline

The backend is built so that adding or removing an IR node, a value tag or a
built-in breaks **every** dependent path loudly:

- The `CC.cpp` `Emitter` visitors (`atom` / `value_expr` / `has_suspension` /
  `emit_expr` / `has_extern`) switch over `IR::EKind` / `AKind` / `AltKind` with
  **no `default`**, so a new IR variant fails `-Wswitch`/`-Werror` until handled;
  impossible arms use `UT_FAIL_MSG`.
- The C runtime switches over `Tag` (`ARESxK.c` `do_apply`, `ARESxVALUE_tag_name`)
  and over the continuation-frame kind with no `default` and ends in a
  `ARESxCHECK_FAIL`; the built-in *arity table* and *dispatch chain*
  (`src/ARESxRT.c`) list the same keys, and a mismatch trips a `ARESxCHECK_FAIL`.

## Supported surface (v1)

Atoms `Local`/`Env`/`Glob`/`LitI`/`LitR`/`LitS`/`MkClosure`; expressions `Ret`,
`Let` (incl. recursive), `App` (with TCO), `Case` (@Int/Real/Con + payload bind +
default), `MkStruct`, `Field`, `MkVariant`, `Unk`; lazy-memoized global CAFs;
built-in @Int/Real operators and the `%array` primitive; **C FFI** (`@extern`,
below); and **algebraic effects** handlers (`do`/`ctl`), operations,
first-class resumptions, and `@finale` (see below).

The backend now lowers the whole IR: **all 24 `examples/` programs compile and
run natively**, matching the interpreter. `CC::unsupported` no longer rejects
anything (it is retained as a fallback seam).

### Foreign calls (FFI)

`@extern "C" "symbol" "lib"` compiles to a **direct, typed C call** -- no
libffi, and no dlopen either: generated programs let the SYSTEM LINKER
resolve every foreign symbol (statically or dynamically, per what it finds).
For each call site `CC::emit_externs` declares the symbol via an asm label
(`extern int64_t ARESx_sym_0(char*) __asm__("puts");` -- the C identifier is
ours, so there is no clash with the runtime's own `<stdio.h>`/`<stdlib.h>`
declarations; the assembler symbol is the real one), with a prototype built
from the Ares signature (`CC::c_type` mirrors `FF::desc_of`), and marshals
arguments/result (`Str`/`Array` pass their byte pointer, `Ptr` is an integer
address, `Real` a double, a `{}` arg is dropped, a `{}` result yields unit).
The wrappers fill the `ARESxRT_extern_table` the runtime dispatches `T_EXTERN`
through; the value side mirrors builtins (curried, fires when saturated).
`@extern` names libraries SYMBOLICALLY ("libc", "libm", "raylib"); the link
line carries one flag per library (`CC::link_flags` -- libc implicit,
"libm" -> `-lm`, "raylib" -> `-lraylib`, an explicit path verbatim;
`--build` adds them automatically, and the generated file's header comment
prints the exact cc line):

```sh
ares --emit-c FILE.ares > prog.c && cc -O2 prog.c -lm -o prog
```

The interpreter keeps dlopen (libffi + the host soname table in
`TG::Target::soname`); dlopen-vs-link is exactly the interpreter/native
split, which is what a no-dlopen target (wasm) needs.

#### The `lib` field: where a symbol comes from

The third string in `@extern "C" "symbol" "lib"` names, symbolically, the
library that provides the symbol. Each engine consults it differently, and one
value (`""`) means "the engine itself provides this symbol; there is no external
library to link or dlopen". (Names below use the current Rust code; the rest of
this section predates the rewrite and still cites the C++ names.)

| `lib` | native (`ares build`) | interpreter (`ares run`) |
| --- | --- | --- |
| `""` (self) | no flag; the symbol is defined in the program's own translation unit (see intrinsics below) | `dlsym(RTLD_DEFAULT)` against the already-loaded process (which includes the `aresstd` support functions linked into the interpreter) |
| `"libc"` / `"libm"` | implicit / `-lm` | already-loaded set (a null dlopen handle, `RTLD_DEFAULT`) |
| `"raylib"`, `"foo"` | `-lraylib`, `-lfoo` (a leading `lib` is stripped) | `dlopen` the host soname (`libraylib.so`, ...) |
| a path or soname (`/x/lib.a`, `bin/libraylib.so`) | passed to the linker verbatim | `dlopen`ed by that path |

`link_flag` (in `utilities::target`) turns the name into a link flag; `""`,
`libc`, and `c` produce none. The interpreter's `resolve` (in `machine::ffi`)
treats `""`, `libc`, `c`, `libm`, `m` as the already-loaded set (a null handle,
`RTLD_DEFAULT`), so every foreign call, libc/libm and the `arx_*` support
functions alike, resolves against the process itself. The interpreter has no
per-symbol host table: `run_extern` (`crates/interpreter/src/machine/data.rs`)
just guards the `"WASM"` abi, hands off to `machine::ffi::call_extern` (libffi),
and `fflush`es C stdio. That is why the `aresstd` support functions must be
linked into the interpreter and its dynamic symbols exported (see below).

##### Engine intrinsics (`lib = ""`)

A few conversions the language cannot express with `@cast` (which crosses only
integer widths) are provided by the standard library's native support functions
and bound in `library/C.ares` with an EMPTY library name: `C.i2d` / `C.d2i`
(@Int and Real), `C.i2f` / `C.f2i` (@Int and `@Float32`), `C.i2p` (@Int to
`@Ptr`), and `C.null` (the null pointer, `i2p 0`); CORE also binds the float
width/format helpers (`arx_f2d`/`arx_d2f`/`arx_real_to_str`/`arx_f32_to_str`).
`MATH` re-exports the numeric ones as `real_of_int` / `int_of_real` /
`f32_of_int` / `int_of_f32`.

These are not libc symbols. They live in ONE C source of truth,
`crates/ccg/src/aresstd.c`, so both engines run identical code (notably the
shortest-round-trip float formatting):

- **Native**: `aresstd.c` is appended to the single `.c` file the backend emits
  (the same translation unit as the generated code), so the linker resolves it
  locally with no `-l`.
- **Interpreter**: the same `aresstd.c` is compiled and linked into the `ares`
  binary by `crates/interpreter/build.rs` (`static:+whole-archive`, forced in
  since no Rust code references it), and the binary is linked with `-rdynamic`
  (emitted from the `ares`/`ccg`/`interpreter` build scripts as a
  `rustc-link-arg`, not via `RUSTFLAGS`/`.cargo/config.toml`, which a CI-set
  `RUSTFLAGS` would override) so its symbols land in the dynamic symbol table. A
  `C.i2d` call then resolves like any `lib = ""` symbol: `dlsym(RTLD_DEFAULT)`
  finds it in-process, and libffi invokes it.

So `""` is accurate: the symbol lives in the program (native) or is linked into
the interpreter binary, not an external library.

## Algebraic effects (the CEK driver)

Handlers/`perform`/`resume`/`@finale` work by capturing and splicing slices of a
**reified continuation stack** the same design as the interpreter (`IT.cpp`),
ported to C in `src/ARESxK.c` (option 1 of the two routes originally sketched
here). The driver owns a single heap continuation stack `kont` of `KFrame`s
(`K_RET` / `K_PROMPT` / `K_DEFER` / `K_THUNKRET` / `K_AFTERCLAUSE`) and drives
the generated block functions:

- **`do body ctl k ...`** lowers to `ARESxK_handle`, which pushes a `K_PROMPT`
  holding the (evaluated) clause closures and the value clause `els`, then runs
  the body block under it. A handler clause is the 2-parameter `Code`
  (`IR::conv_clause`): op argument in slot 0, resumption `k` in slot 1.
- **Performing** an operation (`do_apply` on a `T_OP`) searches down for the
  nearest prompt with a clause, captures the `kont` slice from that prompt to
  here (**including** the prompt -> deep handler) into a `ARESxK_Resump`, truncates
  `kont`, pushes a `K_AFTERCLAUSE`, and runs the clause below the prompt.
- **Resuming** (`do_apply` on a `T_RESUMP`) splices the captured slice back and
  delivers the value. A clause that resumes at most once hands the frames over
  (they move; `seg->seg` is left NULL); one that may resume again splices a
  `segment_copy` and stays resumable. Which it is comes from the clause's
  `ResumeUse`, carried in the prompt's `uses` table; a clause that never mentions
  `k` gets no resumption at all, just a `K_CLEANUPS` frame holding the abandoned
  slice's `@finale` cleanup thunks.
- **`@finale cleanup body`** (a CORE builtin over two thunks) lowers to the
  finalization frame (`T_DEFER`); it installs a `K_DEFER` marker that runs the cleanup
  on normal completion, on abort (the captured continuation is discarded the
  `K_AFTERCLAUSE` runs the pending cleanups with enclosing handlers still
  installed), or when a stored continuation later completes.

The interpreter distinguishes a **stored** resumption (a coroutine that stashed
`k`) from a **discarded** one via a `shared_ptr` use-count; the C runtime has no
refcount, so instead `ARESxK_mark_escape` flags a `ARESxK_Resump` when it is copied
into a closure capture or a struct/variant field (the value constructors in
`src/ARESxRT.c` call it). A stored/escaped `k`'s `@finale` cleanups are not finalized
at the clause boundary they run on later completion. (Limitation, inherited
from the interpreter: a stored continuation dropped without ever resuming never
runs its `@finale` cleanup.)

## Memory: reference counting (the default engine)

The runtime is memory-managed by **precise reference counting**
(`platforms/ARESxMEMRC.c`, selected by default; compile the emitted C with
`-DARX_MEM_BUMP` to fall back to the old never-freeing bump arena a memory
bug that disappears under bump is an RC-discipline bug). Strict + pure data is
acyclic by construction, so plain RC is complete for data; no cycle collector.

The discipline lives in the **runtime seam, not in emitted retain/release
pairs** generated code was already forbidden from touching representation
(every construction/read/store goes through `ARESxRT_*`/`ARESxVALUE_*`/`ARESxK_*`),
so those functions carry the ownership rules:

- **Stores own**: a frame slot (`ARESxK_setlocal`), a constructor child
  (struct/variant fields, closure captures, curried builtin/extern/finale
  operand rows), every kont frame's payload (KRet's frame, KPrompt's
  clauses + `els`, KDefer's cleanup, KThunkRet's saved value, KAfterClause's
  kval), and the global CAF cache each hold `+1`; reads borrow.
- **Lent parameters** (issue #385): a parameter the frontend infers as
  borrowed (`Code::borrowed`, emitted as `ARESxRT_code_borrowed`) is bound
  without a count when the call lends it (`ARESxK_apply_lent`,
  `ARESxK_tailcall_lent`, `ARESxK_tailcall_pass`). The frame's `lent` mask marks
  the slot, so neither a drop nor the frame's death releases it, and a slice
  copy does not retain it. A borrowed parameter entered by a call that did not
  lend takes a counted reference that dies with the frame. See Note [Borrowed
  parameters] in `crates/frontend/src/ir/borrow.rs`. Build the emitted C with
  `-DARES_RC_STATS` to print the retain and release counts at exit.
- **Frames are refcounted** (driver's current activation + every KRet, live or
  captured) and retain the closure whose `env` they borrow.
- **The temp pool**: every fresh value is registered with the pool owning its
  initial reference; the driver drains it after each block bounce. Whatever a
  bounce stored survives via its own retain; the rest the per-iteration
  garbage of a long-running loop dies immediately. (Measured on
  `examples/RC_LOOP.ares`, 200k iterations of struct allocation: **1.3 MB peak
  RSS under RC vs 277 MB under bump**.)
- **Box back-patch** (`ARESxVALUE_patch_box`): only a recursive `let` has a box.
  The KRet delivery content-copies into it, deep-copying payload arrays (a shared array would be freed
  twice) and retaining children *except a child pointing back at the box
  itself*, the **weak self edge** of a recursive-`let` closure that captured
  its own box. That is the one genuine RC cycle (the interpreter's `VRec`
  analog) and both `patch_box` and `ARESxVALUE_destroy` skip it symmetrically.
- **Continuation capture moves**: a performed effect moves the kont slice into
  the resumption segment (no count changes); resuming moves it back; a
  discarded, never-resumed segment releases its contents so aborted
  computations (exceptions) free their captured frames. Segments carry a small
  count of their own since `patch_box` can alias a resumption value.
- **Release is iterative** (a dead-value worklist in `ARESxMEMRC.c`), so dropping
  a long list does not recurse on the C stack.
- **Small blocks are recycled** (`ARESxMEM_cell` / `ARESxMEM_cell_free` in
  `crates/ccg/src/runtime.c`): value cells, frames (one block with their slot
  array) and the child arrays of tuples, records, variants, closures and
  partial applications go on a per-thread free list per 8-byte size class (up
  to 256 bytes, a bounded number of blocks per class) instead of back to
  malloc. Every size is a multiple of 8 and a class holds blocks of exactly its
  size, so a block may come from malloc and go to a list, or the other way
  round. A block on a list is not live, so the leak check stays exact; a worker
  hands its lists back to malloc when it exits.
- **Shared constants**: `{}`, `@true`, `@false` and the integers -128 to 1023
  are static cells (`ARESxRT_unit` / `ARESxRT_bool` / `ARESxRT_int`), immortal under
  threads (`rc == 0`) and out of reach of zero without. A let box is always a
  fresh cell (`fresh_box`), since it is patched in place; patching a constant
  is a runtime fault.

**The built-in leak check:** in the value-printing harness mode the generated
`main` releases the CAF cache and the temp pool on exit and then asserts
`ARESxMEM_live() == 0`, exiting **97** (with a stderr report) if any allocation
survived. Every `build native-test`
example is thereby also a leak regression test. Under `-DARX_MEM_BUMP`,
`ARESxMEM_live()` is 0 and the check passes trivially.

## Missing features and where they plug in

### Unboxing beyond integers *(optimization)*

Integers are unboxed inside and between direct functions (below). Reals,
booleans across calls, and the values flowing through block code are still
pointer-boxed.

## Known limitations

- A stored continuation dropped without ever resuming never runs its `@finale`
  cleanup (inherited from the interpreter) though its memory *is* reclaimed.
- FFI marshals the base scalar/pointer types; aggregates (struct/variant) are
  not passed across the boundary, and only libc-resolvable libraries are
  exercised (others just need the right `-l`/path).
- RC handles the direct self-cycle of a recursive-`let` closure (the weak self
  edge). An *indirect* local value cycle (a recursive `let` whose box is
  reachable only through other fresh nodes, e.g. `let rec x = Cons 1 (Cons 2
  x)`) would leak the same class the interpreter's weak-`VRec` assumption
  covers; such bindings do not occur in practice (recursive lets bind
  functions).
- Outside direct functions values are pointer-boxed. RC is runtime-seam-based,
  not a full Perceus-style compile-time dup/drop pass: a dead cell is reused in
  place only by `@vec_push`/`@vec_set` and by constructor reuse of variants.

## Verification

The interpreter smoke test (`tst/TS.cpp`) forces every global, so any runtime
fault in any top-level definition surfaces; several examples self-check with a
`... else 1 / 0` guard. The compiled binary's generated `ARESx_force_all` mirrors
this exactly, so the equivalence is:

> interpreter smoke test **OK**  <=>  compiled binary **exits 0**

`build native-test` checks this for every example (all 24, effects included),
and the generated main's leak check makes each one a leak regression test too;
`build test` runs the interpreter side. The effect examples (EFFECTS, FINALE,
COROUTINES, PIPES, EFFECT_OVERLOAD) are validated to produce the **same values**
natively as under the interpreter (exceptions 42/-1, generator 42, state 21,
`@finale` 103/9/6/2/10/31), and the emitted binaries are valgrind-clean of invalid
reads/writes.
