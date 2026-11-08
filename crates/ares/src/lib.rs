//! The Ares compiler as a library, and the entry point of the workspace
//! documentation. The compiler is five crates, in pipeline order:
//!
//! * [`utilities`] holds the foundations every phase shares: the bump arena, the
//!   handle-addressed store, the diagnostic model, and the compilation target as
//!   data.
//! * [`frontend`] is the front end and middle end: [`frontend::lexer`],
//!   [`frontend::parser`], [`frontend::typing`], [`frontend::desugar`] (to the
//!   Core), and [`frontend::ir`] (pattern-match compilation, A-normalization,
//!   De-Bruijn indexing, closure conversion).
//! * [`interpreter`] evaluates that IR with the reified-K (CEK) machine.
//! * [`ccg`] emits a standalone C program from the same IR.
//! * `ares` (this crate) ties them together: [`driver`] wires the phases for
//!   `lex`/`parse`/`check`/`run`/`build`/`emit-c`, [`stdlib`] is the standard
//!   library the compiler carries, and [`capi`] exposes the compiler to C. The
//!   crate builds as an rlib, a static library (`libares.a`) and a shared one
//!   (`libares.so`), so a native Ares program can call back into the compiler
//!   for `@lex`, `@parse_str` and `@eval` at run time. The `ares` executable
//!   (the CLI and the GHCi-style shell) is a thin binary over this library.
//!   [`lsp`] is the language server, [`highlight`] the lexical coloring it
//!   shares with the shell, and [`tools`] the source tools (`fmt`, `fix`,
//!   `explain`, `config`).

pub mod capi;
pub mod driver;
pub mod highlight;
pub mod lsp;
pub mod stdlib;
pub mod tools;
