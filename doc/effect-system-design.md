# Thrax Effect System Design Decisions

**Status:** design locked except where noted "OPEN". No implementation yet.
**Date:** 2026-06-26.
**Scope:** the algebraic-effect system and the runtime/IR machinery it requires.
Records _what_ has been decided, _why_, _rejected_, and _deferred_.

---

## 0. Context & goals

Compiler pipeline:

```
EX (parse) -> LL (lower) -> MR (modules) -> TC (type-check)
   -> CR (Core: typed desugared lambda calculus)
   -> IR (closure-converted, control-explicit)
   -> { interpret (IT)  |  compile to C (future) }
```

`CR` (the "Core") exists and is arena-allocated, reference-count-free, the
substrate every backend shares. The **IR** is a _new_ lowering target below the
Core; this document is about the IR and the effect system it has to support.

Goal is to have algebraic effects with handlers expressive enough for: **state, async,
coroutines, exceptions, generators** (and, since we drop lazy data see section 1
**codata / streaming**).

Hard constraints:

- **Reference counting, no GC.**
- Type system is **Hindley-Milner, no type classes**, to be extended with
  **effect rows**.
- A **C backend** is a goal; the IR must lower to C, not just be interpreted.

---

## 1. Evaluation strategy: strict (CBV), no lazy data but codata is its own kind

**Decision.** Thrax is uniformly **call-by-value and pure**. Data constructors
are **eager** there are _no_ thunks in data. Coinductive / infinite structures
are NOT lazy data; they are a **separate kind, `@codata`** (see section 1a), and
streaming may also be expressed through the **effect system** (a `yield`-style
generator consumed by a handler).

