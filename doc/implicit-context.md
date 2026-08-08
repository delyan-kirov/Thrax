# Implicit context parameters (`@ctx`) and `to_string` interpolation

Status: feature 1 (`to_string` interpolation) is **DONE**, and the overload
blocker below is **FIXED**. Feature 2 (`@ctx`) is still **design / TODO**; its
plan is kept below.

## The two features

1. **`to_string` in interpolation** (small). **DONE.** `"{3}"`, `"{person}"`
   desugar `{e}` to `to_string e` (in the parser's `build_string`), resolved by
   `e`'s type through ordinary overloading. Base types ship a `to_string : T ->
   Str` in the implicitly imported `CORE` module (`library/CORE.thx`, auto-loaded
   bare into every module, self-contained so no import cycle); a program adds
   `$ to_string : MyType -> Str` for its own types and the resolver picks it.
   `examples/STRING_INTERP.thx` covers base types, a user overload, and recursion.

2. **`@ctx` implicit parameters** (large). A Scala-`using`/`given`-style
   implicit argument system, resolved *by name*:

   ```
   sort : List A -> List A
       @ctx compare : A -> A -> Ordering
   ```

   `sort xs` resolves `compare` from scope; `sort xs @ctx my_compare` passes it
   explicitly. Multiple context params use record syntax, with `..` to fill the
   rest implicitly:

   ```
   foo : A -> B -> C  @ctx { .bar : ..., .baz : ... }
   foo a b @ctx { .baz = g, .. }
   ```

## FIXED: same-module overloading now dispatches at runtime

Discovered while attempting feature 1: **multiple overloads of one name in one
module collapsed to a single global.** Globals are keyed by `Module.name`
(`crates/frontend/src/ir/lower.rs`), so N same-module overloads shared one key and
only one survived; every call dispatched to it. The repro `(kind 7) + (kind true)
* 10` returned 11 instead of 21.

**Fix (shipped):** overloaded globals whose defining module declares the name
more than once are type-mangled to `name#<type-key>`. The checker derives the key
from the candidate's type (effect-free, variables canonicalized, `overload_key` /
`ty_key` in `typing.rs`), records the mangled name for each definition
(`def_keys`, by body handle) and each resolved use site (`overload_calls`), and
lowering emits it on both sides. `lower_modules` still prefixes the module, so
`M.kind#Int` and `M.kind#Bool` no longer collide. The mangling gate ("this module
contributes >1 candidate to the set") matches the definition-side gate ("declared
>1 time"), so cross-module-only overloads stay unmangled (the module prefix
already disambiguates them, as before). Mangled names are only ever string keys in
both runtimes (the C backend names functions `blk_N` by index), so the special
characters are safe. Regression tests: `same_module_overload_dispatches_by_type`
in `crates/interpreter/tests/run.rs` and `crates/ccg/tests/compile.rs`.

`core/PRELUDE.thx` was **deleted** (it was never loaded). `List`, `Bool`, `true`,
`false` are recognized by the compiler directly. `to_string` now lives in
`library/CORE.thx`, an ordinary standard-library file the driver auto-loads bare
into every module (checked first, imports nothing). Runtime `assert` is still
unbound (it lived in the deleted prelude); a future move of `assert` into `CORE`
would restore it.

## `@ctx` design

Pure elaboration to dictionary passing: `@ctx` params become ordinary trailing
arguments filled at each call site, so backends need no changes.

Reuse map (this is deliberately built from features we already have):

| `@ctx` piece | reuse |
|---|---|
| `@ctx name : T` on an arrow | effect-row `<...>` shape on the arrow type |
| resolve `name` by name+type in scope | the overload resolver (trial-unify vs expected type) |
| namespace-stripped wins on a tie | module-resolution rule (strip-by-default, qualify-one-ref) |
| `@ctx { .bar = f, .baz = g }` | struct/record literal syntax |
| `@ctx { .baz = g, .. }` (rest implicit) | record-update spread `..base` (base = ambient scope) |
| ambiguity error | `AmbiguousName` diagnostic + `note:` convention |

Resolution rules (as specified):

- Prefer an **implicit global** of the wanted name; otherwise the nearest thing
  in scope.
- Resolution is **order-independent**, like ordinary symbol resolution (forward
  references already work).
- On an undecidable tie between two modules, prefer the namespace-stripped
  (bare/local) candidate; else error.

Hard problems (decide before building):

1. **Constraint propagation** is the crux. When a `@ctx` requirement cannot be
   resolved because the type is still polymorphic, it must become a requirement
   on the enclosing definition (Haskell-style). **v1 scope:** resolve only when
   the context type is monomorphic at the call site; otherwise require the caller
   to declare `@ctx` explicitly (no inferred propagation).
2. **Name-based resolution footgun.** Any in-scope function with the right name
   and a matching type is silently used. Consider requiring instances to be
   top-level (not local binders), or a lint. (Scala 3 moved from bare implicits
   to `given`/`using` for this reason.)
3. **No global coherence.** By-name + by-scope means the same call can pick
   different instances in different scopes. Fine for a strategy parameter; state
   it clearly.
4. **Type representation.** Arrows must carry `@ctx` requirements (like a
   `Type::Constrained(reqs, ty)`, Haskell's `=>`), rippling into unification,
   generalization, and display.

## Suggested order

1. ~~Fix same-module overload dispatch.~~ **DONE** (also fixed general
   overloading, which was silently broken).
2. ~~Add `to_string` overloads (in the auto-imported `CORE`) and desugar
   interpolation `{e}` to `to_string e`.~~ **DONE.**
3. Design `@ctx`'s type representation and opt-in rule, then build it v1-scoped.
   (Still TODO. A known gap on the way: `Real -> Str` has no formatter yet, since
   there is no float-formatting primitive; `CORE` ships `Int`/`Bool`/`Str` only.)
