use crate::core::{CoreArm, Pat, Term, Terms};
use super::MatchCompiler;
use utilities::StrId;

/// Compiling a case binds the scrutinee once (an outer `let`), and the arm chain
/// lives in its body.
#[test]
fn case_binds_scrutinee_once() {
    let mut t = Terms::new();
    // is x | 1 => 10 | 2 => 20
    let scrut = t.var("x");
    let (ten, twenty) = (t.add(Term::Int(10)), t.add(Term::Int(20)));
    let case = t.add(Term::Case {
        scrut,
        arms: vec![
            CoreArm { pat: Pat::Int(1), guard: None, body: ten },
            CoreArm { pat: Pat::Int(2), guard: None, body: twenty },
        ],
        default: None,
    });
    let out = MatchCompiler::new().go(&mut t, case);
    let Term::Let { name, val, .. } = t.get(out) else {
        panic!("expected the scrutinee let, got {}", t.render(out));
    };
    assert_eq!(*name, "%ms0");
    assert!(matches!(t.get(*val), Term::Var { name, .. } if *name == "x"));
}

/// A nested variant pattern compiles to shallow cases only, each binding its
/// payload to fresh variables.
#[test]
fn a_nested_variant_compiles_to_shallow_cases() {
    let mut t = Terms::new();
    let id = StrId::intern;
    // is p | Cons.{a, Cons.{b, _}} => a + b
    let inner = Pat::Variant { tag: id("Cons"), fields: vec![Pat::Var(id("b")), Pat::Wild] };
    let outer = Pat::Variant { tag: id("Cons"), fields: vec![Pat::Var(id("a")), inner] };
    let scrut = t.var("p");
    let (a, b) = (t.var("a"), t.var("b"));
    let body = t.bin("+", a, b);
    let zero = t.add(Term::Int(0));
    let case = t.add(Term::Case {
        scrut,
        arms: vec![CoreArm { pat: outer, guard: None, body }],
        default: Some(zero),
    });
    let out = MatchCompiler::new().go(&mut t, case);
    let rendered = t.render(out);
    assert!(!rendered.contains("Wild"), "{rendered}");
    assert!(rendered.contains(r#"Variant { tag: "Cons", fields: [Var("%mv2"), Var("%mv3")] }"#), "{rendered}");
}
