use std::sync::Arc;

use super::super::data::{Atom, Expr};
use super::lower;
use crate::lowering::data::{Program, Term};
use crate::lowering::debruijn::assign_program;

fn caf(name: &str, body: Term) -> Program {
    let mut p = Program {
        module: "M".into(),
        effects: Vec::new(),
        globals: vec![(name.into(), body)],
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
