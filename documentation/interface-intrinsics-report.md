# Interface intrinsics: work report

A record of what was built for the `@compiler_interface_*` overloadable-hook effort
(design in `literal-and-interface-intrinsics.md`), plus the design decisions taken
along the way. Everything below is landed on the `type-intrinsics` branch, built and
tested on **both engines** (the interpreter and the `ccg` native C backend), with the
full `cargo test` suite, the combined `tests/MAIN.thx` corpus, and the `examples/`
green.

## Goal

Replace hardcoded literal/type machinery in the compiler with a small family of
overloadable `@compiler_interface_*` functions, so a user type can adopt the surface
syntax the built-ins enjoy: string/integer/real/sequence literals, `.[..]` indexing,
and literal/sequence patterns. The rule: a `@`-name is user-extensible **iff** it
starts with `@compiler_interface_`; any other `@`-definition is rejected.

## What landed

### Stage 0 — ascription, indexing hook, prefix rule
- **`(e : T)` expression ascription** — new `Expr::Ascribe`, parsed in `parse_group`,
  checked as `check(e, T)`, lowered transparently. Resolves literal / overload
  ambiguity anywhere.
- **`.[..]` indexing** now desugars to `@compiler_interface_indexing` (was a fixed
  `index`); `LA` (tensors), `MAP` (`Map k v -> k -> Option v`), and a `Grid` example
  provide overloads.
- **Prefix rule** — `$ @compiler_interface_*` is a definable hook; every other
  `$ @name` errors "not extensible in user code".

### Stage 4 — type-name cleanup (done before Stage 1)
- Word-size integers are now spelled `@int` / `@nat`; the friendly `Int` / `Nat`
  spellings were **dropped**. `Real` / `Str` keep friendly names; `Bool` was already
  `@bool`-only.
- Internal Con names are unchanged (still `"Int"`/`"Nat"`), so only the source
  spelling and display changed — mirroring how `@bool` / `@list` already worked.
- The **entire** corpus, examples, applications, and Rust test snippets were migrated.

### Stage 1 — literal construction hooks
Four overloadable construction hooks:
- `@compiler_interface_string_literal  : Str -> a`
- `@compiler_interface_integer_literal : @int -> a`
- `@compiler_interface_real_literal    : @float64 -> a`
- `@compiler_interface_sequence_literal : @vec t -> f t`

A literal whose **expected type is a user type** providing the matching hook builds
that type through it — driven by a signature / argument context or a `(e : T)`
ascription. Otherwise the literal keeps its built-in default, which lowering **folds
to a plain constant** (no payload, no runtime conversion). The interception
(`literal_hook_check`) reuses the overload resolver's trial-unify/rollback and records
the resolved hook per literal site. All numeric / sized-int behaviour is untouched.

The string hook takes the **built-in `Str`** (not raw `@array`): the compiler hands
over a fully UTF-8-checked, escape-decoded `Str` and the custom type just maps it.

### Stage 2 — pattern intrinsics
- **Literal patterns via equality** — `is "foo"` / `is 42` on a user type builds the
  literal into that type and matches with `@compiler_interface_equality : t -> t ->
  @bool`. New core `Pat::HookEq`, expanded by `patmat` to a boolean test.
- **Sequence patterns via view** — `is [a, b, ..r]` / `is h :: t` / `is []` on a user
  type unfold `@compiler_interface_sequence_view : f t -> SeqView (f t) t` (new core
  union `SeqView s t = Empty | More t s`). New core `Pat::SeqView`, expanded by
  `patmat` to nested `More`/`Empty` matches; fixed-length patterns require an empty
  tail, `..rest` binds the remainder.

Both are check-directed in `type_pattern` (only user types route; the built-in
`Str`/`List` fast paths are unchanged), and both new `Pat` variants are eliminated by
`patmat` before the ANF / de-Bruijn passes.

