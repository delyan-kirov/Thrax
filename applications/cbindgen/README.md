# cbindgen

A C-header binding generator, **written in Thrax**. It reads a C header and emits
Thrax `@extern` bindings, exercising the C FFI (structs, unions, enums) from the
language side. Macros are not supported (they are stripped, like comments).

## What it generates

| C construct | Thrax binding |
| --- | --- |
| `typedef enum { RED, GREEN = 5 } E;` | `$ E : @alias = @int32` plus `$ red : @int32 = 0`, `$ green : @int32 = 5` |
| `typedef struct { int x; float y; } S;` | `$ S : @struct @extern "C" = x: @int32, y: @float32,` |
| `typedef union { int i; float f; } U;` | `$ U : @union @extern "C" = i: @int32, f: @float32,` |
| `typedef struct Handle Handle;` (opaque) | `$ Handle : @alias = @ptr` |
| `Ret GetOne(A a);` | `$ getOne : A -> Ret = @extern "C" "GetOne" "LIB"` |
| `Ret GetThing(A a, B b);` | `$ getThing : {p0: A, p1: B} -> Ret = @extern "C" "GetThing" "LIB"` |
| `void Tick(void);` | `$ tick : {} -> {} = @extern "C" "Tick" "LIB"` |

A C function has no first-class closure to curry, so an `@extern` takes exactly one
argument: a one-parameter C function is `A -> B`; several C parameters become a
record `{p0: A, p1: B} -> Ret` (fields in C order, so the call site is `getThing
{a, b}` or, reordered, `getThing {.p1 = b, .p0 = a}`); a `void`-taking function
takes unit, `{} -> Ret`.

Naming: Thrax requires value and function names to be lowercase (uppercase names
are types/constructors). So a C function name is lowercased at its first letter
(`GetThing` becomes `getThing`) and an enum constant is fully lowercased (`RED`
becomes `red`); the C symbol in the `@extern` string keeps its original case, and
type names (structs, unions, aliases, the enum type) stay capitalized.

Type mapping: `int`/`unsigned`/`short`/`long` and `char`/`float`/`double`/`bool`
map to the matching sized Thrax numerics; `void` is `{}`; a single `char*` is `Str`
in a signature; any other pointer (including `char**`) is `Ptr`; in a struct field
every pointer is `Ptr`. A struct or union with an **array or bit-field member** is
skipped with a `# skipped ...` note, and any function passing/returning such a type
by value is skipped too (a pointer to it is fine). A nullary `typedef` to a struct
(`typedef Vector4 Quaternion;`) becomes an alias usable as a C-repr field.

## Run it

From this directory (the shell links `library/` so `CORE` resolves). The header
path(s) are the command-line arguments to `main`; the rest stays configured by
environment variables.

```
LIB=libfoo.so  MOD=Foo  OUT=foo.thx  thrax run MAIN.thx header.h [more.h ...]
```

- Positional args: the input header(s). With none given, defaults to `test.h`.
  Several headers are read in order and their bindings concatenated into one
  module. A header that cannot be opened becomes a `# cbindgen: cannot open ...`
  note, so one bad path does not abort the rest.
- `LIB` the shared-object name put in each `@extern` (default `lib.so`).
- `MOD` the generated module name (default `BINDINGS`).
- `OUT` the output file; if unset, the bindings are written to stdout.

`main : [n]Str -> <| e> Int` is a C-style entry: it returns an exit code (`0`)
rather than printing a value, and the open effect row lets it do the file IO.

Example against the bundled `test.h`:

```
LIB=libfoo.so MOD=FOO OUT=foo.thx thrax run MAIN.thx test.h
```

The emitted module type-checks on its own (`thrax check foo.thx`).

## Limitations

No preprocessor/macros (stripped). No function-pointer typedefs, no array or
bit-field struct members, no `#include` following. Pointers become `Ptr`. The C
parser handles the common declaration shapes, not the whole grammar.
