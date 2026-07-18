# Goal: lower patterns *after* type checking (be like Haskell)

Status: **not started** (future work). Motivating blocker: array `[..]` patterns
(`doc/strings-and-arrays.md`, phase 6b).

## The problem

Pattern lowering (`LL`) runs *before* the type checker (`TC`):

```
Parse (EX) -> Lower patterns (LL) -> Link (MR) -> Typecheck (TC) -> Core (CR) -> ...
```

`LL` compiles the rich pattern surface (`when` arms, `[..]`, `::`, structs,
variants) down to a small primitive core `Case` + `if` + `let` +
field-access + `?=` so that no `Pattern` node survives and neither TC nor the
engines need to know about patterns (see `LL.hpp`). That bet holds only while
lowering is **purely structural**: the pattern text alone says how to
destructure (`3` -> `?= 3`; `Person.{x,y}` -> typed field access; `.Cons.{h,t}` ->
tag dispatch).

It breaks for **type-directed pattern syntax** the same surface form that
must desugar *differently depending on the scrutinee's type*. There is exactly
one such form today: `[..]`, which means `List` (cons/nil) on a list and
`array_len`/`array_get`/`array_slice` on an `Array`. Because `LL` runs before
`TC`, it has no type to disambiguate, so `[..]` patterns are hard-wired to
`List` and array patterns are blocked. The literal side hit the same crack one
step earlier and papered over it with the `ExSeqLit` deferral (desugar in `CR`,
post-TC); patterns can't reuse that cheaply because the whole match-compiler
lives in `LL`.

## Why Haskell doesn't have this problem

`case` is *already* a primitive here (`ExCase`/`CaseAlt`, evaluated by both
engines) exactly like GHC Core's `case`. And neither language makes *nested*
patterns primitive: both compile them to trees of one-level `case`s (Wadler's
pattern-match compiler). The **only** difference is ordering. GHC:

```
Parse -> Rename -> Typecheck (patterns intact) -> Desugar to Core case-trees
```

GHC typechecks the rich patterns *first*, then compiles them to case-trees with
full type information in hand which is why `OverloadedLists`, view patterns,
class-method patterns, etc. "just work." We flipped the last two steps.

## The goal

Reorder so pattern compilation has types available:

```
Parse (EX) -> Link (MR) -> Typecheck (TC, patterns intact) -> Lower patterns -> Core (CR) -> ...
```

Keep `Case` as the primitive (already true). Move the `Pattern` -> `Case`-tree
compilation to run **after** TC, so a `[..]` (or any future type-directed
pattern) reads the scrutinee's resolved type and emits the right destructure.

### What has to change

- **TC learns `Pattern`.** Today TC only sees the desugared `Case`; it must
  instead check a `Pattern` against the scrutinee type (bind pattern variables
  at their inferred types, unify literal/constructor patterns with the
  scrutinee, reject ill-typed arms). This is the bulk of the work and is
  well-understood (standard bidirectional pattern checking).
- **`LL` shrinks and moves post-TC.** It stops guessing types; the match
  compiler (`match_pat`, `lower_match*`, `test_of`, `bind_pattern`) runs with
  resolved types and dispatches `[..]` on `List` vs `Array` directly.
- **Delete the deferral hacks.** `ExSeqLit`'s post-TC desugaring in `CR`
  collapses into normal typed lowering; the `is_array` patch-back and the
  pattern-side block both disappear.

### What it unblocks / simplifies

- Array `[..]` patterns (6b) fall out for free symmetric with the 6a
  literals, no `@` annotation.
- One deferral mechanism removed rather than a second one added.
- Any future type-directed pattern sugar (custom deref/view patterns, `Vector T`
  element patterns) stops hitting this wall.

### Cost / risk

TC gains real surface area (typing patterns), and the front-end phase order
changes so error messages and the `MR`/`TC` handoff need re-checking. This is
a genuine restructure, not a local edit; it should land on its own branch with
the full example suite (both engines + RC leak-check) green at each step.
