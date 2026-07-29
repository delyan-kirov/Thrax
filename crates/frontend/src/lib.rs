//! The Thrax compiler front end, in cpp-style phases: `lx` lexes, `ex` parses to
//! the handle-based AST, `tc` type-checks it, and `cr` lowers it to the Core.
//! Each phase splits logic from its data (`ex`/`ex_data`, `tc`/`tc_data`,
//! `cr`/`cr_data`), mirroring the C++ `X` / `XxDATA` layout.
//!
//! Entry points: [`parse`] (source to AST), [`check`] (AST to inferred types),
//! and [`cr::lower_program`] (AST to Core). The Core ([`cr_data`]) is what the
//! `interpreter` and, later, `cgen` crates consume.

pub mod cr;
pub mod cr_data;
pub mod ex;
pub mod ex_data;
mod ex_table;
pub mod lx;
pub mod lx_data;
pub mod tc;
pub mod tc_data;
pub mod tc_engine;

pub use cr::{lower_program, Decls, Resolved};
pub use ex::Parser;
pub use ex_data::*;
pub use lx::Lexer;
pub use lx_data::{Kind, Token};
pub use tc::Checker;
pub use tc_data::Type;
pub use tc_engine::Engine;

use utilities::{Arena, Result};

/// A parsed compilation unit: the [`Program`] root and the [`Ast`] stores that
/// back every handle in it. Reads go through `parsed.ast` (`ast.expr(id)`,
/// `ast.text(name)`, ...).
pub struct Parsed {
    pub ast: Ast,
    pub program: Program,
}

/// Parse `source` into a fresh-[`Ast`] [`Parsed`].
pub fn parse(source: &str) -> Result<Parsed> {
    let (ast, program) = parse_into(Ast::new(), source)?;
    Ok(Parsed { ast, program })
}

/// Parse `source`, appending its nodes to `ast` (moved in and returned). Several
/// modules parsed into one shared `Ast` can reference each other's handles, which
/// cross-module type imports rely on. The lexer decodes string escapes into a
/// scratch [`Arena`] that lives only for the parse; every name is interned into
/// `ast`, so the result owns all its data.
pub fn parse_into(ast: Ast, source: &str) -> Result<(Ast, Program)> {
    let scratch = Arena::new();
    let lex = Lexer::new(source, &scratch);
    let mut parser = Parser::new(lex, ast);
    let program = parser.parse_program()?;
    Ok((parser.into_ast(), program))
}

/// Type-check a program, returning each global definition's generalized type.
pub fn check<'a>(ast: &'a Ast, program: &Program) -> Result<Vec<(&'a str, Type)>> {
    Checker::new(ast).check_program(program)
}

#[cfg(test)]
#[path = "ex_tests.rs"]
mod ex_tests;
#[cfg(test)]
#[path = "tc_tests.rs"]
mod tc_tests;
