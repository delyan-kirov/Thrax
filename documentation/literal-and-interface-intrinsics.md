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
- `@compiler_interface_range            : t -> t -> f`   (`[lo ... hi]`)
- `@compiler_interface_range_from       : t -> f`        (`[lo ...]`)
- `@compiler_interface_slice            : c -> @int -> @int -> c`  (`.[lo ... hi]`)

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

## Stage 1 - literal-hook mechanism, current defaults preserved  [LANDED (hook mechanism); @default defaulting deferred as its own milestone]

Landed: the four construction hooks are overloadable. A literal (`"..."`, an int, a
real, `[..]`) whose EXPECTED type is a user type providing the matching hook builds
that type through it (via a signature/argument context or a `(e : T)` ascription);
otherwise it keeps its built-in default, which lowering folds to a plain constant (no
payload, no conversion), verified on both the interpreter and the ccg native backend.
The interception is check-directed (`Checker::literal_hook_check`): it reuses the
overload resolver's trial-unify/rollback and records the resolved hook per literal site
(`literal_hooks`) for lowering to wrap. This preserves ALL numeric/sized-int behavior
(integer/real literals stay numeric unless aimed at a user type).

Deferred (its own milestone, as flagged below): the `@default` overload attribute and
the unconstrained let-generalization defaulting (a module overriding the core default so
a bare, unconstrained `[1,2,3]` builds a user type). Not needed for the check-directed
tests; it is the risk area the plan calls out.

### Original plan text

- Recognize the `@compiler_interface_*` construction hooks; allow user overloads.
- Add the `@default` overload attribute and the literal-defaulting step.
- Desugar string / list / integer / real literals to their hooks; core provides
  default overloads reproducing today's types exactly (`@array`-backed string,
  `@list`, machine int/float).
- Constant-fold default instances; verify on interpreter and ccg.
- Tests: a user type opting into each literal via context and via `(e : T)`.

Touchpoints: `parser.rs`/`typing.rs` (literal lowering + defaulting),
`lowering.rs`, `ccg/src/gen.rs` (folding), `library/CORE.thx`.

## Stage 2 - pattern intrinsics  [LANDED]

Landed both halves. A literal PATTERN (`is "foo"`, `is 42`) whose scrutinee is a user
type routes through that type's construction hook (to build the literal into the type)
plus `@compiler_interface_equality : t -> t -> @bool`, matching by a boolean test. A
sequence PATTERN (`is [a, b, ..r]`, `is h :: t`, `is []`) on a user type unfolds
`@compiler_interface_sequence_view : f t -> SeqView (f t) t` (core union
`$ SeqView s t = Empty | More t s`), stepping `More elem tail` per leading element and
binding `..rest` (or requiring `Empty` for a fixed length). Check-directed in
`type_pattern` (only user types route; builtin Str/List keep the fast path); lowering
emits new core `Pat::HookEq` / `Pat::SeqView`, which `patmat` expands to boolean tests /
nested `SeqView` matches before the later passes. Verified interpreter + ccg.

### Original plan text

- Add `@compiler_interface_equality` and `@compiler_interface_sequence_view`; core
  `SeqView` union and default overloads for builtin sequences.
- Route literal patterns through equality and sequence patterns through the view,
  after type checking; keep optimized builtin overloads as the fast path.
- Tests: user-type string and sequence patterns.

Touchpoints: `lowering/patmat.rs`, `typing.rs`, `library/CORE.thx`.

Deferred: general extractors / pattern synonyms via a `@compiler_interface_view`
returning an arbitrary sum. Same machinery, later.

## Stage 3 - collapse builtins onto the mechanism  [List half LANDED; @str half deferred]

Prerequisite LANDED (2026-09-05): type-directed resolution of BARE variant tags. A
`.Tag` now takes its union from the expected type (`Checker::union_head_with_tag` + a
`check` arm for `Expr::Variant { ty: None }`), and `infer_variant` CHECKS payloads
bidirectionally so the expectation flows into nested `.Tag`s. This lets two unions share
a constructor name without collision.

List half LANDED (2026-09-05): `List` is now an ordinary `@union` declared in CORE.thx
(`$ List : @union a = Nil: {}, Cons: {a, List a}`), NOT a compiler builtin. Removed the
hardcoded `List`/`Cons`/`Nil` special-cases from the checker (`variant_sig`,
`find_union_by_tag`, `union_head_with_tag`) and from lowering (`Decls::variant`,
`CONS_FIELDS`); they all resolve through the CORE declaration now. The `[..]`/`::`/`[]`
sugar still names `List`/`Cons`/`Nil` (its surface definition), and `@list` stays a
spelling alias for `List`. Verified interpreter + ccg (STRINGS, LISTS examples;
`bare_variant_tag_resolves_by_expected_type` test).

`@str` fold / `Str`-as-library-type: NOT DONE, decided AGAINST (2026-09-05). Explored two
routes and backed out of both:

