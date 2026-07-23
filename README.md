# Thrax

Thrax is a functional language with support for algebraic data
types (structs *and* sum types), pattern matching, and typed **algebraic effects
with handlers** . It compiles to C and can also be interpreted.

```typescript
@mod Testing

$ fib : Int -> Int = \n =
    if n ?= 0 then 0
    else if n ?= 1 then 1
    else fib (n - 1) + fib (n - 2)

$ answer : Int = fib 10   # 55
```

---

## Building

Thrax builds with a single self-contained build program (`build.cpp`).
Any platform with a C++23 compiler works; nix is optional but convenient.

**Without nix** - bootstrap the build program once, then use `./build`:

```sh
clang++ -std=c++23 -Iutilities build.cpp utilities/UTxIO.cpp utilities/AR.cpp -o build && ./build
```

**With nix**: `nix develop` bootstraps `./build` and puts it (and the built
binaries) on `PATH`:

```sh
nix develop
build
```

After that first bootstrap the build program self-rebuilds whenever `build.cpp`
changes. See `build help`.

### FFI

FFI (via libffi) is **on by default**. libffi is resolved from `$LIBFFI` /
`$LIBFFI_DEV` (nix provides these), then `pkg-config`, then a vendored copy. If
it can't be found the build warns and continues without FFI.
A leading option overrides this per build: `build no-ffi`
forces it off, `build ffi` requires it (erroring if libffi is absent). Switching
modes rebuilds automatically, no `build clean` needed.

## Running programs

The compiler is `thrax`. Point it at a `.thx` file or a directory (all `.thx` in
it form one program):

```sh
thrax examples/FIB.thx          # run under the interpreter
thrax --ast examples/FIB.thx    # print the parsed AST
thrax --ir  examples/FIB.thx    # print the closure-converted IR
thrax --emit-c examples/FIB.thx # print the generated C (the native backend)
thrax --build examples/io_example   # compile a project to a native executable
```

---

## A tour of the language

Every snippet below is a real file under [`examples/`](examples/).

### Algebraic data - products (structs)

Structs are named records. A free `` `T `` in a field type is an implicit type
parameter, so declarations are generic and applied by juxtaposition.
([`AGTxPRO.thx`](examples/AGTxPRO.thx))

```typescript
$ Person : @struct =
    name: Str,
    age: Int,

$ person : Person = Person.{ .name = "Will", .age = 21 }
$ who    : Str    = person.name
$ older  : Int    = person.age + 1

# Generic: one declaration, two instantiations.
$ Box : @struct = val: `T,
$ ibox : Box Int = Box.{ .val = 7 }
$ sbox : Box Str = Box.{ .val = "hi" }
```

### Algebraic data sums (unions)

Sum types are a tagged choice of a variant and its payload. Recursion and
generics work as you'd expect; a payload of `{}` is the unit variant.
([`AGTxSUM.thx`](examples/AGTxSUM.thx))

```typescript
$ Maybe : @union =
    Just: `T,
    None: {}

# Bare constructors: the type name is inferred from context (the annotation).
$ some_i : Maybe Int = .Just.{ 5 }
$ none_i : Maybe Int = .None

$ List : @union =
    Cons: {`T, List `T},
    Nil: {}
```

### Pattern matching

`when scrut is pat then e ... else d` matches top to bottom; the first matching arm
wins and binds its variables. Patterns test literals, destructure structs and
variants (nested), and each arm can carry an `if <guard>` that falls through to
the next arm on failure. ([`MATCH.thx`](examples/MATCH.thx),
[`WHEN_GUARDS.thx`](examples/WHEN_GUARDS.thx), [`PATTERNS.thx`](examples/PATTERNS.thx))

```typescript
$ get : Int -> Maybe Int -> Int = \d = \m =
    when m
        is Maybe.Just.{ x } then x
    else d