**Why.** Pure **+** strict ==> a cyclic data value cannot be constructed at all
(cycles need mutation, which we don't have, or laziness, which is dropped).
Therefore every data value is a finite tree/DAG, every refcount reaches zero, and
**reference counting is complete for data, no leaks, no cycle collector, no
weak references for data.**. I was worried that lazy data could introduce bugs
and inconsistancies.

**Rejected: lazy recursive (pure) sum data.** It would have given ergonomic
codata (`ones = Cons 1 ones`), but knot-tied corecursion (`ones` referring back
to itself) builds a genuine **reference cycle**, which plain RC cannot reclaim ->
a leak. Generative corecursion (`go n = Cons n (go (n+1))`) is acyclic and would
have been RC-clean, but we cannot statically separate the two, so we cannot
_guarantee_ cycle-freedom.

This mirrors Koka's reasoning (Koka is strict): pervasive/implicit laziness
fights (a) precise effect ordering, _when_ an effect runs becomes unpredictable;
(b) precise reference counting, a memoizing thunk is a hidden mutable cell with
an unpredictable lifetime.

**Consequences.**

- The existing lazy-union / thunk machinery (`VThunk`, `force`, inline variant
  fields in ANF) is **removed**; variant fields evaluate eagerly like struct
  fields. This is localized to `IT` (runtime) and `CR` (ANF); `TC` is largely
  unaffected (strictness is not a typing concern).
- There's no: lazy sharing / dynamic
  programming -> explicit memoization (a `state` effect or table); cyclic
  immutable graphs -> id/index representation instead of direct pointers.
- The effect system becomes **load-bearing for codata**, not just for IO-like
  effects. This raises its priority.
- **CAFs are unaffected** see section 7; a lazy-memoized top-level _binding_ is a
  different mechanism from a lazy _data field_ and does not create cycles.

---

## 1a. Codata: a distinct, non-memoized, copattern-defined kind

**Decision (revises the original "codata via effects only" stance, 2026-06-27).**
Thrax supports **codata** as a first-class kind, declared `@codata` and defined
by **copatterns** (the data/codata duality: data is built by constructors and
eliminated by pattern matching; codata is built by defining its **observations**
and eliminated by **observing** them). Observations are **non-memoized**.

```
$ Stream : @codata `T = head : `T, tail : Stream `T,

$ from : Int -> Stream Int = \n =
    .head = n,
    .tail = from (n + 1),     # copattern clauses, one per observation
```

**Why this is RC-safe (and lazy data was not).** The cycle problem that killed
lazy data is specific to **knot-tying** (`ones = Cons 1 ones`: a field that is a
pointer *back to the same cell*). Copattern codata is **generative**: recursion
goes through a *function call* (`from (n+1)`) that yields a *fresh* value and
captures only the function's parameters, **never the codata value itself**. So
observing a stream produces V -> V' -> V"..., each independent, old ones freed as you
walk: **no cycle, RC stays complete.** The only way back to a cycle is an
explicit self-referential codata binding (`let s = cocons 1 s`), which copattern
definitions do not produce, that form is forbidden (or accepted as a rare leak,
same stance as cyclic data). This is also how Koka does it (non-memoized `() ->
a` suspensions); a dedicated `@codata` just makes it ergonomic and typed.

**Non-memoized**, specifically: each observation recomputes. This keeps it
trivially cycle-free, avoids a hidden mutable cache cell (the Perceus wrinkle),
and matches codata's purpose (productivity, not sharing). The cost is no sharing
(re-observing `.tail` recomputes). Memoized codata was rejected for the same
reasons lazy data was.

**Runtime (additive to the strict machine).** A codata value is a small record
of observation-closures (`VCodata { head: <closure>, tail: <closure>, ... }`);
observing `s.tail` = **call** the corresponding closure. Data stays strict and
untouched; this adds one value kind and two IR forms (copattern construction,
observation).

**Status:** its own milestone, scheduled **after** the strict transition (M1c);
see section 10. `AGTxSUM`'s old lazy-data infinite stream is rewritten as `@codata`
then, not resurrected as lazy data. Deferred sub-decisions: surface syntax for
copatterns and observation; how `@codata` interacts with the effect rows (an
observation may carry an effect row, effectful codata, once typed effects
land); whether to statically forbid self-referential codata bindings.

---

## 2. Continuation discipline: affine (resume 0 or 1)

**Decision.** A resumption may be invoked **at most once** (affine). Multi-shot
is **not** in the base language; if ever needed it arrives as an explicit,
additive `clone` operation (deep-copies a captured continuation), **deferred**.

**Why affine is enough.** Every target effect is affine:

| Effect      | Resumes the continuation | Affine? |
| ----------- | ------------------------ | ------- |
| Exceptions  | **0** times (abort)      | OK      |
| State       | exactly once             | OK      |
| Coroutines  | once (when rescheduled)  | OK      |
| Async/await | once (on resolve)        | OK      |
| Generators  | once per `yield`         | OK      |

Only handlers that produce **several** continuations from one `perform`
nondeterminism / backtracking / probabilistic search need multi-shot, and
those are not on the list.

**Why affine fits the runtime + RC.** With the reified-K machine (section 6) an affine
resumption is a **move-only stack segment**: capture = O(1) unlink, resume = O(1)
relink, **no deep copy ==> no RC `dup`**. Dropping an unresumed resumption (the
exception case) runs its segment's destructor -> one `drop` per held reference
uniform with the rest of RC / Perceus. Multi-shot is the one case that would have
forced deep-dups of captured state; affine sidesteps it.

**affine vs linear (exactly once).** Linear is _marginally_ simpler at runtime
(no drop/`discontinue` path) but **cannot express exceptions / early-exit /
cancellation / finite generators** (those resume 0 times). It is also no easier
to type to _enforce_ it statically you need a substructural type system, and
linear (no weakening) is _less_ compositional than affine (allows dropping).
Affine = "linear + an explicit `discontinue`", and `discontinue` is exactly the
unwinding exceptions need anyway. So affine is the strictly more useful point.

**Enforcement.** Affine is enforced **dynamically**: a resumption carries a
"used" flag; resuming twice is a runtime error (a la OCaml 5
`Continuation_already_resumed`). No type-system change required for this. A
static substructural guarantee is a possible _later_ option, not a requirement.

**Note (important):** continuation **multiplicity is orthogonal to effect-row
composition.** Affine keeps the _entire_ "combine effects easily" story of effect
rows (union of effects, effect polymorphism, modular/nested handlers). Affine
costs _only_ multi-shot handlers, nothing about composing different effects.

---

## 3. Handlers: deep, clause runs outside its own prompt

**Decision.** Handlers are **deep** (a resumed computation is still handled by
the same handler the captured continuation re-installs the prompt). A handler
operation clause (and the return clause) runs **outside** its own prompt, so a
handler that performs its _own_ operation is caught by an _enclosing_ handler,
never itself (no accidental self-handling loop). To target a specific nested
handler you re-`handle`, or (future) use `mask`/named handlers.

**Why.** This is the standard Eff/Koka/OCaml semantics; deep is the most
ergonomic for state/generators/coroutines (they read as recursive handlers), and
"clause outside its prompt" is the only choice that avoids self-handling
infinite loops.

**Deferred:** `mask<eff>` (skip the innermost handler of an effect) and **named
handlers / instances** (disambiguate multiple handlers of the same effect) the
explicit escape hatches Koka provides. Not needed for the first cut.

---

## 4. Resumptions: first-class, typed

**Decision.** A resumption `k` is a **first-class value** (storable in data
structures), of a real type `resume : (a) -> e b`. Affine still holds: you may
_store_ `k` freely but _resume_ it <= once.

**Why first-class is required.** Schedulers/coroutines store suspended fibers in
a run-queue and resume later; a generator object holds its suspended `k` between
`next()` calls. Scoped-only resumptions cannot express either. (State/exceptions
use `k` immediately and don't need it, but it costs nothing.) This is exactly
OCaml 5's first-class `continuation`, restricted to affine.

**Consequence (RC lifetime).** A stored, never-resumed `k` keeps its whole
captured segment (and everything it closes over) alive until dropped /
discontinued an expected, accepted source of retained memory under RC.

---

## 5. Effect typing: effect rows (Koka-style), on the roadmap

**Decision.** Effects will be **typed with effect rows** (Koka-style): a
computation's type carries a row of the effects it may perform; rows **union**
when code combines; functions are **effect-polymorphic**; handlers are typed.

**What it unlocks.**

- **Evidence passing** (Xie/Leijen, "Effect Handlers, Evidently"): the effect
  type tells the call site _which_ handler an operation targets, so `perform` is
  an O(1) evidence lookup instead of a dynamic stack search.
- **Tail-resumptive optimization:** operations that resume once in tail position
  (state, reader, most handlers) compile to a **direct call with no continuation
  capture at all**.
- **Compile-time** unhandled-effect and exhaustiveness errors.
- **Purity classification** (e.g. which CAF initializers are effectful).

**Cost.** Effect-row unification, an effect variable on the function arrow,
effect-row polymorphism and inference.

**Interim (untyped) model.** Until the rows land, effects are **untyped /
dynamic** (OCaml-5 style): `perform`/`handle`/`resume` are ordinary terms, the
handler is found by **dynamic search up the K stack** for the nearest matching
prompt, and an unhandled effect is a **runtime** error. This keeps the current
checker intact and lets the runtime model be validated independently. The
tail-resumptive optimization is still partly available dynamically; the
O(1)-evidence indexing specifically needs the types.

**Status (2026-06-27): the rows have landed for TYPING (M3.1).** A function's
type now carries an effect row, handlers discharge what they handle, and an
unhandled effect is a **compile-time** error. RUNTIME dispatch is still the
dynamic K-stack search above, evidence passing and the tail-resumptive
optimization (which consume the types) remain M3.2, as does effect subsumption.
See the roadmap (section 10).

---

## 6. Runtime model: reified-K abstract machine

**Decision.** Replace the tree-walking interpreter with a **CEK-style abstract
machine** whose **continuation is an explicit heap data structure** (a linked
stack of frames), not the host C++ stack. The current trampoline becomes a
_special case_ of this machine's step loop. "Context switching / jumping the
stack" = swapping/splicing reified `K` segments, which is what makes effect
capture possible at all (a tree-walker on the C++ stack cannot reify its own
stack).

**Machine state:** `< ctrl, env, K >`.

**Frames (`K` is a heap-linked list of these):**

```
KRet    { term, bind: slot, locals, env }   -- non-tail let continuation
                                             --   (caller's saved activation +
                                             --    where to bind the result)
KPrompt { handlers, ret_clause, locals, env }-- a handler delimiter
KEnd
```

**Core step rules (sketch):**

```
Let x = c in t (non-tail) : push KRet{t, slot(x), locals, env}; run c
App f a (tail)            : overwrite locals/env from f's closure; jump; push NOTHING
                           (this is the trampoline / constant-stack tail call)
Ret v  @ KRet            : restore saved locals/env; locals[slot]=v; run term
Ret v  @ KPrompt P       : run P.ret_clause(v); pop P; continue below
```

**Effect rules (deep, affine):**

```
Handle h body : push KPrompt{h, locals, env}; run body
Perform op a  : walk K down to nearest KPrompt P whose handler set has `op`
                split K into  captured = [frames above P ... , P]   (INCLUDE P => deep)
                              K_rest   = everything below P
                k := Resumption(captured)        -- move-only segment (affine)
                set K := K_rest
                run P.handlers[op] with (a, k)   -- clause runs OUTSIDE its own prompt
Resume k v    : require k unused (affine); mark used
                K := k.segment ++ K              -- splice the saved stack back on
                deliver v at the original perform point
Discontinue k : drop k unresumed; unwind its segment running cleanup (finally)
                -- the resume-0 path (exceptions); semantics DEFERRED, see section 11
```

Because the captured segment **includes** `P`, a resumed computation is again
under the handler ==> **deep**. Because the clause runs with `K := K_rest`, it is
**outside** `P`.

**Tail-resumptive optimization:** when a clause just `resume`s in tail position,
do _not_ reify a segment run the operation in place and return. Reserve real
capture for handlers that stash `k` (generators/schedulers/async). Full O(1)
handler dispatch needs the effect types (section 5); the in-place tail case is partly
detectable dynamically.

---

## 7. IR & closure conversion

**Decision.** The IR is the Core after **closure conversion**, in ANF (the Core
is already ANF + De-Bruijn). Optimize the IR for the **C backend first**; the
interpreter conforms.

**Addressing three explicit modes (vs one flat De-Bruijn env):**

```
atom ::= Local i      -- slot in the current activation (params + let/case binds)
       | Env i        -- field of the current closure's captured record
       | Global g      -- top-level binding
       | Lit k
       | MkClosure(label, [atom])    -- allocate { code label, [captured atoms] }
term ::= Ret atom | Let x = term in term | App atom atom (tail?)
       | Prim op [atom] | Case atom {alts} else term
       | MkStruct/Field | MkData/CaseTag
       | Handle h term | Perform op atom | Resume k atom
top  ::= Code { label, nparams, nlocals, body }   -- a lifted, closed function
```

- Closure conversion lifts every lambda to a top-level **`Code`** (closed except
  params + globals); a `CR::Fun` becomes `MkClosure(label, captures)`. Free vars
  -> `Env i` (the closure record); params + `let`/`Case` binders -> `Local i`. The
  pass also fixes **`nlocals` per `Code`** (a small register allocation) so every
  activation is a known-size slot array, a real C stack frame later.
- This split is what makes the C backend's **closure-record (heap struct) vs
  stack-frame (locals)** distinction fall out for free, and lets a `Global g`
  naming a function compile to a **direct call** (no closure allocation).

**Globals lazy-memoized CAFs.** Each global is evaluated **on first reference**
and memoized (Haskell/Koka-style top-level init). Pure constants are thus
observationally eager; effectful ones are naturally **deferred to first use under
a real handler** (a top-level initializer has no handler installed, so eager-at-
load would have nowhere to send a `perform`). No effect analysis required to get
this behavior. Globals are owned by a **global table**; cross-references go
through it (non-owning lookups), so global mutual recursion creates **no RC
cycle**. Function globals are closures with an empty captured record.

---

## 8. Memory management

**Decision.** **Reference counting, no GC.** For now the interpreter uses host RC
(`shared_ptr<Value>` / the existing `VRec` cell). Later, a **Perceus-like** pass
inserts explicit `dup`/`drop`/`free` over the IR for the C backend; the IR is
already ANF with explicit binds, the form Perceus wants. RC stays a _pass over_
the IR, not part of the IR's primitive meaning.

**Cycle analysis:**

- **Data:** strict + pure => finite DAG ==> no cycles => RC complete (section 1).
- **Recursive closures** (`let rec go = \... go ...`): the closure captures itself
  the one genuine ownership cycle. Handled by the existing **weak-reference
  `VRec` keepalive** (`target.lock()` in the current interpreter).
- **Globals:** table-owned, cross-referenced by non-owning lookup ==> no cycle.

So the only cycle source is recursive closures, already solved; everything else
is acyclic by construction.

---

## 9. Worked examples (model validation)

**State (deep, affine):**

```
handle body
  with get () k -> \s -> resume (k s)  s
       put s' k -> \s -> resume (k ()) s'
  return x      -> \s -> (x, s)
```

Each clause resumes once ==> affine OK; deep keeps get/put handled across the
resume OK.

- **Exceptions:** the handler resumes **0** times (drops `k` ==> `discontinue`).
- **Generators:** `yield v` performs; the consumer is the handler; the suspended
  `k` lives in the generator value (needs first-class `k`).
- **Coroutines / scheduler:** suspended `k`s sit in a run-queue (first-class),
  each resumed once (affine).
- **Async/await:** the continuation after `await` is resumed once on resolve.

All four fit affine + deep + first-class `k`.

---

## 10. Roadmap

**Sequencing: runtime-first** (decided). Build the runtime model first, layer the
type system and the C backend on after the novel risk is in the machine, and
an untyped effect runtime is independently testable. (Alternatives considered and
rejected: _types-first_ cleaner end state but nothing new runs for a long time;
_minimal effect slice ASAP_ fast to validate but throwaway work.)

Status legend: [X] done, [~] in progress, [ ] planned.

- [X] **M1 re-platform onto the IR + reified-K machine.** Strict, no effects, no
  new types. Done in three steps, all landed:
  - [X] **M1a** Core->IR **closure conversion** (`Local/Env/Glob`, lifted `Code`,
    `MkClosure`), `--ir` dump. Module `IR` (`inc/IR.hpp`, `src/IR.cpp`).
  - [X] **M1b** the **reified-K machine** (explicit heap continuation stack,
    constant-stack tail calls, lazy-memoized CAFs, recursive-`let` via `VRec`).
    Module `ITxMACHINE`; runtime value `VCode`.
  - [X] **M1c** **go strict**: lazy data excised from `dat/`, CR tree-walker
    (`IT::eval`/`force`/`VThunk`/`VClosure`) deleted, machine is the sole runtime.
    Parity gate met: all 17 `dat/` examples pass, valgrind-clean.

- [ ] **Mx codata.** `@codata` kind + copatterns + observation; runtime
  `VCodata` (record of non-memoized observation-closures); rewrite `AGTxSUM`'s
  former stream as codata. Needs the strict machine (M1) as substrate;
  independent of the effect milestones, so it can interleave with M2/M3. See section 1a.

- [~] **M2 untyped affine effects.** `Handle` + perform/resume on the K stack:
  deep, affine, first-class resumptions, dynamic handler search. **Mostly landed
  (2026-06-27).** Surface is keyword-frugal (section 12): no `perform`/`resume` calling
  an operation performs, applying `k` resumes; unit `{}` added. Done in substeps:
  - [X] **3a** parse `do <body> ctl k  is op a = e ...  [else x = e]` into
    `ExHandle` (new `do`/`ctl` keywords; `is`/`else` reused).
  - [X] **3c** typing (`CHandle` in TC: `op : A -> B` gives `a:A`, `k:B->result`;
    `else`/identity value clause) + CR/IR lowering (clauses/`else` as lifted
    closures; a `Handle` IR node; `Program.operations`).
  - [X] **3d** the machine (`IT`): `KPrompt` on the `kont` stack, `VOp`
    (perform = search-down + capture + run clause outside its prompt), `VResump`
    (resume = splice, affine-guarded), `ret` runs `els` on normal completion.
    Validated: exceptions (resume 0), generator (resume 1, sum + list), state
    (parameter-passing, deep); affine double-resume faults; valgrind-clean;
    `dat/EFFECTS.thx` in the suite.
  - [X] **3b effect-qualified op resolution (2026-06-27).** Operations are now
    module-scoped symbols whose canonical identity is `Effect.op`, registered in
    MR's symbol table and resolved through the ordinary scope + `ExOverload`
    machinery (TC keys `m_prim`/`m_op_effect` by the identity; the runtime matches
    by it unchanged). Same-named ops from different effects coexist: a bare use
    resolves when unambiguous (one in scope, or the ambient effect picks it) else
    needs `Effect.op`; a perform may also resolve by type via `ExOverload`. Clause
    heads accept `is Effect.op a` and must qualify a shared name. The old
    `AMBIGUOUS_NAME` up-front rejection is gone. `dat/EFFECT_OVERLOAD.thx` covers
    it.
  - [X] **`defer` (2026-06-27).** `defer <cleanup> do <body>` keyword (Go-style
    surface; Koka-style runtime; no `discontinue`). Desugars to an internal
    `%finally` intrinsic (`OP::FINALLY`; not a user identifier). Cleanup runs on
    normal completion (a value returning through a `KDefer` marker), on abort (the
    clause-boundary `KAfterClause` marker finalizes a discarded `k`'s cleanups on
    the live stack, so enclosing handlers are still installed), and when a stored
    continuation completes. Detected via the resumption's `used` flag + a kval
    refcount ("stored") check -- NOT a destructor, so timing is correct. Also made
    `do <body>` (no `ctl`) a plain block. `dat/FINALLY.thx`. Limitation: a stored
    continuation dropped without ever being resumed does not run its `defer`.
  - Not yet: coroutine scheduler example, async.

- [~] **M3 effect-row type system.** Rows in TC, effect polymorphism +
  inference, handler typing; **evidence passing** + the tail-resumptive
  optimization; compile-time unhandled-effect / exhaustiveness.
  - [X] **M3.1 correctness slice (2026-06-27).** Effect rows in the type graph
    (`TRowEmpty`/`TRowExtend`, an `eff` on `TArrow`); Leijen scoped-label row
    unification; ambient-effect threading through `infer`/`infer_against` (App
    unifies the callee's latent row with the ambient, Lam opens a fresh ambient,
    Handle discharges its handled effects, top level is the empty row);
    operations typed with an open tail `<L | mu>`; Koka-style `<...>` surface
    syntax on the arrow (`A -> <E1, E2 | `e> B`, bare arrow = pure). Catches
    unhandled effects at compile time. TC-only (types erased before CR/IR/IT);
    `dat/EFFECTS.thx` + `dat/COROUTINES.thx` annotated and passing,
    valgrind-clean. Known gap: no effect SUBSUMPTION (application uses row
    equality), so an explicitly-pure arrow isn't callable in an effectful
    context unless made row-polymorphic.
  - [~] **M3.2 perf + subsumption.**
    - [X] **Effect subsumption (2026-06-27).** A call site requires the callee's
      latent row to be a SUBROW of the ambient (`subrow` in TC), not equal to it,
      so a pure (or smaller-effect) function is callable in any larger effect
      context. Function-TYPE unification (argument positions) still uses
      equality, so an effectful function cannot be passed where a pure one is
      expected. Closes the M3.1 ergonomic gap.
    - [ ] Evidence passing (O(1) dispatch), the tail-resumptive optimization;
      exhaustiveness policy; parametric effects (effects with type arguments).

