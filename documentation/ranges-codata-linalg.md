# Deferred design: ranges, codata, and the linear-algebra extension

**Status: SHIPPED.** Ranges (pattern form) and codata are done (see their
memories). The LA extension has landed through **increment 4**: sized tensors
(increment 1), a strided data plane (2a), O(1) views (2b), multi-axis slicing with
inclusive `...` ranges (3), and per-axis variance (4). The ops are library Thrax
over a small set of `@tensor_*` primitives. No LA item remains open; the deferred
work below (expression-form ranges, COW, static shape, data/codata) is separate
future work, not part of the LA capstone. Timeline of increment 1 (2026-08-09):

- `[n]T` is a rank-1 sized vector; `n` is a type-level natural, a **distinct KIND**
  from ordinary types (`Type::Nat` + a `nat_vars` set in the engine; a size unifies
  only with a size). Answers the doc's "static shape needs type-level computation
  Thrax lacks today."
- The design choice (per the user): **modular everything.** Indexing `t.[i]` is
  TOTAL and MODULAR (`i mod n`), so no bounds proofs and no partiality; the size
  arithmetic will be modular (Z/2^64) too when it lands. This keeps the whole thing
  in decidable equational-ring land, no `<` constraints, no dependent proofs.
- Frontend-only: `[n]T` erases to the existing `%vec` vector; `t.[i]` lowers to
  `vec_get t (i % vec_len t)`. No new runtime kind. `[m][n]T` nests as vectors of
  vectors for now (NOT the flat buffer+strides view yet).
- **Phase A (nat unification) shipped**: literals and size variables unify
  (`[3]Int`, `first : [n]a -> a`).
- **Phase B (modular type-level arithmetic) also shipped** (2026-08-09):
  `[n+m]T`/`[n*m]T`, modular over Z/2^64. Equality is a canonical polynomial normal
  form (`Type::NatAdd`/`NatMul`; `normalize_size`/`unify_size` in engine.rs), so
  `[n+m] == [m+n]` and `[n+n] == [2*n]`. Unification is forward-eval only (ground
  compare + lone-var bind; no back-solving `n+1 == 5`), which keeps it decidable, no
  `<`, no SMT. A `concat : [n]a -> [m]a -> [n+m]a` builtin (vector append, both
  engines) demonstrates it end to end.
- **LA operations shipped** (2026-08-09): `transpose`, `matmul` (shared `k`
  unifies, so a dimension mismatch is a compile error), `dot`, `concat`, `slice`,
  `row`/`col`, over Int and Real. **`.[..]` is the OVERLOADABLE `index` the doc
  envisioned**: `t.[i]` desugars to `index t i`, an overloaded function with a
  tensor candidate and user-addable candidates (`index : Grid -> Int -> Int`,
  `index : Map k v -> k -> v`), so custom containers use `.[..]` with no compiler
  change. `[m, n]T` shape sugar (== nested `[m][n]T`).
- **De-magicked into the library** (2026-08-12): the ops are NOT built-ins. The
  compiler keeps only a handful of `@`-primitives over the tensor buffer
  (`@tensor_index`/`@tensor_length`/`@tensor_create`/`@tensor_concat`/`@tensor_slice`/
  `@tensor_transpose`, plus `@tensor_index_axis`/`@tensor_slice_axis` for the
  multi-axis form); `transpose`/`matmul`/`dot`/`concat`/`slice`/`row`/`col` all live
  in `library/LA.thx` as ordinary Thrax. Element arithmetic is passed as `@ctx`
  implicits (`@ctx { add, mul, zero }`), not a Num class, so `matmul`/`dot` are
  element-generic over Int and Real by supplying the dictionary; the runtime does no
  arithmetic on its own.
- **Strided data plane shipped** (increment 2a): `[n]T` is a `@tensor`-named struct
  `{ buf, off, shape, strides }` over a flat, refcounted buffer. Both engines
  byte-identical and leak-clean.
- **O(1) views shipped** (increment 2b): `transpose` is a stride swap; `row`/`col`/
  `slice` share the buffer and copy nothing. Views alias the same buffer (no COW
  yet; see below).
- **Multi-axis slicing shipped** (increment 3): `t.[s0, s1, ..]` where each slot is
  an index `i` (REDUCES its axis), an inclusive range `p ... q` (RETAINS it), or `..`
  (retains the whole axis). `m.[.., j]` = column, `m.[p...q, r...s]` = subblock,
  shape-checked (an `Expr::Slice` typed node lowering to `@tensor_index_axis`/
  `@tensor_slice_axis` O(1) view prims). Range syntax is `...` (inclusive), matching
  range patterns.
