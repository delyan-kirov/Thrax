# cbindgen

A C-header binding generator, **written in Thrax**. It reads a C header and emits
Thrax `@extern` bindings, exercising the C FFI (structs, unions, enums) from the
language side. Macros are not supported (they are stripped, like comments).

## What it generates

| C construct | Thrax binding |
| --- | --- |
| `typedef enum { A, B = 5, C } E;` | `$ E : @alias = Int32` plus `$ A : Int32 = 0`, `$ B : Int32 = 5`, `$ C : Int32 = 6` |
| `typedef struct { int x; float y; } S;` | `$ S : @struct @extern "C" = x: Int32, y: Real32,` |
| `typedef union { int i; float f; } U;` | `$ U : @union @extern "C" = i: Int32, f: Real32,` |
| `typedef struct Handle Handle;` (opaque) | `$ Handle : @alias = Ptr` |
| `Ret f(A a, B b);` | `$ f : A -> B -> Ret = @extern "C" "f" "LIB"` |

Type mapping: `int`/`unsigned`/`short`/`long` and `char`/`float`/`double`/`bool`
map to the matching sized Thrax numerics; `void` is `{}`; `char*` is `Str`; any
other pointer `T*` is `Ptr` (hand-edit to `List T` for an array parameter); a known
type name stays itself. A struct or union with an **array or bit-field member** is
skipped with a `# skipped ...` note, since those member shapes are not
representable yet.

## Run it

From this directory (the shell links `library/` so `CORE` resolves):

```
HEADER=path/to/header.h  LIB=libfoo.so  MOD=Foo  OUT=foo.thx  thrax run MAIN.thx
```

- `HEADER` the input header (default `test.h`).
- `LIB` the shared-object name put in each `@extern` (default `lib.so`).
- `MOD` the generated module name (default `BINDINGS`).
- `OUT` the output file; if unset, the bindings are written to stdout (followed by
  the interpreter's `main = {}` line, which a file avoids).

Example against the bundled `test.h`:

```
HEADER=test.h LIB=libfoo.so MOD=FOO OUT=foo.thx thrax run MAIN.thx
```

The emitted module type-checks on its own (`thrax check foo.thx`).

## Limitations

No preprocessor/macros (stripped). No function-pointer typedefs, no array or
bit-field struct members, no `#include` following. Pointers become `Ptr`. The C
parser handles the common declaration shapes, not the whole grammar.
