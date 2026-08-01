//! The Thrax compiler front end, as a pipeline of phases: [`lexer`] lexes,
//! [`parser`] parses to the handle-based AST, [`typing`] type-checks it, and
//! [`lowering`] lowers it to the Core. Each phase keeps its data in a nested
//! module (`parser::data`, `typing::data`/`typing::engine`, `lowering::data`).
//!
//! Entry points: [`parse`] (source to AST), [`check`] (AST to inferred types),
//! and [`lowering::lower_program`] (AST to Core). The Core ([`lowering::data`])
//! is what the `interpreter` and, later, `cgen` crates consume.

pub mod ir;
pub mod lexer;
pub mod lowering;
pub mod parser;
pub mod typing;

pub use lexer::data::{Kind, Token};
pub use lexer::Lexer;
pub use lowering::{lower_program, Decls, Resolved};
pub use parser::data::*;
pub use parser::Parser;
pub use typing::data::Type;
pub use typing::engine::Engine;
pub use typing::Checker;

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
