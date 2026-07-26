# Deferred design: ranges, codata, and the linear-algebra extension

**Status: DEFERRED.** Nothing here is scheduled. This note records how three
future features must compose, so that the near-term surface work does not
foreclose them. One decision is settled: positional `.n` (#10) is tuple/struct
projection ONLY; sequence/tensor/map indexing is a separate operation, spelled
`.[..]`, and belongs to this subsystem (see "Indexing surface" below).

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
  constructors" (see doc/effect-system-design.md and the sum-types work), i.e.
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
    with rc and rc==1 in-place, see doc/strings-and-arrays.md).
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

## What to lock now (cheap forward-compat, even while deferring)

Even before building the subsystem, keep these true so the near-term surface work
does not foreclose it:

- `Range` is a real small type, not a desugar-to-list. Enumeration to a sequence
  is an explicit or contextual conversion.
- Indexing is spelled `.[..]` and desugars to an overloadable `index` call,
  type-directed on the index type from the start (Int vs Range vs tuple).
- Keep `.n` for projection only; never route sequence indexing through it.
- The array view representation carries `offset` + per-axis `(variance, dim,
  stride)`. Variance is a tag carried even under dynamic-rank typing.
- `..=` desugars via a library function (not a core primitive), so its meaning
  can become a resolution target (List / Array / Range / view) later.

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

- Done: positional `.n` projection (#10, tuple/struct only).
- Deferred, and to be done together for composition: `.[..]` indexing surface +
  overloadable `index`, first-class `Range` type, buffer/view/stride tensor
  representation with per-axis variance, type-directed range-indexing, COW, and
  the `data`/`codata` split.
