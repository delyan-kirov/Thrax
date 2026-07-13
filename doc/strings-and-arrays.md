# Strings & Arrays

## Scope decision: `Array` is byte-only; generic vectors come later

`Array` (and `Str`) are **byte vectors** (`Vec<u8>`). A
generic, element-typed container (`Array Int`, `Array Str`, ...) is a SEPARATE
future type, tentatively `Vector T`, with a **boxed** runtime rep (one `Value*`
per element). It is deliberately NOT folded into `Array`: a generic element type
forces boxing, but `Str`/`Array` must stay byte-packed for UTF-8 and so FFI can
pass them as `char*`. So the two cannot share one layout. `Vector T` (boxed
growable vector, its own `vector_*` ops) is deferred; `Array` stays byte-only.

## Goal

Turn `Str`/`Array` from an opaque immutable byte block into a growable byte
vector (`Vec<u8>` with `len` and `cap`), UTF-8 for `Str`, "like Rust `String`"
ergonomically but **without** giving up value semantics. Concretely:

- `++` concatenation (`Str -> Str -> Str`, later `Array -> Array -> Array`).
- Growable operations via built-ins: `array_push`, `array_get`, `array_set`,
  `array_len`, `array_cap`, ...
- Pattern matching on strings (exact literal + literal prefix) and on arrays
  (fixed length + a rest binding).

## Mutation model: opportunistic in-place (rc == 1)

