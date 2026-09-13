# Thrax Metaprogramming Design

**Status:** design draft. Nothing implemented yet. This records the model, the
decisions taken during design, and the intended build order.
**Date:** 2026-09-12.
**Scope:** compile-time metaprogramming: exposing the compiler pipeline
(tokens -> AST -> value) as a library, macro expansion, compiler messages, and
compile-time IO. Builds directly on the effect system
(documentation/effect-system-design.md) and CTFE (`$ @run`).

---

## 0. Goals and non-goals

Goal: application authors get good built-in syntax and never *have* to write a
metaprogram. The macro system exists for everything the compiler does not (and
should not) special-case: the escape hatch for what we forgot or chose to leave
out. Libraries own that surface; a build script may use it, ordinary code just
*calls* what libraries expose.

The design exposes every stage of the pipeline as a library so an author can
enter or leave at any level:

```
@str --@lex--> []@token --@parse--> @code --@check--> @code(typed) --@eval--> value
                                      |
                                      +--@unparse--> @str
```

You can build tokens and parse them yourself, build a typed AST directly, or
build code and run it. All three representations interconvert.

Non-goals for v1: full hygiene (gensym only at first), a stable public AST node
set (`@code` is opaque), and reproducible-build guarantees for compile-time IO
(see section 7).

---

## 1. Naming: everything built-in is `@`-form

Per the base-type convention (no friendly names in the compiler; `@int`,
`@vec`, `@array`, ...), all metaprogramming primitives are lowercase
`@`-intrinsics.

**Types**

- `@token` -- a lexical token: a kind tag plus a `@span`.
- `@code` -- an opaque handle to a typed-AST fragment. Opaque so the internal
  node set can evolve without breaking library code.
- `@span`, `@diag`, `@name`, `@typeinfo` -- supporting builtins.

**Effect row**

- `<@meta>` -- the single metaprogramming effect. It carries both the pipeline
  operations *and* two-way communication with the compiler: if you are doing
  `@meta`, you can send messages to and receive messages from the compiler.
  There is no separate build effect (see section 10).

**`<@meta>` operations**

Pipeline and diagnostics:

```
@lex    : @str      -> <@meta> []@token
@parse  : []@token  -> <@meta> @code
@check  : @code     -> <@meta> @code
@eval   : @code     -> <@meta> a
@emit   : @diag     -> <@meta> ()      -- non-fatal message
@abort  : @diag     -> <@meta> a       -- fatal message, unwinds
@fresh  : @str      -> <@meta> @name
@here   : ()        -> <@meta> @span
```

Compiler messages, receive (query the accumulated compile state):

```
@lookup    : @name  -> <@meta> @typeinfo
@modules   : ()     -> <@meta> []@module     -- what is imported
@functions : ()     -> <@meta> []@code        -- parsed functions
@types     : ()     -> <@meta> []@typeinfo
@flags     : ()     -> <@meta> []@flag        -- flags issued to the build
```

Compiler messages, send:

```
@add_code : @code   -> <@meta> ()      -- inject a top-level definition
@msg      : @a      -> <@meta> ()      -- send a message to the build (section 10)
```

`@lookup` is total: an unresolved name yields an erroneous `@typeinfo` (a
sentinel the value carries) rather than a `Maybe`. Callers test it via
`@typeinfo` predicates; the common case (name resolves) has no wrapper to
unwrap.

Plus AST constructors `@code_app`, `@code_var`, `@code_let`, ... for building
`@code` by hand.

**`@token` kind tags.** `@token` is exposed as a builtin `@union`; its variants
use ordinary capitalized constructors (`Word`, `Op`, `Int`, `LParen`, ...), so
matching reads `when t is Word then ...`. The `@` sigil stays on the builtin
surface (the type and the operations); it is not forced onto every tag. Only a
curated subset of the internal lexer `Kind` is exposed; internal-only tags
(`Comment`, `Eof`) stay hidden.

---

## 2. `@run` is the only eliminator

**Decision.** There is no dedicated macro-invocation syntax (`foo!(...)`).
`$ @run e` is the single construct that runs code at compile time, and it is
the eliminator for `<@meta>`. A macro is just a `<@meta>` computation that
`@run` discharges.

**Why.** `$ @run` already exists as CTFE (`Expr::Run` in the parser AST) and
already drives the CEK machine at compile time. Reusing it means no new parser
state, no token-tree slurping, and one concept instead of two. Macro expansion
becomes "`@run` returned a fragment."

**Arguments are ordinary expressions; raw tokens are opt-in.** Because
`@run (f a b)` is an application, `a` and `b` parse and check as normal Thrax
before `f` runs. Raw-token input is requested explicitly with a quote:

