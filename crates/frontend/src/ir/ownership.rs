//! Ownership: where each local's reference ends.
//!
//! The approach follows Perceus, from Reinking, Xie, de Moura and Leijen,
//! "Perceus: Garbage Free Reference Counting with Reuse" (PLDI 2021): an
//! activation owns one reference per local slot, and this pass marks where
//! that reference ends instead of leaving it until the activation dies.
//! See Note [Last use moves].
//!
//! A backward liveness walk over each code body rewrites
//!
//! - the last use of a local on a path to [`Atom::Move`], which hands the
//!   slot's reference to the consumer and empties the slot;
//! - the last use of a captured field to [`Atom::MoveEnv`], which the engines
//!   honour only while the activation is its closure's sole owner;
//! - a slot whose value dies unread to an [`IrExpr::Drop`]: a parameter or
//!   `let` binder the body never reads, a value live into a `Case` that one
//!   branch no longer needs, or a value used twice by one node (neither use
//!   can move, since the C backend does not fix the order a node's atoms
//!   evaluate in);
//! - a `Case` binder the alternative never reads to `None`, so it is not
//!   bound.
//!
//! Input: freshly closure-converted codes. Output: the same codes with moves
//! and drops marked.
//!
//! Entry point: [`insert_moves_and_drops`].

use std::collections::{BTreeMap, BTreeSet};

use super::data::{Atom, Code, IrExpr};

/// A value an activation can read: one of its local slots, or a field of its
/// closure's captured record.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Place {
    Local(usize),
    Env(usize),
}

type Live = BTreeSet<Place>;

/// What the walk learns about an expression from its continuation.
struct Flow {
    /// What must hold a value when the expression starts.
    live: Live,
    /// Local slots that still hold a value once the expression completes
    /// although nothing reads them again; the nearest enclosing `let` drops
    /// them at the start of its body.
    leftover: BTreeSet<usize>,
}

// Note [Last use moves]
//
// Every engine counts references. A value read from a local slot is shared:
// the slot keeps its reference and the reader takes another. Left at that, a
// value is never uniquely owned while the activation that holds it is alive,
// so no operation can update it in place and no resumption can move its
// slice. See Note [Resume uniqueness].
//
// This pass finds, by backward liveness over each code body, the point where
// each slot's value is used for the last time on each path, and turns that
// read into a move (`Atom::Move`, or `Atom::MoveEnv` for a closure field). A
// value that dies without being read gets an explicit `Expr::Drop` where it
// dies. Reference counts are then exact: a value held once is held by
// whoever is about to use it. Two kinds of slot are exempt, because their
// last use is not a point in this code: a recursive `let`'s slot (closures
// reach it through a weak edge, so the slot keeps it alive) and anything a
// handler body reads from outside it (a resumption can re-enter the body
// after the code that follows it has run). Both engines and the C backend
// read the same annotations, so they agree on when a value dies.

/// Annotate every code block. Run once, on freshly closure-converted codes.
/// See Note [Last use moves].
pub fn insert_moves_and_drops(codes: &mut [Code]) {
    for code in codes {
        let mut body = std::mem::replace(&mut code.body, IrExpr::Fault(String::new()));
        let mut pinned = Live::new();
        handler_reads(&body, &mut pinned);
        let flow = expr(&mut body, &pinned);
        let dead_params = (0..code.nparams)
            .filter(|p| !flow.live.contains(&Place::Local(*p)))
            .collect();
        code.body = with_drops(dead_params, body);
    }
}

/// Collect into `out` what each handler body in `e` reads from outside it.
fn handler_reads(e: &IrExpr, out: &mut Live) {
    e.any_node(&mut |e| {
        if let IrExpr::Handle { body, .. } = e {
            let mut reads = Live::new();
            let mut bound = Live::new();
            body.any_node(&mut |e| {
                match e {
                    IrExpr::Let { slot, .. } => {
                        bound.insert(Place::Local(*slot));
                    }
                    IrExpr::Case { alts, .. } => {
                        for alt in alts {
                            for i in 0..alt.binders.len() {
                                bound.insert(Place::Local(alt.binder_base + i));
                            }
                        }
                    }
                    _ => {}
                }
                e.for_each_atom(&mut |a| reads.extend(var(a)));
                false
            });
            out.extend(reads.difference(&bound));
        }
        false
    });
}

fn var(a: &Atom) -> Option<Place> {
    match a {
        Atom::Local(s) | Atom::Move(s) => Some(Place::Local(*s)),
        Atom::Env(i) | Atom::MoveEnv(i) => Some(Place::Env(*i)),
        _ => None,
    }
}

/// Visit every variable occurrence among `atoms`, including those inside the
/// capture lists of closures built there.
fn each_var<'a>(atoms: impl IntoIterator<Item = &'a mut Atom>, f: &mut impl FnMut(&mut Atom)) {
    for a in atoms {
        match a {
            Atom::Clos { captures, .. } => each_var(captures.iter_mut(), f),
            _ if var(a).is_some() => f(a),
            _ => {}
        }
    }
}

/// One node's atoms, all evaluated together, followed by `out`. An occurrence
/// moves when its variable is dead afterwards and appears nowhere else in the
/// node.
fn node<'a>(atoms: impl IntoIterator<Item = &'a mut Atom>, out: &Live) -> Flow {
    let mut atoms: Vec<&mut Atom> = atoms.into_iter().collect();
    let mut count: BTreeMap<Place, usize> = BTreeMap::new();
    each_var(atoms.iter_mut().map(|a| &mut **a), &mut |a| {
        *count.entry(var(a).expect("a variable occurrence")).or_default() += 1;
    });
    let mut live = out.clone();
    let mut leftover = BTreeSet::new();
    each_var(atoms.iter_mut().map(|a| &mut **a), &mut |a| {
        let v = var(a).expect("a variable occurrence");
        live.insert(v);
        if out.contains(&v) {
            return;
        }
        match (v, count[&v]) {
            (Place::Local(s), 1) => *a = Atom::Move(s),
            (Place::Env(i), 1) => *a = Atom::MoveEnv(i),
            (Place::Local(s), _) => {
                leftover.insert(s);
            }
            (Place::Env(_), _) => {}
        }
    });
    Flow { live, leftover }
}

