# Syntax Roadmap

A backlog of surface-syntax features, ordered by risk and dependency rather than
by wish-list number. The ordering doubles as a suggested build order. Each entry
records the intended syntax, where it lands in the pipeline
(LX -> EX -> grammar -> MR -> TC -> CR), a rough effort, and the key risk or
open decision.

Grounding facts that shape several items (verified against the tree, 2026-07-24):

- **`.n` access already parses.** The postfix `atom DOT INT` rule builds a field
  access (`mk_field(base, "1")`); today that only *means* tuple field access.
  `xs.1` on an array/vec is new **semantics**, not new syntax (see item #10).
- **`with` is already a keyword** (`KW_WITH`), used for imports (`$ with MOD`).
  Item #2 reuses it in a new (statement) position.
- **Tuples are `%tupleN` structs** built on demand (`ensure_tuple`). "`{x}` == `x`"
  (item #4b) means making `%tuple1` transparent in unification -- a real
  type-system change, not sugar.
- **The lexer is per-kind** (`lex_comment`, `lex_number`, `lex_string`, `lex_at`),
  so the literal-level items are localized to one function each.
- **No range tokens** exist yet (item #9 adds them).
- **Struct syntax is `Type.{...}`** and the postfix `.` is load-bearing (field
  access, variant tags, tuple index all hang off it). Dropping it (item #12)
  collides head-on with application juxtaposition and with bare `{a, b}` tuples.

Item numbers below match the original request list, for traceability.

---

## Tier A -- lexer-local, isolated (cheap wins, do first)

### #11 Multi-line comments `#- -#` -- DONE

`lex_comment` now branches when the char after `#` is `-`: it scans to the
matching `-#`, counting newlines for line tracking. **Nestable** (chosen), so a
commented-out block containing `#- -#` closes at the right place; an unterminated
block is a lex error. Pure LX, no parser/TC impact.

### #7 Numeric separators `_` -- DONE

`lex_number` and `lex_radix` scan digit runs through a shared `scan_digits`
helper that allows a `_` only between two digits (leading, trailing, doubled, or
`.`/`e`/prefix-adjacent `_` is a lex error). `emit_int`/`emit_real` strip the
`_`s before `strtoll`/`strtod`. Works in decimal, `0x`/`0b` radix, fractions, and
exponents (`1_000_000`, `0xFF_FF`, `3.14_159`, `1e1_0`). Pure LX.

### #8 Char literal `@char "a"` -- DONE

Shipped on the `@`-intrinsic registry (see doc/at-intrinsics.md). `@char "x"`
parses a following string literal, requires it to be exactly one Unicode scalar
(1-4 UTF-8 bytes), decodes it to its code point, and emits an `EX::ExChar` leaf
typed `Nat32` (a 4-byte code point, not a C byte). CR erases it to a Core integer
constant, like `ExBool`, so no backend change was needed. Reuses the lexer's
string-escape decoding (the parser only needs to decode the one scalar).

---

## Tier B -- parser plus existing machinery (~1-2 days each)

### #1 Or-patterns `when foo is A is B is C then e` -- DONE

`parse_when` collects the `(is pat)+` alternatives of an arm and emits one
`MatchArm` per alternative, all sharing the arm's body and guard. Because each
alternative is its own arm (a matrix row), exhaustiveness works for free: a
`when` over a union may drop its `else` when the alternatives cover every
constructor, and a non-exhaustive one still reports the missing cases. No new AST
node, no TC or lowering change.

- **Decision (taken):** **alternatives bind no variables** in v1. A recursive
  `pat_binds_any` check rejects a binder in any alternative of a multi-pattern
  arm (single-pattern arms bind normally), so there is no inconsistent binding to
  hand the shared body. `examples/OR_PATTERNS.thx`; grammar `alts` rule in
  doc/thrax.y (no new conflicts, `%expect` unchanged).

### #5 Record update `{ .x = 1, .y = 2, ..foo }`

The `..spread` spelling is already established (list patterns parse `..rest`).
Extend the struct/tuple literal parser to accept a `..base` entry. Add a `base`
expr field to `ExStructLit`. Desugar (CR or a pre-pass) to: copy `base`, then
override the named fields.

- **Decision:** does the result type equal `foo`'s type (true functional update)
  or is it row-polymorphic? Recommend **same-type update** (matches nominal
  structs, simplest).

### #6 String interpolation `"Hi {a ++ b}, age {p.age}"`

The deepest Tier-B item: lexer **and** parser. `lex_string` emits a sequence of
segment tokens (or one token carrying parsed pieces): a literal chunk, then a
`{`...`}` holding raw source to be re-lexed/parsed as an expr, repeating.
Desugar to `chunk ++ stringify(expr) ++ chunk ...`.

- **Dependency:** a stringify story. Either require every interpolant to already
  be `Str` in v1 (no coercion), or introduce a `show`/`str` overload as a
  prerequisite.
- **Decision:** literal-brace escape (`\{` vs `{{`).

---

## Tier C -- type-directed, one design decision each

### #10 `xs.1` list/vector indexing

Syntax is free (already parses to `mk_field`). The work is in TC: field-access
on an `Array`/`Vec`/`Str` receiver routes to `array_get`/`vec_get`; on a tuple
it stays positional field lookup. So `ExField` resolution becomes
**type-directed**, resolved after inference like overload sites and PatLower.

- **Decision:** bounds behavior (runtime check vs UB); whether negative or `Real`
  indices are a static error. Reuses the existing `array_get` primitive.

### #9 Ranges `[1 ..= 10]`, `[1 ..< 10]`, and range patterns `is 1 ..= 5`

New tokens `..=` `..<` `..>` (maximal-munch in LX, sharing the operator path).
Expression form desugars to a list/array builder. Pattern form `is lo ..= hi` is
a refutable interval test; the exhaustiveness checker treats it like a literal
(finite = false, contributes nothing to completeness), which is straightforward.

- **Decision:** desugar the expression form to a **library** `range`/`range_incl`
  function (recommended) or a core primitive? Descending `..>` semantics; step /
  stride (recommend none for v1). `@char "a" ..= @char "f"` already works once
  chars are Ints.

### #2 `with p do ...` field-scoping (Jai-style)

Reuse `KW_WITH` in statement position to bring `p`'s struct fields into scope
unqualified. Must be **type-directed** (field names are known only after TC
types `p`), so it desugars in/after TC like PatLower:
`with p do body` -> `let x = p.x in let y = p.y in body` per field.

- **Decisions:** (a) grammar disambiguation from the top-level `$ with MOD`
  import -- different position (statement vs `$`), so likely fine; verify the
  bison `%expect` count. (b) shadowing when two `with`s collide. (c) any struct
  expr, or only a variable. Conceptually depends on #4a.

### #4 Destructuring params, and 1-tuple transparency

Two separable sub-features:

- **#4a -- param/binding destructuring.** `\{x, y} = ...` and
  `foo : {x: X} -> Y` bringing `x` into scope directly. The lambda path already
  carries `param_pat` for structural patterns (see `parse_closure`), and
  pattern-lets already lower. So 4a is mostly **wiring signatures to accept a
  pattern binder** and confirming irrefutable struct/tuple patterns bind their
  fields. Moderate; reuses PatLower; overlaps heavily with #2.
- **#4b -- `{x}` == `x` (1-tuple transparency).** Making `%tuple1 T` unify with
  `T` touches the core unifier and every site that builds or inspects tuples.
  High blast radius and easy to open soundness holes. **Recommend deferring 4b**
  as its own project with its own plan; ship 4a first.

### #3 Multi-clause definitions (Haskell equational style)

```
$ depth Peano.Zero = 0
$ depth Peano.Succ.{ n } = depth n + 1
```

Frontend change: after parsing top-level `$` bindings, group consecutive
same-name clauses that have pattern params and merge them into one
`\args = when args is ...`. Needs matching arities across clauses and a shared
signature. Feeds straight into `when`/exhaustiveness, so a non-exhaustive
multi-clause def warns for free. The biggest *frontend* item, but conceptually
well-trodden.

- **Decision:** where the signature lives -- one leading `$ f : T` then bare
  `f pat = ...` clauses (recommended), vs. repeating the annotation.

---

## Tier D -- ambiguity-first, decide before touching code

### #12 `MyStruct { ... }` (drop the `.` in struct syntax)

Do **not** start this without a decision. `Name {...}` is currently unambiguous
*application* (`f {}` applies `f` to the unit value), and a bare `{a, b}` is
already a **tuple literal** -- the leading `.` is exactly what disambiguates a
struct literal from a tuple today. Options:

- **(i)** Special-case: an uppercase-initial atom immediately followed by `{`
  (no intervening token) is a struct literal, never application. Cost: `Foo {}`
  can no longer mean "apply constructor `Foo` to unit," and it complicates the
  `app : app atom` rule. Variant payloads `Type.Tag.{...}` and bare `.{...}`
  need a parallel answer (`Type.Tag {...}`? does `.{...}` stay?).
- **(ii)** Accept `{` in addition to `.{` -- transitional, but two spellings.
- **(iii)** Layout / leading-space rules -- fragile; avoid.

Recommended: treat #12 as a syntax RFC settled on paper first (enumerate the
ambiguity cases against application and against bare tuples, pick a grammar),
before implementing. It also interacts with #5 (record update spelling).

---

## Suggested order

1. **Tier A (#11, #7, #8)** -- DONE. Cheap, isolated lexer wins, all shipped.
2. **Or-patterns (#1)** -- DONE. **Record update (#5)** -- high value; reuses the
   `..spread` spelling already present. **(next)**
3. **String interpolation (#6)** -- after the stringify story is decided.
4. **`.n` indexing (#10), ranges (#9)** -- type-directed but self-contained.
5. **#4a destructuring -> `with` (#2)** -- shared machinery; do together.
6. **Multi-clause (#3)** -- bigger frontend, well understood.
7. **Decide, then maybe #12; defer #4b** -- the ambiguity and soundness risks.

## Open decisions (blockers to a build-ready plan)

- **#1:** RESOLVED -- v1 ships with no binders in alternatives (see #1 above).
- **#6:** require interpolants to already be `Str`, or add a `show`/stringify
  overload first? Brace-escape spelling?
- **#12:** pursue the `.`-drop at all, given the collision with application and
  with bare `{a, b}` tuples -- or keep `.{`? Settle on paper first.
- **#4:** OK to split into 4a (destructuring, soon) and 4b (1-tuple
  transparency, deferred)?
- **#9:** desugar ranges to a library `range` function (recommended) or a core
  primitive? Descending `..>` in v1 or later?
