//! Reusable, dependency-free foundations shared across the compiler.
//!
//! * [`arena`] is the bump arena; [`handle`] is the handle-addressed store the
//!   AST uses to drop its source lifetime.
//! * [`error`] is the diagnostic model; [`json`] is the language server's wire
//!   format.
//! * [`config`] is `thrax.toml` and the table of options it may set.
//! * [`libc`] holds the C library's real prototypes, which `@extern` bindings
//!   type loosely.
//! * [`interrupt`] is the interrupt flag a signal handler sets and a running
//!   evaluation polls.
//! * [`progress`] prints what a slow command is doing to stderr, as it goes.
//! * [`scc`] is a generic strongly-connected-components pass (Tarjan).
//! * [`target`] is the compilation target as data (the single source of
//!   platform truth: word size, `@extern` library resolution, toolchain).

pub mod arena;
pub mod cabi;
pub mod config;
pub mod error;
pub mod handle;
pub mod interrupt;
pub mod json;
pub mod libc;
pub mod progress;
pub mod scc;
pub mod target;

pub use arena::Arena;
pub use cabi::{CField, CKind, CLayout, ExternArg};
pub use error::{Applicability, Aside, Code, Diagnostic, Edit, Help, Label, Line, Result, Severity, Span};
pub use json::Json;
pub use handle::{Aol, Interner, SecondaryMap, Slice, Store, StrId};
pub use target::{musl_specs, musl_toolchain, toolchain, Arch, Libc, Os, Target, Toolchain};

#[cfg(test)]
mod libc_tests;
#[cfg(test)]
mod target_tests;
