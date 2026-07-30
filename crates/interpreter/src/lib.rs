//! A tree-walking interpreter over the Core produced by `frontend::lowering`. It
//! is one of the backends that consume the Core; the C generator (`cgen`) is
//! another. [`eval`] is the evaluator logic, [`eval::data`] its runtime value
//! representation.

pub mod eval;

pub use eval::data::{Interp, Value};
