# Thrax raylib demo

A small standalone project that drives [raylib](https://www.raylib.com/) from
Thrax through the **native (C) backend's FFI**. It renders eight rectangles
sweeping across a window, recolouring every tenth frame — all in constant stack
via tail recursion, with no mutation or loop construct in the language (the
per-frame state is threaded through a self-recursive `loop`; see `MAIN.thx`).

It is deliberately kept apart from the main build (its own `makefile` and
`flake.nix`) so it can serve as a template for a real FFI project.

## Run

```sh
nix develop     # brings in raylib (exports $RAYLIB), cc and make
make run        # emit C -> compile -> link -> run  (needs a display)
```

`make run` builds `bin/raylib_demo` from `MAIN.thx` (emitting self-contained C
with `thrax -c`, then `cc ... -ldl`) and symlinks raylib's shared object to
`bin/libraylib.so`, which the program's `@extern` bindings load at runtime.

If the compiler is missing it is built from the repo root automatically; you can
also build it yourself with `make` in the repository root first.

## How it works

- Each raylib function is bound with `@extern.{ "Symbol", "bin/libraylib.so" }`;
  the backend emits a `dlopen`/`dlsym` wrapper and a direct, typed C call.
- `Color` is a 4-byte struct that the x86-64 SysV ABI passes in an integer
  register, so it marshals fine as a packed-RGBA `Int` — no struct-by-value
  support needed.
- The void raylib calls are declared to return `Int` so they stay first-class
  function values and can be sequenced with `+` (which forces its operands left
  to right); their results are summed and discarded.

## Files

- `MAIN.thx` — the demo (module `MAIN`, entry `main : Int`).
- `makefile` — emit/compile/link/run; symlinks raylib into `bin/`.
- `flake.nix` — dev shell providing raylib + X11/GL.
