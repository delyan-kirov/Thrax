# Thrax Concurrency and Parallelism Design

**Status:** sections 2.1 to 2.5 implemented, with the gaps listed in
section 6. Tracked by the GitHub issues labelled `multi-threading`.
**Date:** 2026-10-18.
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

plus parallel instances of `formap` for `@Vec` and the LA tensors.

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
   and no `<@IO>`, run by an `atomically` handler that logs reads and writes
   and retries on conflict. The row system then rejects IO inside a
   transaction, which is exactly the property Haskell's design rests on.

STM is not the foundation: it solves contention on shared mutable state, which
Thrax does not have until it adds mutable cells. Its transactional variables
(`@TVar`) are those cells, and the only ones: a `TVar` is written only by a
committed transaction.

### 2.5 `@IO` stays one effect

`@IO` is the one builtin effect: the outside world, handled by the runtime
around `@main` and around each `@spawn` task. The entry and build signature
checks match its literal name (`crates/frontend/src/typing/types.rs`:
`ENTRY_SIG`, `BUILD_SIG`). A split by resource (`@console`, `@fs`, `@net`,
`@env`, `@time`, `@random`, `@ffi`, with `@IO` as a row alias for all of them)
was implemented under #284 and reverted under #297:

- The resources were arbitrary. They followed libc's grouping, and nothing in
  the compiler or runtime treated one differently from another, so they were
  unchecked documentation.
- They were only as accurate as the bindings, which work at the level of C:
  `C.fputs` to stdout counted as `@fs`, while `C.write` and `C.popen` had to
  stay `<@IO>`.
- Only the runtime author can add a builtin label, so the set could never
  cover a GPU, audio or sockets.

The extensible route is an ordinary effect whose handler discharges into
`@IO`:

```thrax
$ Gpu : @effect = draw : Mesh -> {}, present : {} -> {}

$ run_gpu : ({} -> <Gpu | e> a) -> <@IO | e> a = \body =
	do body {}
	ctl k
	| Gpu.draw m => (raylib_draw m; k {})
	| Gpu.present u => (raylib_present {}; k {})
	| r => r
```

It is precise (`<Gpu>` code can only draw), handleable (a test swaps the
handler), and needs no special rule for `@main`. Aliases of `@IO` such as
`$ Console : @alias = <@IO>` are rejected for the library: an alias is
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

Settled under #282 (section 6 has the details):

- **Cross-thread resume.** Yes. A continuation that crosses to another thread
  is marked like any value; marking flags the slice, which is then always
  copied on resume (never moved), and its counts are atomic.
- **`defer` and cancellation.** Tasks are never cancelled, and a program
  waits for its `spawn` tasks before it exits, so a task's `defer` cleanups
  run exactly once, on the thread that runs the task.
- **Declaring a handler thread-safe.** No declaration: a task takes its
  handlers with it (`PAR.spawn_with handler body`), and a handler is ordinary
  code that runs on the task's thread. Only `@IO`, which the runtime serves on
  every thread, reaches a task from outside. A builtin label that other
  threads may serve is added only when the runtime serves one.
- **One primitive for `@par` and `@spawn`.** Kept as two: they admit
  different rows, and only `spawn` tasks are waited for at exit.

Still open:

