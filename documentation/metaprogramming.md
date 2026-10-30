# Thrax Metaprogramming Design

**Status:** design draft. Nothing implemented yet. This records the model, the
decisions taken during design, and the intended build order.
**Date:** 2026-09-12.
**Scope:** compile-time metaprogramming: exposing the compiler pipeline
(tokens -> AST -> value) as a library, macro expansion, compiler messages, and
compile-time IO. Builds directly on the effect system
(documentation/effect-system-design.md) and CTFE (`$ @e`).

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
@Str --@lex--> []@Token --@parse--> @Code --@check--> @Code(typed) --@eval--> value
                                      |
                                      +--@unparse--> @Str
```

You can build tokens and parse them yourself, build a typed AST directly, or
build code and run it. All three representations interconvert.

Non-goals for v1: full hygiene (gensym only at first), a stable public AST node
set (`@Code` is opaque), and reproducible-build guarantees for compile-time IO
(see section 7).

---

## 1. Naming: everything built-in is `@`-form

Per the base-type convention (no friendly names in the compiler; `@Int`,
`@Vec`, `@Array`, ...), all metaprogramming primitives are lowercase
`@`-intrinsics.

**Types**

- `@Token` -- a lexical token. **Opaque** (a distinct builtin type, `is_base_type`);
  inspected via the `@token_kind`/`@token_text` accessors, not pattern-matched.
  (A matchable `@union` form was considered and deferred in favour of the smaller
  opaque surface, consistent with `@Code`.)
- `@Code` -- an opaque handle to a typed-AST fragment. Opaque so the internal
  node set can evolve without breaking library code.
- `@span`, `@diag`, `@name`, `@typeinfo` -- supporting builtins.

**Pure intrinsics (no effect; usable inside `$ @e` today).** Lexing and
parsing need no compiler state, so they are ordinary intrinsics, not `<@Meta>`
operations:

```
@lex        : @Str      -> @Vec @Token   -- LANDED. a lex error traps (fails the build)
@token_kind : @Token    -> @Str          -- LANDED. "Word" | "Op" | "Int" | ...
@token_text : @Token    -> @Str          -- LANDED. the lexeme
@parse_str  : @Str      -> @Code         -- LANDED. parses an expr fragment; a syntax error traps
@parse_items: @Str      -> @Code         -- LANDED. parses top-level item(s), for `$ @e` injection
@parse      : @Vec @Token -> @Code       -- LANDED. detokenizes + validates like @parse_str
@format     : @Str      -> @Str          -- LANDED. items in `thrax fmt` layout; a refusal traps
```

`@Code` is opaque; it currently carries the fragment's source text (enough for
the future `@eval`/splice). `@parse_str` validates syntax now (a bad fragment
fails the build); type-checking and evaluation come with the consumers below.

**Effect row**

- `<@Meta>` -- the single metaprogramming effect. It carries both the pipeline
  operations *and* two-way communication with the compiler: if you are doing
  `@Meta`, you can send messages to and receive messages from the compiler.
  There is no separate build effect (see section 10).

**`<@Meta>` operations**

Pipeline and diagnostics (`@lex`/`@parse` are pure, see above; `@eval` is pure
too, see "Runtime use" below; the rest need the handler's compiler state):

```
@eval   : @Code     -> a               -- LANDED. compiles and runs a pure fragment; also at run time
@abort  : @Str      -> <@Meta> a       -- LANDED. fail the build with this message
@emit   : @Str      -> <@Meta> {}      -- LANDED. print a message and continue
@fresh  : @Str      -> <@Meta> @Str    -- LANDED. a unique identifier (prefix + counter), for hygiene
@link      : @Str   -> <@Meta> @Code   -- LANDED. add a library to the link line (empty code)
@link_path : @Str   -> <@Meta> @Code   -- LANDED. add a search path to the link line (empty code)
@check  : @Code     -> <@Meta> @Code   -- planned
@here   : ()        -> <@Meta> @span   -- planned
```

**LANDED (the `<@Meta>` effect, first cut).** The ops above carry the `<@Meta>`
effect in their types, and **`@e` discharges it** (see section 2). Every `@e`
position (item `$ @e X`, expression `@e X`, type `@e X`) runs its operand under
a CLOSED `<@Meta>` ambient: a meta op inside `@e` type-checks and the
surrounding context stays pure, while a meta op in ordinary code is a clean
"effect `@Meta` is performed but not handled" error (no more runtime
no-op/fault). A function that performs a meta op declares `<@Meta>` in its
signature (like `<@IO>`); e.g. CORE's
`assert : @Bool -> @Str -> <@Meta> @Code`, run by `$ @e <| assert ...`. The
ambient is closed (only `@Meta`), so `@e` stays hermetic: a generator that
performs `@IO` is still rejected. Lexing and parsing
(`@lex`/`@parse`/`@parse_str`/`@parse_items`/`@token_*`) remain PURE. Still to come: a real handler over the live
`Ast`/interner/type-env/diagnostic sink (replacing the thread-local host),
`@lookup`/`@here`/`@check`, and a richer `@diag` (spans) for `@abort`/`@emit`.

`@eval` rides a driver-installed thread-local host rather than a `<@Meta>`
handler.

**Runtime use (#187).** `@lex`, `@token_kind`/`@token_text`, `@parse`,
`@parse_str`, `@parse_items`, `@format` and `@eval` are pure, so ordinary code may
call them (`examples/META_RUNTIME.thx`). `tools/README_GEN.thx` runs `@format`
under `$ @e` to lay out the README's snippets at compile time. `@eval` is pure
because its fragment is checked as a pure def body (a closed effect row, unlike
the shell's open one), so it computes a value from its source and nothing else.
Each engine needs the compiler at run time to serve them:

- The interpreter links the compiler, so `thrax run` and the shell install the
  `@eval` host for the whole run (`driver::install_eval_host`).
- A native program calls the compiler as a C library. The `thrax` crate builds as
  `libthrax.a` and `libthrax.so` too, with the C API in
  `crates/thrax/include/thrax.h` (`thrax_lex`, `thrax_parse`, `thrax_eval`, values
  as an owned `thrax_value` tree). `ccg` notes when a reachable block names one of
  these builtins (the `BuiltinHost::Compiler` entries of
  `frontend::ir::data::BUILTINS`), emits the header and
  `THRAX_WITH_COMPILER` ahead of `runtime.c` (whose bridge converts the tree into
  runtime `Value`s), and `thrax build` links `libthrax.so` with an rpath. It
  uses the shared library because that one carries its own dependencies (libffi).
  Only the host target has the library; a cross build that needs it is an error.

As at compile time, only first-order data crosses (a function, or a `List` whose
lazy constructor fields are still unforced, is rejected), and the result type is
whatever the use site says, so a mismatch is a fault when it runs. `@abort`/`@emit`
take a plain `@Str` for now (a richer `@diag` with spans comes with the handler).
A compile-time `@e` fault, or an `@abort` (`error[META_ABORT]`), is reported at the real `@e` call site
(`file:line:col` + caret), not a synthetic global, since every `@e` site records
its source span.
There is no `@assert` builtin: assert is user code,
`$ @e (if ok => {} else @abort "...")` (see `examples/CT_ASSERT.thx`).

Compiler messages, receive (query the accumulated compile state):

```
@lookup    : @name  -> <@Meta> @typeinfo
@modules   : ()     -> <@Meta> []@module     -- what is imported
@functions : ()     -> <@Meta> []@Code        -- parsed functions
@types     : ()     -> <@Meta> []@typeinfo
@flags     : ()     -> <@Meta> []@flag        -- flags issued to the build
```

Compiler messages, send:

```
@add_code : @Code   -> <@Meta> ()      -- inject a top-level definition
@msg      : @a      -> <@Meta> ()      -- send a message to the build (section 10)
```

`@lookup` is total: an unresolved name yields an erroneous `@typeinfo` (a
sentinel the value carries) rather than a `Maybe`. Callers test it via
`@typeinfo` predicates; the common case (name resolves) has no wrapper to
unwrap.

**LANDED (a first cut of type reflection).** Ahead of the full `<@Meta>`
`@lookup`, three effect-less reflection builtins expose a declared type's shape
to a `$ @e` generator:

```
@type_kind     : @Str -> @Str                -- "struct" | "union"
@type_params   : @Str -> @Vec @Str           -- declared type parameters, in order
@type_fields   : @Str -> @Vec @Str           -- a struct's field names
@type_variants : @Str -> @Vec {@Str, @Int}   -- a union's (tag, arity) pairs
```

The argument is a type name, bare (`"Rgb"`) or qualified (`"MOD.Rgb"`; qualify to
disambiguate a name shared across modules). They resolve through a driver-installed
host (`machine::set_type_host`, fed from `Declarations::reflect`), available only inside
`$ @e`; a stray call at runtime faults. On top of them, `library/DERIVE.thx`
derives a default `to_string` for any struct or union (`$ @e (DERIVE.derive_show
"Rgb")`); see `examples/DERIVE_SHOW.thx`. This is the first real derive-style
macro, exercising reflection + forward-referenced injection (2b) + overload
extension together.

**Generic types.** `@type_params` lets the derive read a type's parameters, so a
generic type derives a single generic instance with one `@ctx to_string`
dictionary per parameter: `$ to_string : Pair a b -> @Str  @ctx to_string : a ->
@Str, to_string : b -> @Str`. Each dictionary renders its parameter, and one
instance covers every instantiation (`Box @Int`, `Pair @Int @Bool`, nesting
through each element's own derived `to_string`). This is `instance (Show a, Show
b) => Show (Pair a b)`, and it needed two checker changes:

1. Lifting the old rule that a name could not be *both* overloaded and carry
   `@ctx` implicits: an overload candidate now carries its implicit requirements
   (sharing the signature's type variables), and a call resolving to it plans the
   dictionary at the site (`Cand::implicits`, `apply_overload_cand`,
   `scheme_with_implicits`).
2. Resolving *same-named* `@ctx` dictionaries **by type**. A body use of
   `to_string` inside the instance selects the dictionary whose parameter matches
   the argument's type; when the argument is a bare type variable, resolution is by
   variable identity (`current_dicts`, `dict_calls`, the `bare_var_id` narrowing in
   `typing.rs`; lowering binds duplicated dictionaries as `@ctx$<slot>` params).
   This also fixed a latent bug where two same-named `@ctx` implicits silently
   shadowed, miscompiling.

The `@ctx` declaration syntax is a flat comma list, repeatable: `@ctx a : A, b :
B`. (The former `@ctx { ... }` block form was removed.) Lowering already composed
overload-rewrite with implicit-arg injection, so it needed no change beyond the
duplicated-dictionary binder naming.

Plus AST constructors `@code_app`, `@code_var`, `@code_let`, ... for building
`@Code` by hand.

**`@Token` kind tags.** `@Token` is exposed as a builtin `@union`; its variants
use ordinary capitalized constructors (`Word`, `Op`, `Int`, `LParen`, ...), so
matching reads `when t is Word then ...`. The `@` sigil stays on the builtin
surface (the type and the operations); it is not forced onto every tag. Only a
curated subset of the internal lexer `Kind` is exposed; internal-only tags
(`Comment`, `Eof`) stay hidden.

---

## 2. `@e` inserts code

**Decision (revised again, reverting #159).** There is no dedicated
macro-invocation syntax (`foo!(...)`) and there is one compile-time construct:
**`@e X` runs `X` at build time and inserts the code it returns at its site.**
`X` must be an `@Code`; any other operand is a type error.
Computing at build time is not a feature of its own: a generator computes
whatever it likes, then renders the result to code (`@parse_str (to_string n)`)
or builds code from data. `X` runs under a closed `<@Meta>` ambient, so the meta
ops (`@abort`, `@emit`, `@fresh`, `@link`, `@link_path`) discharge there and any
other effect, `@IO` included, is rejected. Compile-time IO lives solely in
`@build` (sections 6 and 10).

Empty code inserts nothing, and that is the escape hatch for a site run only for
its effect. `CORE.assert` and `@link`/`@link_path` return empty code, so
`$ @e <| assert ok msg` and `$ @e <| @link "m"` are statements that vanish once
they have run.

A short-lived split (#159) had `@e` run a PURE operand and embed its value, and a
sibling `@run` discharge `<@Meta>`. Both did the same thing apart from the
ambient, and value embedding duplicated what a generator already does by
returning code, so `@run` is gone and `@e` no longer folds values. `$ @run` is a
parse error that points at `$ @e`.

**What `@e X` is, precisely.** `@e` is not an ordinary function. It is a splice
at a fixed site: `X` is a closed compile-time expression, run once, now, whose
`@Code` result replaces the site in source. What the code denotes is known only
once it is spliced and re-checked, so the site types as a fresh variable until
then. In item position the code holds top-level items; in type position, a type.

**Why.** `$ @e` already exists as CTFE (`Expr::Run` in the parser AST) and
already drives the CEK machine at compile time. Reusing it means no new parser
state, no token-tree slurping, and one concept instead of two. Macro expansion
becomes "`@e` returned a fragment."

**Arguments are ordinary expressions; raw tokens are opt-in.** Because
`@e (f a b)` is an application, `a` and `b` parse and check as normal Thrax
before `f` runs. Raw-token input is requested explicitly by passing a string
literal through `@lex` (section 7):

```
@e (f x)                -- f sees the value / AST of x   (common case)
@e (f (@lex "x.y z"))   -- f sees []@Token               (DSL / not-valid-Thrax case)
```

This is more composable than Rust's implicit token slurp: the raw-ness is
visible at the call site, not hidden in the callee's signature.

**Rejected:** a separate `@splice` keyword alongside a value-only `@e`, and a
separate `@run` alongside a pure `@e` (#159, reverted). One construct that
inserts code is enough.

---

## 3. `@e` normalizes its body to a residual AST node (fixpoint)

`@e` does not "return a value" or "return code." It reduces its body to an
AST node with no residual `<@Meta>` and nothing left unparsed. Elaborating
`@e e`:

1. **Check** `e`. Its residual effect row must be a subset of the effects the
   compile-time world discharges (section 6); its result type is `t`.
2. **Eval** `e` on the CEK machine with the compile-time handlers installed,
   giving a compile-time value `v`.
3. **Splice**: `t` is `@Code` (the checker requires it). The fragment may
   contain further `@e` nodes; the next round expands those (recurse), and the
   fragment's source replaces this site.
4. **Re-check** the spliced AST against the context the original `@e` sat in.

Two loops are tangled here and separating them makes the recursion precise:

- The **staging pipeline** `[]@Token -> @Code -> value` is a straight line. Each
  artifact has one direction toward being embeddable: tokens want parsing, code
  wants splicing, a value wants reifying.
- The **recursion** iterates, and comes from two places: a returned `@Code`
  fragment containing further `@e` nodes (step 3), or the meta computation
  calling `@eval`/`@parse`/`@e` internally (step 2). A depth/iteration budget
  bounds it; exceeding the budget is a diagnostic, not a hang.

**A value is not inserted directly.** A generator that computes a value renders
it to code itself (`@parse_str (to_string n)`), so `@e` has exactly one outcome.

### 3a. Cross-stage persistence

This applies to `@eval`, whose fragment's value crosses back into the running
program; `@e` itself only ever inserts code. Not every compile-time value has
an AST denotation. Primitives, strings, and
aggregates of them reify fine. A closure, a live file handle, or an opaque
`@Meta`/`@Code` handle cannot become source. Reification is total on the
reifiable types and produces a clean diagnostic ("cannot embed a compile-time
`T` into the program") on the rest, never a panic.

**Escape hatch for code-as-value.** When an author genuinely wants a
compile-time-computed `@Code` *as a runtime constant* (a program shipping
fragments for its own later use) rather than spliced, they wrap it in a distinct
`@frozen` type that does not trigger splicing: `@e (@freeze e)` embeds instead
of splicing.

---

## 4. The two-phase check

To run the generator, `e` must be checked (well-typed, compiled for the CEK
machine). If it returns `@Code`, the spliced result is checked *again* against
the surrounding context:

```
let n : @Int = @e (gen ()) in ...
--              ^ checked as <@Meta> @Code to run it,
--                then the inserted fragment is checked against @Int
```

So the `@e` node is type-checked twice: once for the generator, once for the
elaborated result. This localizes the "generated code has a type error" story
to the `@e` site.

---

## 5. Messages to the compiler ride the existing diagnostic chain

**Decision.** `@emit`/`@abort` push a `@diag` into the existing diagnostic sink.
No new error machinery.

**Why.** `utilities::error::Diagnostic` is already a frame chain (root cause
first, context frames appended as it unwinds, plus a closing note). The `@e`
handler wraps a macro's diagnostic with `.context(EXPANSION, run_site_span, ..)`
so the rendered chain reads "in expansion of ... : <the macro's message>."
`@here` returns the `@e` node's span for carets. `@abort` is a value-less
control operation (same shape as the `Loop` effect) that unwinds expansion.

This is the "send a useful message instead of a weird error" requirement, using
machinery already shipped.

---

## 6. Compile-time IO happens only inside `@build`

**Decision.** The compile-time runtime discharges exactly the effects its
context installs handlers for; a residual effect after the eliminator is a type
error. Ordinary `@e` installs `<@Meta>` only, so it is **hermetic**: a library
macro can compute, parse, query, and emit, but cannot touch the world.
Compile-time IO is available only inside `@build` (section 10), the context that
additionally discharges `<@IO>`. Building is inherently IO, so this is where IO
belongs and nowhere else.

**Why the boundary is the context, not a keyword.** `@build` is a special
function (like `@main`), and the difference between it and ordinary `@e`
is purely which handlers the compile-time runtime installs around it: `<@Meta>`
for `@e`, `<@Meta>` + `<@IO>` for `@build`. Nothing in the surface syntax
changes; a macro that performs `@IO` simply fails to type-check outside `@build`
because no handler for it is in scope.

An IO result reifies into the AST by the section 3 rules: inside `@build`,
`read_file "x.sql"` genuinely reads during compilation and its bytes embed as a
literal.

Status: **LANDED.** `@IO` is a real builtin effect: the effect-row parser accepts
`@`-form labels, IO primitives (`library/C.thx` OS externs, `library/IO.thx`)
carry `<@IO>`, and the checker enforces it (a pure-typed function that performs
IO is rejected: "effect `@IO` is performed but not handled"). Effects propagate
through the effect-polymorphic combinators and reach the entry, whose `<@IO>` row
is what discharges them at run time.

The compile-time side is in too, and it needed no handler installation: `@build`
*declares* `<@Meta, @IO>` in its own signature (section 10), so its body
type-checks against that row like any other function, and the compile-time
interpreter performs the IO for real (its FFI is the same one the program uses).
`$ @e` keeps its hermetic ambient, so the boundary holds:

```
$ @e <| IO.read_dir "."     # error: effect `@IO` is performed but not handled
$ @build : {} -> <@Meta, @IO> @Code = \_ = ... IO.read_dir "." ...   # fine
```

**There is no compile-time IO primitive, and deliberately so.** Compile-time IO
is the *same* IO a program does at run time: `library/IO.thx` over the `C`
namespace, performed by the compile-time interpreter through the ordinary FFI.
Nothing in the compiler is marked "build-time only", so a function is usable at
both times and the only thing deciding which is the effect row of the context
that calls it. The one addition this needed was `IO.read_dir` (a directory's
entry names, sorted), which goes through `ls` over `C.popen` because `readdir`
returns a `struct dirent *` and the FFI cannot dereference a C pointer.

A consequence worth stating: compile-time paths are relative to the **working
directory**, like every other path a program opens, not to the source file. A
`@build` that reads the tree beside its own module is the caller's business.

**Hermeticity.** Compile-time IO makes the build depend on the world. The
`<@IO>` handler installed around `@build` records what it touched, so the build
system can treat "read `x.sql` at compile time" as a build input for caching,
the way a normal source dependency is tracked. Full reproducibility guarantees
are deferred.

---

## 7. Quotation is a string literal

**Decision.** There is **no dedicated quotation syntax**. Source to be
metaprogrammed is written as an ordinary **string literal** and fed through the
pipeline that already exists: `@lex : @Str -> []@Token` and
`@parse : []@Token -> @Code`.

```
@lex   "let x = 1 in x"          : []@Token
@parse (@lex "let x = 1 in x")   : @Code       -- or a combined @parse_str
```

**Why a string, not a backtick quote.** It adds zero surface syntax: a string is
already the language's source-carrying literal, so quotation is just "lex/parse a
string." It composes with everything that produces an `@Str` (a literal, a file
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

A convenience `@parse_str : @Str -> <@Meta> @Code` (= `@parse` of `@lex`) can be
provided so the common case is a single call.

---

## 8. Phase ordering

A generator must be fully checked and CTFE-evaluable before its first use. This
is the constraint `$ @e` already imposes, not a new one.

- Cross-module: the generator's module is compiled before modules that use it
  (topological order, already required).
- Same-module: allowed only when the generator and everything it transitively
  calls are CTFE-safe.

---

## 9. Hygiene

v1 ships **gensym only**: `@fresh` mints fresh `@name`s for macro-introduced
bindings; identifier capture is a documented footgun. `@Code` carries interned
names and spans, so a later upgrade tags macro-introduced identifiers with a
syntax-context id and resolves names within their own context, without changing
the surface.

---

## 10. `@build`, the special build function

**Decision.** `@build` is a special function, the compile-time analogue of
`@main`: the compiler recognizes it by name and runs it during compilation.
It is **not** a separate effect. Everything it does (query the compiler, inject
definitions, send/receive messages) is expressed through `<@Meta>` (section 1);
its one privilege over ordinary `@e` is that the runtime around it also
discharges `<@IO>` (section 6), because building is IO.

**Messages flow through `<@Meta>`, from anywhere to `@build`.** Any
metaprogramming code can `@msg` a message or `@add_code` an injection, because
those are `<@Meta>` operations and any such code has `<@Meta>`. Sending is not
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
`@e` expansion in section 3). This is the Jai message-loop model expressed as
a fixpoint rather than a manual `while` over `compiler_get_message`.

### Status: the function has LANDED; the message loop has not

```
$ @build : {} -> <@Meta, @IO> @Code = \_ = @parse_items "$ answer : @Int = 42"
```

**One name, one signature**, exactly as for `@main`: `{} -> <@Meta, @IO> @Code`
(`BUILD` / `BUILD_SIG` / `is_build_type` in `typing/types.rs`, beside the entry's).
A `@build` of any other shape is rejected by name before it is ever run.

- **It is a definition, not a directive.** The parser treats `$ @build : T = e`
  like `$ @main : T = e` (one branch in `parse_directive` handles both): an
  ordinary `Item::Def` whose name happens to be one the compiler knows. Nothing
  about the declaration is special-cased in the checker, because the `<@Meta,
  @IO>` row is in its *signature*, so its body is checked against that row the way
  any annotated function's body is.
- **How its result gets in.** `lower_program` notices the `@build` global and
  appends a synthetic `@build#call = @build {}` beside it, recorded in
  `Program.ct_build`. The driver's expansion loop forces that global and
  **appends** the items of the returned `@Code` to its module's source, then
  recompiles. Appending (rather than replacing a span, which is what an `$ @e`
  directive gets) is the mechanical form of "a definition that stays put and
  contributes items to its module".