- [ ] **M4 Perceus + C backend.** Explicit `dup`/`drop` over the IR; uniform
  boxed value representation with RC headers; C emission.

---

## 11. Deferred

- **M2 (effects):**
  - `defer` DONE (2026-06-27): `defer <cleanup> do <body>` keyword runs cleanup
    on completion/abort/stored-completion via `KDefer` + clause-boundary
    `KAfterClause` markers (no `discontinue` keyword; Koka-style runtime,
    Go-style surface; desugars to the internal `%finally` intrinsic). `initially`
    and the stored-then-dropped case remain unaddressed.
  - Effect **surface syntax** **DECIDED 2026-06-27**; see section 12 and
    `doc/syntax-spec.txt` (EFFECTS). `@effect` declaration; `do ... ctl k ... is ... else
    ...`; no `perform` (call the operation), no `resume` (apply the first-class `k`);
    unit `{}`.
  - **Named handlers** vs pure effect-label dispatch; `mask`.
  - **Machine notes (3d/3b, 2026-06-27):**
    - **Constant host stack DONE.** A clause lowers to a single 2-slot `Code`
      (`a`=Local 0, `k`=Local 1) and `perform` jumps into it inline on the single
      reified-`kont`, with no nested `apply`. A 100 000-resume generator runs
      without host-stack overflow, valgrind-clean. (A non-tail resume still grows
      the `kont` vector O(N) on the heap, which is fine; the host stack is
      bounded.)
    - **Coroutines validated** (`dat/COROUTINES.thx`): a two-task round-robin
      scheduler with first-class stored resumptions resumed cross-context works,
      valgrind-clean.
    - **Effects still do not cross a builtin-invoked closure** (`Machine::apply`,
      used for the entry point and any higher-order builtin, starts a fresh
      `kont`). No current builtin takes a function argument, so this is moot;
      revisit if one ever does.
  - **Operation name resolution DONE (3b, 2026-06-27).** Operations are now
    registered as **module-scoped symbols** in MR, each with the canonical
    identity `Effect.op`, and resolved through the ordinary scope + `ExOverload`
    candidate machinery (TC keys its `m_prim`/`m_op_effect` schemes by that
    identity; the runtime matches a performed op to a clause by it, unchanged).
    Same-named operations from different effects **coexist**: a bare op resolves
    when unambiguous (the one in scope, or the ambient effect selects it), may
    resolve by type via `ExOverload`, and is otherwise qualified `Effect.op`.
    Handler clause heads accept `is Effect.op a` and must qualify a shared name
    (a clause cannot be type-resolved). The `m_prim` shortcut and the
    `AMBIGUOUS_NAME` up-front rejection are gone; `m_prim` now holds only the
    `if`/array builtins plus identity-keyed operation schemes.