/// Prefix `body` with a drop of `slots` (merging into a drop already there).
fn with_drops(mut slots: Vec<usize>, body: IrExpr) -> IrExpr {
    if slots.is_empty() {
        return body;
    }
    match body {
        IrExpr::Drop {
            slots: inner,
            body,
        } => {
            slots.extend(inner);
            slots.sort_unstable();
            slots.dedup();
            IrExpr::Drop { slots, body }
        }
        body => {
            slots.sort_unstable();
            slots.dedup();
            IrExpr::Drop {
                slots,
                body: Box::new(body),
            }
        }
    }
}

fn locals_in(vars: impl IntoIterator<Item = Place>) -> Vec<usize> {
    vars.into_iter()
        .filter_map(|v| match v {
            Place::Local(s) => Some(s),
            Place::Env(_) => None,
        })
        .collect()
}

/// Walk `e` given what is live after it (`out`), rewriting it in place.
fn expr(e: &mut IrExpr, out: &Live) -> Flow {
    match e {
        IrExpr::Ret(a) => node([a], out),
        IrExpr::App { fun, arg, .. } => node([fun, arg], out),
        IrExpr::MkStruct { base, fields, .. } => {
            node(base.iter_mut().chain(fields.iter_mut().map(|(_, a)| a)), out)
        }
        IrExpr::Field { rec, .. } => node([rec], out),
        IrExpr::MkVariant { fields, .. } => node(fields.iter_mut(), out),
        IrExpr::MkTuple(items) => node(items.iter_mut(), out),

        // Nothing after a fault runs, so nothing needs to be alive for it.
        IrExpr::Fault(_) => Flow {
            live: Live::new(),
            leftover: BTreeSet::new(),
        },

        IrExpr::Let { slot, rhs, body } => {
            let slot = *slot;
            let mut body_out = out.clone();
            if rhs.mentions(slot) {
                body_out.insert(Place::Local(slot));
            }
            let fb = expr(body, &body_out);
            // While the right-hand side runs, the slot holds the box its value
            // is delivered into, so nothing inside may move or drop it.
            let mut rhs_out = fb.live.clone();
            rhs_out.insert(Place::Local(slot));
            let fr = expr(rhs, &rhs_out);

            let mut dead: Vec<usize> = fr
                .leftover
                .into_iter()
                .filter(|s| !fb.live.contains(&Place::Local(*s)))
                .collect();
            if !fb.live.contains(&Place::Local(slot)) {
                dead.push(slot);
            }
            let b = std::mem::replace(body.as_mut(), IrExpr::Fault(String::new()));
            **body = with_drops(dead, b);

            let mut live = fr.live;
            live.remove(&Place::Local(slot));
            Flow {
                live,
                leftover: fb.leftover,
            }
        }

        IrExpr::Case {
            scrut,
            alts,
            default,
        } => {
            let mut leftover = BTreeSet::new();
            let mut arms: Vec<(Live, bool)> = Vec::with_capacity(alts.len() + 1);
            for alt in alts.iter_mut() {
                let fa = expr(&mut alt.body, out);
                let mut live = fa.live;
                for (i, binder) in alt.binders.iter_mut().enumerate() {
                    let slot = Place::Local(alt.binder_base + i);
                    if !live.contains(&slot) {
                        *binder = None;
                    }
                    live.remove(&slot);
                }
                leftover.extend(fa.leftover);
                arms.push((live, matches!(alt.body, IrExpr::Fault(_))));
            }
            let fd = expr(default, out);
            leftover.extend(fd.leftover);
            arms.push((fd.live, matches!(**default, IrExpr::Fault(_))));

            let union: Live = arms.iter().flat_map(|(l, _)| l.iter().copied()).collect();
            let bodies = alts.iter_mut().map(|a| &mut a.body).chain([default.as_mut()]);
            for ((live, faults), body) in arms.iter().zip(bodies) {
                if *faults {
                    continue;
                }
                let dead = locals_in(union.difference(live).copied());
                let b = std::mem::replace(body, IrExpr::Fault(String::new()));
                *body = with_drops(dead, b);
            }

            let fs = node([scrut], &union);
            leftover.extend(fs.leftover);
            Flow {
                live: fs.live,
                leftover,
            }
        }

        IrExpr::Handle { body, clauses, els } => {
            let fb = expr(body, out);
            let fi = node(clauses.iter_mut().map(|c| &mut c.fun).chain([els]), &fb.live);
            let b = std::mem::replace(body.as_mut(), IrExpr::Fault(String::new()));
            **body = with_drops(fi.leftover.into_iter().collect(), b);
            Flow {
                live: fi.live,
                leftover: fb.leftover,
            }
        }

        IrExpr::Defer { cleanup, body } => {
            let fb = expr(body, out);
            let fi = node([cleanup], &fb.live);
            let b = std::mem::replace(body.as_mut(), IrExpr::Fault(String::new()));
            **body = with_drops(fi.leftover.into_iter().collect(), b);
            Flow {
                live: fi.live,
                leftover: fb.leftover,
            }
        }

        IrExpr::Drop { .. } => unreachable!("ownership: the pass runs once per code"),
    }
}

#[cfg(test)]
mod tests;
