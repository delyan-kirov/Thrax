use super::*;
use crate::ir::data::{Alt, AltKind, Unpack};

fn code(nparams: usize, body: IrExpr) -> IrExpr {
    let mut codes = [Code {
        nparams,
        nlocals: 8,
        body,
        name: "t".into(),
    }];
    insert_moves_and_drops(&mut codes);
    let [c] = codes;
    c.body
}

fn app(fun: Atom, arg: Atom) -> IrExpr {
    IrExpr::App {
        fun,
        arg,
        tail: false,
    }
}

fn glob() -> Atom {
    Atom::Glob { name: "M.f".into() }
}

fn let_(slot: usize, rhs: IrExpr, body: IrExpr) -> IrExpr {
    IrExpr::Let {
        slot,
        rhs: Box::new(rhs),
        body: Box::new(body),
    }
}

#[test]
fn the_last_use_moves_and_earlier_ones_borrow() {
    let body = code(1, let_(1, app(glob(), Atom::Local(0)), app(Atom::Local(1), Atom::Local(0))));
    let IrExpr::Let { rhs, body, .. } = body else {
        panic!("{body:?}")
    };
    assert!(matches!(*rhs, IrExpr::App { arg: Atom::Local(0), .. }));
    assert!(matches!(
        *body,
        IrExpr::App {
            fun: Atom::Move(1),
            arg: Atom::Move(0),
            ..
        }
    ));
}

#[test]
fn an_unread_parameter_or_binder_is_dropped_where_it_is_bound() {
    let body = code(2, let_(2, app(glob(), Atom::Unit), IrExpr::Ret(Atom::LitI(0))));
    let IrExpr::Drop { slots, body } = body else {
        panic!("{body:?}")
    };
    assert_eq!(slots, [0, 1]);
    let IrExpr::Let { body, .. } = *body else {
        panic!()
    };
    assert!(matches!(*body, IrExpr::Drop { ref slots, .. } if slots == &[2]));
}

#[test]
fn a_value_one_branch_no_longer_needs_is_dropped_on_entry_to_it() {
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Bool(true),
            binder_base: 2,
            binders: Vec::new(),
            body: IrExpr::Ret(Atom::Local(1)),
            unpack: Unpack::Borrow,
        }],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let IrExpr::Case {
        scrut,
        alts,
        default,
    } = code(2, case)
    else {
        panic!()
    };
    // Dead after the test in every branch, so the scrutinee itself moves.
    assert!(matches!(scrut, Atom::Move(0)));
    assert!(matches!(alts[0].body, IrExpr::Ret(Atom::Move(1))));
    assert!(matches!(*default, IrExpr::Drop { ref slots, .. } if slots == &[1]));
}

#[test]
fn a_binder_the_alternative_never_reads_is_not_bound() {
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Con("Pair".into()),
            binder_base: 1,
            binders: vec![Some("a".into()), Some("b".into())],
            body: IrExpr::Ret(Atom::Local(2)),
            unpack: Unpack::Borrow,
        }],
        default: Box::new(IrExpr::Fault("no match".into())),
    };
    let IrExpr::Case { alts, default, .. } = code(1, case) else {
        panic!()
    };
    assert_eq!(alts[0].binders, [None, Some("b".into())]);
    assert!(matches!(alts[0].body, IrExpr::Ret(Atom::Move(2))));
    // Nothing is dropped ahead of a fault.
    assert!(matches!(*default, IrExpr::Fault(_)));
}

#[test]
fn two_uses_in_one_node_borrow_and_the_slot_is_dropped_after() {
    let tuple = IrExpr::MkTuple(vec![Atom::Local(0), Atom::Local(0)]);
    let body = code(1, let_(1, tuple, IrExpr::Ret(Atom::Local(1))));
    let IrExpr::Let { rhs, body, .. } = body else {
        panic!()
    };
    assert!(matches!(&*rhs, IrExpr::MkTuple(items)
        if matches!(items[..], [Atom::Local(0), Atom::Local(0)])));
    let IrExpr::Drop { slots, body } = *body else {
        panic!("{body:?}")
    };
    assert_eq!(slots, [0]);
    assert!(matches!(*body, IrExpr::Ret(Atom::Move(1))));
}

#[test]
fn a_recursive_binding_is_never_moved_or_dropped() {
    // `let go = \.. go .. in go 1`: the closure reaches the slot's box weakly.
    let rhs = IrExpr::Ret(Atom::Clos {
        code: 0,
        captures: vec![Atom::Local(0)],
    });
    let body = code(0, let_(0, rhs, app(Atom::Local(0), Atom::LitI(1))));
    let IrExpr::Let { rhs, body, .. } = body else {
        panic!()
    };
    assert!(matches!(&*rhs, IrExpr::Ret(Atom::Clos { captures, .. })
        if matches!(captures[..], [Atom::Local(0)])));
    assert!(matches!(*body, IrExpr::App { fun: Atom::Local(0), .. }));
}

#[test]
fn a_captured_field_moves_at_its_last_use() {
    let body = code(1, let_(1, app(Atom::Env(0), Atom::Unit), app(Atom::Env(0), Atom::Local(1))));
    let IrExpr::Drop { slots, body } = body else {
        panic!("{body:?}")
    };
    assert_eq!(slots, [0]);
    let IrExpr::Let { rhs, body, .. } = *body else {
        panic!()
    };
    assert!(matches!(*rhs, IrExpr::App { fun: Atom::Env(0), .. }));
    assert!(matches!(
        *body,
        IrExpr::App {
            fun: Atom::MoveEnv(0),
            arg: Atom::Move(1),
            ..
        }
    ));
}