- **M3 (types):**
  - Effect-**row representation** (open rows + a polymorphism tail variable;
    duplicate labels / scoped labels a la Leijen).
  - **HM integration** (effect variable on the function arrow; inference).
  - Unhandled-effect & **exhaustiveness** policy (error vs warning).
- **M4 (backend):**
  - **Uniform value boxing** + RC-header layout for Perceus.
  - **C-emission** strategy (how the reified `K` and closures map to C
      explicit stack/heap frames vs native segmented stacks).

---

## 12. Surface syntax (decided 2026-06-27)

The M2 (untyped) surface, recorded in full in `doc/syntax-spec.txt` (EFFECTS).
Chosen to reuse Thrax's existing grammar and spend as few keywords as possible;
the result is **closest to Flix**.

- **Declaration.** `$ Eff : @effect = op : A -> B, ...` an `@effect` annotation
  parallel to `@struct` / `@codata`. Effect = TypeName, operations = lowercase
  vars. Each op is `arg-type -> resume-type`; every op takes exactly one argument
  and names one result. The new **unit** type/value `{}` (empty record, added to
  the language for this) spells "no argument" / "no result".

- **No `perform`.** An operation is performed by **calling it** (`yield v`,
  `get {}`), as in Koka/Flix a use of an operation name _is_ a perform. Resolves
  by the ordinary module name rules.

