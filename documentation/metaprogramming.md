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

- `@token` -- a lexical token. **Opaque** (a distinct builtin type, `is_base_type`);
  inspected via the `@token_kind`/`@token_text` accessors, not pattern-matched.
  (A matchable `@union` form was considered and deferred in favour of the smaller
  opaque surface, consistent with `@code`.)
- `@code` -- an opaque handle to a typed-AST fragment. Opaque so the internal
  node set can evolve without breaking library code.
- `@span`, `@diag`, `@name`, `@typeinfo` -- supporting builtins.

**Pure intrinsics (no effect; usable inside `$ @run` today).** Lexing and
parsing need no compiler state, so they are ordinary intrinsics, not `<@meta>`
operations:

```
@lex        : @str      -> @vec @token   -- LANDED. a lex error traps (fails the build)
@token_kind : @token    -> @str          -- LANDED. "Word" | "Op" | "Int" | ...
@token_text : @token    -> @str          -- LANDED. the lexeme
@parse      : @vec @token -> @code       -- planned; a parse error traps
@parse_str  : @str      -> @code         -- planned; = @parse ∘ @lex
```

**Effect row**

- `<@meta>` -- the single metaprogramming effect. It carries both the pipeline
  operations *and* two-way communication with the compiler: if you are doing
  `@meta`, you can send messages to and receive messages from the compiler.
  There is no separate build effect (see section 10).

**`<@meta>` operations**

Pipeline and diagnostics (`@lex`/`@parse` are pure, see above; the rest need the
handler's compiler state):

```
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
before `f` runs. Raw-token input is requested explicitly by passing a string
literal through `@lex` (section 7):

```
@run (f x)                -- f sees the value / AST of x   (common case)
@run (f (@lex "x.y z"))   -- f sees []@token               (DSL / not-valid-Thrax case)
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

## 7. Quotation is a string literal

**Decision.** There is **no dedicated quotation syntax**. Source to be
metaprogrammed is written as an ordinary **string literal** and fed through the
pipeline that already exists: `@lex : @str -> []@token` and
`@parse : []@token -> @code`.

```
@lex   "let x = 1 in x"          : []@token
@parse (@lex "let x = 1 in x")   : @code       -- or a combined @parse_str
```

**Why a string, not a backtick quote.** It adds zero surface syntax: a string is
already the language's source-carrying literal, so quotation is just "lex/parse a
string." It composes with everything that produces an `@str` (a literal, a file
read at `@build` time, a string built from data), so the same path serves both
templating and source-in. This is the maximally small compiler surface, and it
keeps quotation firmly in the "trust the library" escape-hatch tier.

**Splice is string building.** A hole is filled by building the string: ordinary
concatenation (`++`) or `?(e)` interpolation (which is `to_string e`). So splicing
is **textual**, and the library author owns precedence, exactly as in any
string-based code generator:

- Precedence is not automatic: interpolate a parenthesized sub-expression when it
  matters (`"?(sub) * 2"` may need `"(?(sub)) * 2"`), since the tree cannot do it
  for you.
- Interpolation is `?(e)`, **not** `{e}` (that was changed precisely so quoted
  code stays clean): braces `{` `}` are ordinary literal characters inside a
  string, so records/blocks in quoted code need no escaping. Only a literal `?(`
  needs escaping, written `\?(`. A `"` inside the code still needs `\"` (or build
  with `++`).

**Spans.** A parsed string carries spans relative to the string body; `@parse`
maps a syntax error back to a location within the literal. Mapping that through
the enclosing file's escapes is best-effort, so a diagnostic in generated code is
less precise than one in hand-written source. Acceptable for the escape-hatch
tier; `@emit`/`@abort` (section 5) still let a generator raise a clear message of
its own.

A convenience `@parse_str : @str -> <@meta> @code` (= `@parse` of `@lex`) can be
provided so the common case is a single call.

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

**Landed (BUILD directives).** An `@run` whose value is a `BUILD.Directive`
(`Lib`/`LibPath`) steers the build: `compile_and_run_ct` reads the value via
`machine::eval_value` (not the string rendering) and collects a `BuildPlan`
`cmd_build` applies to the native link line (`-l` / `-L` + rpath), deduped
against the `@extern` libraries. The interpreter's default set already covers
libc/libm and lazily `dlopen`s the rest per `@extern`, so `thrax run` needs no
preload for the common case (a general preload hook is a later refinement).

**Still to come (the metaprogramming layer).** `@run` returning `@code` splices
and re-checks (recurses); `@run` under a `<@meta>` handler with the live `Ast`/
interner/type-env/diagnostic sink; `@emit`/`@abort` into the `Diagnostic` chain;
`@build` additionally installing `<@io>`.

---

## 12. Suggested build order

0. **DONE:** compile-time execution of `$ @run <expr>` (value discarded, trap
   fails the build; section 11), and a `BUILD.Directive` (`Lib`/`LibPath`) result
   steering the native link set / search paths (`library/BUILD.thx`,
   `examples/CT_RUN.thx`). **NEXT:** the typed layer below.
1. **DONE:** `@token` as an opaque builtin type, and `@lex : @str -> @vec @token`
   + `@token_kind`/`@token_text` accessors, as pure intrinsics runnable inside
   `@run` (a lex error traps). **NEXT:** `@parse`/`@parse_str -> @code`.
2. `@code` opaque handle over `Ast` + the `@run`-splices-`@code` path; prove the
   loop with an identity generator.
3. Quotation: none needed as syntax. `@lex`/`@parse`/`@parse_str` over string
   literals (section 7); splice is string building (`++` / `?(e)`).
4. The `<@meta>` effect + handler: start with `@parse`, `@emit`/`@abort`,
   `@here`, `@fresh`; wire `@emit` into the `Diagnostic` chain.
5. `@check` and `@eval` (staging), plus the `@code_*` constructors.
6. Compile-time IO once IO is effect-tracked (section 6).
7. Hygiene upgrade (syntax contexts), then the `@build` function and its
   `@msg`/`@add_code`/query fixpoint.

The load-bearing point: the two hard organs already exist. A compile-time
evaluator (CEK + `$ @run`) and a chainable diagnostic model. The macro system
is mostly *exposing* them, plus one expansion path on `Run` and quotation sugar.
