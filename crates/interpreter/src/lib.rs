//! A tree-walking interpreter over the Core produced by `frontend::cr`. It is one
//! of the backends that consume the Core; the C generator (`cgen`) is another.
//! `it` is the evaluator logic, `it_data` its runtime value representation.

pub mod it;
pub mod it_data;

pub use it_data::{Interp, Value};