- An `@struct @unbox` transparent single-field newtype (erase the wrapper at lowering) was
  built as the enabler, and `Str` was briefly declared `$ Str : @struct @unbox = _buffer :
  @array`. It worked and was zero-cost, but the whole thing was REMOVED at the user's call:
  `Str`'s hard parts are genuinely primitive (UTF-8 validation, escape decoding) and it is
  load-bearing in CORE-less bootstrap contexts (`library/C.thx`'s libc bindings, the
  `main : [n]Str` argv contract), so making it a library type fights all of that for no
  real payoff. `@unbox` itself was also removed (its only motivating use was `Str`; a
  general unboxed-newtype feature can be revisited if a perf need shows up). NOTE for a
  future attempt: automatic (non-opt-in) unboxing of single-field structs is UNSOUND
  because of the `Con ~ Record` bridge (a nominal 1-field struct can be used structurally
  as an open record, e.g. `Box : @struct a = val: a` passed to `{ val | r }`), which needs
  the boxed rep; the suite catches this immediately.
- `@str` -> `@array` fold: also not done -- it discards the useful `Str`-vs-raw-bytes
  distinction, and `Str` handling UTF-8/escapes means it stays a real primitive.

What DID land instead: `Str` stays a built-in primitive, and the string-literal INTERFACE
takes the built-in `Str`. `@compiler_interface_string_literal` is now `Str -> a` (was
`@array -> a`): the compiler builds a fully-formed, UTF-8-checked, escape-decoded `Str` and
the custom type just maps it to its value (default `Str` folds to zero cost). So other
types can adopt string literals while `Str` itself is not de-builtined. `CStr` for C
interop was considered and deferred (the FFI already NUL-terminates a `Str` on marshal, so
`Str -> char*` is already safe). It is worth revisiting: a Thrax `Str` is not
guaranteed NUL-terminated, and the append-a-NUL-on-marshal trick hides that from the
type system, as does the return direction (C hands back a `char*` treated as a `Str`).
A `CStr` would need a conversion primitive that does not exist and a rewrite of
`library/IO.thx`, which is why it was deferred.

### Original plan text

- Delete the `@list` builtin; define `List` as a core `@union`; `@compiler_interface_sequence_literal`
  default builds it; rewrite `library/LIST.thx`.
- Fold `@str` into `@array`; string literals lower to a byte payload; migrate the
  `@array_*` byte-poking in `CORE.thx`/`STR.thx`.
- Keep `@array`, `@vec`, `[n]T` as the three aggregate primitives.

Touchpoints: `typing.rs` (drop `@list`/`@str` builtins), `library/CORE.thx`,
`library/LIST.thx`, `library/STR.thx`, both backends.

## Stage 4 - type-name cleanup  [LANDED, done before Stage 1]

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

## Stage 5 - the whole `[]` surface, one mechanism  [LANDED 2026-09-25, issue #175]

The remaining hardcoded `[]` jobs became overloads, so every bracket form except the
`[n]T` type resolves through the same machinery.

- **Ranges.** `[lo ... hi]` resolves `@compiler_interface_range` and `[lo ...]`
  `@compiler_interface_range_from`; CORE overloads them for `@vec` and `Stream`. The
  checker no longer knows the con name `Stream`, and lowering no longer names `range`
  or `count_from`. `Checker::range_hook` resolves against the expected type, so a
  sequence of your own claims the surface with one overload.
- **`::`.** An ordinary operator (`$ (::) : t -> @vec t -> @vec t` in CORE) instead of
  a lowering-time rewrite to `vcons`, so it is overloadable like `+` and construction
  now agrees with the `h :: t` pattern. Dropped from `parser::table::desugared` and
  from the checker's built-in bindings.
- **Sequence literals.** CORE has a `@vec t -> @vec t` overload, the
  `user_type_head` guard on `literal_hook_check` is gone, and `infer` records the hook
  for the default case too, so lowering has ONE path: collect the elements into the
  `@vec` payload and apply the resolved hook.
- **`@array`.** Joins through its own CORE overloads of `_sequence_literal`
  (`@vec @int -> @array`) and `_sequence_view`. Deleted: the `is_array` arms in
  `check` / `type_pattern`, `array_exprs` / `array_pats` and their `Resolved`
  plumbing, and `lowering::array_arm` with its helpers. Array patterns now nest like
  any other sequence pattern (the old guarded-arm lowering ignored nested
  sub-patterns).
- **Slices.** A lone `recv.[lo ... hi]` over a non-tensor receiver resolves
  `@compiler_interface_slice`; CORE overloads it for `@vec`, `@array`, and `@str`, so
  slicing is no longer tensor-private.

CORE's cons `List` is the worked example of the result: it is an ordinary `@union`
with no compiler support, and three overloads (`_sequence_literal`, `(::)`,
`_sequence_view`) give it literals, cons, and every sequence pattern. Exercised in
`examples/LISTS.thx`, whose second half runs the same surface over a `List` that the
first half runs over a `@vec`.

What stays compiler business, by the type system rather than by convenience: the
sized tensor `[n]T`. A `[..]` literal, a range, or a multi-axis slice aimed at one
carries a static length that no hook payload or signature can express, so those three
keep their type-directed paths (`tensor_exprs`, `infer_slice`'s shape algebra).

Tests: `range_via_hook_on_a_user_type`, `open_range_via_hook_on_a_user_type`,
`cons_operator_is_overloadable`, `slice_hook_covers_sequences_and_user_types`
(interpreter), `bracket_hooks_match_interpreter` (ccg parity).
