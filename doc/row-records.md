# Row-polymorphic records (rows for structs)

Status: **stage 1 implemented.** Open-row record *parameters* work end-to-end on
both engines: `{ x:Int, y:Int | r } -> Int` accepts any struct with those fields,
structural field access (`p.x`) resolves through the row, and a missing field is a
type error. Remaining: anonymous record *literals* + the update/stack value
syntax (`{ .f = v | base }`, `{ .f = v, with base }`) and the `..base` migration;
record-rest patterns `{ .x = a, ..rest }`. Records use the same scoped-row
discipline as effects.

Implementation of stage 1: `Type::Record(row)` + `Type::RowField(label, ty, rest)`
(engine.rs) unified by `unify_record_row`/`rewrite_field` (mirrors the effect
`unify_row`/`rewrite_row`); the hybrid bridge is `Engine::struct_rows` (non-generic
structs' closed rows, set by `Checker::register_struct_rows`) so `Con(struct)`
unifies with an open record row; open-row types parse via `Ty::Record { fields,
tail }`; `infer_field` does a row lookup. Runtime unchanged (name-keyed). Tests:
`open_row_param_accepts_any_matching_struct` (interpreter) and
`open_row_record_param` (ccg).

## The core decision: records are scoped rows, exactly like effects

A record type is a **row** of labeled fields, unified with the same Leijen
scoped-label algorithm as effect rows (`unify_row` / `rewrite_row`,
`RowEmpty` / a row variable in the tail). Concretely:

- **Duplicates are allowed and they stack** (chosen: "A", pure scoped, no
  warning). `@struct T = with A with B` where both `A` and `B` have `x` yields a
  row `{ A.x, B.x, ... }`, no error. Nesting rows is the whole point, same as
  `<Exn | Exn | r>` for effects.
- **Elimination resolves to the head (first) occurrence.** `.x`, `with rec in
  body`, and record-rest patterns all read the first `x` in the row; a shadowed
  duplicate is inert (reachable only through a future restriction operator
  `rec - x`, deferred). This is not a new rule to enforce: the runtime already
  does it. `Value::Struct.fields` is a `Vec<(name, value)>` and access is
  `.find(|(n,_)| n == name)`, which returns the first match; the C backend's
  `THxVALUE_field` is the same. So "first wins" is free, and matches how
  `rewrite_row` pulls the first occurrence of a label to the head for effects.

Consequence: **no runtime change.** Field access in both engines is already
name-keyed; row typing is a type-system-only change.

## The identity decision: named rows (hybrid), not fully structural

A declared `@struct` stays a **named closed row**. The name is the nominal anchor
that:

- lets a struct be **recursive** (a fully-structural record cannot refer to
  itself without iso-recursive types, out of scope),
- keeps **overload dispatch by struct type** and display working,
- is what `with T` / `T.{ .. }` construction name.

On top of that, **function signatures gain open rows**: `{ x:Int, y:Int | r }`
is "any record with at least `x:Int`, `y:Int`". A named struct **satisfies** an
open row it structurally matches (its closed row unifies with the open row,
binding `r` to the leftover fields). So:

```
$ area : { x:Int, y:Int | r } -> Int = \p = p.x * p.y
area (Point3.{ .x=1, .y=2, .z=3 })   # ok: r = { z:Int }
```

Rejected: **fully structural** (drops nominal identity; same-shape structs
collapse; recursion needs iso-recursive types) and **structural-alongside-
nominal** (two record systems to teach). Hybrid is additive and least
disruptive.

## How `with` interacts with rows

They are the same operation (row extension) at two ends:

- `with Other` in a **declaration** is *closed* concatenation by a **known** row.
  `@struct Point3 = with Point, z: Int` is the closed row `{ x, y, z }`. A
  declaration never carries a free row variable.
- `| r` in a **signature** is extension by an **unknown** tail. `with` does not
  appear there; the open tail does.

So `with Other` becomes "splice `Other`'s row", and the declaration-time splice
already implemented (see [[type-with-splice]]) is the closed case of the same
mechanism. `with A with B` stacks both rows (duplicates included).

## `with rec in body` on a row

Binds the statically-known **head** labels into scope:

- a duplicated label binds its head occurrence (total rule, no ambiguity);
- an **open tail** `{ x:Int | r }` binds only the known labels (`x`); the
  polymorphic tail `r` contributes nothing to bind, since its labels are unknown.

This resolves the roadmap's note that `with … in` needs the fields known.

## What this unlocks

- **Record-rest patterns** `is p | { .x = a, ..rest } => ...`, where `rest` has
  the remaining row type (the roadmap's deferred #5 tail-binding, blocked purely
  on this type not existing).
- Row-polymorphic field-updating helpers: `move : { x:Int | r } -> { x:Int | r }`.
- Anonymous record literals typed by their row.

## Representation (implementation sketch)

Effect rows are `RowExtend(String label, Box<Type> rest)` -- a label carries no
type. Records need a **per-label field type**, so add a record-row cons and a
wrapper:

- `Type::Record(Box<Type> row)` where `row` is `RowEmpty`, a `Type::Var` (open
  tail), or the new `Type::RowField(String label, Box<Type> field_ty, Box<Type>
  rest)`.
- Unification of two `Record`s mirrors `unify_row`/`rewrite_row`: pull a label to
  the head of the other row, **also unify the two field types**, recurse on the
  tails; an open tail grows to accept a missing label (that is the row-poly case).
- A named struct's declared fields become its closed record row (built once in
  `register_types`); the name is kept alongside for nominal unification and
  dispatch: two **named** records unify by name (today's behavior); a named
  record unifies with an **open** row structurally (the new behavior).
- Field access `infer_field` becomes a row lookup: head match returns the field
  type; on an open tail, grow the row with the requested label.

Runtime (`Value::Struct`, `Term::Struct`, `Term::Field`, C `THxVALUE_field`) is
**unchanged** -- all name-keyed already.

## Value syntax (locked)

- **Anonymous literal:** `{ .foo = 1, .bar = 2 }` (structural; type is its closed
  row). Nominal construction `Point.{ .. }` stays for named structs.
- **Update:** `{ .foo = f | area }` -- these fields override, the rest come from
  `area`. `|` reads the same as the type tail (`{ x:Int | r }` = these fields,
  rest is `r`), base/rest on the right. This **replaces** the old `..base` spread
  (`examples/RECORD_UPDATE.thx` migrates). Update preserves `area`'s row (no new
  fields).
- **Stack:** `{ .foo = 1, with area }` -- row is `{foo} ++ area`. Duplicates
  allowed (head wins), so this can add a field the target's closed row rejects but
  an open row absorbs. This is the value-level mirror of declaration `with`.

## Deferred / open

- Restriction operator `rec - x` to reach a shadowed duplicate. Not needed for A.
- No `lacks`/absence constraints (that was the no-duplicate model we rejected).
- Reconcile with the existing record-update `T.{ .f = v, ..base }` (base is a
  closed row; listed fields override -- already "later wins" there, vs "head
  wins" for `with`; both are consistent since update is closed).
- Codata (see doc/effect-system-design.md §1a) is separate, but an **observation
  record is itself row-shaped**, so a codata type could reuse this row machinery
  for its observations once both land.
