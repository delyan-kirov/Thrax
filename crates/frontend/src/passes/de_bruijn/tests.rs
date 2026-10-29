use crate::core::{CoreProgram, Term, Terms};
use super::assign_de_bruijn_indices;
use utilities::{Aol, StrId};

fn id(s: &str) -> StrId {
    StrId::intern(s)
}

/// Index the single global `build` makes, and return each variable occurrence's
/// name and index in evaluation order.
fn indices(build: impl FnOnce(&mut Terms) -> Aol<Term>) -> Vec<(String, usize)> {
    let mut p = CoreProgram::new(id("M"));
    let term = build(&mut p.terms);
    p.globals.push((id("f"), term));
    assign_de_bruijn_indices(&mut p);
    let mut out = Vec::new();
    collect(&p, p.globals[0].1, &mut out);
    out
}

fn collect(p: &CoreProgram, t: Aol<Term>, out: &mut Vec<(String, usize)>) {
    if let Term::Var { name, .. } = p.terms.get(t) {
        out.push((name.to_string(), p.de_bruijn.get(t).copied().expect("every occurrence is indexed")));
    }
    p.terms.for_each_child(t, &mut |c, _| collect(p, c, out));
}

fn lam(t: &mut Terms, param: &str, body: Aol<Term>) -> Aol<Term> {
    t.lam(id(param), body)
}

fn occurrences(pairs: &[(&str, usize)]) -> Vec<(String, usize)> {
    pairs.iter().map(|(n, i)| (n.to_string(), *i)).collect()
}

/// `\x = x`: the sole binder is innermost, index 1.
#[test]
fn innermost_binder_is_one() {
    let got = indices(|t| {
        let x = t.var("x");
        lam(t, "x", x)
    });
    assert_eq!(got, occurrences(&[("x", 1)]));
}

/// `\x = \y = x`: `x` is two binders out, index 2; `y` would be 1.
#[test]
fn outer_binder_counts_outward() {
    let got = indices(|t| {
        let x = t.var("x");
        let inner = lam(t, "y", x);
        lam(t, "x", inner)
    });
    assert_eq!(got, occurrences(&[("x", 2)]));
    let got = indices(|t| {
        let y = t.var("y");
        let inner = lam(t, "y", y);
        lam(t, "x", inner)
    });
    assert_eq!(got, occurrences(&[("y", 1)]));
}

/// A free/global name (not on the binder stack) resolves to index 0.
#[test]
fn free_name_is_global() {
    let got = indices(|t| {
        let y = t.var("y");
        lam(t, "x", y)
    });
    assert_eq!(got, occurrences(&[("y", 0)]));
}

/// A qualified reference is always a global, index 0, even if a same-named binder
/// is in scope.
#[test]
fn qualified_is_global() {
    let got = indices(|t| {
        let qualified = t.add(Term::Var { module: Some(id("M")), name: id("x") });
        lam(t, "x", qualified)
    });
    assert_eq!(got, occurrences(&[("x", 0)]));
}

/// An inner binder shadows an outer one of the same name: the reference picks the
/// innermost, index 1.
#[test]
fn shadowing_picks_innermost() {
    let got = indices(|t| {
        let x = t.var("x");
        let inner = lam(t, "x", x);
        lam(t, "x", inner)
    });
    assert_eq!(got, occurrences(&[("x", 1)]));
}

/// A recursive `let` binds its name inside the value; a non-recursive one does
/// not, so the value's reference escapes to the outer scope.
#[test]
fn rec_let_binds_in_value() {
    // rec: `\outer = let f = f in f`: the `f` in the value sees the let binder.
    let got = indices(|t| {
        let (v, b) = (t.var("f"), t.var("f"));
        let l = t.add(Term::Let { name: id("f"), rec: true, val: v, body: b });
        lam(t, "outer", l)
    });
    assert_eq!(got, occurrences(&[("f", 1), ("f", 1)]));

    // non-rec: `\g = let f = g in f`: the value's `g` is the outer lambda param
    // (index 1: the let binder is not in scope yet), the body's `f` is the let (1).
    let got = indices(|t| {
        let (v, b) = (t.var("g"), t.var("f"));
        let l = t.add(Term::Let { name: id("f"), rec: false, val: v, body: b });
        lam(t, "g", l)
    });
    assert_eq!(got, occurrences(&[("g", 1), ("f", 1)]));
}

/// One variable node shared under two binder depths gets an index per
/// occurrence: `\x = (x, \y = x)` with both `x`s the same node.
#[test]
fn a_shared_node_is_indexed_per_occurrence() {
    let got = indices(|t| {
        let x = t.var("x");
        let inner = lam(t, "y", x);
        let pair = t.add(Term::Tuple(vec![x, inner]));
        lam(t, "x", pair)
    });
    assert_eq!(got, occurrences(&[("x", 1), ("x", 2)]));
}

/// A global's own name is not a binder: a self-reference is a global (index 0).
#[test]
fn global_self_reference_is_global() {
    let got = indices(|t| {
        let f = t.var("f");
        lam(t, "x", f)
    });
    assert_eq!(got, occurrences(&[("f", 0)]));
}