### Stage 3 — List de-builtined (the tractable half)
- **`List` is now an ordinary CORE `@union`** (`Nil` / `Cons`), no longer a compiler
  builtin. The hardcoded `List`/`Cons`/`Nil` special-cases were removed from the
  checker (`variant_sig`, `find_union_by_tag`, `union_head_with_tag`) and lowering
  (`Decls::variant`, `CONS_FIELDS`) — they all resolve through the CORE declaration.
- Enabled by a genuinely useful prerequisite: **type-directed resolution of bare
  variant tags**. A `.Tag` now takes its union from the expected type
  (`union_head_with_tag` + a `check` arm), and `infer_variant` checks payloads
  bidirectionally so the expectation flows into nested `.Tag`s. This lets two unions
  share a constructor name (e.g. a user list and the prelude `List`, both with
  `Cons`/`Nil`) without collision — which is what unblocked List-as-a-union.

## Design decisions (explored, then deliberately not shipped)

- **`@str` → `@array` fold: decided against.** It throws away the useful
  `Str`-vs-raw-bytes distinction, and `Str`'s hard parts (UTF-8 validation, escape
  decoding) are genuinely primitive.
- **`Str` as a library `@unbox` newtype: built, then removed.** A transparent
  single-field newtype (`@struct @unbox`, wrapper erased at lowering, zero-cost) was
  implemented and briefly used for `Str`. It was removed at the maintainer's call:
  `Str` is load-bearing in CORE-less bootstrap contexts (`C.thx`'s libc bindings, the
  `main : [n]Str` argv contract), so a library `Str` fights that for no payoff. The
  `@unbox` feature itself was removed too (its only motivator was `Str`). Recorded
  gotcha: automatic (non-opt-in) single-field unboxing is unsound because of the
  `Con ~ Record` bridge (a 1-field struct can be used structurally as an open record).
- **`CStr` for C interop: deferred with a design note.** The FFI already appends a NUL
  when marshalling `Str` → `char*`, so `Str` → C is already safe; a distinct `CStr`
  is type-level clarity, not a correctness fix, and it ripples into `IO.thx`. Written
  up in `documentation/TODO.md` ("Str vs C strings") to revisit.

## Deferred (tracked, not blocking)

- **Stage 1b** — the `@default` overload attribute + unconstrained literal defaulting
  (a module making a bare, unconstrained `[1,2,3]` build a user type by default). The
  plan flags this as its own milestone; the check-directed behaviour needs none of it.
- **`@str` de-builtining** — not pursued (see decisions above).
- **CStr / explicit C-string interop** — see `documentation/TODO.md`.

## Verification

Every stage was validated on **both** the interpreter and the `ccg` native backend,
with dedicated regression tests plus the full existing suite, the combined
`tests/MAIN.thx` corpus, and the `examples/` (including the FFI-heavy `STRINGS` /
raylib paths). New tests include: expression ascription; the indexing hook (incl. a
non-element result); all four construction hooks (context + ascription + cross-module
import + not-hijacking-defaults + ccg parity); equality-hook and sequence-view
patterns (+ ccg parity); the `Int`/`Nat` alias drop; and type-directed bare-variant
resolution coexisting with the prelude `List`.

## Key files

- `crates/frontend/src/parser.rs`, `parser/data.rs` — ascription node, prefix rule,
  the `.[..]` desugar.
- `crates/frontend/src/typing.rs` — the hook family, `literal_hook_check` /
  `literal_pattern_hook_check` / `sequence_pattern_hook_check`, `resolve_hook` /
  `hook_use`, type-directed variant resolution, the `@int`/`@nat` and `List`
  de-builtining.
- `crates/frontend/src/lowering.rs`, `lowering/patmat.rs`, `lowering/data.rs` — hook
  wrapping/folding, `Pat::HookEq` / `Pat::SeqView` and their expansion.
- `library/CORE.thx` — `SeqView`, the `List` union.
- `library/LA.thx`, `library/MAP.thx`, `examples/TENSORS.thx` — indexing-hook
  overloads.
- `documentation/literal-and-interface-intrinsics.md` — the design + staged plan with
  per-stage status; `documentation/TODO.md` — the CStr note.
