//! An evaluator for the closure-converted IR ([`frontend::ir`]): the reified-K
//! (CEK) abstract machine, a port of the C++ `IT`/`THxK` runtime. The
//! continuation is an explicit heap stack, so tail calls run in constant stack,
//! deep non-tail recursion grows the heap, and algebraic-effect handlers can
//! capture and splice the delimited continuation. It is one of the backends that
//! consume the IR; the C generator (`ccg`) is another.

pub mod machine;

pub use machine::{eval, Machine};