#[test]
fn a_slot_a_handler_body_reads_is_never_moved() {
    let handle = IrExpr::Handle {
        body: Box::new(app(glob(), Atom::Local(0))),
        clauses: Vec::new(),
        els: Atom::Clos {
            code: 0,
            captures: Vec::new(),
        },
    };
    let body = code(1, let_(1, handle, app(Atom::Local(0), Atom::Local(1))));
    let IrExpr::Let { rhs, body, .. } = body else {
        panic!()
    };
    let IrExpr::Handle { body: hbody, .. } = *rhs else {
        panic!()
    };
    assert!(matches!(*hbody, IrExpr::App { arg: Atom::Local(0), .. }));
    assert!(matches!(*body, IrExpr::App { fun: Atom::Local(0), .. }));
}

fn cons_alt(binder_base: usize, body: IrExpr) -> Alt {
    Alt {
        kind: AltKind::Con("Cons".into()),
        binder_base,
        binders: vec![Some("h".into()), Some("t".into())],
        body,
        unpack: Unpack::Borrow,
    }
}

fn cons(fields: Vec<Atom>) -> IrExpr {
    IrExpr::MkVariant {
        ty: "List".into(),
        tag: "Cons".into(),
        fields,
        reuse: None,
    }
}

#[test]
fn a_dying_scrutinee_hands_its_cell_to_a_constructor_of_the_same_arity() {
    // `\xs = is xs | Cons h t => Cons t h | _ => xs'`, with xs dead after the test.
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![cons_alt(1, cons(vec![Atom::Local(2), Atom::Local(1)]))],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let mut codes = [Code {
        nparams: 1,
        nlocals: 3,
        body: case,
        name: "t".into(),
    }];
    insert_moves_and_drops(&mut codes);
    let [c] = codes;
    assert_eq!(c.nlocals, 4, "one token slot");
    let IrExpr::Case { scrut, alts, default } = c.body else {
        panic!()
    };
    // The slot holds the scrutinee until the alternative consumes it.
    assert!(matches!(scrut, Atom::Local(0)));
    assert_eq!(
        alts[0].unpack,
        Unpack::Consume {
            slot: 0,
            token: Some(3)
        }
    );
    assert!(matches!(alts[0].body, IrExpr::MkVariant { reuse: Some(3), .. }));
    assert!(matches!(*default, IrExpr::Drop { ref slots, .. } if slots == &[0]));
}

#[test]
fn a_branch_that_builds_nothing_drops_the_token() {
    let inner = IrExpr::Case {
        scrut: Atom::Local(1),
        alts: vec![Alt {
            kind: AltKind::Int(0),
            binder_base: 3,
            binders: Vec::new(),
            body: cons(vec![Atom::Local(1), Atom::Local(2)]),
            unpack: Unpack::Borrow,
        }],
        default: Box::new(IrExpr::Ret(Atom::Local(2))),
    };
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![cons_alt(1, inner)],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let IrExpr::Case { alts, .. } = code(1, case) else {
        panic!()
    };
    let Unpack::Consume { token: Some(t), .. } = alts[0].unpack else {
        panic!("{:?}", alts[0].unpack)
    };
    let IrExpr::Case { alts: inner, default, .. } = &alts[0].body else {
        panic!()
    };
    assert!(matches!(inner[0].body, IrExpr::MkVariant { reuse: Some(r), .. } if r == t));
    assert!(matches!(**default, IrExpr::Drop { ref slots, .. } if slots.contains(&t)));
}

#[test]
fn a_constructor_of_another_arity_takes_no_token() {
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![cons_alt(1, cons(vec![Atom::Local(1)]))],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let IrExpr::Case { alts, .. } = code(1, case) else {
        panic!()
    };
    assert_eq!(alts[0].unpack, Unpack::Consume { slot: 0, token: None });
    assert!(matches!(alts[0].body, IrExpr::MkVariant { reuse: None, .. }));
}

#[test]
fn a_live_scrutinee_is_borrowed() {
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![cons_alt(1, cons(vec![Atom::Local(1), Atom::Local(0)]))],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let IrExpr::Case { alts, .. } = code(1, case) else {
        panic!()
    };
    assert_eq!(alts[0].unpack, Unpack::Borrow);
}

#[test]
fn a_binder_a_handler_body_reads_is_not_dropped_where_it_is_bound() {
    // `is xs | Nil => {} | Cons h t => do f h ...`: `h` is pinned for the
    // handler body, which also makes it look live after the `Nil` branch.
    let handle = IrExpr::Handle {
        body: Box::new(app(glob(), Atom::Local(1))),
        clauses: Vec::new(),
        els: Atom::Clos {
            code: 0,
            captures: Vec::new(),
        },
    };
    let case = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![
            Alt {
                kind: AltKind::Con("Nil".into()),
                binder_base: 1,
                binders: Vec::new(),
                body: IrExpr::Ret(Atom::Unit),
                unpack: Unpack::Borrow,
            },
            cons_alt(1, handle),
        ],
        default: Box::new(IrExpr::Fault("no match".into())),
    };
    let IrExpr::Case { alts, .. } = code(1, case) else {
        panic!()
    };
    assert!(
        !matches!(&alts[1].body, IrExpr::Drop { slots, .. } if slots.contains(&1)),
        "{:?}",
        alts[1].body
    );
}
