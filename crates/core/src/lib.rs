//! The Core: desugaring the front-end AST into a small lambda calculus, and a
//! tree-walking interpreter over it.
//!
//! * [`term`] is the Core representation ([`term::Term`], [`term::Pat`]).
//! * [`lower`] desugars a [`syntax::Program`] into a [`term::Program`].
//! * [`eval`] evaluates lowered modules ([`eval::Interp`], [`eval::Value`]).

pub mod eval;
pub mod lower;
pub mod term;

pub use eval::{Interp, Value};
pub use lower::{lower_program, Decls, Resolved};
pub use term::{Program, Term};
