# Ares

Ares is a functional language with algebraic data types (structs *and* sum
types), pattern matching, and typed **algebraic effects with handlers**. It
compiles to native C, cross-compiles to WebAssembly, and can also be interpreted.

## Links

- **[Try it in your browser](https://delyan-kirov.github.io/ares-applications/)**
- **[Source on GitHub](https://github.com/delyan-kirov/Ares)**: the compiler, standard library, and docs.
- **[Examples](examples/)**: a small program for each feature (also the test corpus).

## Building

**With nix** (recommended):

```sh
nix develop      # dev shell; `ares` is already on PATH here
cargo build      # or build it yourself: target/debug/ares
cargo test       # the frontend, interpreter, and backend suites
```

**Without nix**: any host with a recent `cargo`/`rustc` and a C toolchain:

```sh
cargo build --release   # target/release/ares
```

Linux is the development platform; macOS, Windows and wasm are tested in CI,
and each release ships an SDK for them. See
[documentation/platforms.md](documentation/platforms.md).

## Compiling and running

`ares` takes a subcommand and, optionally, a `.ares` file. With no file it
uses `MAIN.ares` in the current directory.

A program is a module with an entry point, and there is exactly one accepted
shape: `@main` takes the argument vector (`args[0]` is the program) and
returns the exit code. It is the only name the compiler knows.

```ares
$ @main : @Vec @Str -> <@IO> @Int = \args => C.puts "hello"; 0
```

Every other module is a library. Checking one runs its compile-time
assertions (`$ &assert ..`), so a module states its own invariants and
type-checking it is testing it.

```sh
ares run     MAIN.ares          # run under the interpreter
ares check   examples/FIB.ares  # type-check, print types, run `$ &` checks
ares expand  MAIN.ares          # the source after metaprogram expansion
ares emit-c  MAIN.ares          # print the generated C (native backend)
ares build   MAIN.ares          # compile to a native executable
ares --target=wasm32-wasi build MAIN.ares   # cross-compile to wasm
```

The compiler is also the tooling. Every error carries a stable code
(`ares explain CODE` says what it means), keeps going past the first
mistake, and offers a fix when it knows one; `ares fix` applies the sure
ones. `???` is a typed hole: the error names the type that goes there and
the names in scope that fit.

```sh
ares fmt     MAIN.ares          # format in place (--check only reports)
ares fix     MAIN.ares          # apply every fix the compiler is sure of
ares explain TYPE_MISMATCH     # what a diagnostic code means
ares lsp                       # language server for an editor
```

`ares lsp` serves diagnostics, fixes, hover, go-to-definition, completion,
symbols and formatting. It ranks errors so the one that matters most stands
out; see documentation/lsp.md, and tools/editors/ for Neovim and Helix.

## Features

- **Algebraic data types**: structs and sum types, generic by default; a declaration can copy another's members with a `&` splice.
- **Pattern matching**: nested, literal, string, and range patterns, `++` sequence patterns that match a prefix, a suffix, or both ends, and fall-through guards.
- **Typed algebraic effects with handlers**: the effect row is part of the type; continuations are first-class and resumable.
- **A native C backend**: the whole IR, effects included, lowers to self-contained C via a CEK machine, with reference-counted memory.
- **C FFI with no ceremony**: `@extern "C" "sym" "lib"` binds a foreign function; C structs pass by value both ways.
- **WebAssembly**: cross-compile with `--target=wasm32-wasi`, or run the whole compiler in the browser.
- **Interfaces resolved by type**: an interface is a struct, an implementation is a value of it, and a function takes one as a named `@use` field the compiler infers. One mechanism; no overloading, so a name's printed type is its contract.
- **And more**: row-polymorphic records, lazy recursive types and streams, sized tensors, tail-call optimization, and compile-time metaprogramming (`&` inserts generated code, plus a `@build` entry that generates part of a module from what is on disk).

## Example: exact probabilistic inference

Example implementation of exact probabilistic inference by enumeration.

```ares
$ Prob : @effect = pick : @Int -> @Int, observe : @Bool -> {},

# The weight of a set of worlds: `yes` of them made the model true, `all`
# survived every `observe`, each worth `1 / den`.
$ Mass : @struct = yes: @Int, all: @Int, den: @Int

$ gcd : @Int -> @Int -> @Int = \a b => if b == 0 => a else gcd b (a % b)

$ impl_IAdd_for_Mass : IAdd Mass = .{
    .add = \a b =>
        let den = a.den / gcd a.den b.den * b.den in
        let sa = den / a.den in
        let sb = den / b.den in
        Mass.{ .yes = a.yes * sa + b.yes * sb, .all = a.all * sa + b.all * sb, .den = den }
}

$ nothing : Mass = Mass.{ .yes = 0, .all = 0, .den = 1 }

# Resume `k` with every outcome `i ..< n` and add up the worlds that follow.
$ each : (@Int -> Mass) -> @Int -> @Int -> Mass = \k i n =>
    if i == n => nothing else k i + each k (i + 1) n

$ infer : ({} -> <Prob> @Bool) -> Mass = \model =>
    do model {}
    ctl k
    | Prob.pick n     => let m = each k 0 n in Mass.{ .yes = m.yes, .all = m.all, .den = m.den * n }
    | Prob.observe ok => if ok => k {} else nothing
    | b               => Mass.{ .yes = if b => 1 else 0, .all = 1, .den = 1 }

# P(model is true | every observation held), reduced.
$ chance : ({} -> <Prob> @Bool) -> @Str = \model =>
    let m = infer model in let g = gcd m.yes m.all in "?(m.yes / g)/?(m.all / g)"

# Two children, at least one a boy. Both boys? 1/3
$ two_boys : {} -> <Prob> @Bool = \u =>
    let a = pick 2 in let b = pick 2 in observe (a == 0 || b == 0); a == 0 && b == 0

# The same, but that boy was born on a Tuesday: 13/27
$ tuesday_boy : {} -> <Prob> @Bool = \u =>
    let a = pick 2 in
    let a_day = pick 7 in
    let b = pick 2 in
    let b_day = pick 7 in
    observe (a == 0 && a_day == 2 || b == 0 && b_day == 2); a == 0 && b == 0

# Monty Hall: does switching win? 2/3
$ switching_wins : {} -> <Prob> @Bool = \u =>
    let car = pick 3 in
    let guess = pick 3 in
    let left = (guess + 1) % 3 in
    let right = (guess + 2) % 3 in
    let opened =
        if car == guess =>
            (if pick 2 == 0 => left else right)
        else if car == left =>
            right
        else
            left
    in
    3 - guess - opened == car
```

## Native backend and FFI

Ares lowers the whole IR to self-contained C, algebraic effects included, via
a CEK-style machine emitted in C. Foreign C functions are bound with `@extern`;
the library name is symbolic (no path or soname in source).

```ares
$ puts : @Str -> <@IO> @Int = @extern "C" "puts" "libc"
$ @main : @Vec @Str -> <@IO> @Int = \args => puts "Hello world"; 0
```

## Project layout

| Directory | Contents |
| --- | --- |
| `crates/frontend` | lexer, parser, type checker, lowering, IR |
| `crates/interpreter` | the reified-K (CEK) machine and the `@extern` FFI |
| `crates/ccg` | the C backend (emits standalone C plus its runtime) |
| `crates/ares` | the `ares` driver (CLI) |
| `library/` | the standard library (`C` libc namespace, `CORE`, and the rest) |
| `examples/` | annotated `.ares` programs (also the test corpus) |
| `tools/` | dev tooling, incl. this README generator |
| `documentation/` | language spec and design notes |

## License

MIT, see [LICENSE](LICENSE).

## Autogenerated

> This file is GENERATED by `tools/README_GEN.ares`, the single source of
> truth for its examples (each is validated at build time). Do not edit by
> hand; edit the generator and regenerate with `ares run tools/README_GEN.ares`.

