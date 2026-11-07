use super::*;
use crate::ir::data::{Alt, AltKind, Code, Unpack};

/// A program whose global `M.f` is the lambda `f_body` (code 1), called by
/// `caller` (code 2, one parameter). Code 0 is `M.f`'s CAF.
fn program(f_body: IrExpr, caller: IrExpr) -> IrProgram {
    let code = |nparams: usize, body: IrExpr| Code {
        nparams,
        nlocals: 8,
        body,
        name: "t".into(),
        borrowed: vec![false; nparams],
    };
    let mut program = IrProgram {
        codes: vec![
            code(0, IrExpr::Ret(Atom::Clos { code: 1, captures: Vec::new() })),
            code(1, f_body),
            code(1, caller),
        ],
        globals: vec![("M.f".into(), 0)],
        ..IrProgram::default()
    };
    infer_borrowed_params(&mut program);
    program
}

fn f() -> Atom {
    Atom::Glob { name: "M.f".into() }
}

fn app(fun: Atom, arg: Atom) -> IrExpr {
    IrExpr::App {
        fun,
        arg,
        tail: false,
        lend: false,
    }
}

fn let_(slot: usize, rhs: IrExpr, body: IrExpr) -> IrExpr {
    IrExpr::Let {
        slot,
        rhs: Box::new(rhs),
        body: Box::new(body),
    }
}

/// `x |> \ | true => 1 | _ => 0`: reads its parameter only.
fn inspects() -> IrExpr {
    IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Bool(true),
            binder_base: 1,
            binders: Vec::new(),
            body: IrExpr::Ret(Atom::LitI(1)),
            unpack: Unpack::Borrow,
        }],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    }
}

/// `let r = M.f x in r`: a non-tail call.
fn calls_then_returns(arg: Atom) -> IrExpr {
    let_(1, app(f(), arg), IrExpr::Ret(Atom::Local(1)))
}

fn lends(e: &IrExpr) -> bool {
    e.any_node(&mut |e| matches!(e, IrExpr::App { lend: true, .. }))
}

#[test]
fn a_parameter_only_inspected_is_borrowed_and_a_waiting_caller_lends_to_it() {
    let p = program(inspects(), calls_then_returns(Atom::Local(0)));
    assert_eq!(p.codes[1].borrowed, [true]);
    assert!(lends(&p.codes[2].body));
}

#[test]
fn a_stored_or_returned_parameter_is_owned() {
    let stored = IrExpr::MkVariant {
        ty: "T".into(),
        tag: "A".into(),
        fields: vec![Atom::Local(0)],
        reuse: None,
    };
    for body in [stored, IrExpr::Ret(Atom::Local(0))] {
        let p = program(body, calls_then_returns(Atom::Local(0)));
        assert_eq!(p.codes[1].borrowed, [false]);
        assert!(!lends(&p.codes[2].body));
    }
}

#[test]
fn a_code_with_no_known_call_borrows_nothing() {
    let p = program(inspects(), IrExpr::Ret(Atom::LitI(0)));
    assert_eq!(p.codes[1].borrowed, [false]);
}

#[test]
fn passing_a_parameter_to_an_owned_one_owns_it() {
    let stores = IrExpr::MkTuple(vec![Atom::Local(0)]);
    let p = program(stores, calls_then_returns(Atom::Local(0)));
    assert_eq!(p.codes[1].borrowed, [false]);
    assert_eq!(p.codes[2].borrowed, [false]);
}

#[test]
fn a_builtin_that_may_update_in_place_owns_its_operand() {
    let len = app(Atom::Glob { name: "@vec_len".into() }, Atom::Local(0));
    assert_eq!(program(len, calls_then_returns(Atom::Local(0))).codes[1].borrowed, [true]);
    let push = let_(
        1,
        app(Atom::Glob { name: "@vec_push".into() }, Atom::Local(0)),
        app(Atom::Local(1), Atom::LitI(1)),
    );
    assert_eq!(program(push, calls_then_returns(Atom::Local(0))).codes[1].borrowed, [false]);
}

#[test]
fn a_handler_body_neither_borrows_nor_lends() {
    let handled = |body: IrExpr| IrExpr::Handle {
        body: Box::new(body),
        clauses: Vec::new(),
        els: Atom::Glob { name: "M.f".into() },
    };
    let p = program(handled(inspects()), calls_then_returns(Atom::Local(0)));
    assert_eq!(p.codes[1].borrowed, [false]);
    let p = program(inspects(), handled(calls_then_returns(Atom::Local(0))));
    assert_eq!(p.codes[1].borrowed, [true]);
    assert!(!lends(&p.codes[2].body));
}

#[test]
fn a_tail_call_lends_only_what_outlives_the_caller() {
    let again = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Bool(true),
            binder_base: 1,
            binders: Vec::new(),
            body: app(f(), Atom::Local(0)),
            unpack: Unpack::Borrow,
        }],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let p = program(again, calls_then_returns(Atom::Local(0)));
    assert_eq!(p.codes[1].borrowed, [true]);
    assert!(lends(&p.codes[1].body), "a parameter the caller borrowed");
    let fresh = let_(1, IrExpr::MkTuple(Vec::new()), app(f(), Atom::Local(1)));
    let p = program(inspects(), fresh);
    assert!(!lends(&p.codes[2].body), "a value the caller's activation owns");
}

#[test]
fn a_part_used_as_owned_owns_the_whole() {
    let part_returned = IrExpr::Case {
        scrut: Atom::Local(0),
        alts: vec![Alt {
            kind: AltKind::Con("Some".into()),
            binder_base: 1,
            binders: vec![Some("x".into())],
            body: IrExpr::Ret(Atom::Local(1)),
            unpack: Unpack::Borrow,
        }],
        default: Box::new(IrExpr::Ret(Atom::LitI(0))),
    };
    let p = program(part_returned, calls_then_returns(Atom::Local(0)));
    assert_eq!(p.codes[1].borrowed, [false]);
}
