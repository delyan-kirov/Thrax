use std::sync::Arc;

use super::super::data::Term;
use super::{is_atom, Anf};

fn app(f: Term, x: Term) -> Term {
    Term::App(Arc::new(f), Arc::new(x))
}

fn is_var(t: &Term, expect: &str) -> bool {
    matches!(t, Term::Var { name, .. } if name == expect)
}

/// A nested application names its inner call: `f (g x)` => `let %a0 = g x in f %a0`.
#[test]
fn names_nested_application() {
    let anf = Anf::default();
    let out = anf.term(&app(Term::var("f"), app(Term::var("g"), Term::var("x"))));
    let Term::Let {
        name, val, body, ..
    } = out
    else {
        panic!("expected a let, got {out:?}");
    };
    assert_eq!(name, "%a0");
    let Term::App(g, x) = val.as_ref() else {
        panic!("value is not the inner application");
    };
    assert!(is_var(g, "g") && is_var(x, "x"));
    let Term::App(f, a) = body.as_ref() else {
        panic!("body is not the outer application");
    };
    assert!(is_var(f, "f") && is_var(a, "%a0"));
}

/// An application of two atoms is already in A-normal form.
#[test]
fn atoms_pass_through() {
    let anf = Anf::default();
    let out = anf.term(&app(Term::var("f"), Term::var("x")));
    let Term::App(f, x) = out else {
        panic!("expected an application, got {out:?}");
    };
    assert!(is_var(&f, "f") && is_var(&x, "x"));
}

/// Both operands of a call are named left-to-right: `f (g a) (h b)`.
#[test]
fn both_operands_named_in_order() {
    let anf = Anf::default();
    // ((f (g a)) (h b))
    let inner = app(Term::var("f"), app(Term::var("g"), Term::var("a")));
    let out = anf.term(&app(inner, app(Term::var("h"), Term::var("b"))));
    // Outermost binding is the first-normalized operand: `g a` -> %a0.
    let Term::Let { name, val, .. } = &out else {
        panic!("expected a let, got {out:?}");
    };
    assert_eq!(name, "%a0");
    assert!(matches!(val.as_ref(), Term::App(g, _) if is_var(g, "g")));
}

/// A field access on a computation names the record first.
#[test]
fn field_names_its_record() {
    let anf = Anf::default();
    let rec = app(Term::var("mk"), Term::var("z"));
    let out = anf.term(&Term::Field(Arc::new(rec), "x".into()));
    let Term::Let { name, body, .. } = out else {
        panic!("expected a let, got {out:?}");
    };
    assert_eq!(name, "%a0");
    let Term::Field(r, f) = body.as_ref() else {
        panic!("body is not a field access");
    };
    assert!(is_var(r, "%a0") && f == "x");
}

#[test]
fn atom_classification() {
    assert!(is_atom(&Term::Int(1)));
    assert!(is_atom(&Term::var("x")));
    assert!(is_atom(&Term::Lam {
        param: "y".into(),
        body: Arc::new(Term::var("y")),
    }));
    assert!(!is_atom(&app(Term::var("f"), Term::var("x"))));
}