```
@run (f x)          -- f sees the value / AST of x   (common case)
@run (f `x.y z`)    -- f sees []@token               (DSL / not-valid-Thrax case)
```

This is more composable than Rust's implicit token slurp: the raw-ness is
visible at the call site, not hidden in the callee's signature.

**Rejected:** a separate `@splice` keyword alongside a value-only `@run`. One
eliminator is simpler and the result type disambiguates (section 3).

---

## 3. `@run` normalizes its body to a residual AST node (fixpoint)

`@run` does not "return a value" or "return code." It reduces its body to an
AST node with no residual `<@meta>` and nothing left unparsed. Elaborating
`@run e`:

1. **Check** `e`. Its residual effect row must be a subset of the effects the
   compile-time world discharges (section 6); its result type is `t`.
2. **Eval** `e` on the CEK machine with the compile-time handlers installed,
   giving a compile-time value `v`.
3. **Reify**, dispatching on `t`:
   - `t = []@token` -> `@parse v` to `@code`, then fall into the `@code` case.
   - `t = @code` -> the fragment may contain further `@run` nodes; expand those
     (recurse), then splice the resulting typed AST at this site.
   - any other `t` -> emit the AST node that denotes `v` (a literal leaf for a
     primitive, a constructor subtree for an aggregate).
4. **Re-check** the spliced/embedded AST against the context the original
   `@run` sat in.

Two loops are tangled here and separating them makes the recursion precise:

- The **staging pipeline** `[]@token -> @code -> value` is a straight line. Each
  artifact has one direction toward being embeddable: tokens want parsing, code
  wants splicing, a value wants reifying.
- The **recursion** iterates, and comes from two places: a returned `@code`
  fragment containing further `@run` nodes (step 3), or the meta computation
  calling `@eval`/`@parse`/`@run` internally (step 2). A depth/iteration budget
  bounds it; exceeding the budget is a diagnostic, not a hang.

**A value is the base case of code.** Reifying a value is producing the AST
subtree that denotes it, so `value` is not a separate outcome from `@code`, it
is where `@code` bottoms out.

### 3a. Cross-stage persistence

Not every compile-time value has an AST denotation. Primitives, strings, and
aggregates of them reify fine. A closure, a live file handle, or an opaque
`@meta`/`@code` handle cannot become source. Reification is total on the
reifiable types and produces a clean diagnostic ("cannot embed a compile-time
`T` into the program") on the rest, never a panic.

**Escape hatch for code-as-value.** When an author genuinely wants a
compile-time-computed `@code` *as a runtime constant* (a program shipping
fragments for its own later use) rather than spliced, they wrap it in a distinct
`@frozen` type that does not trigger splicing: `@run (@freeze e)` embeds instead
of splicing.

---

## 4. The two-phase check

To run the generator, `e` must be checked (well-typed, compiled for the CEK
machine). If it returns `@code`, the spliced result is checked *again* against
the surrounding context:

```
let n : @int = @run (gen ()) in ...
--              ^ checked as <@meta> @code to run it,
--                then the inserted fragment is checked against @int
```

So the `@run` node is type-checked twice: once for the generator, once for the
elaborated result. This localizes the "generated code has a type error" story
to the `@run` site.

---

## 5. Messages to the compiler ride the existing diagnostic chain

**Decision.** `@emit`/`@abort` push a `@diag` into the existing diagnostic sink.
No new error machinery.

**Why.** `utilities::error::Diagnostic` is already a frame chain (root cause
first, context frames appended as it unwinds, plus a closing note). The `@run`
handler wraps a macro's diagnostic with `.context(EXPANSION, run_site_span, ..)`
so the rendered chain reads "in expansion of ... : <the macro's message>."
`@here` returns the `@run` node's span for carets. `@abort` is a value-less
control operation (same shape as the `Loop` effect) that unwinds expansion.

This is the "send a useful message instead of a weird error" requirement, using
machinery already shipped.

---

## 6. Compile-time IO happens only inside `@build`

**Decision.** The compile-time runtime discharges exactly the effects its
context installs handlers for; a residual effect after the eliminator is a type
error. Ordinary `@run` installs `<@meta>` only, so it is **hermetic**: a library
macro can compute, parse, query, and emit, but cannot touch the world.
Compile-time IO is available only inside `@build` (section 10), the context that
additionally discharges `<@io>`. Building is inherently IO, so this is where IO
belongs and nowhere else.

**Why the boundary is the context, not a keyword.** `@build` is a special
function (like `MAIN.main`), and the difference between it and ordinary `@run`
is purely which handlers the compile-time runtime installs around it: `<@meta>`
for `@run`, `<@meta>` + `<@io>` for `@build`. Nothing in the surface syntax
changes; a macro that performs `@io` simply fails to type-check outside `@build`
because no handler for it is in scope.

An IO result reifies into the AST by the section 3 rules: inside `@build`,
`read_file "x.sql"` genuinely reads during compilation and its bytes embed as a
literal.

Status: `@io` is now a real builtin effect. The effect-row parser accepts
`@`-form labels, IO primitives (`library/C.thx` OS externs, `library/IO.thx`)
carry `<@io>`, and the checker enforces it (a pure-typed function that performs
IO is rejected: "effect `@io` is performed but not handled"). Effects propagate
through the effect-polymorphic combinators and are absorbed by `main`'s open
row. What remains for this design is the compile-time side: `@build` installing
an `<@io>` handler so the effect is *discharged* (performed for real) during
compilation, rather than only absorbed at runtime by `main`.

**Hermeticity.** Compile-time IO makes the build depend on the world. The
`<@io>` handler installed around `@build` records what it touched, so the build
system can treat "read `x.sql` at compile time" as a build input for caching,
the way a normal source dependency is tracked. Full reproducibility guarantees
are deferred.

---

## 7. Quotation (surface sugar)

There is **one** quotation primitive: a backtick pair, which produces `[]@token`.
`@code` is not a quoter, it is `@parse` applied to a token quote. So quotation
adds only a lexer rule, no new function (`@parse` already exists).

```
`let x = ?(body) in x`          : []@token         -- the sole quotation primitive
@parse `let x = ?(body) in x`   : <@meta> @code     -- code is parse-of-tokens
```

**Why backticks.** The backtick is free: it is not lexed for anything, and type
variables are plain lowercase-initial words, not `` `a `` (the `` `a `` in the
`Ty::Var` doc comment is stale). It carries the universal "quote" meaning (Lisp
quasiquote, markdown, shell). Unlike braces it collides with none of records /
codata / blocks / `{e}` string interpolation, and unlike a string container it
needs no escaping: quoted Thrax freely contains `"`, `{`, and `}` because none of
them is the delimiter. Tokens keep real source spans (section 5 depends on this),
and a backtick pair reads verbatim across multiple lines.