- **Where it runs in the loop.** `@build` runs **first and alone** in its round:
  it contributes the definitions other compile-time code may read, so a `$ @e`
  that references them must not run before it. Each module's `@build` is forced
  **once** (nothing consumes it, so the `built` set is what terminates the loop);
  whatever it injects is expanded by the rounds that follow, so a generated `$ @e`
  still runs, and a generated `$ with M` loads M (the loop reopens the module
  set each round).
- **Any module may have one**, not just the root, unlike `@main`. The entry is
  singular because a program has one way in; a build has as many contributors as
  it has modules, and a library that generates part of itself is the point. A
  library's `@build` therefore runs when a program imports it, which also means
  importing a module grants it IO at *your* build time: the same trust an
  imported `build.rs` or `setup.py` carries.
- **Inspecting it.** `thrax expand <file> [MODULE]` prints each module's source
  after the expansion loop finishes, so the generated text is readable: `$ @e`
  sites appear as the code they inserted, and a `@build`'s items appear
  appended to its module. Without it the only windows on a metaprogram are
  `check`'s inferred types and the emitted C.
- `tests/MAIN.thx` is the real use: it reads `examples/` at compile time and
  generates its own `$ with` imports and `@main` from what is there, so adding an
  example is adding a file. `examples/META_BUILD.thx` is the small demonstration.

