# Thrax Launcher

A tiny program launcher written in Thrax against [raylib](https://www.raylib.com/).
Click a button to spawn the program it names.

The whole raylib API is imported from `RAYLIB.thx`, which is **generated** by the
cbindgen tool (`applications/cbindgen`) from raylib's header. `MAIN.thx` just does
`$ with RAYLIB` and calls the bindings. It showcases Thrax's **C-struct FFI**:
raylib's `Color` is a real `@struct @extern "C"` passed **by value**, built with
an ordinary struct literal.

A C function is never curried, so a raylib call with several parameters takes a
**record** of its arguments (fields named `p0`, `p1`, ... in C order); a
one-parameter call stays `A -> B`, and a `void`-taking call takes unit:

```
$ with RAYLIB
...
clearBackground (Color.{ .r = 24, .g = 24, .b = 32, .a = 255 })      # one arg
drawRectangle { .p0 = x, .p1 = y, .p2 = w, .p3 = h, .p4 = col }      # a record
beginDrawing {}                                                       # unit
```

The generated bindings use the `@`-sigil built-in types (`@int32`, `@nat8`, ...),
so coordinates are `@int32`; the app stays integer-only (positions, sizes, the
mouse via `getMouseX`/`getMouseY`), no floats.

Regenerate the bindings after a raylib upgrade:

```
LIB=bin/libraylib.so MOD=RAYLIB OUT=RAYLIB.thx \
  thrax run ../cbindgen/MAIN.thx <path-to>/raylib.h
```

## Run it

From the repo, build the `thrax` binary once:

```
nix develop -c cargo build -p thrax     # produces target/debug/thrax
```

Then, from **this directory**:

```
nix develop            # links bin/libraylib.so and library/ into place
thrax run MAIN.thx     # or: ../../target/debug/thrax run MAIN.thx
```

`nix develop`'s shell hook symlinks `bin/libraylib.so` (the library the
`@extern` paths name) and `library/` (the Thrax standard library the interpreter
resolves `CORE` from), so this directory is self-contained.

### Build a native binary instead

The C backend emits a real `typedef struct { ... } Color;` and passes it by
value, so the compiled program uses the platform ABI directly:

```
thrax build MAIN.thx   # emits MAIN.c, compiles and links -> ./MAIN
./MAIN
```

## Edit the app list

The launchable programs are a plain list of records near the top of `MAIN.thx`:

```
$ App : @struct = label: Str, cmd: Str,
$ apps : @list App =
    [ App.{ .label = "Terminal", .cmd = "xterm &" }
    , App.{ .label = "Files",    .cmd = "xdg-open . &" }
    , App.{ .label = "Browser",  .cmd = "firefox &" } ]
```

Each `cmd` is handed to `system(3)`; the trailing `&` spawns it in the
background so the launcher stays responsive.
