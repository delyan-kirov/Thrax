//! Ownership over the IR, in the spirit of Perceus (Reinking et al. 2021): an
//! activation owns one reference per local slot, and this pass marks where that
//! reference ends instead of leaving it until the activation dies.
//!
//! A backwards liveness walk over each code body rewrites
//!
//! - the last use of a local on a path to [`Atom::Move`], which hands the
//!   slot's reference to the consumer and empties the slot;
//! - the last use of a captured field to [`Atom::MoveEnv`], which the engines
//!   honour only while the activation is its closure's sole owner;
//! - a slot whose value dies unread to an [`Expr::Drop`]: a parameter or `let`
//!   binder the body never reads, a value live into a `Case` that one branch
//!   no longer needs, or a value used twice by one node (neither use can move,
//!   since the C backend does not fix the order a node's atoms evaluate in);
//! - a `Case` binder the alternative never reads to `None`, so it is not bound.
//!
//! The payoff is that a reference count read at runtime is exact: a value held
//! once is held by whoever is about to use it. A resume uses that to move its
//! slice rather than copy it (see `ResumeUse`), and a built-in can update a
//! uniquely owned operand in place.
//!
//! A slot a recursive `let` binds is exempt. Closures in its right-hand side
//! reach it through a weak edge to the slot's box, so the slot itself has to
//! keep the box alive for as long as the activation lasts.

use std::collections::{BTreeMap, BTreeSet};

use super::data::{Atom, Code, Expr};
use super::lower::mentions;

/// A value an activation can read: one of its local slots, or a field of its
/// closure's captured record.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Var {
    Local(usize),
    Env(usize),
}

type Live = BTreeSet<Var>;

/// What the walk learns about an expression from its continuation.
struct Flow {
    /// What must hold a value when the expression starts.
    live: Live,
    /// Local slots that still hold a value once the expression completes
    /// although nothing reads them again; the nearest enclosing `let` drops
    /// them at the start of its body.
    leftover: BTreeSet<usize>,
}

/// Annotate every code block. Run once, on freshly closure-converted codes.
pub fn annotate(codes: &mut [Code]) {
    for code in codes {
        let mut body = std::mem::replace(&mut code.body, Expr::Fault(String::new()));
        let flow = expr(&mut body, &Live::new());
        let dead_params = (0..code.nparams)
            .filter(|p| !flow.live.contains(&Var::Local(*p)))
            .collect();
        code.body = with_drops(dead_params, body);
    }
}

fn var(a: &Atom) -> Option<Var> {
    match a {
        Atom::Local(s) | Atom::Move(s) => Some(Var::Local(*s)),
        Atom::Env(i) | Atom::MoveEnv(i) => Some(Var::Env(*i)),
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
    let mut count: BTreeMap<Var, usize> = BTreeMap::new();
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
            (Var::Local(s), 1) => *a = Atom::Move(s),
            (Var::Env(i), 1) => *a = Atom::MoveEnv(i),
            (Var::Local(s), _) => {
                leftover.insert(s);
            }
            (Var::Env(_), _) => {}
        }
    });
    Flow { live, leftover }
}

/// Prefix `body` with a drop of `slots` (merging into a drop already there).
fn with_drops(mut slots: Vec<usize>, body: Expr) -> Expr {
    if slots.is_empty() {
        return body;
    }
    match body {
        Expr::Drop {
            slots: inner,
            body,
        } => {
            slots.extend(inner);
            slots.sort_unstable();
            slots.dedup();
            Expr::Drop { slots, body }
        }
        body => {
            slots.sort_unstable();
            slots.dedup();
            Expr::Drop {
                slots,
                body: Box::new(body),
            }
        }
    }
}

fn locals_in(vars: impl IntoIterator<Item = Var>) -> Vec<usize> {
    vars.into_iter()
        .filter_map(|v| match v {
            Var::Local(s) => Some(s),
            Var::Env(_) => None,
        })
        .collect()
}

