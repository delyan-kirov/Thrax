# The language server and the diagnostics behind it

`thrax lsp` is the compiler speaking the Language Server Protocol over stdio.
It is not a second front end: every diagnostic, fix and type it shows is the
one `thrax check` produces, so the editor and the command line never disagree.
This document describes what it does, how it stays fast, and the ideas it takes
from Flix, GHC and rustc, plus a few from the research literature.

## What an editor gets

| Request | What Thrax does |
|---|---|
| diagnostics | every error and warning, ranked, with the most important one marked |
| code actions | each `help:` that carries edits, as a quick fix; plus "apply every safe fix" |
| hover | the type of the expression under the cursor, or a definition's signature |
| go to definition | globals, across modules (locals are not tracked yet) |
| completion | visible names with their types, a module's members after `M.`, a hole's fits |
| document symbols | the file's definitions, structs, unions, aliases and effects |
| formatting | `thrax fmt`, whole file or a range (widened to whole items) |
| semantic tokens | highlighting from the lexer itself, so an editor needs no Thrax grammar |

Text sync is incremental. Positions are negotiated: UTF-8 when the client
offers it (Neovim and Helix do), UTF-16 otherwise.

## Diagnostics that act

A diagnostic has a stable code (`error[TYPE_MISMATCH]`), a message that says
what is wrong, `note:` lines that say why, and `help:` lines that say what to
do. A `help:` may carry machine edits and an applicability, after rustc:

- `MachineApplicable`: certainly right. `thrax fix` and the editor's "apply
  every safe fix" use only these (a missing `$ with IO`, `2.` to `2.0`).
- `MaybeIncorrect`: a likely fix (a did-you-mean).
- `HasPlaceholders`: an edit with a hole left to fill.

On the command line a fix shows the patched line under its `help:`. `thrax
explain CODE` prints the code's entry in `documentation/errors.md`.

`???` is a typed hole (after GHC). It has whatever type its context needs, and
it is always an error that names that type and lists the names in scope that
fit, locals first, each offered as a fix. Completion on a hole offers the same
fits.

## Many errors, not one

The compiler used to stop at the first error. Now each phase keeps going, the
way Flix's do:

- **Parser.** Every item starts with `$` at the start of a line, so a broken
  item is skipped to the next one and parsing continues. An error that runs
  into the next item points at the end of the broken one, where the missing
  piece goes, and offers to insert a missing closer.
- **Checker.** A definition (a strongly connected group) that fails is
  recorded, its half-finished state is unwound, and it is given its signature
  if it has one (so its callers are still checked against it) or the error
  type otherwise. The error type unifies with everything, so one mistake does
  not cascade into its uses (rustc's `ty::Error`). Uses of a name whose item did
  not even parse get the error type too.
- **Driver.** A module that fails still exports what checked, and its
  dependents are checked against it. `thrax check` prints every error, in
  dependency order, up to `[check] max_errors`.

Inside one definition the checker still stops at its first error. Recovering
within a definition (Hazel's marked lambda calculus makes that total) is the
natural next step.

## The most important error first

An editor that underlines everything in the same red is a flashlight: it
shows that something is wrong everywhere and nothing about where to start.
Thrax ranks its diagnostics and maps the rank onto the severities every
editor already renders differently. Each diagnostic carries its role in
`data.role`:

| role | when | severity |
|---|---|---|
| `focus` | the error that matters most in the file | Error |
| `root` | another independent error | Error (Warning with `focus = "one"`) |
| `consequence` | an error in a definition that names one that failed | Information, linked to its cause |
| `grace` | an error inside the definition being typed, before typing pauses | Hint |
| `lint` | an unreachable arm and the like | Warning, faded as unnecessary |

The default order is cheap: syntax before types before warnings, then the
order the checker met them (dependencies before their users), then source
order. Two refinements come from the literature:

- **Counterfactual ranking.** When typing pauses and a file has several
  errors, each error site (up to 8) is replaced by a hole and the file is
  checked again. The error whose hole clears the most *other* errors is the
  focus, and its message says how many it may clear. This is the question
  SHErrLoc (Zhang and Myers, POPL 2014) and counterfactual typing (Chen and
  Erwig, POPL 2014) ask, answered by brute force, which is affordable because
  one module re-checks in milliseconds.
- **Typing grace.** The definition under the cursor is usually half-written.
  Its errors are held back as hints until typing pauses for `grace_ms`, while
  errors the edit causes elsewhere (a caller it broke) show at once.

`tools/editors/neovim/thrax.lua` turns this into one loud error: only the
focus gets inline text, and quieter roles are underlined with dots.

## Fixes the compiler has tried

A fix the compiler is unsure of (`MaybeIncorrect`) is applied to a copy of the
file and checked before it is offered. A fix that removes errors is marked
preferred and says so ("checked: 1 error fewer"); one that adds errors is not
offered at all. This is Seminal's idea (Lerner, Flower, Grossman, Chambers,
PLDI 2007: search for the edit that makes the program type-check) applied to
the fixes the compiler already proposes. It runs only when the editor asks for
code actions, at most 8 trials per request.

## Two tiers: as you type, and on save

- **As you type** (debounced by `debounce_ms`): parse, resolve, type-check and
  check exhaustiveness. No compile-time code runs. In a module whose `$ @e` or
  `@build` may define names, an unbound name is only a hint until saved.
- **On save**: the whole compile-time pipeline runs as `thrax check` would
  (every `$ @e` expansion, `@build`, the asserts), and its faults show in the
  editor. Spans in generated code are mapped back to the site that generated
  it. With `check = "save"` everything waits for save, as with rust-analyzer's
  check-on-save.

Compile-time code may print. The server moves its own protocol onto a copy of
stdout and points stdout at stderr, so nothing a program prints can corrupt the
protocol.

## Fast

A document is checked over its whole import closure, but the closure minus the
document (its imports, `CORE`, `C`) changes rarely. That part is the *base*:
parsed and checked once and kept while none of its files changes (open
buffers by content, files by modification time). Each edit then costs one
module: the base's tree is copied (its stores keep their tags, so every handle
still resolves), the document is parsed onto the copy, and only it is checked,
importing from the base's checkers. The type store is shared and append-only;
before each check it is rewound to the base's watermark, the arena-reset
discipline, so memory does not grow with edits. Trials (verified fixes,
ranking) rewind to a second watermark, which leaves the current snapshot
intact.

