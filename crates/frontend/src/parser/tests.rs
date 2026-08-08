use crate::*;
use utilities::Aol;

fn prog(src: &str) -> Parsed {
    parse(src).unwrap_or_else(|e| panic!("{}", e.render(src, "test.thx")))
}

#[test]
fn type_names_must_be_capitalized() {
    // A declared type name must be capitalized.
    let err = |src: &str| match parse(src) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected a parse error for {src:?}"),
    };
    assert!(err("@mod M\n$ point : @struct = a: Int,").contains("capital letter"));
    // A lowercase name in type-use position is a type variable and parses.
    assert!(parse("@mod M\n$ id : a -> a = \\x = x").is_ok());
    // A capitalized type also parses.
    assert!(parse("@mod M\n$ n : Int = 5").is_ok());
}

/// The single global's body handle, asserting the module has exactly one def.
fn only_def_body(p: &Parsed) -> Aol<Expr> {
    assert_eq!(p.program.items.len(), 1);
    match &p.program.items[0] {
        Item::Def { body, .. } => *body,
        other => panic!("expected a single def, got {other:?}"),
    }
}

#[test]
fn module_and_simple_def() {
    let p = prog("@mod M\n$ x = 1");
    assert_eq!(p.ast.text(p.program.module), "M");
    assert_eq!(p.program.items.len(), 1);
    match &p.program.items[0] {
        Item::Def {
            name, sig, body, ..
        } => {
            assert_eq!(p.ast.text(*name), "x");
            assert!(sig.is_none());
            assert!(matches!(p.ast.expr(*body), Expr::Int(1)));
        }
        other => panic!("expected a def, got {other:?}"),
    }
}

#[test]
fn precedence_and_associativity() {
    let p = prog("@mod M\n$ y = 1 + 2 * 3");
    // `1 + (2 * 3)`: top node is `+`, its rhs is `*`.
    let Expr::BinOp { op, lhs, rhs } = p.ast.expr(only_def_body(&p)) else {
        panic!("expected a binop")
    };
    assert_eq!(p.ast.text(*op), "+");
    assert!(matches!(p.ast.expr(*lhs), Expr::Int(1)));
    let Expr::BinOp { op: inner, .. } = p.ast.expr(*rhs) else {
        panic!("expected a nested binop")
    };
    assert_eq!(p.ast.text(*inner), "*");
}

#[test]
fn application_is_left_associative_and_tight() {
    let p = prog("@mod M\n$ z = f a b + 1");
    // `((f a) b) + 1`.
    let Expr::BinOp { op, lhs, .. } = p.ast.expr(only_def_body(&p)) else {
        panic!("expected `+`")
    };
    assert_eq!(p.ast.text(*op), "+");
    let Expr::App(inner, _) = p.ast.expr(*lhs) else {
        panic!("expected an application")
    };
    assert!(matches!(p.ast.expr(*inner), Expr::App(..)));
}

#[test]
fn lambda_if_and_comparison() {
    let p = prog("@mod M\n$ f = \\n = if n ?= 0 => 1 else n");
    let Expr::Lambda { body, .. } = p.ast.expr(only_def_body(&p)) else {
        panic!("expected a lambda")
    };
    assert!(matches!(p.ast.expr(*body), Expr::If { .. }));
}

#[test]
fn struct_decl_and_literal_and_field() {
    let src = "@mod M\n$ Person : @struct =\n name: Str,\n age: Int,\n\
                   $ p : Person = Person.{ .name = \"a\", .age = 1 }\n$ n = p.age";
    let p = prog(src);
    assert!(
        matches!(&p.program.items[0], Item::Struct { name, fields, .. } if p.ast.text(*name) == "Person" && fields.len() == 2)
    );
    match &p.program.items[1] {
        Item::Def { body, .. } => {
            let Expr::StructLit { ty, fields, .. } = p.ast.expr(*body) else {
                panic!("expected a struct literal")
            };
            assert_eq!(ty.map(|t| p.ast.text(t)), Some("Person"));
            assert_eq!(fields.len(), 2);
        }
        other => panic!("expected a def, got {other:?}"),
    }
    match &p.program.items[2] {
        Item::Def { body, .. } => {
            let Expr::Field { name, .. } = p.ast.expr(*body) else {
                panic!("expected a field access")
            };
            assert_eq!(p.ast.text(*name), "age");
        }
        other => panic!("expected a def, got {other:?}"),
    }
}

