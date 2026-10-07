use std::sync::Arc;

use super::super::data::{Alt, AltKind, Atom, Expr, ResumeUse};
use super::{lower, resume_use};
use crate::lowering::data::{Program, Term};
use crate::lowering::debruijn::assign_program;

fn caf(name: &str, body: Term) -> Program {
    let mut p = Program {
        module: "M".into(),
        effects: Vec::new(),
        globals: vec![(name.into(), body)],
        crepr_layouts: Vec::new(),
        ct_runs: Vec::new(),
        ct_build: None,
        ct_evals: Vec::new(),
        ct_types: Vec::new(),
    };
    assign_program(&mut p);
    p
}

/// `$ id = \x = x` lifts the lambda to its own code; the global is a CAF whose
/// body returns a closure to it; the lifted code returns its `Local 0` parameter.
#[test]
fn identity_lifts_a_code() {
    let core = caf(
        "id",
        Term::Lam {
            param: "x".into(),
            body: Arc::new(Term::var("x")),
        },
    );
    let ir = lower(&core);
    assert_eq!(ir.codes.len(), 2);
    let (_, caf_idx) = ir.globals[0];
    let Expr::Ret(Atom::Clos { code, captures }) = &ir.codes[caf_idx].body else {
        panic!("CAF body is not a closure return: {:?}", ir.codes[caf_idx].body);
    };
    assert!(captures.is_empty());
    let lam = &ir.codes[*code];
    assert_eq!(lam.nparams, 1);
    assert!(matches!(lam.body, Expr::Ret(Atom::Local(0))));
}

/// `\x = \y = x` captures `x` into the inner closure's environment: the inner
/// code reads `Env 0`, and the outer closure's capture list passes its `Local 0`.
#[test]
fn nested_lambda_captures_free_var() {
    let core = caf(
        "k",
        Term::Lam {
            param: "x".into(),
            body: Arc::new(Term::Lam {
                param: "y".into(),
                body: Arc::new(Term::var("x")),
            }),
        },
    );
    let ir = lower(&core);
    // Find the inner code: the one whose body reads an Env.
    let inner = ir
        .codes
        .iter()
        .find(|c| matches!(c.body, Expr::Ret(Atom::Env(0))))
        .expect("inner code should read Env 0");
    assert_eq!(inner.nparams, 1);

    // The outer lambda builds the inner closure capturing its own Local 0 (x).
    let outer = ir
        .codes
        .iter()
        .find(|c| matches!(&c.body, Expr::Ret(Atom::Clos { .. })))
        .expect("outer code should return a closure");
    let Expr::Ret(Atom::Clos { captures, .. }) = &outer.body else {
        unreachable!()
    };
    assert_eq!(captures.len(), 1);
    assert!(matches!(captures[0], Atom::Local(0)));
}

// -- clause continuation classification (ResumeUse) -------------------------

/// `k` is `Local(1)` of a clause's code, so these are clause bodies.
fn app_k() -> Expr {
    Expr::App {
        fun: Atom::Local(1),
        arg: Atom::Unit,
        tail: false,
    }
}

#[test]
fn a_clause_that_ignores_its_continuation_needs_no_capture() {
    assert_eq!(resume_use(&Expr::Ret(Atom::LitI(0)), false), ResumeUse::Never);
    // The operation argument is Local 0, and a body binder is 2 or above.
    assert_eq!(resume_use(&Expr::Ret(Atom::Local(0)), false), ResumeUse::Never);
}

#[test]
fn one_application_of_the_continuation_still_moves() {
    assert_eq!(resume_use(&app_k(), false), ResumeUse::Once);
    // Exclusive alternatives: the worst branch, not the sum.
    let case = Expr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Bool(true),
            binder_base: 2,
            binders: Vec::new(),
            body: app_k(),
        }],
        default: Box::new(app_k()),
    };
    assert_eq!(resume_use(&case, false), ResumeUse::Once);
}

#[test]
fn two_applications_or_an_escape_mean_a_copy_per_resume() {
    let twice = Expr::Let {
        slot: 2,
        rhs: Box::new(app_k()),
        body: Box::new(app_k()),
    };
    assert_eq!(resume_use(&twice, false), ResumeUse::Many);

    // Stored in a constructor, captured by a closure, or passed as an argument:
    // the uses happen where the walk cannot see them.
    let stored = Expr::MkVariant {
        ty: "Task".into(),
        tag: "Susp".into(),
        fields: vec![Atom::Local(1)],
    };
    let captured = Expr::Ret(Atom::Clos {
        code: 0,
        captures: vec![Atom::Local(1)],
    });
    let passed = Expr::App {
        fun: Atom::Glob { name: "M.go".into() },
        arg: Atom::Local(1),
        tail: false,
    };
    for e in [&stored, &captured, &passed] {
        assert_eq!(resume_use(e, false), ResumeUse::Many);
    }
}

#[test]
fn oneshot_covers_an_escape_but_not_a_visible_double_resume() {
    let stored = Expr::MkVariant {
        ty: "Task".into(),
        tag: "Susp".into(),
        fields: vec![Atom::Local(1)],
    };
    assert_eq!(resume_use(&stored, true), ResumeUse::Once);

    let twice = Expr::Let {
        slot: 2,
        rhs: Box::new(app_k()),
        body: Box::new(app_k()),
    };
    assert_eq!(resume_use(&twice, true), ResumeUse::Many);
}
