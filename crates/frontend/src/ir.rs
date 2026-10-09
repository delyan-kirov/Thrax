//! The intermediate representation and closure conversion (the port of the C++
//! `IR` namespace). [`data`] is the IR itself (closure-converted, A-normal,
//! shallow-pattern Core); [`lower::lower`] converts a lowered + pattern-compiled
//! + A-normalized + De-Bruijn-indexed Core [`Program`](crate::lowering::data::Program)
//! into it, and [`ownership`] then marks where each local's reference ends. The
//! `interpreter`'s abstract machine and the `ccg` backend both consume this.

pub mod data;
pub mod lower;
pub mod ownership;

pub use data::Program;
pub use lower::{lower, lower_modules};