**Splice is token-level but typed, and parenthesizes to stay structural.** At a
`?(e)` site inside a quote:

- `e : []@token` -> splice the tokens verbatim.
- `e : @code` -> splice its token form wrapped in `( ... )`, so expression
  precedence is preserved: `?(a + b)` in `?(x) * 2` yields `(a + b) * 2`, never
  `a + b * 2`.
- `e : @value` (int/str/...) -> splice the literal token.

`?..(es)` splices a list (matches the `..rest` / `..base` spread convention). The
residue is non-expression positions (splicing into a pattern, a type, a statement
list), where blind parens do not apply and the library author is responsible for
splicing sensible tokens. That residue is acceptable: the token layer is the
"trust the library" escape hatch by design.

**Why `?(...)` for splice.** `?` is otherwise free: it appears only in the
compound operators `?=`/`?<`/`?>`. Parenthesizing (`?(`, never a bare `?<`) keeps
it clear of those, and `?` will never be used for optional types, so nothing in
quoted type position collides. It reads as "hole here."

**Cost of the single primitive.** A quote's syntax is checked at *expansion* time
(when the macro runs `@parse`), not at the macro's own compile time. If early
checking is wanted, keep an `@code `...`` sugar defined as `@parse `...`` plus
AST-hole splice; it is sugar, never a second primitive.

**Rejected: strings as the quote container.** `@code "let x = {body}"` is the C
preprocessor model. `{e}` already means `to_string e`, so splice would be
textual; code inside `"..."` must escape every `"`; and re-lexed strings lose
span precision. Strings stay the right tool for *source-in* (`@lex`/`@parse` over
an `@str` read from a file or built from data), just not for templating.

**Literal backtick in a DSL** (a foreign DSL that itself uses backticks): a
triple-backtick fenced form is the escape hatch. Not needed for v1.

---

## 8. Phase ordering

A generator must be fully checked and CTFE-evaluable before its first use. This
is the constraint `$ @run` already imposes, not a new one.

- Cross-module: the generator's module is compiled before modules that use it
  (topological order, already required).
- Same-module: allowed only when the generator and everything it transitively
  calls are CTFE-safe.

---

## 9. Hygiene

