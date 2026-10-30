# Compiler style

How the Rust code in `crates/` is laid out, named and explained. The rules aim
at one thing: a reader who knows the textbook algorithms should be able to find
each one in the compiler by its name, and should find the reasons for the
surprising parts written down next to the code.

`crates/frontend` follows these rules first (issue #379). The interpreter, ccg
and the driver follow in a later issue. New code follows them everywhere.

## 1. A module is one named algorithm or one subsystem

That is the only rule for splitting a file.

- An **algorithm module** implements one algorithm from the literature and is
  named for it: `unify/rows.rs` (scoped-label row unification),
  `usefulness.rs` (Maranget), `a_normal_form.rs`, `tarjan.rs`.
- A **subsystem module** owns one concern of the compiler and is named for
  that concern: `declarations.rs`, `imports.rs`, `externs.rs`.

Size is not a reason to split. A long file that does one thing stays one file,
divided into sections (rule 4). Small helpers never get a module of their own;
they live beside their callers, or in the shared traversal of the data they
walk.

Dependencies point from later phases to earlier ones. Typing never imports from
desugaring or the passes. Something both need belongs to the earlier one.

## 2. The module header

Every module starts with a `//!` header that says, in this order:

1. What the module does, in one sentence.
2. The algorithm by name, with a citation when there is one (author, title,
   venue, year).
3. How it works, briefly: the data it builds and the steps it takes. Enough
   that the code reads as an instance of the explanation.
4. Input and output: what it consumes and what it produces.
5. The entry point, as an intra-doc link.

```rust
//! Exhaustiveness and redundancy checking for `is` matches.
//!
//! The algorithm is Maranget's usefulness, from "Warnings for pattern
//! matching" (JFP 2007). Each arm becomes a row of [`DPat`]s over a small
//! constructor algebra. A vector is useful against a matrix when some value
//! matches the vector and no row of the matrix ...
//!
//! Input: the typed arms of one match. Output: a missing-case witness, and
//! the arms no value can reach.
//!
//! Entry point: [`check_match`].
```

A subsystem header skips the citation and says which part of the language the
module is responsible for.

Headers stay current. A header that names a removed pass, an old file name or a
retired feature is a bug, fixed in the same change that made it stale.

## 3. Notes

An invariant that is not obvious from the code gets a Note, in the style of
GHC's. A Note explains why something is the way it is. It never narrates how
the code was written or what was tried before.

A **definition** is a line comment whose whole text is `Note [Title]`, followed
by the body. It sits beside the code that most depends on it.

```rust
// Note [Rémy levels]
//
// Every type variable carries the let-depth at which it was created.
// Generalizing at a let then means quantifying the variables deeper than the
// let, with no scan of the environment ...
```

Every **reference** is written `See Note [Title]`, in any comment, on one line:

```rust
// Bind at the inner level so the body's variables generalize.
// See Note [Rémy levels].
```

The rules, which `crates/thrax/tests/notes.rs` enforces over every `.rs` file
in the workspace:

- Every `See Note [Title]` has exactly one matching definition, anywhere in the
  workspace.
- A title is defined once.
- Every `Note [` in a comment is either a definition or follows `See`, so a
  misspelled reference cannot slip through as prose.

Titles name the invariant, not the code: `[Case is never lowered to
application]`, not `[lower_case]`. A Note is plain `//`, never `///` or `//!`;
those document one item, a Note may be shared by several.

## 4. Sections

Inside a file, code for one concern goes in one `impl` block (or one run of free
functions) headed by a section marker:

```rust
// ## Row unification

impl Unifier { ... }
```

A file reads top to bottom as its sections. Section titles are short and name
a concern, not a function.

## 5. Doc comments

- Every `pub` item has a doc comment. `cargo doc` stays warning-free.
- A private item gets one only when it is not obvious: a non-trivial
  precondition, an argument whose meaning is not in its type, a result that
  can surprise.
- A doc comment says what the item is or does. It does not restate the
  signature.

## 6. Tests

Unit tests for a module live in a file of their own, always laid out the same
way:

```rust
#[cfg(test)]
mod tests;
```

with the tests in `<module>/tests.rs` (so `typing.rs` has `typing/tests.rs`, and
`typing/unify.rs` has `typing/unify/tests.rs`). No sibling `*_tests.rs`
files, no `#[path]` attributes, no inline test modules.

A module with children is `name.rs` beside a `name/` directory, never
`name/mod.rs`.

Tests that run the whole compiler (the corpus, the negative suite, the
formatter over the library) live under the driver's `crates/thrax/tests/`.

## 7. Names

- Types are named for their role: `Unifier`, `ClosureConverter`,
  `MatchCompiler`. Never `Engine`, `Conv`, `Ctx`, `State` or `Helper`.
- Entry points are named for what they do to what: `check_program`,
  `convert_closures`, `to_a_normal_form`. Not `compile_program` for a pass
  that does one step of compiling.
- No two types in a crate share a name, even in different modules. A reader
  should never have to check which `Var` is meant.
- Algorithm vocabulary wins over invented vocabulary. If the paper calls it a
  matrix, a witness or a level, so does the code.

## Changing behaviour

A tidiness change does not change what the compiler produces. Before and after
a pure restructuring, `thrax emit-c` output is byte-identical on a few examples,
and `cargo test`, `thxdo corpus` and `thxdo grammar` pass. A change that does
alter behaviour is its own step, and says so.