# Guards fall through, even across arms that share a constructor.
$ grade : Box -> Int = \x =
    when x
        is Box.Some.{ v } if v ?> 100 then 3
        is Box.Some.{ v } if v ?> 0   then 2
        is Box.Some.{ _ }             then 1
        is Box.Nil.{}                 then 0
        else 0 - 1
```

Irrefutable patterns also destructure directly in `let` and lambda parameters,
positionally or by name:

```typescript
$ get_name : Person -> Str = \Person.{ n, _ } = n
$ start_x  : Int = let Line.{ .from = Point.{ x, y }, .to = t } = seg in x
```

### Algebraic effects and handlers

Effects are declared as a set of operations; performing one is just calling it. A
function's type carries the effects it may perform as a **row** on its arrow
(`A -> <E> B`); a bare arrow is pure, and an *unhandled* effect is a compile-time
error. A handler is `do <body> ctl k is op a = e ... [else x = e]`; the captured
continuation `k` is resumed by applying it (affine, **You only get one shot!**).
([`EFFECTS.thx`](examples/EFFECTS.thx))

```typescript
$ Exn   : @effect = throw : Str -> `a,
$ Yield : @effect = yield : Int -> {},
$ State : @effect = get : {} -> Int, put : Int -> {},

# Exception: the handler ignores k, so it resumes zero times.
$ safeDiv : Int -> Int -> Int = \a b =
    do if b ?= 0 then throw "div0" else a / b
    ctl k is throw msg = 0 - 1

# Generator: resume once per yield, summing the results.
$ sumGen : ({} -> <Yield> {}) -> Int = \gen =
    do gen {}
    ctl k is yield v = v + k {}
          else _ = 0
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
    ctl k is yield v = .Susp.{ v, k }
          else _ = .Fin.{}
```

There's also `defer <cleanup> do <body>` (Go-style): the cleanup runs when the
body's scope exits, on normal completion, on abort, or when a stored
continuation holding it finally completes ([`FINALLY.thx`](examples/FINALLY.thx)).

---

## Native backend and FFI

Beyond the interpreter, Thrax lowers the whole IR to self-contained C, including
algebraic effects, via a CEK-style machine emitted in C, with reference-counted
memory management. Foreign C functions are bound with `@extern`:
([`io_example`](examples/io_example/MAIN.thx))

```typescript
$ puts : Str -> Int = @extern "C" "puts" "libc"
$ main : Int = puts "Hello world"; 0
```

The library name is symbolic -- no path or soname appears in source. The
interpreter resolves it with dlopen at run time; the native backend emits a
direct call and a link flag, and the system linker does the rest.

```sh
thrax --build examples/io_example   # -> examples/io_example/bin/<name>{.ir,.c,exe}
```

## Raylib demo

The [`examples/raylib_demo`](examples/raylib_demo) project drives
[raylib](https://www.raylib.com/) entirely through the native backend's FFI,
eight rectangles sweeping a window, recoloured every tenth frame, in constant
stack via tail recursion with no loop construct in the language. It's a
standalone project (its own `build.cpp` and `flake.nix`) meant as a template for
a real FFI program; see its README to run it.

![Raylib demo](https://raw.githubusercontent.com/delyan-kirov/blobs/main/raylib-demo.gif)

---

## Project layout

| Directory | Contents |
| --- | --- |
| `compiler/` | lexer, parser, type checker, lowering |
| `engines/` | the interpreter (`IT`) and C backend (`CC`), plus FFI (`FF`) |
| `platforms/` | the C runtime (`THx*`): machine, memory, values |
| `utilities/` | shared build/support headers |
| `examples/` | annotated `.thx` programs (also the test corpus) |
| `doc/` | language spec and design notes |

More detail lives in [`doc/`](doc/): the [syntax spec](doc/syntax-spec.txt), the
[effect-system design](doc/effect-system-design.md), and the
[native backend](doc/native-backend.md).

## License

MIT see [LICENSE](LICENSE).