- **No `resume`.** The continuation `k` is a **first-class value** of function
  type; **applying it** (`k v`) resumes, as in Flix/OCaml. Affine: applied <= once
  (runtime-checked), droppable (= the exception case), storable (= generators /
  coroutines).

- **Handling.** `do <body> ctl k  <is-clauses>  [else x = e]`. `do` takes the body
  (no parens `ctl` delimits it). `ctl k` binds the one continuation, shared by
  all clauses (exactly one `k` per performed op). `is op a = e` handles `op`,
  binding its argument `a`. `else x = e` is the value clause (normal completion),
  binding the result; optional, identity by default. `is` / `else` are reused from
  `match`; handlers are deep (section 3).

Net new keywords: `@effect`, `do`, `ctl` (plus reused `is` / `else`); all
contextual. `ctl` is borrowed from Koka and reserved as the slot for a future
`fun` (tail-resumptive, no-capture) clause kind see section 6.

---

## 13. Glossary

- **Affine resumption** a captured continuation invoked _at most once_.
- **Deep handler** the handler stays installed for the resumed computation
  (vs **shallow**, which does not).
- **Tail-resumptive** a handler operation that resumes once in tail position;
  compilable with no continuation capture.
- **Evidence passing** threading handler "evidence" (chosen by the effect
  type) so `perform` finds its handler in O(1) (needs effect types).
- **Reified continuation** the call stack represented as an explicit,
  manipulable heap value, rather than the host machine stack.
- **CAF** Constant Applicative Form: a top-level binding evaluated once and
  memoized.
- **Perceus** precise, compiler-inserted reference counting (`dup`/`drop`),
  the GC-free memory model used by Koka/Lean.