v1 ships **gensym only**: `@fresh` mints fresh `@name`s for macro-introduced
bindings; identifier capture is a documented footgun. `@code` carries interned
names and spans, so a later upgrade tags macro-introduced identifiers with a
syntax-context id and resolves names within their own context, without changing
the surface.

---

## 10. `@build`, the special build function

**Decision.** `@build` is a special function, the compile-time analogue of
`MAIN.main`: the compiler recognizes it by name and runs it during compilation.
It is **not** a separate effect. Everything it does (query the compiler, inject
definitions, send/receive messages) is expressed through `<@meta>` (section 1);
its one privilege over ordinary `@run` is that the runtime around it also
discharges `<@io>` (section 6), because building is IO.

**Messages flow through `<@meta>`, from anywhere to `@build`.** Any
metaprogramming code can `@msg` a message or `@add_code` an injection, because
those are `<@meta>` operations and any such code has `<@meta>`. Sending is not
IO, so it stays within the hermetic default; the messages are queued and *run*
at compile time. `@build` is the consumer: it receives them (and queries state
via `@modules`/`@functions`/`@types`/`@flags`) and, being the only IO-capable
context, is where any world-touching reaction happens. A library's `@msg` is
therefore a request that the consumer's own `@build` answers, which keeps the
consumer in control.

**Iterative to a fixpoint.** Injection must land before dependent code is
checked, so `@build` is not a single end-of-run pass. The compiler runs it,
applies its injections and drains its messages, re-processes the delta, and
repeats until nothing new is produced, under a budget (the same discipline as
`@run` expansion in section 3). This is the Jai message-loop model expressed as
a fixpoint rather than a manual `while` over `compiler_get_message`.

Deferred until inline macros (`@run` + quotation) land.

---

## 11. What changes in the compiler

`@run`/`@assert` are top-level items (`Item::Run`/`Item::Assert`), not
expressions. In the Rust port they were parsed but inert (the C++ CTFE was never
ported).

**Landed (compile-time execution of `@run`).** `$ @run <expr>` now runs at
compile time on every backend:

1. Checker: after all defs are checked, each `Item::Run` expression is inferred
   under a fresh pure ambient, so it resolves like a top-level body and an effect
   it performs that the compile-time runtime cannot discharge (e.g. `<@io>`) is
   rejected (`typing.rs`).
2. Lowering: each `@run e` becomes a synthetic global `@run#i` pushed into
   `Program.globals`, with its bare name recorded in the new `Program.ct_runs`
   (`lowering.rs`, `lowering/data.rs`). It flows through `ir::lower` as an
   ordinary global, so no IR change was needed.
3. Driver: `compile_and_run_ct` builds the interpreter IR and forces each
   `Module.@run#i` global via `machine::eval`. The value is discarded; a trap
   becomes a build error. Runs before the entry, shared by `run`/`build`/
   `emit-c` (`driver.rs`). A user-land `assert` is therefore just an `@run` whose
   expression traps on a false condition; no `@assert` builtin is needed.

**Still to come (the metaprogramming layer).** `@run` returning `@code` splices
and re-checks (recurses); `@run` under a `<@meta>` handler with the live `Ast`/
interner/type-env/diagnostic sink; `@emit`/`@abort` into the `Diagnostic` chain;
`@build` additionally installing `<@io>`. A `BUILD.Directive` result steering the
link set (slice 2) is the next increment.

---

## 12. Suggested build order

0. **DONE:** compile-time execution of `$ @run <expr>` (value discarded, trap
   fails the build; section 11). **NEXT:** a `BUILD.Directive` result from `@run`
   steering the link set / search paths (`library/BUILD.thx`, `examples/CT_RUN.thx`).
1. Surface `@token`/kind tags as a Thrax type.
2. `@code` opaque handle over `Ast` + the `@run`-splices-`@code` path; prove the
   loop with an identity generator.
3. Quotation: the backtick token quote (pure lex), then `@parse` for `@code`
   and the typed, parenthesizing `?(...)` splice.
4. The `<@meta>` effect + handler: start with `@parse`, `@emit`/`@abort`,
   `@here`, `@fresh`; wire `@emit` into the `Diagnostic` chain.
5. `@check` and `@eval` (staging), plus the `@code_*` constructors.
6. Compile-time IO once IO is effect-tracked (section 6).
7. Hygiene upgrade (syntax contexts), then the `@build` function and its
   `@msg`/`@add_code`/query fixpoint.

The load-bearing point: the two hard organs already exist. A compile-time
evaluator (CEK + `$ @run`) and a chainable diagnostic model. The macro system
is mostly *exposing* them, plus one expansion path on `Run` and quotation sugar.
