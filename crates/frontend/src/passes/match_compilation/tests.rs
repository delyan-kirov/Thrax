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

/// An `if` fails at one point only, so its else branch is spliced into the
/// case's default instead of being wrapped in a fallthrough thunk.
#[test]
fn a_sole_failure_point_needs_no_thunk() {
    let mut t = Terms::new();
    // if c => 1 else 2
    let scrut = t.var("c");
    let (one, two) = (t.add(Term::Int(1)), t.add(Term::Int(2)));
    let case = t.add(Term::Case {
        scrut,
        arms: vec![CoreArm { pat: Pat::Bool(true), guard: None, body: one }],
        default: Some(two),
    });
    let out = MatchCompiler::new().go(&mut t, case);
    let rendered = t.render(out);
    assert!(!rendered.contains("Lam"), "{rendered}");
    assert!(rendered.contains("(Int 2)"), "{rendered}");
}

/// A guard fails inside its pattern's binders, so splicing the rest there would
/// let the binder `x` capture the rest's own `x`: the thunk stays.
#[test]
fn a_failure_under_a_capturing_binder_keeps_its_thunk() {
    let mut t = Terms::new();
    let id = StrId::intern;
    // is v | x if g => x | _ => x
    let scrut = t.var("v");
    let (g, x_body, x_rest) = (t.var("g"), t.var("x"), t.var("x"));
    let case = t.add(Term::Case {
        scrut,
        arms: vec![
            CoreArm { pat: Pat::Var(id("x")), guard: Some(g), body: x_body },
            CoreArm { pat: Pat::Wild, guard: None, body: x_rest },
        ],
        default: None,
    });
    let out = MatchCompiler::new().go(&mut t, case);
    let rendered = t.render(out);
    assert!(rendered.contains("Lam"), "{rendered}");
}