**Still deferred:** the message loop. `@msg`/`@add_code` from arbitrary
metaprogramming code, the `@modules`/`@functions`/`@flags` queries, and draining
a queue to a fixpoint are not implemented; `@build` currently returns its code
rather than sending it, and `@type_*` (section 11) is the only reflection in.

---

## 11. What changes in the compiler

`@e`/`@assert` are top-level items (`Item::Run`/`Item::Assert`), not
expressions. In the Rust port they were parsed but inert (the C++ CTFE was never
ported).

**Landed (compile-time execution of `@e`).** `$ @e <expr>` now runs at
compile time on every backend:

1. Checker: after all defs are checked, each `Item::Run` expression is inferred
   under a fresh pure ambient, so it resolves like a top-level body and an effect
   it performs that the compile-time runtime cannot discharge (e.g. `<@IO>`) is
   rejected (`typing.rs`).
2. Lowering: each `@e e` becomes a synthetic global `@e#i` pushed into
   `Program.globals`, with its bare name recorded in the new `Program.ct_runs`
   (`desugar.rs`, `core.rs`). It flows through `ir::lower` as an
   ordinary global, so no IR change was needed.
3. Driver: `compile_and_run_ct` builds the interpreter IR and forces each
   `Module.@e#i` global via `machine::eval`. The value is discarded; a trap
   becomes a build error. Runs before the entry, shared by `run`/`build`/
   `emit-c` (`driver.rs`). A user-land `assert` is therefore just an `@e` whose
   expression traps on a false condition; no `@assert` builtin is needed.

