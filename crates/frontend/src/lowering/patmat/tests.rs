use std::sync::Arc;

use super::super::data::{Arm, Pat, Term};
use super::Pm;

/// Compiling a case binds the scrutinee once (an outer `let`), and the arm chain
/// lives in its body.
#[test]
fn case_binds_scrutinee_once() {
    let mut pm = Pm { n: 0 };
    // is x | 1 => 10 | 2 => 20
    let case = Term::Case {
        scrut: Arc::new(Term::var("x")),
        arms: vec![
            Arm {
                pat: Pat::Int(1),
                guard: None,
                body: Arc::new(Term::Int(10)),
            },
            Arm {
                pat: Pat::Int(2),
                guard: None,
                body: Arc::new(Term::Int(20)),
            },
        ]
        .into(),
        default: None,
    };
    let out = pm.go(&case);
    let Term::Let { val, .. } = &out else {
        panic!("expected the scrutinee let, got {out:?}");
    };
    assert!(matches!(val.as_ref(), Term::Var { name, .. } if name == "x"));
}

/// A shallow variant pattern's payload becomes fresh `Var` binders, so the arm is
/// directly a flat-alt-shaped case.
#[test]
fn nested_variant_compiles_without_panic() {
    let mut pm = Pm { n: 0 };
    // is p | Cons.{a, Cons.{b, _}} => a + b
    let inner = Pat::Variant {
        tag: "Cons".into(),
        fields: vec![Pat::Var("b".into()), Pat::Wild],
    };
    let outer = Pat::Variant {
        tag: "Cons".into(),
        fields: vec![Pat::Var("a".into()), inner],
    };
    let case = Term::Case {
        scrut: Arc::new(Term::var("p")),
        arms: vec![Arm {
            pat: outer,
            guard: None,
            body: Arc::new(Term::app(
                Term::app(Term::var("+"), Term::var("a")),
                Term::var("b"),
            )),
        }]
        .into(),
        default: Some(Arc::new(Term::Int(0))),
    };
    // Just ensure it produces a term (no panic, terminates).
    let _ = pm.go(&case);
}
