//! Reusable, dependency-free foundations shared across the compiler.
//!
//! * [`arena`] is the bump arena; [`handle`] is the handle-addressed store the
//!   AST uses to drop its source lifetime.
//! * [`error`] is the diagnostic model.
//! * [`scc`] is a generic strongly-connected-components pass (Tarjan).

pub mod arena;
pub mod error;
pub mod handle;
pub mod scc;

pub use arena::Arena;
pub use error::{Code, Diagnostic, Line, Result, Span};
pub use handle::{Aol, Interner, SecondaryMap, Slice, Store, StrId};
