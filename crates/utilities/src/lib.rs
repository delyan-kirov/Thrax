//! Reusable, dependency-free foundations shared across the compiler.
//!
//! * [`arena`] is the bump arena; [`handle`] is the handle-addressed store the
//!   AST uses to drop its source lifetime.
//! * [`error`] is the diagnostic model.
//! * [`scc`] is a generic strongly-connected-components pass (Tarjan).
//! * [`target`] is the compilation target as data (the single source of
//!   platform truth: word size, `@extern` library resolution, toolchain).

pub mod arena;
pub mod cabi;
pub mod error;
pub mod handle;
pub mod scc;
pub mod target;

pub use arena::Arena;
pub use cabi::{CField, CKind, CLayout, ExternArg};
pub use error::{Code, Diagnostic, Line, Result, Span};
pub use handle::{Aol, Interner, SecondaryMap, Slice, Store, StrId};
pub use target::{toolchain, Arch, Os, Target, Toolchain};

#[cfg(test)]
mod target_tests;
