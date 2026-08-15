# Thrax Launcher

A tiny program launcher written in Thrax against [raylib](https://www.raylib.com/).
Click a button to spawn the program it names.

It is the first showcase of Thrax's **C-struct FFI**: raylib's `Color` is a real
`@struct @extern "C"` passed to raylib **by value**, built with an ordinary
struct literal:

```
$ Color : @struct @extern "C" = r: Nat8, g: Nat8, b: Nat8, a: Nat8,
$ clear_background : Color -> {} = @extern "C" "ClearBackground" "bin/libraylib.so"
...
clear_background (Color.{ .r = 24, .g = 24, .b = 32, .a = 255 })
```

The rest of the raylib API is kept integer-only (positions, sizes, the mouse via
`GetMouseX`/`GetMouseY`), so the app needs no floats.

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
$ apps : List App =
    [ App.{ .label = "Terminal", .cmd = "xterm &" }
    , App.{ .label = "Files",    .cmd = "xdg-open . &" }
    , App.{ .label = "Browser",  .cmd = "firefox &" } ]
```

Each `cmd` is handed to `system(3)`; the trailing `&` spawns it in the
background so the launcher stays responsive.
