# Thrax

Thrax is a functional language with algebraic data types (structs *and* sum
types), pattern matching, and typed **algebraic effects with handlers**. It
compiles to native C, cross-compiles to WebAssembly, and can also be interpreted.

## Links

- **[Try it in your browser](https://delyan-kirov.github.io/Thrax/)**:
  Explore the interactive examples in your browswer.
- **[Applications](https://github.com/delyan-kirov/thrax-applications)**: Small
  projects written in Thrax, using the language as an SDK.
- **[Source on GitHub](https://github.com/delyan-kirov/Thrax)**: The compiler,
  the standard library, and docs.
- **[Examples](examples/)**: See how each feature of the language is used in
  small examples.

---

## Building

**With nix** (recommended):

```sh
nix develop      # dev shell; `thrax` is already on PATH here
cargo build      # or build it yourself: target/debug/thrax
cargo test       # the frontend, interpreter, and backend suites
```

Inside the dev shell `thrax` is a wrapper that rebuilds the workspace on demand
and runs it, so it works from a fresh checkout with nothing compiled yet.

**Without nix**: any host with a recent `cargo`/`rustc` and a C toolchain works:

```sh
cargo build --release   # target/release/thrax
```

## Compiling and running

`thrax` takes a subcommand and, optionally, a `.thx` file. With no file it uses
`MAIN.thx` in the current directory (or the sole `.thx` file there), so an app
directory just runs `thrax run`:

```sh
thrax run     examples/FIB.thx   # run under the interpreter
thrax check   examples/FIB.thx   # type-check only, print inferred types
thrax parse   examples/FIB.thx   # print the parsed syntax tree
thrax emit-c  examples/FIB.thx   # print the generated C (the native backend)
thrax build   examples/FIB.thx   # compile to a native executable beside the source
thrax --target=wasm32-wasi build examples/FIB.thx   # cross-compile to wasm

thrax run                        # run ./MAIN.thx
thrax run app.thx a b            # run app.thx, passing `a b` to the program
```

Run `thrax --help` for the full list of subcommands and flags.

## A taste: algebraic effects

An effect is a set of operations, and performing one is just a call. A function's
type carries the effects it may perform as a **row** on its arrow (`A -> <E> B`);
a plain arrow is pure, and an unhandled effect is a compile-time error. A handler
runs a body and, for each operation, receives the captured continuation `k` to
resume (or not).

```typescript
@mod Demo

$ Yield : @effect = yield : Int -> {},

# `emit` performs the effect; the type says so: {} -> <Yield> {}.
$ emit : {} -> <Yield> {} = \_ =
    Yield.yield 1 ; Yield.yield 2 ; Yield.yield 3

# The handler resumes once per yield, summing every value it produces.
$ main : Int =
    do emit {}
    ctl k | Yield.yield v => v + k {}
          else _ => 0          # 6
```

Because `k` is a first-class value, generators, coroutines, and state are all
ordinary library code rather than built-ins. See the
[tour](#a-tour-of-the-language) below and
[`examples/EFFECTS.thx`](examples/EFFECTS.thx).

## Features

- **Algebraic data types**: structs and sum types, generic by default, composed
  with `with`-splicing.
- **Pattern matching**: `is`-expressions with nested, literal, string, and range
  patterns, `..rest`, and fall-through guards; irrefutable patterns also
  destructure in `let` and lambda parameters.
- **Typed algebraic effects with handlers**: the effect row is part of the type,
  handlers are deep, and continuations are first-class and resumable (so
  generators, coroutines, and state are library code), plus Go-style `defer`.
- **A native C backend**: the whole IR, effects included, lowers to self-contained
  C through a CEK machine emitted in C, with reference-counted memory.
- **C FFI with no binding ceremony**: `@extern "C" "sym" "lib"` binds a foreign
  function; the interpreter resolves it via `dlopen`/libffi and the backend emits
  a direct call. C structs pass by value in both directions.
- **WebAssembly**: cross-compile a program with `--target=wasm32-wasi`, or run the
  entire compiler in the browser as wasm (the playground linked above).
- **And more**: row-polymorphic records, codata and streams, sized tensors,
  implicit (`@ctx`) parameters, function overloading, tail-call optimization, and
  compile-time evaluation (`$ @run`, `@assert`).

The tour below has a runnable snippet for each.

---

## A tour of the language

Every snippet below is a real file under [`examples/`](examples/).

### Algebraic data - products (structs)

Structs are named records. A free lowercase name (`t`) in a field type is an
implicit type parameter, so declarations are generic and applied by
juxtaposition. ([`AGTxPRO.thx`](examples/AGTxPRO.thx))

```typescript
$ Person : @struct =
    name: Str,
    age: Int,

$ person : Person = Person.{ .name = "Will", .age = 21 }
$ who    : Str    = person.name
$ older  : Int    = person.age + 1

# Generic: one declaration, two instantiations.
$ Box : @struct = val: t,
$ ibox : Box Int = Box.{ .val = 7 }
$ sbox : Box Str = Box.{ .val = "hi" }
```

A declaration may start with `with Other` to splice in another struct's fields
(a copy-paste convenience, not subtyping), then add its own.
([`TYPE_SPLICE.thx`](examples/TYPE_SPLICE.thx))

```typescript
$ Point  : @struct = x: Int, y: Int,
$ Point3 : @struct = with Point, z: Int,   # x, y, then z
```

### Algebraic data sums (unions)

Sum types are a tagged choice of a variant and its payload. Recursion and
generics work as you'd expect; a payload of `{}` is the unit variant.
([`AGTxSUM.thx`](examples/AGTxSUM.thx))

```typescript
$ Maybe : @union =
    Just: t,
    None: {}

# Constructors are qualified by their type; a `{}` payload needs no braces.
$ some_i : Maybe Int = Maybe.Just.{ 5 }
$ none_i : Maybe Int = Maybe.None

$ List : @union =
    Cons: {t, List t},
    Nil: {}
```

Unions take `with` too, splicing another union's variants in before their own:

```typescript
$ Base  : @union = Red: {}, Green: {},
$ Color : @union = with Base, Blue: {}   # Red, Green, then Blue
```

### Pattern matching

`is scrut | pat => e ... else d` matches top to bottom; the first matching arm
wins and binds its variables. The leading `is` distinguishes it from the boolean
`if c => t else e`. Patterns test literals, destructure structs and variants
(nested), and each arm can carry an `if <guard>` that falls through to the next
arm on failure. ([`MATCH.thx`](examples/MATCH.thx),
[`WHEN_GUARDS.thx`](examples/WHEN_GUARDS.thx), [`PATTERNS.thx`](examples/PATTERNS.thx))

```typescript
$ get : Int -> Maybe Int -> Int = \d = \m =
    is m
        | Maybe.Just.{ x } => x
    else d

# Guards fall through, even across arms that share a constructor.
$ grade : Maybe Int -> Int = \m =
    is m
        | Maybe.Just.{ v } if v ?> 100 => 3
        | Maybe.Just.{ v } if v ?> 0   => 2
        | Maybe.Just.{ _ }             => 1
        | Maybe.None                   => 0
        else 0 - 1
```

Irrefutable patterns also destructure directly in `let` and lambda parameters,
by name (`_` ignores a field):

```typescript
$ get_name : Person -> Str = \Person.{ name, _ } = name
$ sum_xy   : Point -> Int  = \Point.{ x, y } = x + y
```

### Algebraic effects and handlers

Effects are declared as a set of operations; performing one is just calling it. A
function's type carries the effects it may perform as a **row** on its arrow
(`A -> <E> B`); a bare arrow is pure, and an *unhandled* effect is a compile-time
error. A handler is `do <body> ctl k | op a => e ... [else x => e]`; the captured
continuation `k` is resumed by applying it (affine, **You only get one shot!**).
([`EFFECTS.thx`](examples/EFFECTS.thx))

```typescript
$ Exn   : @effect = throw : Str -> a,
$ Yield : @effect = yield : Int -> {},
$ State : @effect = get : {} -> Int, put : Int -> {},

# Exception: the handler ignores k, so it resumes zero times.
$ safeDiv : Int -> Int -> Int = \a b =
    do if b ?= 0 => Exn.throw "div0" else a / b
    ctl k | Exn.throw msg => 0 - 1

# Generator: resume once per yield, summing the results.
$ sumGen : ({} -> <Yield> {}) -> Int = \gen =
    do gen {}
    ctl k | Yield.yield v => v + k {}
          else _ => 0
```

Because the continuation is first-class it can be *stored* and resumed later,
from a different context, that is all coroutines are:
([`COROUTINES.thx`](examples/COROUTINES.thx))

```typescript
$ Co   : @effect = yield : Int -> {},
$ Task : @union  = Fin: {}, Susp: { Int, {} -> Task },

# Capture the suspended continuation instead of resuming in place.
$ spawn : ({} -> <Co> {}) -> Task = \t =
    do t {}
    ctl k | Co.yield v => Task.Susp.{ v, k }
          else _ => Task.Fin.{}
```

There's also `defer <cleanup> do <body>` (Go-style): the cleanup runs when the
body's scope exits, on normal completion, on abort, or when a stored
continuation holding it finally completes ([`FINALLY.thx`](examples/FINALLY.thx)).

---

## Native backend and FFI

Beyond the interpreter, Thrax lowers the whole IR to self-contained C, including
algebraic effects, via a CEK-style machine emitted in C, with reference-counted
memory management. Foreign C functions are bound with `@extern`.

```typescript
$ puts : Str -> Int = @extern "C" "puts" "libc"
$ main : Int = puts "Hello world"; 0
```

The library name is symbolic -- no path or soname appears in source. The
interpreter resolves it with dlopen at run time; the native backend emits a
direct call and a link flag, and the system linker does the rest.

```sh
thrax build examples/io_example/MAIN.thx   # native executable beside the source
```

---

## Project layout

| Directory | Contents |
| --- | --- |
| `crates/frontend` | lexer, parser, type checker, lowering, IR |
| `crates/interpreter` | the reified-K (CEK) machine and the `@extern` FFI |
| `crates/ccg` | the C backend (emits standalone C plus its runtime) |
| `crates/thrax` | the `thrax` driver (CLI) |
| `crates/utilities` | shared support: arena, target/platform, diagnostics |
| `library/` | the standard library: the auto-injected `C` (libc) namespace, the implicitly imported `CORE`, and the rest of the modules |
| `examples/` | annotated `.thx` programs (also the test corpus) |
| `doc/` | language spec and design notes |

More detail lives in [`doc/`](doc/): the [syntax spec](doc/syntax-spec.txt), the
[effect-system design](doc/effect-system-design.md), and the
[native backend](doc/native-backend.md).

## License

MIT see [LICENSE](LICENSE).