The language is pure and reference-counted (see `native-backend.md`, the RC work
in #70). We keep it pure. A mutating built-in (`array_push`, `array_set`, `++`)
**mutates its buffer in place when the value's refcount is 1, and copies-then-
mutates otherwise**, "functional but in-place" (FBIP), as in Roc and Koka.

Why this and not unconditional in-place:

- **Value semantics are preserved.** No two observers ever disagree about a
  value, so the type system, equational reasoning, the
  algebraic-effect machinery stay sound.
- **It composes with effects for free.** When a continuation captures an array,
  the captured environment retains it -> refcount > 1 -> the uniqueness check
  fails -> the op copies. The same reference counting that manages memory makes
  mutation safe across `ctl`/resume boundaries. (Resumptions here are affine, so
  even the multi-shot hazard is bounded, but we do not rely on that.)
- **We already have the substrate.** RC is in place; opportunistic mutation is
  its payoff. In the common linear case (build a string with repeated `push`,
  never alias it) every op is genuinely in-place.

Cost, accepted knowingly: no *guarantee* of in-place (an accidental extra
reference silently downgrades to a copy, a performance cliff, not a bug), and
the in-place hit-rate is only as good as RC drop-precision. Guaranteed in-place
(linear/unique arrays, or a `Mut` region effect) is a possible **later** layer
and is explicitly out of scope here. Auditing drop-precision so `push` actually
sees rc==1 is a **follow-up** once functional behaviour is correct.

## Representation

Both `Str` and `Array` remain one runtime kind (the runtime does not distinguish
them; the *type checker* keeps `Str` vs `Array` nominally distinct). Extend the
byte block with a capacity:

- **Interpreter** (`engines/ITxDATA.hpp`): `VStr { std::string val }`. It is the
  **semantic reference** and simply ALWAYS returns a fresh value from a mutator
  (value semantics are identical to in-place, so copying is always correct and
  simplest). `array_cap` reports `std::string::capacity()`.
- **Native** (`platforms/THxVALUE.h`): add `size_t cap` to the `s` arm
  (`{ char *p; size_t n; size_t cap; }`). `THxRT_str`/`THxRT_bytes` set
  `cap = n`. A mutator grows in place (realloc-and-append; there is no
  `THxMEM_realloc`, so alloc+memcpy+free) when `THxMEM_unique(v)`, added to the
  allocator seam: `rc == 1` under the RC engine (every durable holder retains,
  so anything shared is `rc >= 2`; a value still in the temp pool carries the
  pool's extra ref and so reads non-unique, conservative but sound), and NEVER
  under the bump engine (no counts). Otherwise it builds a fresh, higher-capacity
  copy. In-place returns `v` retained, balancing the caller-builtin's later
  release of its operand row. `payload_destroy` already frees `u.s.p`.

`len` and `cap` are surfaced to the language via `array_len` / `array_cap`
built-ins, not as struct fields (the block stays opaque; no user-visible layout).

## Built-ins  *(implemented)*

The byte-vector ops carry ordinary reserved names (not `%`-prefixed): they are
directly user-callable primitives, like `if`. `Str` and `Array` are kept
nominally distinct (Str carries the UTF-8 invariant, `Array` is raw bytes, Rust
`String` vs `Vec<u8>`) but share the one runtime rep, so **each op is
OVERLOADED on both** via `overload_db` (TCxDATA.cpp), resolved by the first
operand's type; a mutator returns the same kind it took. Each op needs: an
overload set (`overload_db`), an impl in IT `impls`, and an arity entry + dispatch
arm in `platforms/THxRT.c`. `%array` (from `@array.{n}`) stays a `%`-internal
`m_prim` primitive. All byte-valued args/results are `Int` (a byte is `0..255`;
`Nat8` later).

| Built-in       | Type (Array shown; also on Str)      | Meaning                                   |
|----------------|--------------------------------------|-------------------------------------------|
| `@array.{n}`   | `Int -> Array`                       | allocate n zeroed bytes (`%array`)        |
| `array_len`    | `Array -> Int`                       | byte length (`n`)                         |
| `array_cap`    | `Array -> Int`                       | capacity                                  |
| `array_get`    | `Array -> Int -> Int`                | byte at index (bounds-checked)            |
| `array_set`    | `Array -> Int -> Int -> Array`       | set byte i (opportunistic in-place)       |
| `array_push`   | `Array -> Int -> Array`              | append a byte (opportunistic in-place)    |
| `array_slice`  | `Array -> Int -> Int -> Array`       | subarray [start, end) (fresh copy)        |
| `++` (`%concat`)| `Array -> Array -> Array` / `StrxStr->Str` | concat; reuse lhs buffer if unique  |
| `?=@Str`       | `Str -> Str -> Int`                  | byte equality (an `?=` overload)          |

> **Overload resolver change.** Overloading the array ops (and nested `++`)
> exposed that `resolve_sites` resolved sites in a fixed order and eagerly
> defaulted unconstrained operands to `Int`. That breaks `a ++ b ++ c` (inner
> must resolve first) *and* `let b = push a .. in push b ..` (outer first),
> opposite orders. It now runs a **fixpoint**: resolve every site whose operands
> are already concrete (no defaulting), repeat while progress is made, then
> Int-default whatever genuinely remains. See `resolve_one_site` /
> `resolve_sites` in TC.cpp.

## `++` concatenation

- Lexer (`LXxDATA.hpp` `operator_db`): add `++`.
- `OP.hpp`: `CONCAT = "++"`; include in `is_operator`.
- `EXxDATA.hpp` `infix_db`: `{ "++", { 18, 19 } }`, left-assoc, looser than
  `+`/`*`, tighter than comparison, so `a ++ b ?= c` groups as `(a ++ b) ?= c`.
- `overload_db` (`TCxDATA.cpp`): `{ CONCAT, { {Str,Str,Str} -> mono(CONCAT,Str),
  {Array,Array,Array} -> mono(CONCAT,Array) } }`. Both resolve to `%concat`.
- IT `impls` + native dispatch: `%concat`.

`<>` stays the empty effect row in type position; `++` is the value operator.

## Pattern matching strings

Two forms in a `when ... is` arm:

1. **Exact**: `is "hello"`. `PatStr` already parses; today `test_of` errors.
   Implement it as `%eq scrut "hello"` (an equality guard), reusing the existing
   literal-arm machinery. Enables string `case`.
2. **Literal prefix**: `is "GET " ++ rest`. Parser: a `PatStr` followed by
   `++ <var/_/pat>` becomes a new `PatPrefix { lit, rest }`. LL lowers it to a
   `%starts_with scrut "GET "` test; on success bind `rest = %slice scrut 4 len`.
   Refutable; falls through like any other refutable arm.

`%starts_with : Array -> Array -> Int` is added alongside the built-ins.

## Sequence literals type-directed `[..]`  *(implemented)*

No new glyph: `[e1, .., en]` is one **sequence literal** whose container is
inferred, a blessed `List` by default (and in a `List`-typed context), an
`Array` (byte vector, elements must be `Int`) in an `Array`-typed context.
Unannotated defaults to `List` (backward-compatible with the list literals).

Because pattern lowering (LL) runs *before* the type checker, the choice must be
deferred: the parser emits `ExSeqLit` (no Cons desugaring); TC types the elements
against one shared type and registers a **lit-site** (like a bare struct/variant
literal) that settles `use` to `List elem` or `Array` and patches
`ExSeqLit::is_array`; **CR** then desugars, `List.Cons/Nil` for a List,
`array_push (.. (%array 0) ..)` for an Array. The `array_push` overload's impl
key *is* `array_push`, so emitting it post-TC dispatches correctly.

## Open list patterns  *(DONE)*

`[p1, .., pn, ..rest]` binds the leading cells and a `rest` tail (the remaining
list), so it matches *at least* n elements; it is pure parser sugar
`[a, ..t]` folds to `a :: t` (open) exactly as `[a, b]` folds to
`a :: b :: Nil` (closed). `..` is two `.` tokens (a lone `.` is a variant tag).
Both engines reuse the existing `List.Cons` nested-pattern machinery.
`examples/LISTS.thx`.

## Pattern matching arrays  *(TODO blocked pre-TC)*

`[..]` *patterns* stay `List` for now (the parser still lowers them straight to
Cons/Nil). Making them type-directed like the literals is **not** symmetric: LL
lowers every pattern into `case`/if-chains *before* TC, so at lowering time the
scrutinee's `List`-vs-`Array` type is unknown.

**The real fix is a pipeline reorder, tracked as its own goal:
`doc/pattern-lowering-after-tc.md` ("be like Haskell" typecheck patterns
first, then compile them to `Case`-trees with types in hand).** Once LL runs
post-TC, array patterns fall out for free and the `ExSeqLit` deferral can go
away. Options short of that:

- Move only seq-pattern lowering after TC (defer the pattern through inference).
- Meanwhile, array destructuring already works via the built-ins + guards:
  `when arr is a if (array_len a) ?= 2 then <array_get a 0 ..>`.

When built, array patterns lower to a length test (`array_len scrut ?= k`, or
`>= k` with a `..rest`), element binds via `array_get scrut i`, and
`rest = array_slice scrut k (array_len scrut)`, nesting through the same
`match_pat` fallthrough as list patterns
(`[[list-literals-and-nested-patterns]]`).

## Phasing (each phase ends green on both engines)

1. **DONE Representation + `array_len`/`array_cap`/`array_get`.** Added `cap`;
   read-only built-ins. `examples/A1` proof; `ARRAY.thx`/`STRINGS.thx` unchanged.
2. **DONE `array_push`/`array_set`/`array_slice` (opportunistic in-place).**
   The mutation core (`THxMEM_unique` gate). `examples/ARRAYS_MUT.thx`.
3. **DONE `++` operator + `%concat` + `?=@Str`.** Concatenation and string
   equality, overloaded on Str/Array. `examples/STR_OPS.thx`. (Also: the
   `resolve_sites` fixpoint rewrite, above.)
4. **DONE Exact string patterns.** `test_of` / `match_pat` emit a `?=@Str`
   test; string matches route to `lower_match_guarded`. `examples/STR_MATCH.thx`.
5. **DONE Literal-prefix string patterns** (`is "GET " ++ rest`). A new
   `PatStrPrefix` (parser: a `Str` literal followed by `++`) lowers, in LL's
   guarded path, to a length-safe prefix test (`?=@Str (array_slice s 0 plen)
   "GET "`, the slice clamps so a short subject just compares unequal) plus a
   tail binding (`rest = array_slice s plen (array_len s)`); the `rest`
   sub-pattern may itself be refutable and nest. `examples/STR_PREFIX.thx`.
6a. **DONE Type-directed `[..]` sequence literals** (List vs Array inferred,
    no `@`). `examples/SEQ_INFER.thx`.
6b. **TODO Array `[..]` patterns** blocked on pre-TC pattern lowering; needs the
    pipeline reorder in `doc/pattern-lowering-after-tc.md` (lower patterns after
    TC), after which it is symmetric with 6a.

Phases 1-4 + 6a (DONE) deliver the core "Rust `String`" experience (grow,
concat, compare, switch) and inferred sequence literals; 5 and 6b are the
richer pattern surface.

## Follow-ups

- **RC drop-precision audit** (deferred, as agreed): the temp pool means a
  freshly-built, uniquely-owned buffer often reads `rc >= 2`, so `array_push`
  copies where it could mutate. Correct, but the in-place hit-rate is lower than
  ideal. Sharpening it (earlier drops / pool-aware uniqueness / a reuse token)
  is separate from the functional behaviour landed here.
- Guaranteed in-place (linear/unique array types or a `Mut` region effect), a
  possible later, opt-in layer; out of scope.

## Testing

Every phase adds/updates an `examples/*.thx` that the interpreter suite
(`build test`) force-evaluates and the native suite (`build native-test`)
compiles+runs, so both engines and the RC leak-check (exit 97) cover it. Prefer
`ck` globals (`if <cond> then 0 else 1/0`) so a wrong answer faults visibly.
