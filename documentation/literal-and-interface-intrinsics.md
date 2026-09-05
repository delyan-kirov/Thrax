# Literal and interface intrinsics

## Goal

Remove hardcoded literal/type machinery from the compiler and replace it with a
small family of overloadable `@compiler_interface_*` functions. Literals (`"..."`,
`[...]`, integers, floats), indexing (`.[..]`), and the structural side of pattern
matching all desugar to these functions. The compiler knows only a few irreducible
primitives; every friendly type (`Str`, `List`, `Real`) moves to core and can be
substituted by user types.

## Principles

- `@`-names are compiler-blessed. A `@`-name is user-extensible **iff** it starts
  with `@compiler_interface_`. Defining any other `@`-name in user code errors:
  "this compiler intrinsic is not extensible in user code".
- Each interface intrinsic has a default overload in core. A module may define its
  own overload; a local default beats core's when a literal is otherwise
  unconstrained.
- Reuse the existing overload resolver (trial-unify + rollback) and type-directed
  `check`. The only new inference machinery is literal defaulting.
- Patterns lower after type checking (unchanged architecture): once inference knows
  the scrutinee type, a non-builtin type routes through its interface overload.

## Irreducible primitives (after this work)

Three aggregates the compiler still knows directly:

- `@array` packed byte buffer (absorbs `@str`).
- `@vec t` boxed dynamic array.
- `[n]T` (`@tensor`) unboxed numeric fixed-size tensor, size in the type.

Deleted as builtins: `@str` (folded into `@array`), `@list` (redefined as a core
`@union`).

Scalars stay primitive: `@int`, `@nat`, `@float64` (and the sized `@intN`/`@natN`/
`@float32`), `@bool` (control flow needs a concrete branch value; `Bool` is out of
scope for literal work).

## The interface-intrinsic family

Construction / access:

- `@compiler_interface_string_literal  : @array -> a`
- `@compiler_interface_sequence_literal : @vec t -> f t`
- `@compiler_interface_integer_literal  : @int -> a`
- `@compiler_interface_real_literal     : @float64 -> a`
- `@compiler_interface_indexing         : c -> k -> *`   (the `.[..]` hook)

Matching:

- `@compiler_interface_equality      : t -> t -> @bool`
- `@compiler_interface_sequence_view : f t -> SeqView (f t) t`

where core defines `$ SeqView : @union s t = Empty | More t s`.

## Desugarings

- `"foo"`            ->  `@compiler_interface_string_literal <bytes:@array>`
- `[a, b, c]`        ->  `@compiler_interface_sequence_literal <@vec t payload>`

  (except when the expected type is a sized tensor `[n]T`: that stays the existing
  type-directed build, since a `@vec` payload cannot carry the static length `n`.)
- `42`               ->  `@compiler_interface_integer_literal 42`
- `1.5`              ->  `@compiler_interface_real_literal 1.5`
- `m.["k"]`          ->  `@compiler_interface_indexing m "k"`
- `is "foo"`         ->  `@compiler_interface_equality scrut (@compiler_interface_string_literal <bytes>)`
- `[a, b, ..rest]`   ->  nested match on `@compiler_interface_sequence_view`:

```
when @compiler_interface_sequence_view scrut is
  More a t1 => when @compiler_interface_sequence_view t1 is
    More b rest => <bind a, b, rest; success>
    _           => <fail>
  _ => <fail>
```

## Type annotation (disambiguation)

General expression ascription `(e : T)`, valid on any expression, added in
`parse_group` (the colon is currently free there). New node `Expr::Ascribe(e, T)`,
checked as `check(e, T)`. Resolves literal ambiguity and overloaded-call ambiguity
alike.

## Defaulting (the one new inference step)

A literal with no expected type falls back to the `@default`-marked overload,
committed at let-generalization (Haskell-style defaulting). Local module default
beats core default. This is the main risk area; treat it as its own milestone.

## Constant folding (no perf regression)

When a literal's interface call resolves to the default (identity-shaped) overload,
fold it to a static constant so `"hi"` / `[1,2,3]` / `42` do not build a payload and
run a conversion at runtime. Must hold on both the interpreter and the ccg native
backend.

---

# Staged plan

Each stage is independently buildable and testable.

## Stage 0 - immediate win, no new mechanism  [LANDED]

- Add `Expr::Ascribe(e, T)`: parse in `parse_group`, check as `check(e, T)`, lower
  transparently.
- Repoint `.[..]` desugar from plain `index` to `@compiler_interface_indexing`.
  - Move `LA.index` to a `@compiler_interface_indexing` overload for tensors.
  - Add `@compiler_interface_indexing : Map k v -> k -> Option v` in MAP.
- Enforce the `@compiler_interface_` prefix rule for user `@`-definitions.

Delivers `m.["key"]` and general `(e : T)` ascription.

Touchpoints: `parser.rs` (`parse_group`, `parse_postfix`), `parser/data.rs`
(new node), `typing.rs` (ascribe check, prefix-rule error), `library/LA.thx`,
`library/MAP.thx`.

## Stage 1 - literal-hook mechanism, current defaults preserved

- Recognize the `@compiler_interface_*` construction hooks; allow user overloads.
- Add the `@default` overload attribute and the literal-defaulting step.
- Desugar string / list / integer / real literals to their hooks; core provides
  default overloads reproducing today's types exactly (`@array`-backed string,
  `@list`, machine int/float).
- Constant-fold default instances; verify on interpreter and ccg.
- Tests: a user type opting into each literal via context and via `(e : T)`.

Touchpoints: `parser.rs`/`typing.rs` (literal lowering + defaulting),
`lowering.rs`, `ccg/src/gen.rs` (folding), `library/CORE.thx`.

## Stage 2 - pattern intrinsics

- Add `@compiler_interface_equality` and `@compiler_interface_sequence_view`; core
  `SeqView` union and default overloads for builtin sequences.
- Route literal patterns through equality and sequence patterns through the view,
  after type checking; keep optimized builtin overloads as the fast path.
- Tests: user-type string and sequence patterns.

Touchpoints: `lowering/patmat.rs`, `typing.rs`, `library/CORE.thx`.

Deferred: general extractors / pattern synonyms via a `@compiler_interface_view`
returning an arbitrary sum. Same machinery, later.

## Stage 3 - collapse builtins onto the mechanism

- Delete the `@list` builtin; define `List` as a core `@union`; `@compiler_interface_sequence_literal`
  default builds it; rewrite `library/LIST.thx`.
- Fold `@str` into `@array`; string literals lower to a byte payload; migrate the
  `@array_*` byte-poking in `CORE.thx`/`STR.thx`.
- Keep `@array`, `@vec`, `[n]T` as the three aggregate primitives.

Touchpoints: `typing.rs` (drop `@list`/`@str` builtins), `library/CORE.thx`,
`library/LIST.thx`, `library/STR.thx`, both backends.

## Stage 4 - type-name cleanup

- Make `@int`/`@nat` canonical; drop the `Int`/`Nat` aliases entirely.
- Keep `Real` as a core alias to `@float64`, the single surviving alias (float has
  no obvious default name; int/nat do). Drop every other alias.
- Drop the friendly `Bool` spelling; `@bool` only.
- Migrate corpus, examples, and docs.

Touchpoints: `typing/data.rs` (`display_con`, base-type set), `library/CORE.thx`,
`examples/`, `tests/`, `documentation/`.

## Open questions to confirm before Stage 1

- Exact spelling of the hook names (long descriptive `@compiler_interface_*` agreed;
  final words TBD).
