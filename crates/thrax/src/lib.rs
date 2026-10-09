//! The Thrax compiler as a library, and the entry point of the workspace
//! documentation. The compiler is five crates, in pipeline order:
//!
//! * [`utilities`] holds the foundations every phase shares: the bump arena, the
//!   handle-addressed store, the diagnostic model, and the compilation target as
//!   data.
//! * [`frontend`] is the front end and middle end: [`frontend::lexer`],
//!   [`frontend::parser`], [`frontend::typing`], [`frontend::lowering`] (to the
//!   Core), and [`frontend::ir`] (pattern-match compilation, A-normalization,
//!   De-Bruijn indexing, closure conversion).
//! * [`interpreter`] evaluates that IR with the reified-K (CEK) machine.
//! * [`ccg`] emits a standalone C program from the same IR.
//! * `thrax` (this crate) ties them together: [`driver`] wires the phases for
//!   `lex`/`parse`/`check`/`run`/`build`/`emit-c`, [`stdlib`] is the standard
//!   library the compiler carries, and [`capi`] exposes the compiler to C. The
//!   crate builds as an rlib, a static library (`libthrax.a`) and a shared one
//!   (`libthrax.so`), so a native Thrax program can call back into the compiler
//!   for `@lex`, `@parse_str` and `@eval` at run time. The `thrax` executable
//!   (the CLI and the GHCi-style shell) is a thin binary over this library.

pub mod capi;
pub mod driver;
pub mod stdlib;
