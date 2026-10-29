use std::sync::Arc;

use super::*;

fn prog(globals: Vec<(&str, Term)>) -> CoreProgram {
    CoreProgram {
        module: "M".into(),
        effects: Vec::new(),
        globals: globals
            .into_iter()
            .map(|(n, t)| (n.to_string(), t))
            .collect(),
        crepr_layouts: Vec::new(),
        ct_runs: Vec::new(),
        ct_build: None,
        ct_evals: Vec::new(),
        ct_types: Vec::new(),
    }
}

/// `\a b = @iadd a b`, the shape every CORE arithmetic instance field has.
fn eta_add() -> Term {
    Term::Lam {
        param: "a".into(),
        body: Arc::new(Term::Lam {
            param: "b".into(),
            body: Arc::new(Term::app(
                Term::app(Term::var("@iadd"), Term::var("a")),
                Term::var("b"),
            )),
        }),
    }
}

fn instance(field: Term) -> Term {
    Term::Struct {
        name: "IAdd".into(),
        base: None,
        fields: Arc::from(vec![("add".to_string(), field)]),
    }
}

fn projection() -> Term {
    Term::Field(
        Arc::new(Term::Var {
            module: Some("CORE".into()),
            name: "impl_IAdd_for_int".into(),
            idx: 0,
        }),
        "add".into(),
    )
}

#[test]
fn a_saturated_projection_of_an_eta_primitive_folds() {
    let use_site = Term::app(Term::app(projection(), Term::Int(1)), Term::Int(2));
    let mut p = prog(vec![
        ("CORE.impl_IAdd_for_int", instance(eta_add())),
        ("M.sum", use_site),
    ]);
    fold_primitive_methods(&mut p);
    let folded = format!("{:?}", p.globals[1].1);
    assert!(folded.contains("@iadd"), "{folded}");
    assert!(!folded.contains("Field"), "{folded}");
}

/// An UNDER-applied projection keeps the field read: there is nothing to
/// eta-reduce, and the primitive is not a first-class value.
#[test]
fn an_under_applied_projection_is_left_alone() {
    let use_site = Term::app(projection(), Term::Int(1));
    let mut p = prog(vec![
        ("CORE.impl_IAdd_for_int", instance(eta_add())),
        ("M.add1", use_site),
    ]);
    fold_primitive_methods(&mut p);
    assert!(format!("{:?}", p.globals[1].1).contains("Field"));
}

/// A method that is not an eta-expanded primitive (it reorders its arguments)
/// must not be folded: the rewrite is only sound when the body applies the
/// primitive to its own parameters in order.
#[test]
fn a_method_that_reorders_its_arguments_is_left_alone() {
    let flipped = Term::Lam {
        param: "a".into(),
        body: Arc::new(Term::Lam {
            param: "b".into(),
            body: Arc::new(Term::app(
                Term::app(Term::var("@iadd"), Term::var("b")),
                Term::var("a"),
            )),
        }),
    };
    let use_site = Term::app(Term::app(projection(), Term::Int(1)), Term::Int(2));
    let mut p = prog(vec![
        ("CORE.impl_IAdd_for_int", instance(flipped)),
        ("M.sum", use_site),
    ]);
    fold_primitive_methods(&mut p);
    assert!(format!("{:?}", p.globals[1].1).contains("Field"));
}
