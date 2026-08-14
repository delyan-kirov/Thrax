# Syntax Roadmap

A backlog of surface-syntax features, ordered by risk and dependency rather than
by wish-list number. The ordering doubles as a suggested build order. Each entry
records the intended syntax, where it lands in the pipeline
(LX -> EX -> grammar -> MR -> TC -> CR), a rough effort, and the key risk or
open decision.

Grounding facts that shape several items (verified against the tree, 2026-07-24):

- **`.n` access already parses and works.** The postfix `atom DOT INT` rule
  builds a field access (`mk_field(base, "1")`), meaning positional tuple/struct
  field access (`examples/TUPLES.thx`). Sequence/tensor indexing is NOT this rule;
  it is deferred to the LA subsystem as `.[..]` (see item #10).
- **`with` is already a keyword** (`KW_WITH`), used for imports (`$ with MOD`).
  Item #2 reuses it in a new (statement) position.
- **Tuples are `%tupleN` structs** built on demand (`ensure_tuple`). "`{x}` == `x`"
  (item #4b, SCRAPPED) would mean making `%tuple1` transparent in unification -- a
  real type-system change, not sugar, and not worth the blast radius.
- **The lexer is per-kind** (`lex_comment`, `lex_number`, `lex_string`, `lex_at`),
  so the literal-level items are localized to one function each.
- **No range tokens** exist yet (item #9 adds them).
- **Struct syntax is `Type.{...}`** and the postfix `.` is load-bearing (field
  access, variant tags, tuple index all hang off it). Dropping it (item #12,
  SCRAPPED) collides head-on with application juxtaposition and with bare
  `{a, b}` tuples, so `.{` stays.

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

### #5 Record update `Type.{ .x = 1, ..base }` -- DONE

**Same-type update** (the recommended nominal choice): `base` is an expression of
the same struct type, the result is that type, and the literal takes its listed
fields from itself and every unlisted field from `base`. `..base` must be the
final entry. The type may be written (`Person.{ ..base }`) or inferred: a bare
`.{ ..base }` is settled at its lit site, and the type flows from either the
annotation/context OR from `base`'s own type (TC unifies `base`'s type with the
literal's), so `.{ ..base }` resolves whenever `base` is typed.

Implementation: a `base` `Expr *` on `ExStructLit` (parsed by recognizing `..`
as two `Dot` tokens inside `parse_struct_lit`). MR traverses `base` in both its
walks. The completeness relaxation must live in TC, not the desugar, because
**PatLower runs *after* inference**. Two TC paths: the qualified literal's inline
`StructLit` inference types `base` against the struct and drops the missing-field
check when `base` is present; the bare literal's lit site (`LitSite::has_base`)
unifies `base` with the lit-site `use` var and skips the missing-field check in
`resolve_lit_sites` (which already patches the node's resolved `type_name`). The
actual fill then happens in PatLower's `lower_record_update`: it binds `base`
once to a struct-typed temp and appends `.<unlisted> = $base.<unlisted>` for
every omitted field, yielding a complete literal CR/IR handle unchanged.
`examples/RECORD_UPDATE.thx`; grammar `struct_lit_body` in doc/thrax.y (no new
conflicts, `%expect` unchanged).

- **DONE: record-rest binding `{ .x = a, ..rest }` in *patterns*.** Now that
  records are row-polymorphic ([[row-records]]), `rest` gets the row-tail type and
  binds the leftover fields. The runtime repack is a `record_without` primitive in
  both engines (drop the head occurrence of each matched label). `.._` still
  discards. Record update also migrated to `| base` (the old `..base` spread is
  gone) and generic structs now bridge to open rows. See doc/row-records.md.

### #6 String interpolation `"Hi {name}, age {STR.from_int p.age}"` -- DONE

Pure parser-level desugaring, so the typer, lowering, and both backends are
untouched. `lex_string` finds only the literal's extent (brace/quote aware, so a
`"` or `{}` inside an interpolant does not end the literal); the parser splits
`source[span]` into literal chunks and `{...}` interpolants, re-lexes each
interpolant with a base-offset `Lexer::sub` (absolute spans, so errors inside
`{...}` point at the right place), and folds into `chunk ++ expr ++ chunk ...`.

- **Stringify (decided):** v1 requires each interpolant to already be `Str`; no
  implicit `show`. A literal chunk seeds the `++` chain so the whole expression
  types as `Str` and each interpolant is forced to `Str` by `++`. A `show`
  overload can be layered on later without changing the syntax.
- **Escape (decided):** `\{` and `\}` are literal braces; a bare `}` is literal
  too. Interpolants may nest strings and braces.

`examples/STRING_INTERP.thx`; parser tests `string_interpolation_*`.

---

## Tier C -- type-directed, one design decision each

### #10 positional `.n` -- DONE (tuple/struct only); sequence/tensor indexing -> LA

`.n` is positional field access on tuples and structs, and that already works via
`atom DOT INT` -> `ExField` -> `settle_field_site` struct lookup
(`examples/TUPLES.thx`). No new work was needed.

Sequence and tensor indexing (`xs.[i]`, `m.[i, p ..= q, j]`, `map.["key"]`) is a
**separate** operation and is **deferred into the LA subsystem**, not layered onto
`.n`. Reason: `.n` is static heterogeneous projection (literal index, result type
per-field), whereas indexing is dynamic homogeneous access that must grow to a
runtime index, multi-dimensional/tuple indices, ranges (slices -> views), and
user-extensible containers (maps). Chosen surface: `.[..]` (the leading `.` keeps
it unambiguous vs list literals and juxtaposition application; `.` becomes a
uniform access prefix: `.n`/`.field` = projection, `.[..]` = index, `.{..}` =
struct literal). It desugars to an overloadable `index` call (type-directed on the
index type), and axis variance (co/contra) lives in the tensor *type*, not the
index. See doc/ranges-codata-linalg.md. A brief sequence-indexing prototype on
`.n` was implemented and then reverted once this split was settled.

### #9 Ranges: pattern form DONE (`is lo ... hi` / `is lo ...`); expression form DONE (`[lo ... hi]` / `[lo ...]`)

**Range PATTERNS shipped** (inclusive only, Jai-style): `is n | 90 ... 100 => ...`.
Chosen spelling is `...` (a new `Ellipsis` lexer token; `..` stays free for
`..rest`/`[..]`), inclusive at both ends, numeric-literal bounds, refutable, binds
nothing. Frontend-only: no IR/runtime/backend change, since a range lowers in
`patmat` to two comparison tests (`sv >= lo` and `sv <= hi`) reusing the existing
`<=`/`>=` builtins, exactly like a string-literal pattern lowers to `?=`.
`Pattern::Range`/`Pat::Range`; `examples/RANGES.thx`. There is no static
exhaustiveness checker, so a range match falls through to the runtime "no pattern
matched" fault when no `else` and nothing matches. An OPEN pattern `is n | lo ...`
(no upper bound) matches when `lo <= n`, lowering to the single `sv >= lo` test
(`Pat::Range.hi` is optional).

- **Deferred: half-open ranges** (`..<`). Patterns almost always want inclusive
  (Rust `..=`, Zig `...`); revisit `..<` if/when a use needs it.
- **EXPRESSION form `[lo ... hi]` DONE** (2026-08-13): a TYPE-DIRECTED inclusive
  range literal, `Expr::Range { lo, hi }` (a real node, not a fixed desugar), so the
  target is chosen by the CHECKER from the expected type, like a numeric literal or
  the `[a, b, c]` sequence literal:
  - expected sized tensor `[n]T`: build the tensor. The bounds must be LITERALS so
    the length `n = hi - lo + 1` is a compile-time constant (checked, `[4]Int = [1
    ... 4]`, `[0]T` when `hi < lo`); bounds are checked against `T`, so `[4]Nat = [1
    ... 4]` types the ends as Nat. Lowering expands the literal bounds to element
    consts and `@tensor_stack`s them.
  - otherwise / no annotation: `List Int` (the default), lowered to the inclusive
    `CORE.range lo hi`.
  - A non-literal bound against a sized tensor is a clean compile-time ERROR (its
    length would not be statically known).
  - an OPEN range `[lo ...]` (no upper bound) is infinite, so it always builds a
    `Stream Int` (lowered to `CORE.count_from lo`); against a `List` or a sized tensor
    it is a type error (neither can be unbounded). `Expr::Range.hi` is optional.

  `range` is the single canonical INCLUSIVE builder in the auto-imported `CORE`, so
  the `List` form needs no import and reads consistently with the `...` of range
  patterns and tensor slices. The old half-open `LIST.range` `[lo, hi)` was DELETED
  to remove the name clash; its call sites moved to inclusive (`range 0 500` ->
  `range 0 499`, same list). `Stream` and `count_from` are also canonical in `CORE`.
  `examples/RANGES.thx`, `examples/CODATA.thx`.
  - **Deferred (documented):** (a) a compile-time-CONSTANT bound that is not a
    literal (`let a = 4 in [1 ... a]`, or a global const) against a sized tensor;
    this needs a small const-eval/propagation pass and currently errors. (b) the
    Array/Vec targets, riding the same type-directed node. (c) step/stride and
    descending ranges. The codata-STREAM target is now DONE via the open form.
- **Compose with the future:** ranges, `.n` indexing (#10), codata, and a
  linear-algebra layer are entangled (ranges-as-slice-descriptors, type-directed
  indexing, strict-data-vs-codata). If #9/#10 are built as the LA on-ramp, follow
  the "lock now" constraints in **doc/ranges-codata-linalg.md** (that subsystem is
  DEFERRED, but the surface work should not foreclose it).

### #2 `with p in body` field-scoping (Jai-style) -- DONE

`with p in body` brings `p`'s struct fields into scope unqualified for `body`.
Spelled `with .. in ..` (not `do`), reusing `KW_WITH`/`KW_IN` in expression
position (no grammar conflict vs `$ with MOD`: different position; `%expect`
unchanged). Type-directed: it parses to a pattern-let with a `bind_all` struct
pattern (`PatStruct::bind_all`, empty `type_name`/`fields`); TC's `type_pattern`
resolves the struct from the subject's type, writes `type_name`, and binds every
field to its own name in the env; PatLower emits `let $s = p in let f = $s.f in
.. in body`. The subject may be any expression (evaluated once). `with`s nest,
and a bound field shadows an outer local.

Also usable in a **signature**: a `with`-prefixed named-record parameter field,
`foo : {with p: Person} -> Int`, takes the parameter `p` and scopes its fields
into the body, i.e. the same as `\p = with p in ..` (the parameter `p` itself
stays in scope too). This rides the #4a named-record sugar: the field carries a
`with_scope` flag (`RecField`), and `desugar_record_params` wraps the body with
`mk_with_scope` per marked field. Works mixed with plain fields
(`{k: Int, with q: Point}`).

- **Resolved:** (a) subject can be any expression; (b) an unknown subject type is
  an error ("annotate it") -- you cannot infer the struct from field usage
  (needs row polymorphism, deferred). (c) MR guards the `bind_all` pattern
  (skips type rewrite). **Known limit:** a field whose name coincides with a
  module-level global resolves to the global (MR mangles it before TC knows the
  fields); fields not colliding with globals work. `examples/WITH_SCOPE.thx`.

### #4 Destructuring params, and 1-tuple transparency

Two separable sub-features:

- **#4a -- param/binding destructuring.** DONE, as pure parse-time sugar. A
  named-record parameter type `{x: T, y: U} -> R` erases to the positional tuple
  `{T, U} -> R` and the field names become a destructuring binder on the value:
  `$ f : {x:T,y:U} -> R = body` rewrites to `$ f : {T,U} -> R = \$p = let {x,y}
  = $p in body`. A one-field record `{x: T}` **collapses** to a plain named
  parameter `T` (call `f 5`, not `f {5}`), which is why `{x:Int} -> Int` really
  is `Int -> Int` WITHOUT #4b. Lexed via a lowercase field name + `:` after `{`
  (a type never starts lowercase). Kept on `TyCon::rec_fields`; erased in
  `desugar_record_params` (parse_global) which also errors on a named record
  outside a leading parameter position. Reuses the existing tuple pattern-let
  lowering. Lambda `\{x,y}` destructuring already worked. `examples/RECORD_PARAMS.thx`.
- **#4b -- `{x}` == `x` (1-tuple transparency). SCRAPPED.** Making `%tuple1 T`
  unify with `T` touches the core unifier and every site that builds or inspects
  tuples: too much blast radius for too little gain. The #4a arity-1 collapse
  already gives the `{x:Int}->Int` == `Int->Int` ergonomics; positional `{Int}`
  stays a distinct 1-tuple, and that is fine. Not revisiting.

### #3 Multi-clause definitions (Haskell equational style) -- SCRAPPED

Considered and dropped: too much surface sugar for too little value. Equivalent
code is already expressible with `when` and a single lambda. Not revisiting.

---

## Tier D -- SCRAPPED

### #12 `MyStruct { ... }` (drop the `.` in struct syntax) -- SCRAPPED

Dropped. The leading `.` is what disambiguates a struct literal from application
(`f {}`) and from a bare tuple `{a, b}`; removing it buys one character at the
cost of real grammar ambiguity. Keeping `.{ ... }`. Not revisiting.

---

## Suggested order

1. **Tier A (#11, #7, #8)** -- DONE. Cheap, isolated lexer wins, all shipped.
2. **Or-patterns (#1)** -- DONE. **Record update (#5)** -- DONE.
3. **String interpolation (#6)** -- after the stringify story is decided. **(next)**
4. **positional `.n` (#10)** -- DONE (tuple/struct only). **Sequence/tensor
   indexing `.[..]`** and **ranges (#9)** -- deferred into the LA subsystem
   (doc/ranges-codata-linalg.md).
5. **#4a destructuring + `with` (#2)** -- DONE together.
6. **Multi-clause (#3)** -- SCRAPPED (too much sugar for the value).
7. **#12 and #4b** -- SCRAPPED (see Tier D / #4b).

The surface-syntax backlog is now closed: everything is DONE, SCRAPPED, or
deferred into the LA subsystem (`.[..]` indexing + ranges #9, see
doc/ranges-codata-linalg.md). String interpolation (#6) shipped in its v1 form
(interpolants must be `Str`); auto-stringify via a `to_string` overload, and the
larger `@ctx` implicit-parameter feature, are designed in
doc/implicit-context.md. Both are blocked on a latent bug documented there:
same-module overloading does not dispatch at runtime (globals collide on
`Module.name`). Future work is substantive (fix overloading, LA, `@ctx`,
effects) rather than more syntax.

## Open decisions (blockers to a build-ready plan)

- **#1:** RESOLVED -- v1 ships with no binders in alternatives (see #1 above).
- **#6:** require interpolants to already be `Str`, or add a `show`/stringify
  overload first? Brace-escape spelling?
- **#9:** desugar ranges to a library `range` function (recommended) or a core
  primitive? Descending `..>` in v1 or later?
