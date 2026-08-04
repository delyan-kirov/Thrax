use std::sync::Arc;

use super::super::data::{Program, Term};
use super::{assign, assign_program};

fn idx_of(t: &Term) -> usize {
    match t {
        Term::Var { idx, .. } => *idx,
        _ => panic!("expected a Var, got {t:?}"),
    }
}

fn lam(param: &str, body: Term) -> Term {
    Term::Lam {
        param: param.into(),
        body: Arc::new(body),
    }
}

/// `\x = x`: the sole binder is innermost, index 1.
#[test]
fn innermost_binder_is_one() {
    let mut names = Vec::new();
    let out = assign(&lam("x", Term::var("x")), &mut names);
    let Term::Lam { body, .. } = out else {
        panic!()
    };
    assert_eq!(idx_of(&body), 1);
}

/// `\x = \y = x`: `x` is two binders out, index 2; `y` would be 1.
#[test]
fn outer_binder_counts_outward() {
    let mut names = Vec::new();
    let out = assign(&lam("x", lam("y", Term::var("x"))), &mut names);
    let Term::Lam { body, .. } = out else {
        panic!()
    };
    let Term::Lam { body, .. } = body.as_ref() else {
        panic!()
    };
    assert_eq!(idx_of(body), 2);

    let mut names = Vec::new();
    let out = assign(&lam("x", lam("y", Term::var("y"))), &mut names);
    let Term::Lam { body, .. } = out else {
        panic!()
    };
    let Term::Lam { body, .. } = body.as_ref() else {
        panic!()
    };
    assert_eq!(idx_of(body), 1);
}

/// A free/global name (not on the binder stack) resolves to index 0.
#[test]
fn free_name_is_global() {
    let mut names = Vec::new();
    let out = assign(&lam("x", Term::var("y")), &mut names);
    let Term::Lam { body, .. } = out else {
        panic!()
    };
    assert_eq!(idx_of(&body), 0);
}

/// A qualified reference is always a global, index 0, even if a same-named binder
/// is in scope.
#[test]
fn qualified_is_global() {
    let qualified = Term::Var {
        module: Some("M".into()),
        name: "x".into(),
        idx: 0,
    };
    let mut names = Vec::new();
    let out = assign(&lam("x", qualified), &mut names);
    let Term::Lam { body, .. } = out else {
        panic!()
    };
    assert_eq!(idx_of(&body), 0);
}

/// An inner binder shadows an outer one of the same name: the reference picks the
/// innermost, index 1.
#[test]
fn shadowing_picks_innermost() {
    let mut names = Vec::new();
    let out = assign(&lam("x", lam("x", Term::var("x"))), &mut names);
    let Term::Lam { body, .. } = out else {
        panic!()
    };
    let Term::Lam { body, .. } = body.as_ref() else {
        panic!()
    };
    assert_eq!(idx_of(body), 1);
}

/// A recursive `let` binds its name inside the value; a non-recursive one does
/// not, so the value's reference escapes to the outer scope.
#[test]
fn rec_let_binds_in_value() {
    // rec: `\outer = let f = f in f` -- the `f` in the value sees the let binder.
    let rec = lam(
        "outer",
        Term::Let {
            name: "f".into(),
            rec: true,
            val: Arc::new(Term::var("f")),
            body: Arc::new(Term::var("f")),
        },
    );
    let mut names = Vec::new();
    let Term::Lam { body, .. } = assign(&rec, &mut names) else {
        panic!()
    };
    let Term::Let { val, body, .. } = body.as_ref() else {
        panic!()
    };
    assert_eq!(idx_of(val), 1); // the let binder
    assert_eq!(idx_of(body), 1);

    // non-rec: `\f = let f = f in f` -- the value's `f` is the outer lambda param
    // (index 1: the let binder is not in scope yet), the body's `f` is the let (1).
    let nonrec = lam(
        "f",
        Term::Let {
            name: "f".into(),
            rec: false,
            val: Arc::new(Term::var("f")),
            body: Arc::new(Term::var("f")),
        },
    );
    let mut names = Vec::new();
    let Term::Lam { body, .. } = assign(&nonrec, &mut names) else {
        panic!()
    };
    let Term::Let { val, body, .. } = body.as_ref() else {
        panic!()
    };
    assert_eq!(idx_of(val), 1); // outer lambda `f`, no let binder in scope yet
    assert_eq!(idx_of(body), 1); // the let binder
}

/// A global's own name is not a binder: a self-reference is a global (index 0).
#[test]
fn global_self_reference_is_global() {
    let mut prog = Program {
        module: "M".into(),
        effects: Vec::new(),
        globals: vec![("f".into(), lam("x", Term::var("f")))],
    };
    assign_program(&mut prog);
    let Term::Lam { body, .. } = &prog.globals[0].1 else {
        panic!()
    };
    assert_eq!(idx_of(body), 0);
}
