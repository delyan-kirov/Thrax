use super::*;
use crate::ir::data::{Alt, AltKind};

fn code(nparams: usize, body: Expr) -> Expr {
    let mut codes = [Code {
        nparams,
        nlocals: 8,
        body,
        name: "t".into(),
    }];
    annotate(&mut codes);
    let [c] = codes;
    c.body
}

fn app(fun: Atom, arg: Atom) -> Expr {
    Expr::App {
        fun,
        arg,
        tail: false,
    }
}

fn glob() -> Atom {
    Atom::Glob { name: "M.f".into() }
}

fn let_(slot: usize, rhs: Expr, body: Expr) -> Expr {
    Expr::Let {
        slot,
        rhs: Box::new(rhs),
        body: Box::new(body),
    }
}

#[test]
fn the_last_use_moves_and_earlier_ones_borrow() {
    let body = code(1, let_(1, app(glob(), Atom::Local(0)), app(Atom::Local(1), Atom::Local(0))));
    let Expr::Let { rhs, body, .. } = body else {
        panic!("{body:?}")
    };
    assert!(matches!(*rhs, Expr::App { arg: Atom::Local(0), .. }));
    assert!(matches!(
        *body,
        Expr::App {
            fun: Atom::Move(1),
            arg: Atom::Move(0),
            ..
        }
    ));
}

#[test]
fn an_unread_parameter_or_binder_is_dropped_where_it_is_bound() {
    let body = code(2, let_(2, app(glob(), Atom::Unit), Expr::Ret(Atom::LitI(0))));
    let Expr::Drop { slots, body } = body else {
        panic!("{body:?}")
    };
    assert_eq!(slots, [0, 1]);
    let Expr::Let { body, .. } = *body else {
        panic!()
    };
    assert!(matches!(*body, Expr::Drop { ref slots, .. } if slots == &[2]));
}

#[test]
fn a_value_one_branch_no_longer_needs_is_dropped_on_entry_to_it() {
    let case = Expr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Bool(true),
            binder_base: 2,
            binders: Vec::new(),
            body: Expr::Ret(Atom::Local(1)),
        }],
        default: Box::new(Expr::Ret(Atom::LitI(0))),
    };
    let Expr::Case {
        scrut,
        alts,
        default,
    } = code(2, case)
    else {
        panic!()
    };
    // Dead after the test in every branch, so the scrutinee itself moves.
    assert!(matches!(scrut, Atom::Move(0)));
    assert!(matches!(alts[0].body, Expr::Ret(Atom::Move(1))));
    assert!(matches!(*default, Expr::Drop { ref slots, .. } if slots == &[1]));
}

#[test]
fn a_binder_the_alternative_never_reads_is_not_bound() {
    let case = Expr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Con("Pair".into()),
            binder_base: 1,
            binders: vec![Some("a".into()), Some("b".into())],
            body: Expr::Ret(Atom::Local(2)),
        }],
        default: Box::new(Expr::Fault("no match".into())),
    };
    let Expr::Case { alts, default, .. } = code(1, case) else {
        panic!()
    };
    assert_eq!(alts[0].binders, [None, Some("b".into())]);
    assert!(matches!(alts[0].body, Expr::Ret(Atom::Move(2))));
    // Nothing is dropped ahead of a fault.
    assert!(matches!(*default, Expr::Fault(_)));
}

#[test]
fn two_uses_in_one_node_borrow_and_the_slot_is_dropped_after() {
    let tuple = Expr::MkTuple(vec![Atom::Local(0), Atom::Local(0)]);
    let body = code(1, let_(1, tuple, Expr::Ret(Atom::Local(1))));
    let Expr::Let { rhs, body, .. } = body else {
        panic!()
    };
    assert!(matches!(&*rhs, Expr::MkTuple(items)
        if matches!(items[..], [Atom::Local(0), Atom::Local(0)])));
    let Expr::Drop { slots, body } = *body else {
        panic!("{body:?}")
    };
    assert_eq!(slots, [0]);
    assert!(matches!(*body, Expr::Ret(Atom::Move(1))));
}

#[test]
fn a_recursive_binding_is_never_moved_or_dropped() {
    // `let go = \.. go .. in go 1`: the closure reaches the slot's box weakly.
    let rhs = Expr::Ret(Atom::Clos {
        code: 0,
        captures: vec![Atom::Local(0)],
    });
    let body = code(0, let_(0, rhs, app(Atom::Local(0), Atom::LitI(1))));
    let Expr::Let { rhs, body, .. } = body else {
        panic!()
    };
    assert!(matches!(&*rhs, Expr::Ret(Atom::Clos { captures, .. })
        if matches!(captures[..], [Atom::Local(0)])));
    assert!(matches!(*body, Expr::App { fun: Atom::Local(0), .. }));
}

#[test]
fn a_captured_field_moves_at_its_last_use() {
    let body = code(1, let_(1, app(Atom::Env(0), Atom::Unit), app(Atom::Env(0), Atom::Local(1))));
    let Expr::Drop { slots, body } = body else {
        panic!("{body:?}")
    };
    assert_eq!(slots, [0]);
    let Expr::Let { rhs, body, .. } = *body else {
        panic!()
    };
    assert!(matches!(*rhs, Expr::App { fun: Atom::Env(0), .. }));
    assert!(matches!(
        *body,
        Expr::App {
            fun: Atom::MoveEnv(0),
            arg: Atom::Move(1),
            ..
        }
    ));
}

#[test]
fn a_slot_live_across_a_handler_body_survives_it() {
    let handle = Expr::Handle {
        body: Box::new(app(glob(), Atom::Local(0))),
        clauses: Vec::new(),
        els: Atom::Clos {
            code: 0,
            captures: Vec::new(),
        },
    };
    let body = code(1, let_(1, handle, app(Atom::Local(0), Atom::Local(1))));
    let Expr::Let { rhs, body, .. } = body else {
        panic!()
    };
    let Expr::Handle { body: hbody, .. } = *rhs else {
        panic!()
    };
    assert!(matches!(*hbody, Expr::App { arg: Atom::Local(0), .. }));
    assert!(matches!(*body, Expr::App { fun: Atom::Move(0), .. }));
}
