# Ares Metaprogramming Design

**Status:** design draft, largely implemented; the build log in sections 11 and
12 records what landed when.
**Date:** 2026-09-12.
**Scope:** compile-time metaprogramming: exposing the compiler pipeline
(tokens -> AST -> value) as a library, macro expansion, compiler messages, and
compile-time IO. Builds directly on the effect system
(documentation/effect-system-design.md) and CTFE (`$ &`).

**Spelling (#457, 2026-11-10).** The splice is the prefix operator `&`, which
replaced the intrinsic `@e` in every position: `$ @e <| assert ..` is now
`$ &assert ..`, the pattern `@e (X)` is `&X`, the row entry `<@e (X)>` is
`<&X>`. The design notes and build log below predate the change and keep the
old spelling where they record history; section 2 describes the current form.
Since step 3 of #457 a string literal expected as `@Code` is a quotation
(section 7), checked where it is spliced, and the per-category parsers
`@parse_items`, `@parse_type`, `@parse_row`, `@parse_fields` and
`@parse_variants` are gone; `@code` turns computed text into code. Since step 4
a quotation is hygienic (section 9). Step 5 made row aliases `@Code`
globals and #458 removed them: a row is written out, and `@alias` takes a type
only (section 2).
Since step 6 a literal expected as a `Template` is a macro argument whose holes
are quoted (section 7).

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
@Str --@lex--> @Vec Token --@parse--> @Code --@eval--> value
                                        |
                                        +--@check--> its type, as @Str
                                        +--@unparse--> @Str
```

You can build tokens and parse them yourself, build a typed AST directly, or
build code and run it. All three representations interconvert.

Non-goals for v1: a stable public AST node set (`@Code` is opaque), and
reproducible-build guarantees for compile-time IO (see section 7). Hygiene was
one too, and landed with #457 step 4 (section 9).

---

## 1. Naming: everything built-in is `@`-form

Per the base-type convention (no friendly names in the compiler; `@Int`,
`@Vec`, `@Array`, ...), all metaprogramming primitives are lowercase
`@`-intrinsics.

**Types**

- `Token` -- a lexical token, an ordinary CORE `@union` (#151): the variant is
  the kind (`Word`, `Op`, `Int`, `LParen`, ...) and every variant carries its
  lexeme as `@Str`, so a generator matches `Token.Word.{ w }`. The
  `@token_kind`/`@token_text` accessors remain. A token built by hand parses like
  a lexed one, since `@parse` only rejoins lexemes.
- `@Code` -- an opaque handle to a code fragment, carrying its source text, which
  `@unparse` returns. Opaque so the internal node set can evolve without
  breaking library code.
- `Decl`, `Site`, `Diag` -- CORE types the reflection and diagnostic operations
  speak (section 1, `<@Meta>` operations).

**Pure intrinsics (no effect; usable inside `$ @e` today).** Lexing and
parsing need no compiler state, so they are ordinary intrinsics, not `<@Meta>`
operations:

```
@lex        : @Str      -> @Vec Token    -- LANDED. a lex error traps (fails the build)
@token_kind : Token     -> @Str          -- LANDED. "Word" | "Op" | "Int" | ...
@token_text : Token     -> @Str          -- LANDED. the lexeme
@parse_str  : @Str      -> @Code         -- LANDED. parses an expr fragment; a syntax error traps
@code       : @Str      -> @Code         -- #457. text as code, unchecked; the splice site checks it
@str_literal: @Str      -> @Str          -- #457. the string literal spelling a string
@quote_open : {}        -> @Code         -- #457. empty code in a fresh quotation scope
@code_text  : @Code -> @Str -> @Code     -- #457. append text written in the code's scope
@code_cat   : @Code -> @Code -> @Code    -- #457. append code, keeping its scopes
@quote      : @Str -> @Code -> @Code     -- #457. record the module a scope was written in
@parse      : @Vec Token -> @Code        -- LANDED. detokenizes + validates like @parse_str
@unparse    : @Code     -> @Str          -- LANDED. the fragment's source
@format     : @Str      -> @Str          -- LANDED. items in `ares fmt` layout; a refusal traps
```

`@Code` is opaque; it carries the fragment's source text and no syntax
category. A `&` site decides the category (expression, type, row entries,
pattern, fields, variants or items) and checks the text there; `@parse_str`
checks an expression on the spot, for code that only exists at run time.

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
@import_path : @Str -> <@Meta> {}     -- LANDED. add a module directory; `BUILD.ares`'s `@build` only
@check  : @Code     -> <@Meta> @Str    -- LANDED. type-check a fragment at the site; its type
@here   : {}        -> <@Meta> Site    -- LANDED. the running `@e`'s file, line, column
@fail   : Diag      -> <@Meta> a       -- LANDED. fail the build with message, notes, helps
@warn   : Diag      -> <@Meta> {}      -- LANDED. a located warning; the build goes on
```

`@check` compiles the fragment as the body of a probe definition in a scratch
module that imports the site's module and everything it imports, so it sees what
code written at the `@e` would see. It returns the type as `ares check` prints
it; an ill-typed fragment fails the build (`META_ABORT`, the checker's error as a
note). It answers "what would this be", where `@Code` stays untyped text.

`Diag` is `{ message, notes, helps }` and `diag msg` is one with no asides.
`@fail` raises `META_ABORT` and `@warn` a `META_WARNING` warning, each at the
running `@e` with a `note:` line per note and a `help:` line per help. `@abort`
and `@emit` stay as the one-string shorthands.

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
(`@lex`/`@parse`/`@parse_*`/`@unparse`/`@token_*`) remain PURE. The compiler
side of every operation is a driver-installed host (type tables, the `@check`
compile, the running site), set around each site rather than a handler over
the live compiler state; nothing a generator can observe depends on which.

`@eval` rides a driver-installed thread-local host rather than a `<@Meta>`
handler.

**Runtime use (#187).** `@lex`, `@token_kind`/`@token_text`, `@parse`,
`@parse_str`, `@code`, `@format` and `@eval` are pure, so ordinary code may
call them (`examples/META_RUNTIME.ares`). `tools/README_GEN.ares` runs `@format`
under `$ @e` to lay out the README's snippets at compile time. `@eval` is pure
because its fragment is checked as a pure def body (a closed effect row, unlike
the shell's open one), so it computes a value from its source and nothing else.
Each engine needs the compiler at run time to serve them:

- The interpreter links the compiler, so `ares run` and the shell install the
  `@eval` host for the whole run (`driver::install_eval_host`).
- A native program calls the compiler as a C library. The `ares` crate builds as
  `libares.a` and `libares.so` too, with the C API in
  `crates/ares/include/ares.h` (`ares_lex`, `ares_parse`, `ares_eval`, values
  as an owned `ares_value` tree). `ccg` notes when a reachable block names one of
  these builtins (the `BuiltinHost::Compiler` entries of
  `frontend::ir::data::BUILTINS`), emits the header and
  `ARES_WITH_COMPILER` ahead of `runtime.c` (whose bridge converts the tree into
  runtime `Value`s), and `ares build` links `libares.so` with an rpath. It
  uses the shared library because that one carries its own dependencies (libffi).
  Only the host target has the library; a cross build that needs it is an error.

As at compile time, only first-order data crosses (a function, or a `List` whose
lazy constructor fields are still unforced, is rejected), and the result type is
whatever the use site says, so a mismatch is a fault when it runs. `@fail`/`@warn`
are the structured forms of `@abort`/`@emit`.
A compile-time `@e` fault, or an `@abort` (`error[META_ABORT]`), is reported at the real `@e` call site
(`file:line:col` + caret), not a synthetic global, since every `@e` site records
its source span.
There is no `@assert` builtin: assert is user code,
`$ @e (if ok => {} else @abort "...")` (see `examples/CT_ASSERT.ares`).

Compiler messages, receive (query the accumulated compile state):

```
@lookup    : @Str   -> <@Meta> Decl          -- LANDED
@scope     : {}     -> <@Meta> @Vec {@Str, @Str}  -- LANDED. names in scope, with types
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

`@lookup` is total (LANDED, #146): it returns a CORE `Decl`,

```
$ Decl : @union =
    Struct: { params: @Vec @Str, fields: @Vec { @Str, @Str } },
    Union: { params: @Vec @Str, variants: @Vec { @Str, @Str } },
    Value: @Str,
    Missing: {},
```

and a name nobody declared is `Decl.Missing`, a value to match rather than a
fault or a `Maybe` to unwrap. A field is `{name, type}` and a variant
`{tag, payload}` (the payload empty for a bare tag), each type as source over
the declaration's own `params` (#442), so a generator can copy members into
another declaration. A type whose own `@e` entries have not been spliced in yet
faults instead of answering with part of its members: the generator asking
runs again next round. It is the matchable form of the `@type_*`
operations below, which remain as shorthands (and fault on a missing name). See
`examples/META_REFLECT.ares`.

`@scope {}` lists every name a bare reference at the running site can resolve
to (the module's own globals, then its imports), each with its type as source.

**The shell runs at compile time.** Each prompt recompiles the session, so the
shell is a compile-time site: its items run with the same hosts a `$ @e` has, and
`$ _= @scope {}`, `$ _= @type_of "f"` or `$ _= @check <| @parse_str "1 + 2"` work
at the prompt. A `$ @e` item (or an `@e` inside a definition) expands as in a
file, and the shell keeps the expanded items. The shell has no `:` commands:
every session imports `library/ARES_SHELL.ares`, whose `type "1 + 2"` and
`scope "vec"` print what that reflection finds, and whose `quit`/`reset`/`list`/
`help` call `@shell_*` builtins of the built-in `<@Shell>` effect. The shell is
their host (`interpreter::machine::set_shell_host`), like the compiler is
`<@Meta>`'s; a program has none, so calling one there is a type error.

**LANDED (a first cut of reflection).** Ahead of the full `@lookup`, these
`<@Meta>` operations expose a declared type's shape, or a global value's type, to
an `@e` generator:

```
@type_kind     : @Str -> <@Meta> @Str               -- "struct" | "union" | "value"
@type_params   : @Str -> <@Meta> @Vec @Str          -- declared type parameters, in order
@type_fields   : @Str -> <@Meta> @Vec @Str          -- a struct's field names
@type_variants : @Str -> <@Meta> @Vec {@Str, @Int}  -- a union's (tag, arity) pairs
@type_of       : @Str -> <@Meta> @Str               -- a global value's type, as source
```

The argument is a name, bare (`"Rgb"`) or qualified (`"MOD.Rgb"`; qualify to
disambiguate a name shared across modules). They resolve through a driver-installed
host (`machine::set_type_host`, fed from `Declarations::reflect` plus each round's
checked global types). They read the program being compiled, so they carry
`<@Meta>`: a call in ordinary code is a type error, and a helper that reflects
declares `<@Meta>` (as `DERIVE.derive_show` does). `@type_of "fib"` is
`"@Int -> @Int"`, exactly as `ares check` prints it, `@use` fields included. In a
round that still has unexpanded sites a type may be partial, so a generator that
depends on it should not run before the sites it reads. On top of them, `library/DERIVE.ares`
derives a default `to_string` for any struct or union (`$ @e (DERIVE.derive_show
"Rgb")`); see `examples/DERIVE_SHOW.ares`. This is the first real derive-style
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

**`Token` kind tags (LANDED, #151).** `Token` is an ordinary CORE `@union`
(a CORE type has no `@` in its name, like `List`), one variant per lexer kind
with ordinary capitalized tags (`Word`, `Op`, `Int`, `LParen`, ...), each
carrying the lexeme: `t |> \ | Token.Word.{ w } => ... | _ => ...`. `Eof` is not
a token. Both engines build and read it as a plain variant (the native runtime's
`libares` bridge already crossed `ARES_VARIANT`).

---

## 2. `&` splices code or data

**Decision (#457, after #159).** There is no dedicated macro-invocation syntax
(`foo!(...)`) and there is one compile-time construct: **`&X` runs `X` at build
time and replaces itself with the result.** An `@Code` result is spliced in as
source (and expanded again while it holds `&` sites); first-order data (a
number, a `@Bool` or a `@Str`) is inserted as a literal. Any other result, such
as a function, is a type error at the `&`. `X` runs under a closed `<@Meta>`
ambient, so the meta ops (`@abort`, `@emit`, `@fresh`, `@link`, `@link_path`)
discharge there and any other effect, `@IO` included, is rejected. Compile-time
IO lives solely in `@build` (sections 6 and 10).

Empty code inserts nothing, and that is the escape hatch for a site run only for
its effect. `CORE.assert` and `@link`/`@link_path` return empty code, so
`$ &assert ok msg` and `$ &@link "m"` are statements that vanish once they have
run.

**Syntax, not a function.** `&` is a prefix operator at the binding power of
`-`, `!` and `~~~`, so it scopes over an application: `&f a b` splices `f a b`,
`&f a ++ b` is `(&(f a)) ++ b`, and a pipe chain is parenthesized, `&(x |> f)`.
Its result may be an expression, a type, a pattern, members or items, so it has
no function type: like `&&` and `|>` the parser rewrites it, into an
`Expr::Splice` node in expression position and the matching splice node
elsewhere. There is no `(&)` section, no `x |> &` and no `$ (&)` definition. A
`&` glued to the operator before it lexes on its own when the run would
otherwise be unknown, so `<&Logging>` is `<` then `&Logging`, and `!&x` is `!`
then `&x`.

**Positions.**

```ares
$ &assert (fib 10 == 55) "fib 10 should be 55"           # item
$ fib10 : @Int = &fib 10                                 # expression: inserts `55`
$ n : &wide {} = 1024                                    # type
$ Point3 : @struct = &DERIVE.fields_of "Point", z: @Int  # member
$ run : {} -> <&logging {}, State @Int> @Int = ...      # row
x |> \ | &answer => a                                   # pattern: a constant
       | answer  => b                                    # always binds
```

**Constant patterns** are the zero-argument case. Because the marker is at the
use site, importing a constant can never turn a binder into a match (the
footgun of implicit constant patterns, cf. Rust `const` patterns). The value is
inserted as a real literal, so exhaustiveness and `UNREACHABLE_ARM` treat
`&answer` like `42`.

**Rows have no aliases.** An effect row or a record row is written out where it
is used. A `&X` entry in a row is for entries a generator computes, as
`<&logging {}, State @Int>` above; the spliced entries join the row as if
written there, so a label it repeats occurs twice. An effect label is keyed by
its bare name in every module, so hygiene has nothing to qualify in a row.

**What `&X` is, precisely.** `X` is a closed compile-time expression, run once,
now, whose result replaces the site in source. A data result has the operand's
type at once; what `@Code` denotes is known only once it is spliced and
re-checked, so such a site types as a fresh variable until then.

**History.** A short-lived split (#159) had `@e` run a PURE operand and embed its
value, and a sibling `@run` discharge `<@Meta>`; `@run` went and `@e` inserted
code only, so a value had to be rendered with a hand-written `lit` helper
(`@parse_str (to_string n)`). #457 replaced `@e` with `&` and brought data
insertion back for first-order values, where the driver can write the literal
itself, which retired those helpers.

**Arguments are ordinary expressions; raw tokens are opt-in.** Because
`&f a b` splices an application, `a` and `b` parse and check as normal Ares
before `f` runs. Raw-token input is requested explicitly by passing a string
literal through `@lex` (section 7):

```
&f x                 -- f sees the value / AST of x   (common case)
&f (@lex "x.y z")    -- f sees @Vec Token               (DSL / not-valid-Ares case)
```

This is more composable than Rust's implicit token slurp: the raw-ness is
visible at the call site, not hidden in the callee's signature.

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

- The **staging pipeline** `@Vec Token -> @Code -> value` is a straight line. Each
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
`@`-form labels, IO primitives (`library/C.ares` OS externs, `library/IO.ares`)
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
$ @build : {} -> <@Meta, @IO> @Code = \_ => ... IO.read_dir "." ...   # fine
```

**There is no compile-time IO primitive, and deliberately so.** Compile-time IO
is the *same* IO a program does at run time: `library/IO.ares` over the `C`
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
metaprogrammed is written as an ordinary **string literal**, and a literal
expected as `@Code` is a quotation (#457 step 3): the same type-directed literal
protocol that makes `"..."` a `Sql` or an `Html` (language reference, section
1.6) makes it code. CORE's `@IStrLit @Code` instance covers a literal without
holes, and the `@IInterp @Code @Code` instances one with holes, so the compiler
has no quotation path of its own.

```
$ sum   : @Code = "1 + 2 * 3"
$ times : @Int -> @Code -> @Code = \n c => "(?(c)) * ?(n)"
$ x     : @Int = &times 3 sum      -- inserts `(1 + 2 * 3) * 3`
```

**Why a string, not a backtick quote.** It adds zero surface syntax: a string is
already the language's source-carrying literal. It composes with everything that
produces text (a file read at `@build` time, a string built from data, through
`@code`), so the same path serves templating and source-in, and `@lex`/`@parse`
remain for working on tokens.

**Holes run in the generator.** A `@Code` hole pastes its source; a number, a
`@Bool` or a `@Str` hole pastes the literal spelling its value (open question 3
of #457, settled as implicit, matching how `&` inserts data). A computed name is
pasted as code with `?(@code name)`. Splicing is still **textual**, so the
author owns precedence, exactly as in any string-based code generator:

**Macro arguments (#457 step 6).** A literal expected as CORE's
`Template` (`pieces: @Vec @Str, holes: @Vec @Code`) keeps its pieces as text and
quotes its holes instead of evaluating them: `&sql "... ?(name) ..."` hands
`sql` the code `name`, which names a run-time parameter of the function the
literal sits in. The parser gives every interpolated literal a third reading for
this (`Expr::Interp.quoted`, `.{ .pieces = [..], .holes = [@code "name"] }`),
which the checker takes when the expected type is CORE's `Template`; a literal
with no holes goes through `impl_IStrLit_for_Template`. Hole code is `@code`
text, so it reads as written at the site. For a macro to learn a hole's type,
`@check` at an expression `&` sees the locals in scope there: the checker
records them per splice (`Checker::splice_locals`), and the check host declares
each as a global of its type ahead of the fragment. Precedent: C# converting a
lambda to an `Expression<...>` by target type. `examples/META_TEMPLATE.ares`
numbers a statement's holes and binds each as the parameter its type picks.

**Spans (LANDED, #151).** A syntax error in a fragment, from `@parse_str` or
from the check at a splice site, carries the fragment it was raised in
(`Diagnostic::with_fragment`). The driver
looks for a string literal that decodes to exactly that fragment, first inside
the `@e` site and then anywhere in the module (the literal may sit in a helper the
`@e` calls, which then gets a label at the `@e`), and maps the offset through the
literal's escapes, so the caret lands on the character the author wrote. A
fragment no literal spells (built with `++` or interpolation) stays at the `@e`
site with the fragment quoted in a note. Errors in spliced code that only show up
when it is checked are reported in the expanded source. Acceptable for the escape-hatch
tier; `@emit`/`@abort` (section 5) still let a generator raise a clear message of
its own.


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

**Decision (#457 step 4, LANDED 2026-11-10).** Hygiene by **scopes**. A name a
generator's quotation spells means what it means in the generator's module,
wherever the code is spliced, and a binder it spells binds only the names the
same quotation spells. Code the site hands in keeps the site's meaning.

```
$ @mod GEN
$ scale    : @Int -> @Int = \x => x * 10
$ scaled   : @Code -> @Code = \c => "scale (?(c))"
$ plus_one : @Code -> @Code = \c => "let tmp = 1 in tmp + ?(c)"

$ @mod USE
$ @import GEN
$ scale   : @Int -> @Int = \x => x + 1
$ forty   : @Int = &scaled "4"                   -- GEN's scale: 40, not 5
$ add_one : @Int -> @Int = \tmp => &plus_one "tmp" -- the caller's tmp: 101, not 2
```

**Why scopes, not resolve-at-write.** Resolving a quotation's names where it is
written needs a parse, but the syntax category is only known at the splice
site (section 7), and renaming tokens without one breaks field names. So the
quotation stays text and remembers *who wrote which bytes*; the splice site,
which parses anyway, resolves with that.

**Mechanism.** Every evaluation of a quotation opens a fresh scope
(`@quote_open`). Its text pieces are marked with it (`@code_text`), and a hole
keeps the marks of the code it pastes (`@code_cat`). Lowering wraps a quotation
in a function body in `@quote "MODULE"`, which records the scope's module. A
quotation written in a `&` operand is written where its code lands, so lowering
leaves it unrecorded, and `@code` text has no scope: both read as written at the
site. At the splice the driver parses the code in its category and rewrites it:

- a binder of a recorded scope is renamed apart, `tmp` to `tmp__6` (the scope's
  id), together with the names of the same scope it binds; a field-pattern
  shorthand `.w` is spelled out as `.w = w__6`;
- a free name of a recorded scope that the scope's module defines or imports is
  qualified with its home, `GEN.scale`; a type the site reaches bare as the same
  one is left bare;
- the site imports a module it does not reach under its own name only
  (`$ @import MATH = MATH`), which brings no bare name in;
- names the fragment's own items declare stay as written, since the items are
  spliced to define them, and so does a name the quotation's module does not
  know, which resolves at the site.

**Pattern binders** follow (open question 4): a binder the generator spells is
invisible to the arm body, so a generator that binds for its caller takes the
binder as code, `\v => "Option.Some.{?(v)}"` used as `&some_of "x"`. Spelling it
with `@code` text (`$ it : @Code = @code "it"`) is the deliberate escape: the
binder then reads as written at the site. An arm body that names a renamed
binder gets an unbound-name error saying so.

**Limits.** Operators, effect operations and effect names in rows resolve at the
site. A name that is private to the generator's module is qualified like any
other and is then rejected as private.

The driver side is `crates/ares/src/driver/hygiene.rs`; Note [Quotation scopes]
in `interpreter::machine::data` describes the marks.

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
$ @build : {} -> <@Meta, @IO> @Code = \_ => "$ answer : @Int = 42"
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
  still runs, and a generated `$ @import M` loads M (the loop reopens the module
  set each round).
- **Any module may have one**, not just the root, unlike `@main`. The entry is
  singular because a program has one way in; a build has as many contributors as
  it has modules, and a library that generates part of itself is the point. A
  library's `@build` therefore runs when a program imports it, which also means
  importing a module grants it IO at *your* build time: the same trust an
  imported `build.rs` or `setup.py` carries.
- **Inspecting it.** `ares expand <file> [MODULE]` prints each module's source
  after the expansion loop finishes, so the generated text is readable: `$ @e`
  sites appear as the code they inserted, and a `@build`'s items appear
  appended to its module. Without it the only windows on a metaprogram are
  `check`'s inferred types and the emitted C.
- `tests/MAIN.ares` is the real use: it reads `examples/` at compile time and
  generates its own `$ @import` imports and `@main` from what is there, so adding an
  example is adding a file. `examples/META_BUILD.ares` is the small demonstration.

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
sniffing, no magic library. (This replaced an earlier `library/BUILD.ares` module
whose `Directive` values the driver recognized by name, which broke the "compiler
magic is `@`-named" rule.) The interpreter's default set already covers libc/libm
and lazily `dlopen`s the rest per `@extern`, so `ares run` needs no preload for
the common case.

**Landed (`@build`).** `$ @build : {} -> <@Meta, @IO> @Code` parses as a definition
(`parser.rs`, the `@main` branch), is shape-checked by name (`is_build_type` via
the driver), is backed by a synthetic `@build#call` global (`Program.ct_build` in
`desugar.rs`), and is forced first in each expansion round by `lower_all`, which
appends its items to the module's source (`driver.rs`). It needed no new
intrinsic: the IO is `library/IO.ares` over the `C` namespace, run by the
compile-time interpreter through the normal FFI. Section 10 has the details.

**Still to come (the metaprogramming layer).** `@e` returning `@Code` splices
and re-checks (recurses); `@e` under a `<@Meta>` handler with the live `Ast`/
interner/type-env/diagnostic sink; `@emit`/`@abort` into the `Diagnostic` chain;
the `@build` message loop (`@msg`/`@add_code`/`@modules`).

---

## 12. Suggested build order

0. **DONE:** compile-time execution of `$ @e <expr>` (value discarded, trap
   fails the build; section 11), and `@link`/`@link_path` builtins steering the
   native link set / search paths (`examples/CT_RUN.ares`). **NEXT:** the typed
   layer below.
1. **DONE:** `@Token` as an opaque builtin type (since #151 the CORE union `Token`), and `@lex : @Str -> @Vec @Token`
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
   `$ @import` imports of the module whose directive runs it, and compiles against
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
   node's span. Nests inside larger types (`@Vec (@e ...)`). Every type position
   is collected (#160): def signatures, struct fields, union payloads, alias
   bodies, effect operations, effect-row arguments (`<State (@e X)>`), `@use`
   fields, and the annotations inside a body (`let x : T`, `\(x : T)`,
   `(e : T)`). Before #160 a member-type `@e` parsed but was never spliced, so it
   stood as an unchecked variable. See `examples/META_TYPE.ares`.
2f. **DONE: declaration entries `@e X` (#442).** An entry of a `@struct` or
   `@union` that starts with `@e` is a splice; `X` ends at the entry's comma and
   builds the `@Code` of fields (`@parse_fields`) or variants
   (`@parse_variants`), spliced bare in place of the entry (`ct_rows`, like a
   `<@e (X)>` row entry). DERIVE's `fields_of "Point"` / `variants_of "Base"`
   copy another declaration's members, which replaced the compiler's `with
   Other` include. While a module has a type with a pending entry, a lenient
   round holds back any global (or `$ @e`) whose check fails, treating its body
   as a deferred site: the entries it is missing may still arrive. See
   `examples/TYPE_SPLICE.ares`.
2d. **DONE: pattern-position `@e (X)` (#160, #197).** The operand is always
   parenthesized, so it ends at `)` before the arm's `=>`, `|` or `::`.
   `Pattern::MetaE` types as a match of any value that binds nothing; coverage is
   skipped for a match holding one (the final compile checks the spliced
   pattern). Lowering emits a synthetic global `@e_pat#n` per site
   (`Program.ct_patterns`) and the driver splices its source bare, since a
   pattern has no parentheses. The enclosing global counts as deferred until
   then, so no `@e` runs the unspliced match. See `examples/META_PATTERN.ares`.
2e. **DONE: an `@e` never runs unresolved code (#416).** A lenient round records
   each site it defers (`Checker::deferred_sites`); lowering lists the globals
   holding one (`Program.deferred`), and the driver does not run an `@e` whose
   global reaches one. If a round makes no progress while sites wait, the strict
   check reports the deferred error instead of whatever fault the run would have
   raised. See Note [Deferred sites hold back generators] in `desugar.rs`.
3. **DONE (#457 step 3): quotation.** None needed as syntax: a string literal
   expected as `@Code` is one, its holes paste code or literals (section 7), and
   the splice site checks the syntax category, which retired the per-category
   `@parse_*`.
4. **PARTIALLY DONE: the `<@Meta>` effect.**
   `@abort`/`@emit`/`@fresh`/`@link`/`@link_path` are typed `<@Meta>` (`@eval` was
   too, until #187 made it pure), and `@e` (item/expression/type position)
   discharges it: it runs its operand under a closed `<@Meta>` ambient. A meta op
   outside `@e` is an "effect `@Meta` not handled" error, and `@e` is hermetic
   (`@IO` inside it is still rejected). A brief `@e`/`@run` split (#159), with
   `@e` pure and `@run` the eliminator, was reverted: computing is not the
   point of `@e`, inserting code is, so one construct does it. See section 2
   and section 1's "LANDED" note. #146 added `@lookup`, `@here`, `@check`,
   `@fail`/`@warn` and `@type_of`, and moved the `@type_*` reflection under
   `<@Meta>`.
5. **DONE:** `@check` and `@eval` (staging). Still open: the `@code_*`
   constructors.
6. **DONE: compile-time IO, and the `@build` function that holds it.**
   `$ @build : {} -> <@Meta, @IO> @Code` is a definition whose `@Code` result is
   appended to its module, run first in each expansion round, once per module, in
   any module (sections 6 and 10). No compile-time IO primitive came with it: the
   IO is ordinary `library/IO.ares` over the `C` namespace, so paths are relative
   to the working directory. `IO.read_dir` (a sorted directory listing, over
   `C.popen`) was the one library addition. `tests/MAIN.ares` generates its imports
   and `@main` from `examples/`; `examples/META_BUILD.ares` demonstrates it.
7. Hygiene (scopes, LANDED with #457 step 4, section 9), then `@build`'s `@msg`/`@add_code`/query
   fixpoint (the function itself is in; the message loop is not).
8. **DONE (#457 step 5, #458):** `@alias` takes a type only, and rows have no
   aliases.
9. **DONE (#457 step 6):** `Template` macro arguments with quoted holes, and
   `@check` seeing the locals at an expression `&`.

The load-bearing point: the two hard organs already exist. A compile-time
evaluator (CEK + `$ @e`) and a chainable diagnostic model. The macro system
is mostly *exposing* them, plus one expansion path on `Run` and quotation sugar.
