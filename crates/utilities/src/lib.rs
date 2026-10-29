//! Reusable, dependency-free foundations shared across the compiler.
//!
//! * [`handle`] is the handle-addressed store the AST uses to drop its source
//!   lifetime.
//! * [`error`] is the diagnostic model; [`json`] is the language server's wire
//!   format.
//! * [`config`] is `thrax.toml` and the table of options it may set.
//! * [`libc`] holds the C library's real prototypes, which `@extern` bindings
//!   type loosely.
//! * [`interrupt`] is the interrupt flag a signal handler sets and a running
//!   evaluation polls.
//! * [`progress`] prints what a slow command is doing to stderr, as it goes.
//! * [`tarjan`] computes strongly connected components (Tarjan).
//! * [`target`] is the compilation target as data (the single source of
//!   platform truth: word size, `@extern` library resolution, toolchain).

pub mod cabi;
pub mod config;
pub mod error;
pub mod handle;
pub mod interrupt;
pub mod json;
pub mod libc;
pub mod progress;
pub mod tarjan;
pub mod target;

pub use cabi::{CField, CKind, CLayout, ExternArg};
pub use error::{Applicability, Aside, Code, Diagnostic, Edit, Help, Label, Line, Result, Severity, Span};
pub use json::Json;
pub use handle::{Aol, Interner, SecondaryMap, Slice, Store, StrId};
pub use target::{musl_specs, musl_toolchain, toolchain, Arch, Libc, Os, Target, Toolchain};
