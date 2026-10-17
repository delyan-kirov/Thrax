# Thrax Concurrency and Parallelism Design

**Status:** sections 2.1, 2.2, 2.3 and 2.5 implemented, with the gaps listed
in section 6; STM (2.4) is a proposal. Tracked by the GitHub issues labelled `multi-threading`.
**Date:** 2026-10-17.
**Scope:** how Thrax programs run work concurrently (interleaved) and in
parallel (on several cores), what the runtime must change to allow it, and how
the effect system decides what may cross a thread boundary.

---

## 0. Context

Two different problems hide behind the word "multi-threading":

1. **Concurrency.** Many activities in flight at once, interleaved: a server
   with many connections, a UI waiting on input and a timer. This can be done
   on a single core.
2. **Parallelism.** Using several cores to finish one computation sooner: a
   map over a large vector, a matrix product.

They want different tools, and a language that conflates them usually ends up
with locks everywhere. This design treats them separately.

### What Thrax already has

- **Pure, strict, no mutation, no cycles** (effect-system-design.md, section 1).
  Every data value is an immutable finite DAG. There is no shared mutable state
  to protect, which is the problem most threading designs exist to solve.
- **Algebraic effects with reified continuations.** The C runtime keeps the
  continuation as a heap stack of `KFrame`s (`crates/ccg/src/runtime.c`:
  `g_kont`, `Resump`), resumable any number of times since #228. A suspended
  computation is an ordinary value, so green threads cost nothing extra.
- **Typed effect rows** on every arrow (`A -> <E> B`), with row variables for
  effect polymorphism, and handlers that discharge what they handle.
