//! Reusable, dependency-free foundations shared across the compiler.
//!
//! * [`ar`] is the bump arena; [`handle`] is the handle-addressed store the AST
//!   uses to drop its source lifetime.
//! * [`er`] is the diagnostic model (a Rust port of the C++ `ER` module).
//! * [`scc`] is a generic strongly-connected-components pass (Tarjan).

pub mod ar;
pub mod er;
pub mod handle;
pub mod scc;

pub use ar::Arena;
pub use er::{Code, Diagnostic, Line, Result, Span};
pub use handle::{Aol, Interner, SecondaryMap, Slice, Store, StrId};
