# TODO

The project backlog: open items in one place. Larger subsystems keep their own
design notes under `documentation/`, linked below.

## Engine / type system / FFI

- **Standard-library native support lib.** Move the int/float/pointer
  conversions (`C.i2d`/`d2i`/`i2f`/`f2i`/`i2p`, and `C.null` = `i2p 0`) out of the
  engine runtimes (the interpreter host table in
  `crates/interpreter/src/machine/data.rs`, and the `thx_*` C functions in
  `crates/ccg/src/runtime.c`) into a std-owned native library bound via `@extern`
  (C now, a Rust cdylib later), so the compiler stops accreting per-symbol special
  cases. Bindings live in `library/C.thx` (lib `""`), re-exported by
  `library/MATH.thx` as `real_of_int` / `int_of_real` / `f32_of_int` /
  `int_of_f32`. Open sub-problem is distribution: the interpreter cannot compile C
  at runtime, so the lib must be a prebuilt, findable artifact. Recommended: build
  `library/libthraxstd.so`, add `library/` to the loader path, reference it as
  `"thraxstd"`; alternative: teach the FFI resolver (`machine::ffi`) to resolve it
  relative to the stdlib dir. The pure-wasm target has no dlopen, so those symbols
  need a separate story. Proven feasible: a companion `.so` bound via `@extern`
  already works in both engines. See `documentation/native-backend.md` ("The
  `lib` field: where a symbol comes from").

- **Building funcitons** In jai, you can use the
  programming language like a library and even interact with it with an event loop.
  I think that's useful.

- **lsp, syntax highlighting, tags** There should be more language tooling. 

- **Overloading of `@extern` with record params.** Same-name overloads of a C
  function do not resolve: a positional call fails overload resolution (tuple to
  record promotion does not run during overload trial-unify), and a named-record
  call type-checks but the extern record-flatten misfires. Distinct-name
  fixed-arity bindings work. This is the mechanism a `printf` family would want.

- **Operator overloading is parsed but not implemented.** `$ @operator.{ op } :
  ty = expr` produces an `Item::OperatorDef` that nothing in typing or lowering
  consumes, so a user operator never resolves. Wire it up or drop the surface.
  The syntax should be updated to `@opr "<operator>"`.

## Tooling / testing

- **Negative test suite.** Every suite today is positive (asserts good programs
  pass), which structurally cannot catch a soundness hole (a bad program that
  wrongly passes, which is how the struct-literal arity bug shipped). Need
  programs each paired with the diagnostic (or error code) they must produce; a
  run fails if such a program compiles, or fails with the wrong error. Seed cases:
  positional struct literal with wrong arity/field count; bare `.{..}` with no
  inferable struct; over/under-applied type constructors; scalar/record promotion
  that should not fire. Mechanism (file format, stability against message churn,
  corpus vs beside the Rust tests) still to design.

## Larger subsystems (own design docs)

- **Effects M3.2:** evidence passing (O(1) dispatch) and the tail-resumptive
  optimization; the handler is still a dynamic search up the stack. See
  `documentation/effect-system-design.md`.
- **Project reorg:** largely done already (the Rust rewrite delivered the modular
  `crates/` layout and retired make for cargo + nix; the only makefiles left are
  in the vendored `external/libffi`). Residual ideas: differential parser testing
  against the bison grammar (`documentation/thrax.y`), and a fuller platform
  split.

## Surface syntax (open decisions)

- **Interpolation:** require interpolants to already be `Str`, or add a
  `show`/stringify overload first? Brace-escape spelling? Perhaps new syntax?
- **Ranges:** desugar to a library `range` function (recommended) or a core
  primitive? Descending `..>` now or later?
- **More intrinsics** `@if` `@else` `@inc` `@tokens` `@char`
- **Effects to std lib functions** why does printing not have an effect?
- **More operators** for example the map operator `<!>`, exponential `^`
  and potentially more.