- **Exact, non-atomic reference counting.** `struct Value { Tag tag; unsigned rc; ... }`.
  `THxMEM_unique` (`rc == 1`) drives in-place reuse, and the ownership pass
  (#230) emits moves and drops, so many values are provably uniquely owned.

### What blocked OS threads (before section 6)

- `rc` is a plain `unsigned`, incremented and decremented without atomics.
- Machine state is global: `g_kont`, `g_act`, `g_v`, `g_fn`, `g_arg`,
  `g_blk`, `g_result`, the value pool `g_pool`, the deferred-free list
  `g_dead`, the interned names `g_names`, and the leak counter `g_live`.
- Lazy recursive slots (`T_THUNK`) are forced in place, which is a write to a
  value other threads could be reading.
- The interpreter's values are `Rc<RefCell<Value>>`
  (`crates/interpreter/src/machine/data.rs`), single-threaded by construction.

---

## 1. Survey

### Haskell: STM, sparks, green threads

Haskell offers three separate tools:

- `forkIO`: cheap green threads multiplexed over a few OS threads.
- `par` and `Strategies`: pure parallelism. Because the code is pure, evaluating
  it on another core cannot change the answer.
- **STM (software transactional memory)** for shared mutable state.

STM works like this. Shared variables are `TVar`s. Code inside
`atomically { ... }` reads and writes them without taking any lock; the runtime
logs every access. At the end it checks whether any variable it read was
changed by someone else meanwhile. If not, all writes are committed at once. If
so, the block is discarded and run again. `retry` means "block until a variable
I read changes", and `orElse` tries an alternative transaction.

The gains over locks: no deadlocks, no forgotten unlock, and transactions
**compose**. Two correct transactions glued together form a correct
transaction, which is false of two correct lock-protected sections. The whole
scheme rests on one type-system fact: a transaction cannot perform IO, because
it may be run several times and must be undoable. Haskell enforces this by
giving transactions their own monad, `STM`, distinct from `IO`.

### Koka

Koka has no built-in thread model in the language proper. Asynchrony is a
library effect handled by an event-loop handler. Its relevant contribution is
in the runtime (Perceus reference counting): counts are non-atomic by default,
and an object is marked **thread-shared** (Koka uses a negative count) when it
escapes to another thread. Only marked objects pay for atomic operations, so
single-threaded code runs at full speed.

Koka also shows how to keep IO precise: its `io` is a row alias over finer
effects, roughly `<exn, div, ndet, console, net, fsys, ui, st<global>>`, where
`st<h> = <read<h>, write<h>, alloc<h>>` describes its mutable reference heap.

### Lean 4

`Task` for pure parallelism, with the same marking trick as Koka: objects
reachable from a value passed to another task are marked multi-threaded, and
only those use atomic counts. Like Thrax, Lean is refcounted, pure and reuses
uniquely owned values in place.

### OCaml 5

**Domains** are OS threads for parallelism; **effects** implement fibers inside
a domain. Libraries such as Eio build structured concurrency (task groups,
cancellation, IO scheduling) entirely on effect handlers. This is the closest
architectural match to Thrax: effects for concurrency, a separate mechanism for
parallelism.

### Others

- **Erlang / Elixir:** actors, shared-nothing, a heap per process, message
  passing only. Very robust, but every message is a copy.
- **Go:** goroutines and channels over a shared, garbage-collected heap. Easy
  to use; data races remain possible.
- **Rust:** OS threads guarded by the `Send` and `Sync` traits; ownership and
  borrowing prove the absence of data races.
- **Swift:** structured concurrency (task groups), actors, `Sendable`, atomic
  reference counting everywhere.
- **Pony:** reference capabilities in the type system; actors; race freedom by
  construction.
- **Clojure:** immutable data plus STM refs and atoms.
- **Futhark, Chapel, NESL:** data parallelism, parallel map and reduce over
  arrays.

---

## 2. Decisions

Thrax sits in the Koka, Lean 4 and OCaml 5 corner: pure, refcounted, with
effects. The design reuses their proven parts. The one Thrax-specific idea is
section 2.3.

Syntax in this section is a sketch, not settled surface.

### 2.1 Concurrency is an effect (one OS thread)

A library module provides an `Async` effect and a scheduler handler written in
Thrax over the existing continuations.

```thrax
$ Async : @effect = spawn : ({} -> <Async> {}) -> {}, yield : {} -> {},
$ run_async : ({} -> <Async> a) -> a = ...   # round-robin over stored continuations
```

- **Structured:** a `run_async` scope is a task group. It returns only after
  every task spawned inside it has finished. No task outlives its scope.
- Channels and `await` are further operations of the same effect, or a
  separate `Chan` effect handled by the same scheduler.
- IO readiness plugs in through `poll(2)`, the way applications/launcher
  already gates its feed coroutine.
- No runtime change is needed. Both engines run it identically, and the
  interpreter stays the reference semantics.

### 2.2 Pure parallelism: `par` and parallel `formap`

```thrax
$ par : ({} -> b) -> Future b = ...          # the argument must be PURE (empty row)
$ get : Future b -> b = ...
```

plus parallel instances of `formap` for `@vec` and the LA tensors.

- The argument's row must be empty. Pure code gives the same result on any
  core in any order, so parallelism never changes meaning.
- The interpreter may run `par` sequentially and remains the reference.

C runtime work:

- **Per-thread machine state.** `g_kont`, `g_act`, `g_v`, `g_fn`, `g_arg`,
  `g_blk`, `g_result`, `g_pool`, `g_dead` become `_Thread_local`.
  `g_names` is either built before threads start or guarded.
- **Shared marking (Koka/Lean).** Values local to one thread keep plain
  counts. When a value is handed to another task it is traversed and marked
  shared, and marked values use atomic increment and decrement. A value with
  `rc == 1` is **moved**, not shared: no marking, no atomics. The ownership
  pass (#230) already tells us when that holds.
- **Thunks.** Forcing a shared thunk must be a compare-and-swap with a
  blackhole state, so two threads never both evaluate it, and a waiter blocks
  until the value lands.
- **Leak check.** `g_live` becomes per-thread counters summed at exit, keeping
  the exit-97 leak check exact.
- **Scheduler.** A small work-stealing pool over pthreads (already linked).

### 2.3 Effect rows as the thread-safety check

Rust decides what may cross threads with the `Send` and `Sync` traits. Thrax
can decide it with the effect row instead:

```thrax
$ spawn_par : ({} -> <e> a) -> <Par | e> Future a = ...
```

is accepted only when every effect in `e` is handled by a handler declared
**thread-safe**: the builtin IO effects, a channel handler, the parallel
scheduler itself. Ordinary handlers, such as a `State` handler, are
**thread-local**. An effect handled by one must be discharged inside the task,
so a task can never perform an operation against another thread's handler.

This gives race freedom without new type-level machinery: the typechecker
already computes and unifies rows. Pure tasks (section 2.2) are the special
case `e = <>`.

### 2.4 Shared mutable state, later and only if needed

1. **Channels first.** Message passing fits immutable data, and with
   uniqueness a send is a move, so it costs nothing.
2. **STM as a library effect.** A transaction is a function with row `<Stm>`
   and no `<@io>`, run by an `atomically` handler that logs reads and writes
   and retries on conflict. The row system then rejects IO inside a
   transaction, which is exactly the property Haskell's design rests on.

STM is not the foundation: it solves contention on shared mutable state, which
Thrax does not have until it adds mutable cells.

### 2.5 `@io` stays one effect

`@io` is the one builtin effect: the outside world, handled by the runtime
around `@main` and around each `@spawn` task. The entry and build signature
checks match its literal name (`crates/frontend/src/typing/data.rs`:
`ENTRY_SIG`, `BUILD_SIG`). A split by resource (`@console`, `@fs`, `@net`,
`@env`, `@time`, `@random`, `@ffi`, with `@io` as a row alias for all of them)
was implemented under #284 and reverted under #297:

- The resources were arbitrary. They followed libc's grouping, and nothing in
  the compiler or runtime treated one differently from another, so they were
  unchecked documentation.
- They were only as accurate as the bindings, which work at the level of C:
  `C.fputs` to stdout counted as `@fs`, while `C.write` and `C.popen` had to
  stay `<@io>`.
- Only the runtime author can add a builtin label, so the set could never
  cover a GPU, audio or sockets.

The extensible route is an ordinary effect whose handler discharges into
`@io`:

```thrax
$ Gpu : @effect = draw : Mesh -> {}, present : {} -> {}

$ run_gpu : ({} -> <Gpu | e> a) -> <@io | e> a = \body =
	do body {}
	ctl k
	| Gpu.draw m => (raylib_draw m; k {})
	| Gpu.present u => (raylib_present {}; k {})
	| r => r
```

It is precise (`<Gpu>` code can only draw), handleable (a test swaps the
handler), and needs no special rule for `@main`. Aliases of `@io` such as
`$ Console : @alias = <@io>` are rejected for the library: an alias is
transparent, so `<Console>` would check nothing while reading as a
restriction.

A builtin label comes back only when the compiler or runtime consumes it, for
example `@build` rejecting nondeterminism, or `@spawn` rejecting thread-unsafe
C state such as `rand`.

---

## 3. Rejected

- **Locks and mutexes as the primary API.** They do not compose, they
  deadlock, and they protect shared mutable state Thrax does not have. They may
  appear later inside library handlers, never as the user-facing model.
- **STM as the foundation.** Right tool for shared mutable cells, which come
  last if at all (section 2.4).
- **A tracing garbage collector.** Thrax is refcounted by design
  (effect-system-design.md, section 0). Shared marking keeps refcounting viable
  across threads.
- **Atomic counts everywhere (Swift).** Taxes all single-threaded code for the
  benefit of the few shared values.
- **Per-thread heaps with copying messages (Erlang).** Uniqueness already makes
  most transfers moves, and shared immutable data needs no copy.

---

## 4. Open questions

- **Cross-thread resume.** Can a continuation captured on one thread be
  resumed on another? If so, its frames must be marked shared, and the
  multi-shot copy rule (`seg->rc == seg->nbound + 1` means move) must use the
  shared counts.
- **`defer` and cancellation.** When a task group is cancelled, the `defer`
  cleanups of its abandoned tasks must run exactly once, in the right thread.
- **Scheduler fairness.** Cooperative scheduling (section 2.1) only switches at
  `yield` and IO; a long pure loop starves its siblings. Options: compiler
  inserted yield points, or pushing heavy pure work to `par`.
- **Declaring a handler thread-safe.** Syntax and checking for section 2.3: an
  attribute on the handler, or on the effect declaration.
- **Interpreter.** Running `par` sequentially is enough for the reference
  semantics, but section 2.3 checks are static and must behave identically.
- **Evidence passing (M3.2).** Handler evidence passed in registers interacts
  with per-thread handler stacks; settle together with M3.2.

---

## 5. Order of work

1. `Async` effect and scheduler library (section 2.1). No runtime change.
2. C runtime thread readiness (section 2.2, runtime part).
3. `par`, `Future`, parallel `formap` (section 2.2, surface).
4. Effect rows as the thread-safety check (section 2.3).
5. Channels, then STM (section 2.4).
6. Independently: split `@io` (section 2.5).

---

## 6. Implementation status

### Runtime (#280)

All of it lives in `crates/ccg/src/runtime.c` behind `THRAX_THREADS`. The
emitter defines it only for a program that names `@par`, `@spawn` or `@await`
and targets a platform with threads (not wasm), and then links `-lpthread`. A
sequential program compiles exactly as before, so its timings are unchanged.

- **Per-thread machine state.** The continuation stack, the next-action
  registers, the temp pool, the dead list and the leak counter are
  `_Thread_local` with the local-exec model (the general-dynamic model a PIC
  default picks would call `__tls_get_addr` on every machine step).
- **Three sharing levels** in a byte of the value header (`shared`, beside the
  tag): `SH_LOCAL` (plain counts), `SH_SHARED` (atomic counts) and
  `SH_IMMORTAL` (no counting at all). The closure given to `@par` is marked
  shared with everything it reaches before it is queued; a task's result is
  marked before it is published. Every global is marked immortal when the pool
  starts (and a CAF forced later is immortal at birth): it lives until exit
  anyway, and counting a hot global closure such as a recursive function from
  several cores made the cache line bounce. An immortal value is freed by one
  sweep at exit, so the Show-mode leak check stays exact.
- **Thunks.** Forcing a shared lazy slot swaps `T_THUNK` for `T_BLACKHOLE`
  with a compare-and-swap; the winner evaluates, writes the payload, and
  publishes the new tag with release order. Other threads wait for the tag.
- **Globals.** A CAF is claimed by compare-and-swap. A thread that finds a CAF
  claimed by another thread helps with queued tasks until it lands, because the
  claimant may itself be awaiting one of them. A holding lock deadlocked here.
- **Leak check.** `g_live` is per thread and signed (a block can be freed by a
  thread other than the one that allocated it); each worker folds its count
  into a global at exit.
- **Pool.** One mutex-guarded deque per thread, `THRAX_THREADS` threads in all
  (default: online cores), started on the first `@par`. The owner works
  newest-first, thieves take oldest-first. `@await` runs a task nobody has
  started yet in place, and otherwise helps with queued work, so a pool whose
  every thread awaits cannot deadlock.
- **Continuations do not cross threads.** Marking a captured continuation
  faults: a resume writes its frames and slot boxes in place.

Measured on a 12-thread machine: a CPU-bound parallel reduction scales about
5.8x. A parallel-enabled program run with `THRAX_THREADS=1` is about 5 to 19
percent slower than the same program without `THRAX_THREADS`, depending on the
workload (the extra branch in every retain and release). The tests compile a
parallel program with `-fsanitize=thread` at 2, 4 and 8 threads and require a
clean report.

### Surface (#281, #282)

- Builtins: `@par : ({} -> b) -> @future b` (a pure task),
  `@spawn : ({} -> <@io> b) -> <@io> @future b`, and
  `@await : @future b -> b`. `@future` is an opaque base type.
- `library/PAR.thx`: `Future`, `par`, `spawn`, `get`, `both`, and over
  `@vec` the parallel `map`, `formap` (pure body: a task has no `Loop` to break
  out of its siblings), `map_chunks` and `reduce`.
- The interpreter runs a task where it is spawned and its future is its
  result. It stays the reference semantics.
- **The thread-safety check is the task's row (section 2.3).** For now the only
  thread-safe effect is the builtin `@io`, the one effect no handler on the
  spawning thread serves. Every handled effect must be discharged inside the
  task. Cross-thread handlers (declaring a user handler thread-safe) remain open
  (section 4). A task that performs a handled effect is reported as such, with
  `help:` to handle it inside the task (`tests/diagnostics/TASK_*.thx`).

### `@io` stays one effect (#297)

- The resource split of #284 is reverted (section 2.5). `@io` is a plain
  builtin label again, every `library/C.thx` and `library/IO.thx` binding is
  `<@io>`, and the entry and build checks compare label sets over `@io` itself.
- A `par` task that performs `@io` is reported as "a pure parallel task
  performs effect `@io`", with `help:` to start it with `spawn`.

### `Async` on one thread (#279)

- `library/ASYNC.thx`: the `Async` effect (`spawn`, `yield`, `chan`, `send`,
  `recv`) and `run_async : ({} -> <Async> a) -> Option a`. A deep handler turns
  each task into a `Step` (finished, or stopped at an operation with its
  continuation), and a pure round-robin loop serves the steps. An operation
  that can complete at once resumes the same task; `yield` requeues it; a full
  channel parks its sender and an empty one its receiver (capacity 0 is a
  rendezvous).
- Structured: `run_async` returns once no task in the group can run. `None`
  means `main` never finished, which is a deadlock (every remaining task is
  parked).
- Deterministic, so both engines agree (`examples/STDLIB_ASYNC.thx`:
  producer/consumer, fan-in, interleaving, deadlock).
- Not done: IO readiness through `poll(2)`. A task's row is `<Async>` only,
  because an operation's type cannot carry a row variable, so a task cannot
  perform `@io`; an IO-capable scheduler needs effect declarations that take
  parameters. For the same reason channels carry `@int`. (An operation type
  with a free type variable, `recv : @int -> a`, is accepted today and
  generalized at each use, which is unsound for this purpose.)

