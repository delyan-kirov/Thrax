# Thrax Module Resolution

**Status:** implemented (Rust port). Combined corpus (`tests/MAIN.thx`) links all
50 example modules into one program and runs green on both the interpreter and
the C backend.

**Scope:** how a name written in one module resolves to a definition when many
modules are linked together, for both terms (functions and values) and types
(structs, unions, aliases, and effects).

---

## 1. The model

Every source file opens with `@mod NAME`. Files that share a name are one
module. A module exposes its public names to any module that imports it with
`$ with NAME`. The guiding rule for a name that an importer uses:

**Strip by default.** A public name is used unqualified. `$ with LIST` makes
`length`, `map`, ... available as bare `length`, `map`, and they also stay
reachable qualified as `LIST.length`.

That default only gets interesting when two imported modules expose the same
name. There are exactly three cases.

### 1a. Same name, different type: dispatch by type

If `A` and `B` both export `foo`, but with different types, a bare `foo` is an
overload set. The type checker picks the candidate that fits the use site by
trial-unifying each against the actual arguments and result. Exactly one fits,
so `foo x` just works and resolves to the right module. There is no
most-specific-wins: a candidate either fits or it does not.

### 1b. Same name, same type: genuine ambiguity, an error

If `A.foo` and `B.foo` have the *same* type, no use site can tell them apart.
This is a real ambiguity, so it is an error rather than a silent pick:

```
error[AMBIGUOUS_NAME]: ambiguous overloaded use of `foo`; several imported
modules define `foo` with a matching type. Qualify just this reference to pick
one, e.g. `A.foo` or `B.foo` (the rest of the module keeps using the bare name)
```

The fix is per reference, not per module. You do not have to import the whole
module qualified. Write `A.foo` at the one call that needs `A`'s version; every
other use of `foo` in the file keeps the bare form (and may resolve to `B`, or
stay ambiguous and get its own qualifier). You can also qualify both if you
prefer to be explicit.

### 1c. Qualified access always works

`MOD.name` names a definition directly and never needs an annotation. If `MOD`
exports several `name` overloads, the qualifier narrows to that module and the
usual type-directed pick runs among just its candidates.

---

## 2. Types and effects are namespaced per module

Type, effect, and constructor names follow the same "strip by default" model,
but with one structural guarantee: **every user-declared type belongs to its
module**. Two modules may each declare a `Pair` struct, a `Maybe` union, or an
`Exn` effect with entirely different shapes, and they never collide. Inside a
module, a bare type name resolves to that module's own declaration first, then
to an imported one. A type may also be written qualified (`A.Pair`), including
its constructors and patterns.

The prelude's built-in-ish global types are the exception and stay bare and
global everywhere: `@int`/`@nat`/`Real` and the sized numerics, `Str`, `Ptr`,
`Bool`, `Array`, `Vec`, and `List` (with its `Cons`/`Nil`). The auto-injected
`C` libc namespace is likewise reachable only as `C.name`.

Because each module is type-checked in its own scope (it sees only its own
declarations plus what it imports), same-named types in unrelated modules are
already distinct during checking. The only place they could be confused is
lowering, which is handled below.

---

## 3. How resolution is implemented

The Rust port has no separate MR pass; it resolves during type-checking and
lowering, but the outcome matches the C++ `MR` layer (which mangles every name
to its owning module up front).

**Terms.** The checker resolves every bare reference to a definition and records
the owning module, so lowering emits a canonical `Module.name` for it: a
local definition resolves to the current module; a single imported value to its
owner; an overloaded use to whichever candidate the types selected. Only names
the checker leaves bare reach the runtime unqualified: effect operations, the
program entry point, and built-in operators.

**The runtime glob order** (the interpreter's `machine::glob` and the C
backend's `glob_atom`) resolves a name in this order, and the order matters:

1. an EXACT canonical global (`Module.name`);
2. a built-in operator;
3. a `TARGET.` reflection member;
4. an effect operation (`Effect.op`, or a bare op with a single declaring
   effect);
5. LAST, the unqualified fallback, matched by last name-segment.

Effect operations sit *above* the unqualified fallback on purpose. An imported
global's last segment aliases into that fallback table, so if the fallback came
first, an unrelated imported `get` could shadow a `State.get` operation. Putting
operations first keeps a same-named import from capturing an operation
reference. (Variant matching is by tag only; the union type name a value carries
is not consulted when matching.)

**Types at lowering.** Positional struct and variant fields are labelled and
ordered from the type's declaration. Declarations are grouped by module, and a
layout lookup resolves against the lowering module's own types first, then the
modules it imports, then a global fallback. This is what keeps one module's
`Pair.{ x, y }` pattern bound to its own field order rather than a same-named
`Pair` from another module.

---

## 4. Tests

- `crates/interpreter/tests/run.rs`:
  - `cross_module_overload_dispatches_by_type` (1a),
  - `imported_global_does_not_shadow_a_same_named_effect_op` (the glob order),
  - `same_named_struct_types_in_two_modules_do_not_collide` (per-module layout).
- `tests/MAIN.thx` links every example module and is byte-identical between the
  interpreter and the compiled C program.