- **Per-axis variance shipped** (increment 4, 2026-08-13): each tensor axis carries
  a variance tag, spelled `@contra` (upper/contravariant, a vector index) and `@co`
  (lower/covariant, a covector index); a bare axis is `Neutral`. Surface:
  `[@contra m, @co n]Int` (the `@`-sigil matches the other type-level intrinsics).
  Internally the tag rides the `@tensor` spine as a nullary con (`@tensor <variance>
  size elem`). Unification is variance-compatible: `Neutral` is a wildcard (so plain
  `[n]T` code interoperates), `@co` and `@contra` clash. `matmul` is retyped
  `[@contra m, @co k]a -> [@contra k, @co n]a -> [@contra m, @co n]a`, so it
  contracts a `@co` axis against a `@contra` axis (the Einstein-summation rule) and
  a flipped-variance factor is a type error, not just a shape mismatch. The
  `@tensor_*` primitives are variance-polymorphic; the other library ops stay
  neutral and still work on any variance. `examples/TENSORS.thx` (the variance
  block).
- **Still open (separate from LA):** COW value semantics (views currently alias, no
  copy-on-write on rc>1 writes), static result-shape typing (rank is checked but not
  fully folded over the index), and indexed-write/lens spelling.

The rest of this note is now split: the data-plane, indexing, and view design
below is **as-built** (kept for reference); variance, ranges-as-slice-descriptors,
and the data/codata split remain unbuilt design. One decision is settled:
positional `.n` (#10) is tuple/struct projection ONLY; sequence/tensor/map indexing
is the separate `.[..]` operation and belongs to this subsystem.

## Why capture this now

Ranges, codata, and a linear-algebra (LA) layer look like three separate
features, but their designs constrain each other:

- If ranges ship as throwaway eager-list sugar, LA slicing cannot reuse them and
  we get a syntax fork later.
- If laziness stays baked into ordinary data constructors (today's model), the
  cost model for both streams and dense arrays stays murky.
- If dense tensors are built on the wrong (lazy) representation, LA performance
  is lost from the start.

So the three should be designed together and, when built, the LA-facing parts
built together. Decide the shape now; implement later.

## The three pieces

### 1. Ranges

> **As-built note:** the spelling that shipped is `...` (inclusive), not `..=`.
> Range PATTERNS (`is lo ... hi`) and index-position ranges (`m.[p ... q]`) both use
> `...`. The `..=`/`..<`/`..>` design below predates that choice; read `...` for
> `..=` wherever an inclusive range appears. Exclusive/descending forms remain
> unbuilt.

- Surface: `..=` (inclusive), `..<` (exclusive), `..>` (descending-inclusive).
  Safe because bare `<`/`>` are type-level-only (effect rows) and `.` is a
  standalone delimiter, so `..` never fuses. One lexical caveat: the trailing
  `=`/`<`/`>` maximal-munches with an adjacent operator char, so a negative
  endpoint needs a space (`1 ..> -3`, not `1 ..>-3`).
- Ranges are NOT eager list sugar. A `Range` is a first-class small descriptor
  `{ start, stop, step }`. It is type-directed on how it is used:
  - sequence-materializing context: enumerate to `List`/`Array` (reuses the
    existing type-directed `[..]` lit-site mechanism),
  - index context: consumed as a slice descriptor (never materialized),
  - infinite range: codata stream (see below).
- Range patterns are bare (`when n is 1 ..= 5`), a refutable interval test
  lowering to `lo <= x && x <= hi`; they contribute nothing to exhaustiveness.
  Range expressions may be bracket-delimited (`[1 ..= 10]`) to mean "materialize
  this range into a sequence". The pattern/expression asymmetry is intentional:
  brackets mean "build a sequence", bare-in-a-pattern means "interval test".
- STEP is required eventually (strided views, `start .. step .. stop`); leave
  room. `..=`/`..<`/`..>` do not express it.

### 2. Codata
- Data is inductive (constructors, pattern matching, finite, strict). Codata is
  coinductive (observations/destructors, projection, potentially infinite,
  lazy-by-construction).
- Opportunity, not just a new type: Thrax today is "CBV except lazy data
  constructors" (see documentation/effect-system-design.md and the sum-types work), i.e.
  lazy DATA, which is the source of murky cost. The principled cleanup is a
  `data`/`codata` split: make `data` strict (predictable cost, refcount-in-place
  friendly) and confine ALL laziness behind explicit `codata`, visible in the
  type.
- Lightweight here: Thrax is not a proof assistant, so codata can be lazy
  observation records on the existing thunk machinery. No productivity or
  coinduction totality checker required.
- Overlaps with existing generators (`Yield` effect / coroutines). Generators are
  scoped, effectful, control-flow stream producers. Codata gives streams as
  first-class lazy values (pass around, store, compose, reason via bisimulation).
  Decide whether both earn their place.

### 3. Linear-algebra extension

**Indexing surface (settled):** `.[..]`, not `.n`. `.` becomes a uniform access
prefix disambiguated by the next token: `.n`/`.field` = static projection
(tuples/records, heterogeneous, compile-time), `.[..]` = indexing (dynamic,
overloadable), `.{..}` = struct literal. The leading `.` is what makes bracket
indexing unambiguous vs list literals `[a,b]` and juxtaposition application
`f [x]`, with no space-sensitivity. Kept distinct from `.n` on purpose: `.n` is
heterogeneous static projection; indexing is homogeneous, dynamic, and must grow
a runtime index, multi-dim, ranges, and user containers.

`m.[i, j]` desugars to `index m {i, j}` (the comma-list builds a tuple index;
`m.[k]` is `index m k`, no 1-tuple). `index` is an OVERLOADABLE call resolved
type-directionally on (container, index type) via the existing overloading
machinery (no typeclasses needed) -- this is the "compiler interface custom types
tap into". Built-in impls: `Array`/`Vec`/`Str` (`index _ Int -> elem`), `Map k v`
(`index _ k -> v`, so `map.["key"]` just works), `Tensor` (`index _ {Int..} ->
Scalar`, `index _ {Range/.. ..} -> view`).

- Mixed indexing (`m.[n, p ..= q, ..]`) is allowed and central: an `Int` slot
  REDUCES its axis (dropped from the result), a `Range`/`..` slot RETAINS it, so
  result rank = number of Range/`..` slots. `..` alone = whole axis (full-extent
  range), giving rows (`m.[i, ..]`) and columns (`m.[.., j]`). Inside `.[..]` a
  range is always a descriptor, never materialized.
- Variance (co/contra) is NOT at the index site. It is a per-axis property of the
  tensor TYPE (`Axis = Up Nat | Down Nat`; `Vec n = Tensor [Up n]`, `Covec n =
  Tensor [Down n]`, `Matrix m n = Tensor [Up m, Down n]`). `index` is
  variance-blind (returns a component); variance governs which OPERATIONS
  type-check (`matmul` pairs an `Up` k with a `Down` k = contraction; `transpose`
  flips; raising/lowering needs a metric). Einstein/named-index summation, where
  variance WOULD appear syntactically, is a separate larger DSL, deferred; the
  variance-in-type baseline is forward-compatible with adding it.
- Result-shape typing is the expensive part (result rank depends on the Int-vs-
  Range pattern of the index). Baseline: dynamic rank (`index` returns `Tensor`,
  or `Scalar` when the overload sees an all-`Int` tuple), variance still checked
  as a per-axis tag. Static shape (fold the index over the axis list) is an
  additive upgrade needing type-level computation Thrax lacks today.
- "Slices for free": indexing with a `Range` returns a VIEW, not a copy. It is
  the same `index` primitive, one overload keyed on the index type.
- Mechanism: split storage from shape.
  - Buffer: refcounted contiguous storage (already exists: the `%vec` byte-vector
    with rc and rc==1 in-place, see documentation/strings-and-arrays.md).
  - Array/Tensor is a view: `{ buffer(strong ref), offset, shape[], strides[] }`.
  - A slice is a new view over the same buffer (rc 1 becomes 2), O(1), no copy.
- Strides are the LA payoff, all O(1): row, column, transpose (swap
  shape/strides), diagonal (stride = ncols+1), subblock. Carry `strides` in the
  view from day one even if v1 uses unit stride only.
- Value semantics via copy-on-write, driven by refcounting: rc==1 mutates in
  place; a live view makes rc>1 so a write copies. COW falls out of the existing
  rc machinery. Reads through a slice are free (covers most LA: matmul,
  reductions, dot products read views and write a fresh result).
- Codata is the ANTI-use-case here: dense tensors must be strict, contiguous
  buffers for BLAS/cache/SIMD. Codata is for the infinite/iterator side. Keep
  them separate.
- Layering: `Vec T` (growable, owns buffer, 1-D) stays distinct from
  `Array`/`Tensor T` (dense, N-d, non-growable, sliceable, strided). LA lives in
  the tensor type.

## How they compose (the load-bearing constraints)

1. One `..=` surface, three roles, type-directed: materialize to data
   (List/Array), stay a `Range` descriptor (as a slice index), or observe as
   codata (infinite range as a stream). The descriptor is the common core.
2. Indexing is one overloadable, type-directed primitive (`index`, surface
   `.[..]`) keyed on the index type: Int gives an element (and reduces its axis),
   Range/`..` gives a slice/view (and retains its axis). #9 (ranges) is the
   Range half of it; positional `.n` (#10) is the separate projection operation,
   not part of this.
3. Codata clarifies where laziness lives (streams); strict data + views serve
   dense LA. Ranges bridge them: finite is data, infinite is codata, as-index is
   a descriptor. The data/codata split gives each a clear cost.
4. COW value semantics via rc keeps read-slicing free without introducing
   aliasing into the functional core.

## What was locked (forward-compat kept while the rest is deferred)

These were the "keep true so the surface work does not foreclose the subsystem"
constraints. Status now:

- **DONE.** Indexing is spelled `.[..]` and desugars to an overloadable `index`
  call, type-directed on the index type (Int reduces an axis, a range/`..` retains
  it).
- **DONE.** `.n` stays projection only; sequence indexing never routes through it.
- **DONE.** Variance is a per-axis tag, but it lives in the TYPE (`@tensor
  <variance> size elem`), not the runtime view, since it is erased before runtime
  (the runtime is variance-blind, as intended). The view still carries `offset` +
  per-axis `(dim, stride)` only.
- **PARTIAL.** Ranges in index position are lowered directly to the axis-slice
  prims rather than reified as a first-class `Range` descriptor. A standalone
  `Range` type (and `[lo ... hi]` materialization via a library function) is still
  the deferred expression-form ranges feature.

## Open decisions to settle before implementing the subsystem

- COW value semantics vs aliasing mutable views (recommend COW default, aliasing
  only as an opt-in imperative escape hatch).
- Result-shape typing: dynamic rank (baseline) vs static shape (fold index over
  the axis list; needs type-level computation).
- Update/lens spelling for indexed writes (`m.[i, j] := v`?).
- Whether to layer Einstein/named-index summation on top (where variance markers
  would become syntactic) or stay with explicit typed ops.
- Where a range materializes: bracket `[a ..= b]` as a sequence vs bare
  `a ..= b` as a `Range` descriptor, reconciled type-directionally.
- Scope of the data/codata split (full evaluation-model change vs a `Stream`-like
  add-on).
- Codata vs the `Yield`-effect generators: replace, coexist, or unrelated.
- STEP/stride syntax for ranges (`by`, or `start, next .. stop`).

## Sequencing

- **Done:** positional `.n` projection (#10, tuple/struct only). `.[..]` indexing
  surface + overloadable `index`. Buffer/view/stride tensor representation
  (increment 2a). O(1) views: transpose/row/col/slice (increment 2b). Multi-axis
  slicing with inclusive `...` ranges in index position (increment 3). Range
  PATTERNS (`is lo ... hi`). Element-generic ops via `@ctx` dictionaries. Per-axis
  variance `@contra`/`@co` with a variance-typed `matmul` (increment 4). The LA
  capstone is complete.
- **Done (separate from LA):** expression-form ranges `[lo ... hi]` as a
  TYPE-DIRECTED literal (`Expr::Range`): materializes to a sized tensor `[n]T` when a
  tensor is expected and the bounds are literals (static length), else a `List Int`
  (default). A non-literal bound against a sized tensor is a compile-time error. See
  [[range-patterns]].
- **Done (separate from LA): OPEN ranges `[lo ...]` as codata streams.** An open
  range (no upper bound) is infinite, so it always builds a `Stream Int` (lowered to
  the canonical `CORE.count_from lo`); against a `List` or sized tensor it is a type
  error. This is the "first-class lazy Range", realized by making the literal target
  codata: laziness = target a `Stream`, no separate `Range` type. Open range PATTERNS
  `is n | lo ...` ship too (match `lo <= n`, one comparison). `Stream`/`count_from`
  are canonical in `CORE`. `Expr::Range.hi`/`Pattern::Range.hi`/`Pat::Range.hi` are
  optional.
- **Deferred (separate from LA):**
  - Array/Vec targets for the closed form, riding the same `Expr::Range` node.
  - A compile-time-CONSTANT (non-literal) bound against a sized tensor (`let a = 4
    in [1 ... a]`): needs a const-eval pass; currently errors.
  - Step/stride and descending ranges.
  - COW value semantics (views alias today; rc>1 writes should copy).
  - Static result-shape typing (fold the index over the axis list).
  - The `data`/`codata` split and its interaction with streams.
