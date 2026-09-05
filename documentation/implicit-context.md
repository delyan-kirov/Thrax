# Implicit context parameters (`@ctx`) and `to_string` interpolation

Status: feature 1 (`to_string` interpolation) is **DONE**, the overload blocker
below is **FIXED**, and feature 2 (`@ctx`) is **implemented (v1)**. The design and
the v1 scope/limitations are recorded below.

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
`M.kind#@int` and `M.kind#Bool` no longer collide. The mangling gate ("this module
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

## `@ctx` (implemented, v1)

Surface syntax:

```
$ max_of : a -> a -> a  @ctx compare : a -> a -> Ordering = \x y =
    is compare x y | Ordering.GT => x else y

max_of 3 7                 # `compare` resolved by name from scope
max_of 3 7 @ctx flip       # single explicit override
foo a b @ctx { .lt = f, .. }   # record override; `..` resolves the rest by name
```

A definition may append `@ctx name : Type` clauses after its signature (repeat
`@ctx`, or a `@ctx { a : A, b : B }` block). The implicit names are in scope in
the body. See `examples/IMPLICITS.thx`.

**Elaboration: leading dictionary passing.** `lowering::def` prepends one lambda
per implicit (`f = \c1 = \c2 = <body>`); every use site injects the resolved
values as leading arguments (`f c1 c2 x`). Backends are unchanged (both engines
green, byte-identical). The function's *checked* type stays the plain arrow, so
callers apply only the explicit parameters.

**Resolution (by name, `typing.rs`).** At a use site, `infer_var` intercepts a
`@ctx`-bearing global, instantiates its signature and each requirement with one
shared type-variable map (so `List a` and `compare : a -> a -> Ordering` share
`a`), and plans the implicits:

- an explicit `@ctx` override for that name is type-checked and used;
- else a **local** binder of the name wins (the caller's own `@ctx` param, so
  implicits chain: `max3` passes its `compare` down to `max_of`);
- else a **global** provider, deferred to the definition boundary and resolved
  once inference has pinned the requirement type (an overload is picked by that
  type). This is the monomorphic-at-boundary rule; a still-polymorphic
  requirement with no provider is the "no `name` in scope" error, pointed at the
  call site with a `note:` to pass it explicitly.

Deferring the global case (via `implicit_pending`, solved in
`resolve_pending_implicits` after `solve_pending`) is what makes `max_of 3 7`
know its implicit is over `@int` rather than a bare variable. The resolved
arguments are recorded per site (`implicit_calls` -> `Resolved::implicit_args`)
as `Bare` / `Qualified` (mangled if overloaded, reusing `overload_key`) / `Expr`.

Fixed along the way: `infer_app` no longer dispatches an overloaded name when a
local binder shadows it (a general scoping bug the local `@ctx` param exposed).

**v1 limitations (all documented, none silent):**

1. **No inferred constraint propagation.** A global provider is resolved only when
   the requirement is monomorphic at the enclosing definition's boundary;
   otherwise it errors. Propagation works *by name* instead: declare the same
   `@ctx name` on the caller and it chains (the local param satisfies the callee).
2. **Not on overloaded names.** A name cannot be both overloaded and `@ctx`-bearing
   (errors at registration).
3. **Qualified cross-module use does not inject.** A bare-imported `@ctx` function
   resolves (its metadata is copied in `import_from`); a `MOD.f` qualified use does
   not yet. Same gap family as qualified cross-module operators.
4. **Local (lexical) provider wins over a global**, not the reverse. This is what
   makes chaining/override authoritative; if a global default should win instead,
   flip the order in `plan_implicits`.
5. **No global coherence.** By-name + by-scope means different scopes can pick
   different providers. Intended (this is a strategy parameter, not a type class).

Not carried out from the original sketch: a `Type::Constrained(reqs, ty)`
representation. Keeping the requirements in a side table (`global_implicits`) off
the `Type` was enough for top-level defs and avoided rippling through unify /
generalize / display. First-class functions carrying implicits would need the
`Type`-level representation.

## Suggested order

1. ~~Fix same-module overload dispatch.~~ **DONE** (also fixed general
   overloading, which was silently broken).
2. ~~Add `to_string` overloads (in the auto-imported `CORE`) and desugar
   interpolation `{e}` to `to_string e`.~~ **DONE.**
3. ~~Build `@ctx` v1 (declaration, by-name resolution, dictionary passing,
   explicit override).~~ **DONE.** Possible next steps: qualified cross-module
   injection; inferred constraint propagation (needs `Type::Constrained`); a
   `Real -> Str` formatter for `CORE` (no float-formatting primitive exists yet).
