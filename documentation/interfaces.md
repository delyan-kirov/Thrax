# Interfaces and the `@ctx` context parameter

Status: **implemented** (issue #213). This replaces both the old by-name `@ctx`
design and function overloading, which is gone.

Thrax has two polymorphism mechanisms: generics, and interfaces resolved by type.
There used to be a third, function overloading, and it made a name's type
dishonest: `:type to_string` printed `to_string : a`, and `$ _ = to_string` failed
with a `RUNTIME_FAULT` instead of a type error. One name now has one definition and
one type.

## The whole mechanism

An interface is an ordinary `@struct` whose fields are its operations. Implementing
it is defining a value of that type. A function that needs one declares it as a
`@ctx` FIRST parameter: ordinary in every respect (it has a type, the body binds
it, `:type` prints it), except that call sites do not write it.

```
$ Ito_string : @struct t = show: t -> @str,

$ to_string : @ctx Ito_string t -> t -> @str = \d x = d.show x

$ show_int : Ito_string @int = .{ .show = int_to_str }

$ a : @str = to_string 1                 # the context is found by type
$ b : @str = to_string (@ctx show_hex) 255   # or written by hand
```

`:type to_string` prints `@ctx Ito_string a -> a -> @str`. The caller-visible type
is the arrow with that parameter stripped, so `to_string 1` is a complete call.

Rules, all of them:

1. **One context parameter, and it is first.** More than one, or one that is not
   first, is a declaration error. Several requirements travel as one tuple:
   `@ctx {IOrd t, ISub t, IZero t} -> t -> t`, and the call site BUILDS the tuple,
   so no global of the bundled type has to exist.
2. **The head must be nominal.** `@ctx t`, `@ctx @int` and
   `@ctx (a -> a -> Ordering)` are declaration errors: once grounded, `@ctx t`
   means "any value of this type in scope", which is a search over every value.
   Wrapping costs one line and moves the error to the declaration.
3. **Resolution happens at the enclosing definition's boundary**, after numeric
   defaulting, so `1 + 2` has `t` pinned to `@int` before any search runs.
4. **Search order**: the innermost matching local binder first (this includes the
   enclosing definition's own `@ctx` parameter, which is what makes generic code
   chain), then the instances in scope. Exactly one must match: zero and two are
   both errors, because global scope is flat.
5. **Recursive resolution.** A matching instance may itself carry a context
   (`$ eq_vec : @ctx IEq t -> IEq (@vec t)`), so resolution recurses into its
   requirement, bounded at 32 steps.
6. **No overlapping instances.** A ground instance and a generic one that both
   match is an ambiguity error, not most-specific-wins.
7. **Partial grounding is allowed.** The search unifies, so a unique match may pin
   the requirement's remaining variables. This is how `[lo ... hi]` finds its
   target type. Rule 2 keeps it from degenerating.
8. **Explicit form**: `f (@ctx e) x`, in argument position, first argument. The
   `@ctx` keyword is required there because the parameter is otherwise not
   addressable.

Four diagnostics, each with its own escape:

- *not determined here*: the requirement is still open at the boundary
  (`$ g : a -> a = \x = foo x`). Annotate the call, or declare the same `@ctx`
  parameter on `g` so chaining applies. The constraint is never inferred and added
  to `g`'s type silently; that would reintroduce the dishonesty.
- *no value of type `I T` is in scope*: define an instance.
- *ambiguous*: two instances of one grounded type. Annotation cannot help, so the
  note offers the explicit `(@ctx e)` form, or removing one.
- *non-nominal head*: rule 2, at the declaration.

## Elaboration

`lowering::def` leaves the body alone (the author wrote the parameter), and every
use site injects the resolved value as a leading argument (`apply_ctx`). A resolved
context is a `CtxVal`: a bare local name, a qualified global, an instance applied to
the context resolved for it, a tuple built here, a projection out of a bundle
already in scope, or an explicit expression. The backends need no support at all.

## What CORE provides

Arithmetic is one interface per operation, so a type implements exactly what it
has (`Money` adds but does not divide): `IAdd ISub IMul IDiv IMod IZero IOne IPow`.
An operator takes ONE type for both operands, so mixed-width arithmetic is written
explicitly (`@cast`, `f32_to_f64`, `CPX.of_real`); the 220 mixed-width operator
pairs are gone.

Comparison is `IEq` and `IOrd`. There is no structural fallback for `==`: a type
needs an `IEq` instance, by hand or from `DERIVE.derive_eq` / `derive_ord`. CORE has
instances for the base types, tuples, `@vec`, `@array` and `Cpx`.

The rest: `Ito_string` / `Ifrom_string` (which `"?(e)"` interpolation uses), `ICat`
(`++`), `ICons` (`::`), `IFor` / `IFormap` (`for` / `formap`, whose effect row is an
ordinary interface parameter: `$ IFor : @struct s a e = ...`), and `DERIVE`'s
three-way `ICmp`.

`MATH.min` / `max` / `clamp` / `abs` / `pow` are ONE definition each over these
interfaces, so they work for any type that has them, `@str` included.

## Blessed interfaces

Ten interface types are the compiler's own: a desugar site resolves a value of the
applied type and calls its single field. Their names carry the `@` sigil, only CORE
declares them, and each has exactly one field (validated at the declaration), so
the compiler needs the type name and never the method name.

```
$ @IRange : @struct b t = range: b -> b -> t,          # in CORE

$ range_span : @IRange @int Span = .{ .range = \lo hi = Span.{ lo, hi } }
```

The set and what reaches it:

| interface | surface |
| --- | --- |
| `@IIntLit` `@IRealLit` `@IImagLit` `@IStrLit` | a literal whose expected type is a user type |
| `@ISeqLit` | `[a, b, c]` |
| `@IRange` `@IRangeFrom` | `[lo ... hi]`, `[lo ...]` |
| `@IIndex` | `recv.[i]`, `recv.[i, j]` |
| `@ISlice` | `recv.[lo ... hi]` on a non-tensor |
| `@ISeqView` | `[]` / `[a, b, ..r]` / `h :: t` PATTERNS |

A literal PATTERN on a user type uses the construction interface plus the type's
ordinary `IEq` instance, so no separate equality hook exists.

Where nothing constrains a range's result, the FIRST instance declared wins, so
CORE's declaration order decides that a bare `[lo ... hi]` is a `@vec` and a bare
`[lo ...]` is a `Stream`. An annotation picks any other.

These replace the `@compiler_interface_*` hook family, which was built on the
overload resolver and is deleted.

## One name, one definition

- Two definitions of a name in one module is an error.
- A local definition SHADOWS an imported one of the same name.
- A name two imports bring in has no bare meaning: using it says so and names the
  owners, and `Module.name` still reaches either.

The one thing left that picks a definition from argument types is an effect
OPERATION whose name several effects declare (`ask` in both `Reader` and `Config`),
which resolves against the ambient effect. That is not name overloading: the
operations belong to different effects, and a handler clause always qualifies.