/// Walk `e` given what is live after it (`out`), rewriting it in place.
fn expr(e: &mut Expr, out: &Live) -> Flow {
    match e {
        Expr::Ret(a) => node([a], out),
        Expr::App { fun, arg, .. } => node([fun, arg], out),
        Expr::MkStruct { base, fields, .. } => {
            node(base.iter_mut().chain(fields.iter_mut().map(|(_, a)| a)), out)
        }
        Expr::Field { rec, .. } => node([rec], out),
        Expr::MkVariant { fields, .. } => node(fields.iter_mut(), out),
        Expr::MkTuple(items) => node(items.iter_mut(), out),

        // Nothing after a fault runs, so nothing needs to be alive for it.
        Expr::Fault(_) => Flow {
            live: Live::new(),
            leftover: BTreeSet::new(),
        },

        Expr::Let { slot, rhs, body } => {
            let slot = *slot;
            let mut body_out = out.clone();
            if mentions(rhs, slot) {
                body_out.insert(Var::Local(slot));
            }
            let fb = expr(body, &body_out);
            // While the right-hand side runs, the slot holds the box its value
            // is delivered into, so nothing inside may move or drop it.
            let mut rhs_out = fb.live.clone();
            rhs_out.insert(Var::Local(slot));
            let fr = expr(rhs, &rhs_out);

            let mut dead: Vec<usize> = fr
                .leftover
                .into_iter()
                .filter(|s| !fb.live.contains(&Var::Local(*s)))
                .collect();
            if !fb.live.contains(&Var::Local(slot)) {
                dead.push(slot);
            }
            let b = std::mem::replace(body.as_mut(), Expr::Fault(String::new()));
            **body = with_drops(dead, b);

            let mut live = fr.live;
            live.remove(&Var::Local(slot));
            Flow {
                live,
                leftover: fb.leftover,
            }
        }

        Expr::Case {
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
                    let slot = Var::Local(alt.binder_base + i);
                    if !live.contains(&slot) {
                        *binder = None;
                    }
                    live.remove(&slot);
                }
                leftover.extend(fa.leftover);
                arms.push((live, matches!(alt.body, Expr::Fault(_))));
            }
            let fd = expr(default, out);
            leftover.extend(fd.leftover);
            arms.push((fd.live, matches!(**default, Expr::Fault(_))));

            let union: Live = arms.iter().flat_map(|(l, _)| l.iter().copied()).collect();
            let bodies = alts.iter_mut().map(|a| &mut a.body).chain([default.as_mut()]);
            for ((live, faults), body) in arms.iter().zip(bodies) {
                if *faults {
                    continue;
                }
                let dead = locals_in(union.difference(live).copied());
                let b = std::mem::replace(body, Expr::Fault(String::new()));
                *body = with_drops(dead, b);
            }

            let fs = node([scrut], &union);
            leftover.extend(fs.leftover);
            Flow {
                live: fs.live,
                leftover,
            }
        }

        Expr::Handle { body, clauses, els } => {
            let fb = expr(body, out);
            let fi = node(clauses.iter_mut().map(|c| &mut c.fun).chain([els]), &fb.live);
            let b = std::mem::replace(body.as_mut(), Expr::Fault(String::new()));
            **body = with_drops(fi.leftover.into_iter().collect(), b);
            Flow {
                live: fi.live,
                leftover: fb.leftover,
            }
        }

        Expr::Defer { cleanup, body } => {
            let fb = expr(body, out);
            let fi = node([cleanup], &fb.live);
            let b = std::mem::replace(body.as_mut(), Expr::Fault(String::new()));
            **body = with_drops(fi.leftover.into_iter().collect(), b);
            Flow {
                live: fi.live,
                leftover: fb.leftover,
            }
        }

        Expr::Drop { .. } => unreachable!("ownership: the pass runs once per code"),
    }
}

#[cfg(test)]
mod tests;
