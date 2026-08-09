# Row-polymorphic records (rows for structs)

Status: **implemented** end-to-end on both engines. Records are **real,
first-class, name-keyed types** (an earlier "decay to pairs" model was replaced).

- Record types: closed `{ x:Int, y:Int }` and open `{ x:Int | r }` (row variable
  tail). Values: `{ .x = 1, .y = 2 }`; access `p.x`; a missing field is a type error.
- **Order-independent** by construction (rows unify by name), so named arguments
  can be reordered: `f { .y = 2, .x = 1 }`.
- **Open rows** accept any record/struct with (at least) the named fields:
  `{ x:Int, y:Int | r } -> Int` takes `Point`, `{ .x, .y, .tag }`, etc.
- **Promotion at call arguments**: a bare scalar or a positional tuple passed
  where a record is expected is wrapped into it -- `foo 1` -> `foo { .x = 1 }`,
  `foo {1,2}` -> `foo { .x=1, .y=2 }` (declaration order). This keeps positional
  calls working and gives keyword-argument ergonomics.
- **Auto-bind parameter sugar** (kept): `add : { x:Int, y:Int } -> Int = x + y`
  binds `x`, `y` in the body (an implicit record destructuring).
- **Update / stack** on an open-row value: `{ .x = v | p }` preserves the shape
  (tail flows through), `{ .x = 1, with p }` concatenates.
- **Destructuring patterns**: `is p | { .x = a, .y = b, .._ } => ...` and lambda
  shorthand `\{ .x, .y } = ...`, on open-row values and nominal structs.

Remaining: **`..name` rest-BINDING** in patterns (needs a runtime record-restriction
op; `.._` discard works, `..name` errors clearly); **generic structs** at open rows
(only non-generic structs are bridged); the `..base` -> `{ | base }` migration (old
spread still works).

### Model

Records use the same scoped-row discipline as effects (duplicates stack, head
wins). There are three product-ish things: **nominal structs** (`Point`),
**records** (`{ x:Int, y:Int }` / `{ x:Int | r }`), and **tuples** (`{1, 2}`,
positional). They are distinct types; the only implicit conversion is the
call-argument **promotion** (scalar/tuple -> record).

Promotion is scoped to argument positions and kept out of general unification (to
preserve principal inference): at a call, the checker tries a direct unification of
the argument against the record parameter first -- which covers a record value and
a nominal struct (the `Con ~ Record` bridge) -- and only if that fails wraps a
scalar / tuple / struct into a **closed** record (an open row has no known field
names to wrap into, so a mismatch there stays a real error). A numeric literal
(still an undefaulted variable) is treated as a scalar and promoted rather than
unified.

Implementation: `Type::Record(row)` + `Type::RowField(label, ty, rest)` (engine.rs)
unified by `unify_record_row` / `rewrite_field` (mirrors the effect
`unify_row` / `rewrite_row`); `Engine::struct_rows` (non-generic structs' closed
rows, from `Checker::register_struct_rows`) is the `Con ~ Record` bridge; open/closed
record types parse via `Ty::Record { fields, tail }` and `ty_of_ast`; `infer_field`
is a row lookup; the param sugar auto-binds via field access (`bind_record_param` /
lowering `record_param`); promotion is recorded in `Checker::promotions` and wrapped
in lowering `promote_to_record`. Runtime is unchanged (already name-keyed).

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

- **Anonymous literal:** `{ .foo = 1, .bar = 2 }` -- a record value (name-keyed,
  order-independent). Positional `{1, 2}` and a bare scalar promote to a record at
  a call argument (see "The model"). Nominal construction `Point.{ .. }` stays.
- **Update:** `{ .foo = f | base }` -- these fields override, the rest come from
  `base` (which must be a record, i.e. an open-row value). `|` reads the same as
  the type tail (`{ x:Int | r }` = these fields, rest is `r`), base/rest on the
  right. Intended to replace the old `..base` spread (still present until migrated).
  Update preserves `base`'s row.
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