#[test]
fn variant_literal_and_when_match() {
    let src = "@mod M\n$ f = \\l = is l | List.Nil => 0 | List.Cons.{_, xs} => 1 else 2";
    let p = prog(src);
    let Expr::Lambda { body, .. } = p.ast.expr(only_def_body(&p)) else {
        panic!("expected a lambda")
    };
    let Expr::Match { arms, default, .. } = p.ast.expr(*body) else {
        panic!("expected a match")
    };
    assert!(default.is_some());
    assert_eq!(arms.len(), 2);
    let Pattern::Variant { ty, tag, .. } = p.ast.pat(arms[1].patterns[0]) else {
        panic!("expected a variant pattern")
    };
    assert_eq!(ty.map(|t| p.ast.text(t)), Some("List"));
    assert_eq!(p.ast.text(*tag), "Cons");
}

#[test]
fn cons_and_list_and_function_type() {
    let src = "@mod M\n$ g : List t -> Int = \\xs = 0\n$ xs = 1 :: [2, 3]";
    let p = prog(src);
    assert!(matches!(
        &p.program.items[0],
        Item::Def { sig: Some(sig), .. } if matches!(p.ast.ty(*sig), Ty::Arrow { .. })
    ));
    match &p.program.items[1] {
        Item::Def { body, .. } => {
            let Expr::BinOp { op, rhs, .. } = p.ast.expr(*body) else {
                panic!("expected a binop")
            };
            assert_eq!(p.ast.text(*op), "::");
            let Expr::List(elems) = p.ast.expr(*rhs) else {
                panic!("expected a list")
            };
            assert_eq!(elems.len(), 2);
        }
        other => panic!("expected a def, got {other:?}"),
    }
}

#[test]
fn union_effect_import_and_directives() {
    let src = "@mod M\n$ with List\n$ @private\n$ Color : @union = Red, Green, Blue\n\
                   $ State : @effect = get : Int, put : Int -> Int\n$ @assert 1";
    let p = prog(src);
    assert!(matches!(p.program.items[0], Item::Import { .. }));
    assert!(matches!(
        p.program.items[1],
        Item::Visibility(Visibility::Private)
    ));
    assert!(matches!(&p.program.items[2], Item::Union { variants, .. } if variants.len() == 3));
    assert!(matches!(&p.program.items[3], Item::Effect { ops, .. } if ops.len() == 2));
    assert!(matches!(p.program.items[4], Item::Assert(_)));
}

#[test]
fn pipes_and_sequencing() {
    let p = prog("@mod M\n$ r = a ; b |> f");
    // `;` is loosest and right-assoc: `a ; (b |> f)`.
    let Expr::BinOp { op, rhs, .. } = p.ast.expr(only_def_body(&p)) else {
        panic!("expected a binop")
    };
    assert_eq!(p.ast.text(*op), ";");
    let Expr::BinOp { op: inner, .. } = p.ast.expr(*rhs) else {
        panic!("expected a nested binop")
    };
    assert_eq!(p.ast.text(*inner), "|>");
}

#[test]
fn string_interpolation_desugars_to_concat() {
    // A plain string is a single `Str` node.
    let p = prog("@mod M\n$ s = \"hi\"");
    assert!(matches!(p.ast.expr(only_def_body(&p)), Expr::Str(_)));

    // An interpolated string becomes a `++` chain.
    let p = prog("@mod M\n$ s = \"a {x} b\"");
    let Expr::BinOp { op, .. } = p.ast.expr(only_def_body(&p)) else {
        panic!("expected a `++` chain");
    };
    assert_eq!(p.ast.text(*op), "++");

    // A sole interpolant is still `++` (seeded by a chunk), so it types as Str.
    let p = prog("@mod M\n$ s = \"{x}\"");
    assert!(matches!(p.ast.expr(only_def_body(&p)), Expr::BinOp { .. }));
}

#[test]
fn string_interpolation_nesting_and_escapes() {
    // A nested string literal inside an interpolant.
    assert!(parse("@mod M\n$ s = \"x {f \"y\"} z\"").is_ok());
    // Nested braces (a unit / record literal) inside an interpolant.
    assert!(parse("@mod M\n$ s = \"p {g {}} q\"").is_ok());
    // `\{` is a literal brace, not an interpolation.
    assert!(parse("@mod M\n$ s = \"lit \\{ ok\"").is_ok());
    // An unclosed interpolation is an error.
    assert!(parse("@mod M\n$ s = \"bad {1 + \"").is_err());
}