Edits are coalesced; when new messages arrive mid-analysis, the remaining
documents yield to them.

Measured on alec (27 modules, 7.6k lines), release build:

| | before | now |
|---|---|---|
| open `TEST.thx` (builds the base) | 105 ms | 108 ms |
| each edit of `TEST.thx` | 90 ms | 4 to 9 ms |
| each edit of `PARSE.thx` (a large module) | 55 ms | about 21 ms |

The next step is per-definition reuse inside the edited module: a definition
with a signature is checked from that signature, so a body edit need only
re-check that body.

## Configuration

`thrax.toml`, found in the file's directory or above, holds the options.
`thrax config` prints the ones in force and documents every one; the table in
`crates/utilities/src/config.rs` is the single source, so adding an option is
adding a row. A misspelled option gets a did-you-mean fix.

```toml
[format]
width = 100
indent = 4

[check]
max_errors = 20

[lsp]
check = "type"     # or "save"
focus = "roots"    # "all", "roots" or "one"
debounce_ms = 150
grace = true
grace_ms = 1500
max_diagnostics = 50

[lints]
unreachable_arm = "warn"   # "allow", "warn" or "deny"
```

An editor's settings (`initializationOptions`, or `workspace/didChangeConfiguration`
under `thrax`) use the same sections and keys, and override the file.

## Formatting

`thrax fmt` prints tokens verbatim and chooses only the whitespace between
them, so it cannot change what a program means; it re-lexes its own output and
refuses if a single token or comment differs, or if the result stops parsing.
It works on tokens rather than the AST because the AST already desugars forms
(string interpolation, `&&`, `<|>`) that must print as written. The structure
that decides line breaks (items, brackets, `let`/`if`/`is`/`\`/`do` forms and
their arms) is read off the token stream, and a Wadler-style printer fits each
group into `width` or breaks it. Indentation is 4 spaces.

The rules, in short: a group fits on one line or breaks at every one of its
line points; an `is` or handler with more than two arms puts each arm on its
own line under the head; `let ... in` ends its line with `in`; `else if` chains
stay flat; a list written across lines with a trailing comma stays one entry
per line; blank lines between items collapse to one; comments stay where they
were written.

## Editors

- Neovim: `tools/editors/neovim/thrax.lua`. Highlighting comes from the
  server's semantic tokens.
- Helix: `tools/editors/helix/languages.toml`. Helix ignores semantic tokens
  and highlights only through tree-sitter, so it also needs the grammar:
  `thxdo tree-sitter` generates it and `hx --grammar build` compiles it, and the
  hand-written `tools/editors/helix/queries/thrax/highlights.scm` maps its
  nodes to colors.

The tree-sitter grammar is not written by hand: `tools/editors/TREE_SITTER_GEN.thx`
(a Thrax program) reads the Bison spec `documentation/thrax.y` and writes
`grammar.js`, carrying over the rules and their precedence, removing empty
productions the way tree-sitter needs, and spelling each terminal. The output is
ignored by git. The tree-sitter CLI comes from `tools/editors/flake.nix`, kept
out of the root dev shell. Generating it also keeps the spec honest: parsing the
whole corpus with it found the places the spec had drifted from the language.
Two known divergences remain: the real parser ends an `is` at a `| Op.op x`
handler clause (more lookahead than LR(1) has), and `@e <| ..` inside a type
is not in the spec.

## Where the code is

- `crates/utilities/src/error.rs`: the diagnostic model (severity, fixes,
  applicability, the rendered patch), `closest` for did-you-mean.
- `crates/utilities/src/config.rs`, `json.rs`: `thrax.toml`, the wire format.
- `crates/frontend/src/parser.rs`: item-level recovery. `typing.rs`: the error
  type, per-definition recovery, holes and their fits.
- `crates/frontend/src/format.rs`: the formatter.
- `crates/thrax/src/lsp/`: the server (`server.rs`), the incremental analysis
  (`analysis.rs`), documents and positions (`text.rs`).
- `crates/thrax/src/tools.rs`: `fmt`, `fix`, `explain`, `config`.
