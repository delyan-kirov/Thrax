//! The Thrax compiler front end, as a pipeline of phases:
//!
//! 1. [`lexer`] turns source into tokens.
//! 2. [`parser`] builds the handle-based AST ([`parser::data`]).
//! 3. [`typing`] checks it and records what it resolved ([`typing::resolved`]).
//! 4. [`desugar`] turns the checked AST into the [`core`] lambda calculus.
//! 5. [`ir`] runs the [`passes`] over the Core and closure-converts it to the
//!    IR the `interpreter` and `ccg` crates consume.
//!
//! [`format`](mod@format) is the source formatter, which runs on tokens alone.
//!
//! Entry points: [`parse`] (source to AST), [`check`] (AST to inferred types),
//! [`desugar_program`] (AST to Core), and [`ir::lower_modules`] (Core to IR).

pub mod core;
pub mod desugar;
pub mod format;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod passes;
pub mod typing;

pub use desugar::desugar_program;
pub use lexer::data::{Kind, Token};
pub use lexer::Lexer;
pub use parser::data::*;
pub use parser::Parser;
pub use typing::types::{
    is_build_type, is_entry_type, Type, TypeNode, Types, BUILD, BUILD_SIG, ENTRY, ENTRY_SIG,
};
pub use typing::unify::Unifier;
pub use typing::Checker;
pub use typing::{collect_resolved, Declarations, ReflectInfo, Resolved};

use utilities::Result;

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
/// cross-module type imports rely on. Tokens carry only spans and names/strings
/// are interned into `ast`, so the result borrows nothing from `source`.
pub fn parse_into(ast: Ast, source: &str) -> Result<(Ast, Program)> {
    let (ast, program, errors) = parse_recovering(ast, source);
    match (program, errors.into_iter().next()) {
        (Ok(program), None) => Ok((ast, program)),
        (Err(d), _) | (Ok(_), Some(d)) => Err(d),
    }
}

/// [`parse_into`] that keeps going past a broken item: every item that parses is
/// in the program, and every error is returned in source order. The program is
/// `Err` only when the `$ @mod` header itself is unreadable.
pub fn parse_recovering(ast: Ast, source: &str) -> (Ast, Result<Program>, Vec<utilities::Diagnostic>) {
    let lex = Lexer::new(source);
    let mut parser = Parser::new(lex, ast);
    let program = parser.parse_program();
    let errors = parser.take_errors();
    (parser.into_ast(), program, errors)
}

/// Type-check a program, returning each global definition's generalized type.
pub fn check<'a>(ast: &'a Ast, program: &Program) -> Result<Vec<(&'a str, Type)>> {
    Checker::new(ast, std::rc::Rc::new(Types::new())).check_program(program)
}