- **Scheduler fairness.** Cooperative scheduling (section 2.1) only switches at
  `yield` and IO; a long pure loop starves its siblings. Options: compiler
  inserted yield points, or pushing heavy pure work to `par`.
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
6. Independently: split `@IO` (section 2.5).

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
- **The level is also the sign of the count** (Lean 4's encoding, #293):
  `rc > 0` is a local count, `rc < 0` a shared one, `rc == 0` immortal. The
  fast path of retain and release is one relaxed load and one compare, and
  only a shared or immortal value takes the out-of-line slow path, where the
  `shared` byte decides. A shared value later made immortal keeps its negative
  count, because other threads may be counting it at that moment; the
  reference its global holds keeps the count from reaching zero. The `shared`
  byte is read and written with relaxed atomics for the same reason.
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
- **Continuations cross threads by copy (#282).** Marking a captured
  continuation marks everything its frames hold, and the slices of
  continuations captured inside it, and sets the slice's `shared` flag. A
  resume of a shared slice always splices a copy (the multi-shot path) and
  never moves the frames, which another thread may be reading. A slice's
  counts are atomic in a threaded build.
- **Exit.** A `@main` program waits for every `@spawn` task to finish before
  it exits, helping with queued work meanwhile. `@par` tasks nobody awaits
  are dropped: they are pure, so their results were unobservable.

Measured on a 12-thread machine: a CPU-bound parallel reduction scales about
5.8x. A parallel-enabled program run with `THRAX_THREADS=1` is about 1 to 4
percent slower than the same program without `THRAX_THREADS`, depending on the
workload (5 to 19 percent before the sign encoding of #293). Still open from
#293: passing the machine state as a context pointer instead of thread-locals,
moving rather than marking a value handed to a task with `rc == 1`, and
borrowed reads of shared data, which would cut atomic contention while running
in parallel. The tests compile a
parallel program with `-fsanitize=thread` at 2, 4 and 8 threads and require a
clean report.

### Surface (#281, #282)

- Builtins: `@par : ({} -> b) -> @Future b` (a pure task),
  `@spawn : ({} -> <@IO> b) -> <@IO> @Future b`, and
  `@await : @Future b -> b`. `@Future` is an opaque base type.
- `library/PAR.thx`: `Future`, `par`, `spawn`, `get`, `both`, and over
  `@Vec` the parallel `map`, `formap` (pure body: a task has no `Loop` to break
  out of its siblings), `map_chunks` and `reduce`.
- `library/LA.thx`: `map` over a tensor's leading axis, and its parallel
  counterparts `par_map` and `par_map_chunks` (#294), which split that axis
  into O(1) slices, map each slice in a `@par` task, and assemble the result
  with `create`. About 6x on 12 threads for a CPU-bound map.
- The interpreter stays the reference semantics, on one thread. A `@par`
  task runs where it is spawned, and its future is its result. A `@spawn`
  task runs lazily: when it is awaited, when another task would block on a
  channel or a retrying transaction, or before the program exits. A task
  waiting on a task that is itself suspended below it on the interpreter's
  stack is reported as a deadlock, though threads would make progress (for
  example two tasks that exchange several messages back and forth).
- `PAR.par_with` / `PAR.spawn_with` run a task under a handler of its own
  (section 4).
- **The thread-safety check is the task's row (section 2.3).** For now the only
  thread-safe effect is the builtin `@IO`, the one effect no handler on the
  spawning thread serves. Every handled effect must be discharged inside the
  task, which `par_with`/`spawn_with` do (section 4). A task that performs a handled effect is reported as such, with
  `help:` to handle it inside the task (`tests/diagnostics/TASK_*.thx`).

### Channels and STM (#283)

- Builtins, all `<@IO>`: `@chan_new`, `@chan_send`, `@chan_recv` over an
  opaque `@Chan a`, an unbounded channel (a send never waits, a receive
  waits); `@tvar_new`, `@tvar_read` over `@TVar a`; and the transaction log,
  `@stm_begin`, `@stm_read`, `@stm_write`, `@stm_commit`, `@stm_wait`,
  `@stm_save`, `@stm_rollback`.
- Runtime: a channel is a mutex-guarded ring; a sent value is marked shared,
  since any thread may receive it. A receiver with nothing to take helps with
  queued tasks, then sleeps on the pool's condition variable, which every
  send broadcasts. A transaction logs each read with the version it saw and
  buffers its writes in a thread-local log; a commit validates the reads and
  publishes the writes under one global lock. `@stm_wait` (retry) moves the
  read set out of the log, since a task it runs while helping uses the same
  log, and waits until a version changes. Without any other task, a receive
  on an empty channel or a retry is a deadlock fault.
- `library/PAR.thx`: `Channel`, `channel`, `send`, `recv`.
- `library/STM.thx`: `TVar`, the `Stm` effect (`read`, `write`, `retry`, plus
  `save`/`rollback` for `or_else`), `modify`, `or_else`, and `atomically`, a
  handler that runs the transaction against the log, commits, and reruns it
  on a conflict or after a retry. A transaction's row is `<Stm>`, so
  `@IO` in one is a compile-time error
  (`tests/diagnostics/STM_IO_IN_TRANSACTION.thx`).
- The operations are polymorphic in the variable's type (`read : TVar a ->
  a`), which effect typing allows since #291: a handler clause sees such a
  type as opaque, and `atomically` only passes values through.
- `examples/STDLIB_CHANNELS.thx` (a worker pool) and
  `examples/STDLIB_STM.thx` (transfers from six tasks, a `retry` waiting for
  deposits, `or_else` undoing writes); both perform `@IO`, so the corpus runs
  them rather than a compile-time assert. Clean under `-fsanitize=thread` at
  2, 4 and 8 threads.

### `@IO` stays one effect (#297)

- The resource split of #284 is reverted (section 2.5). `@IO` is a plain
  builtin label again, every `library/C.thx` and `library/IO.thx` binding is
  `<@IO>`, and the entry and build checks compare label sets over `@IO` itself.
- A `par` task that performs `@IO` is reported as "a pure parallel task
  performs effect `@IO`", with `help:` to start it with `spawn`.

### `Async` on one thread (#279, #291, #292)

- `library/ASYNC.thx`: the effect `Async m e` (`spawn`, `yield`, `chan`,
  `send`, `recv`, `wait_fd`) and `run_async : ({} -> <Async m e | e> a) -> <e>
  Option a`. A deep handler turns each task into a `Step` (finished, or
  stopped at an operation with its continuation), and a round-robin loop
  serves the steps. An operation that can complete at once resumes the same
  task; `yield` requeues it; a full channel parks its sender and an empty one
  its receiver (capacity 0 is a rendezvous).
- The effect's parameters (#291) are fixed by the handler for the whole group:
  `m` is the message type of its channels (`Chan m`), and `e` the effects a
  task may perform besides `Async`, which flow out to the handler around
  `run_async`. A task can therefore perform `@IO`, or any effect handled
  outside the group.
- IO readiness (#292): `wait_fd {fd, events}` parks a task until the
  descriptor is ready. `run_io` runs a group whose tasks may perform `@IO`;
  when no task is ready and some wait on descriptors, it sleeps in poll(2)
  until one is. Because the FFI does not read the `pollfd` array back, it then
  probes each parked descriptor with a zero-timeout poll(2) to see which woke.
  `run_async` polls nothing, so a task waiting on a descriptor there counts as
  blocked.
- Structured: a group returns once no task in it can run. `None` means `main`
  never finished, which is a deadlock (every remaining task is parked).
- Deterministic apart from descriptor readiness, so both engines agree
  (`examples/STDLIB_ASYNC.thx`: producer/consumer, fan-in, interleaving,
  typed messages, a task effect served outside the group, deadlock;
  `examples/STDLIB_ASYNC_IO.thx`: a task reading a pipe while another
  computes, checked at run time because it performs `@IO`).
- Open: channels and tasks of one group share one message type `m` (a group
  that needs several uses a union); channels between OS threads (#283).
