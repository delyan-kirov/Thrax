//! The middle-end passes over the Core, in the order [`crate::ir::lower_modules`]
//! runs them:
//!
//! 1. [`eta_fold`]: a call through an eta-expanded primitive method becomes the
//!    primitive itself.
//! 2. [`match_compilation`]: nested patterns become shallow, one-constructor
//!    tests.
//! 3. [`a_normal_form`]: every intermediate result gets a name.
//! 4. [`de_bruijn`]: every variable gets its binder's De Bruijn index.
//!
//! Each pass rewrites a whole merged [`crate::core::CoreProgram`] in place.

pub mod a_normal_form;
pub mod de_bruijn;
pub mod eta_fold;
pub mod match_compilation;
