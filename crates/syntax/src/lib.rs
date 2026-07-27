//! Surface syntax for Thrax: the AST and a Pratt parser over the [`lexer`].
//!
//! Entry point: [`parse`], which lexes and parses a whole compilation unit into
//! a [`Program`] whose nodes are arena-allocated and borrow the source.

pub mod ast;
mod parser;
mod table;

use arena::Arena;
use diag::Result;
use lexer::Lexer;

pub use ast::*;
pub use parser::Parser;

/// Parse `source` into a [`Program`]. Nodes borrow `source` and `arena`.
pub fn parse<'a>(source: &'a str, arena: &'a Arena) -> Result<Program<'a>> {
    let lex = Lexer::new(source, arena);
    Parser::new(lex, arena).parse_program()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog<'a>(src: &'a str, arena: &'a Arena) -> Program<'a> {
        parse(src, arena).unwrap_or_else(|e| panic!("{}", e.render(src, "test.thx")))
    }

    #[test]
    fn module_and_simple_def() {
        let arena = Arena::new();
        let p = prog("@mod M\n$ x = 1", &arena);
        assert_eq!(p.module, "M");
        assert_eq!(p.items.len(), 1);
        match p.items[0] {
            Item::Def { name, sig, body } => {
                assert_eq!(name, "x");
                assert!(sig.is_none());
                assert!(matches!(body, Expr::Int(1)));
            }
            other => panic!("expected a def, got {other:?}"),
        }
    }

    #[test]
    fn precedence_and_associativity() {
        let arena = Arena::new();
        let p = prog("@mod M\n$ y = 1 + 2 * 3", &arena);
        // `1 + (2 * 3)`: top node is `+`, its rhs is `*`.
        match p.items[0] {
            Item::Def {
                body: Expr::BinOp { op: "+", lhs, rhs },
                ..
            } => {
                assert!(matches!(lhs, Expr::Int(1)));
                assert!(matches!(rhs, Expr::BinOp { op: "*", .. }));
            }
            other => panic!("expected `+` at the root, got {other:?}"),
        }
    }

    #[test]
    fn application_is_left_associative_and_tight() {
        let arena = Arena::new();
        let p = prog("@mod M\n$ z = f a b + 1", &arena);
        // `((f a) b) + 1`.
        match p.items[0] {
            Item::Def {
                body:
                    Expr::BinOp {
                        op: "+",
                        lhs: Expr::App(Expr::App(..), _),
                        ..
                    },
                ..
            } => {}
            other => panic!("unexpected shape: {other:?}"),
        }
    }

    #[test]
    fn lambda_if_and_comparison() {
        let arena = Arena::new();
        let p = prog("@mod M\n$ f = \\n = if n ?= 0 then 1 else n", &arena);
        assert!(matches!(
            p.items[0],
            Item::Def {
                body: Expr::Lambda {
                    body: Expr::If { .. },
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn struct_decl_and_literal_and_field() {
        let arena = Arena::new();
        let src = "@mod M\n$ Person : @struct =\n name: Str,\n age: Int,\n\
                   $ p : Person = Person.{ .name = \"a\", .age = 1 }\n$ n = p.age";
        let p = prog(src, &arena);
        assert!(matches!(p.items[0], Item::Struct { name: "Person", fields } if fields.len() == 2));
        assert!(matches!(
            p.items[1],
            Item::Def { body: Expr::StructLit { ty: Some("Person"), fields, .. }, .. } if fields.len() == 2
        ));
        assert!(matches!(
            p.items[2],
            Item::Def {
                body: Expr::Field { name: "age", .. },
                ..
            }
        ));
    }

    #[test]
    fn variant_literal_and_when_match() {
        let arena = Arena::new();
        let src =
            "@mod M\n$ f = \\l = when l is List.Nil then 0 is List.Cons.{_, xs} then 1 else 2";
        let p = prog(src, &arena);
        match p.items[0] {
            Item::Def {
                body:
                    Expr::Lambda {
                        body:
                            Expr::Match {
                                arms,
                                default: Some(_),
                                ..
                            },
                        ..
                    },
                ..
            } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(
                    arms[1].patterns[0],
                    Pattern::Variant {
                        ty: Some("List"),
                        tag: "Cons",
                        ..
                    }
                ));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn cons_and_list_and_function_type() {
        let arena = Arena::new();
        let src = "@mod M\n$ g : List `T -> Int = \\xs = 0\n$ xs = 1 :: [2, 3]";
        let p = prog(src, &arena);
        assert!(matches!(
            p.items[0],
            Item::Def {
                sig: Some(Ty::Arrow { .. }),
                ..
            }
        ));
        assert!(matches!(
            p.items[1],
            Item::Def { body: Expr::BinOp { op: "::", rhs: Expr::List(e), .. }, .. } if e.len() == 2
        ));
    }

    #[test]
    fn union_effect_import_and_directives() {
        let arena = Arena::new();
        let src = "@mod M\n$ with List\n$ @private\n$ Color : @union = Red, Green, Blue\n\
                   $ State : @effect = get : Int, put : Int -> Int\n$ @assert 1";
        let p = prog(src, &arena);
        assert!(matches!(p.items[0], Item::Import { .. }));
        assert!(matches!(p.items[1], Item::Visibility(Visibility::Private)));
        assert!(matches!(p.items[2], Item::Union { variants, .. } if variants.len() == 3));
        assert!(matches!(p.items[3], Item::Effect { ops, .. } if ops.len() == 2));
        assert!(matches!(p.items[4], Item::Assert(_)));
    }

    #[test]
    fn pipes_and_sequencing() {
        let arena = Arena::new();
        let p = prog("@mod M\n$ r = a ; b |> f", &arena);
        // `;` is loosest and right-assoc: `a ; (b |> f)`.
        assert!(matches!(
            p.items[0],
            Item::Def {
                body: Expr::BinOp {
                    op: ";",
                    rhs: Expr::BinOp { op: "|>", .. },
                    ..
                },
                ..
            }
        ));
    }
}