**Landed (build directives).** `@link : @Str -> {}` / `@link_path : @Str -> {}`
are `@`-builtins that, run at compile time via `$ @e (@link "curl")`, record a
library / search path (`machine::push_link`); `lower_all` drains them
(`take_link_directives`) into a `BuildPlan` that `cmd_build` applies to the native
link line (`-l` / `-L` + rpath), deduped against the `@extern` libraries. They
return `{}` (the effect is the directive, not a value) — no `@Code`/value
sniffing, no magic library. (This replaced an earlier `library/BUILD.thx` module
whose `Directive` values the driver recognized by name, which broke the "compiler
magic is `@`-named" rule.) The interpreter's default set already covers libc/libm
and lazily `dlopen`s the rest per `@extern`, so `thrax run` needs no preload for
the common case.

**Landed (`@build`).** `$ @build : {} -> <@Meta, @IO> @Code` parses as a definition
(`parser.rs`, the `@main` branch), is shape-checked by name (`is_build_type` via
the driver), is backed by a synthetic `@build#call` global (`Program.ct_build` in
`desugar.rs`), and is forced first in each expansion round by `lower_all`, which
appends its items to the module's source (`driver.rs`). It needed no new
intrinsic: the IO is `library/IO.thx` over the `C` namespace, run by the
compile-time interpreter through the normal FFI. Section 10 has the details.

**Still to come (the metaprogramming layer).** `@e` returning `@Code` splices
and re-checks (recurses); `@e` under a `<@Meta>` handler with the live `Ast`/
interner/type-env/diagnostic sink; `@emit`/`@abort` into the `Diagnostic` chain;
the `@build` message loop (`@msg`/`@add_code`/`@modules`).

---

## 12. Suggested build order

0. **DONE:** compile-time execution of `$ @e <expr>` (value discarded, trap
   fails the build; section 11), and `@link`/`@link_path` builtins steering the
   native link set / search paths (`examples/CT_RUN.thx`). **NEXT:** the typed
   layer below.
1. **DONE:** `@Token` as an opaque builtin type, and `@lex : @Str -> @Vec @Token`
   + `@token_kind`/`@token_text` accessors, as pure intrinsics runnable inside
   `@e` (a lex error traps). `@parse_str : @Str -> @Code` also LANDED (opaque
   `@Code` = source text, syntax errors trap), and `@parse : @Vec @Token -> @Code`
   (detokenizes a `@lex` result and validates like `@parse_str`), so the full
   `@Str -> @Token -> @Code` pipeline is closed. **NEXT:** consumers of `@Code`.
2. **DONE: `@eval : @Code -> a`** (compile + run a fragment at build time).
   `@Code` consumers need the driver's pipeline, but the interpreter crate cannot
   depend on the driver, so the mechanism is a **driver-installed thread-local
   host** (`machine::set_meta_eval`) the interpreter calls (`meta_eval`), plus
   **value reification across the re-entrant compile boundary**
   (`machine::OwnedValue` + `reify`/`embed`): the nested compile yields a value
   tied to its own `Program`; only first-order data (`Int`/`Str`/`Bool`/tuples/
   structs/variants/vectors) crosses, a closure/opaque handle is rejected. The
   host reuses `compile_session` (the REPL's re-entrant compile) via
   `driver::eval_fragment`. Since #187 `@eval` is pure and also runs at run time
   (see "Runtime use" in section 1). Result type is polymorphic
   `a` (embedded as-is; a mismatch is a compile-time fault, not a static error).
   **NEXT:** `@e` splicing an `@Code` result back into the program
   (re-check/recurse) is the remaining consumer.
2b. **DONE: item-position `$ @e X` injection, including forward references.** A
   top-level `$ @e X` whose result is `@Code` (build it with `@parse_items`)
   injects its item(s) in place of the directive, then re-compiles; a
   `@link`/`@link_path` call steers the build; any other value is a discarded
   compile-time run. The whole `$ @e X` directive carries a source span
   (`Item::Run(expr, span)`) so injection replaces it cleanly. Forward references
   now work: `$ @e (gen "double")` followed by a hand-written `test` that calls
   `double`, or a module whose only entry is injected. The expand loop compiles
   each round **leniently** (`Checker::set_lenient`): an unbound value name, a
   no-viable-overload, and the missing-entry check are all deferred (an unbound
   name becomes a fresh type var) so the generators still run. Once no `@e`
   remains, one final **strict** compile validates the fully-injected program, so a
   genuinely unbound name or a bad entry is still reported. This is what makes a
   derive-style macro (below) callable by hand-written code. A site that faults
   stays in the source and runs again next round, so a `$ @e <| assert ...` over
   operators a `$ @e` derives in the same round sees them once they land; faults
   are reported only when a round makes no progress. The loop gives up after
   `MAX_META_ROUNDS` (100) rounds with an `EXPANSION_LIMIT` error naming every
   site still pending, so a generator that injects a fresh `@e` each round
   cannot hang the build. An `@eval` fragment run during expansion sees the
   `$ with` imports of the module whose directive runs it, and compiles against
   the sources the round already holds rather than re-reading them from disk.
2a. **DONE: expression-position `@e X`.** `@e` is a universal compile-time
   splice at any expression site, composing with `|>`/`<|`. `@e (gen "+")` where
   `gen` builds a `@Code` from data splices `1 + 2 * 3` and yields `7`; `@e` that
   produces `@e` recurses to a fixpoint. Mechanism (unified on **source
   substitution**): the checker requires `X : @Code` and types the site as a
   fresh var (deferred until the splice re-checks); `lower_all` is an expand loop
   that compiles, forces each `@e` site, substitutes the code's source at the
   site's span, and re-compiles until no `@e` remains. Value folding (rendering a
   scalar or aggregate result back to a literal) existed briefly and was removed
   with `@run`: a generator renders its own values to code. Caveat: a nested
   `@e (@e X)` inside one expression is not handled (recursion works across
   rounds, e.g. generated code containing `@e`).
2c. **DONE: type-position `@e X` (`Foo : @e X = ...`).** `@e` now completes all
   three positions (expression, item, type). In type position `X` must build an
   `@Code` denoting a type (with `@parse_str`); the expand loop forces it and
   splices the type source into the annotation. Mechanism: a `Ty::MetaE(expr)` AST
   node (parsed by `@e` in `parse_type_atom_inner`); `ty_of_ast` infers `expr` (so
   its calls/overloads resolve for lowering) and stands the unknown type in as a
   fresh variable; lowering's `collect_meta_types` walks each def signature and
   emits a synthetic global `@e_type#n` per site (recorded in `Program.ct_types`);
   the driver evaluates it, requires an `@Code`, and splices its source at the
   node's span. Nests inside larger types (`@Vec (@e ...)`). Scope: def signatures
   (struct/union/alias member types are a follow-up). See `examples/META_TYPE.thx`.
3. Quotation: none needed as syntax. `@lex`/`@parse`/`@parse_str` over string
   literals (section 7); splice is string building (`++` / `?(e)`).
4. **PARTIALLY DONE: the `<@Meta>` effect.**
   `@abort`/`@emit`/`@fresh`/`@link`/`@link_path` are typed `<@Meta>` (`@eval` was
   too, until #187 made it pure), and `@e` (item/expression/type position)
   discharges it: it runs its operand under a closed `<@Meta>` ambient. A meta op
   outside `@e` is an "effect `@Meta` not handled" error, and `@e` is hermetic
   (`@IO` inside it is still rejected). A brief `@e`/`@run` split (#159), with
   `@e` pure and `@run` the eliminator, was reverted: computing is not the
   point of `@e`, inserting code is, so one construct does it. See section 2
   and section 1's "LANDED" note. STILL TO COME: a real handler over the live compiler
   state (replacing the thread-local host), `@lookup`/`@here`/`@check`, and wiring
   `@emit`/`@abort` into the `Diagnostic` chain (spans).
5. `@check` and `@eval` (staging), plus the `@code_*` constructors.
6. **DONE: compile-time IO, and the `@build` function that holds it.**
   `$ @build : {} -> <@Meta, @IO> @Code` is a definition whose `@Code` result is
   appended to its module, run first in each expansion round, once per module, in
   any module (sections 6 and 10). No compile-time IO primitive came with it: the
   IO is ordinary `library/IO.thx` over the `C` namespace, so paths are relative
   to the working directory. `IO.read_dir` (a sorted directory listing, over
   `C.popen`) was the one library addition. `tests/MAIN.thx` generates its imports
   and `@main` from `examples/`; `examples/META_BUILD.thx` demonstrates it.
7. Hygiene upgrade (syntax contexts), then `@build`'s `@msg`/`@add_code`/query
   fixpoint (the function itself is in; the message loop is not).

The load-bearing point: the two hard organs already exist. A compile-time
evaluator (CEK + `$ @e`) and a chainable diagnostic model. The macro system
is mostly *exposing* them, plus one expansion path on `Run` and quotation sugar.
