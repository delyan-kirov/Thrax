//! Observational equality: the computation rules of `a ~ b` and `@cast`.
//!
//! The system is TT^obs, from Pujet and Tabareau, "Observational Equality:
//! Now for Good" (POPL 2022). Equality is a proposition that computes by the
//! structure of its type instead of being an inductive family:
//!
//! * at a Pi type, `f ~ g` is `(x: A) -> f x ~ g x` (function
//!   extensionality holds by computation);
//! * at a Sigma type, it is the conjunction of the first components' equality
//!   and the second components', the left one cast along the first;
//! * at an inductive type, it compares constructors: different ones give
//!   `@Bottom`, the same one the conjunction of its fields' equalities;
//! * at a universe, `A ~ B` compares type formers: the domains (contravariant
//!   for Pi) and codomains of two Pi or Sigma types, the parameters of two
//!   applications of one inductive type, and `@Bottom` for different heads;
//! * at `@Type 0`, `P ~ Q` is `(P -> Q) * (Q -> P)` (propositional
//!   extensionality), and at a proposition it is `@Top` (proof irrelevance).
//!
//! `@cast A B e t` moves `t : A` to `B` along `e : A ~ B`, computing by the
//! heads of `A` and `B`: through a lambda by casting the argument back, into
//! a pair componentwise, and into a constructor fieldwise. A cast between two
//! types with different heads, or with a neutral one, is stuck.
//!
//! See Note [Proofs evaluate to an irrelevant witness].
//!
//! Input: values in weak head normal form. Output: the reduct, or a stuck
//! neutral.
//!
//! Entry points: [`eq`], [`cast`].

use std::rc::Rc;

use crate::inductive::{field_type, param_type, ConstructorVal};
use crate::level::Level;
use crate::nbe::{apply, fst, snd, BindingVal, Closure, Neutral, Val};
use crate::signature::Signature;
use crate::term::mentions;

// ## Building propositions

fn proposition_pi(name: &str, dom: Val, dom_level: Level, cod: impl Fn(&Signature, Val) -> Val + 'static) -> Val {
    Val::Pi(Rc::new(BindingVal {
        name: name.into(),
        dom,
        cod: Closure::Native(Rc::new(cod)),
        dom_level,
        cod_level: Level::Zero,
    }))
}

/// `a * b` for two propositions.
fn and(a: Val, b: Val) -> Val {
    Val::Sigma(Rc::new(BindingVal {
        name: "_".into(),
        dom: a,
        cod: Closure::Native(Rc::new(move |_, _| b.clone())),
        dom_level: Level::Zero,
        cod_level: Level::Zero,
    }))
}

fn and_all(props: Vec<Val>) -> Val {
    let mut props = props.into_iter().rev();
    match props.next() {
        None => Val::Top,
        Some(last) => props.fold(last, |rest, p| and(p, rest)),
    }
}

/// `a -> b` for two propositions.
fn implies(a: Val, b: Val) -> Val {
    proposition_pi("_", a, Level::Zero, move |_, _| b.clone())
}

fn stuck_eq(level: Level, ty: Val, a: Val, b: Val) -> Val {
    Val::neutral(Neutral::Eq { level, ty, a, b })
}

// ## Equality

/// `a ~ b` at type `ty`, where `ty : @Type level`.
pub fn eq(sig: &Signature, level: Level, ty: Val, a: Val, b: Val) -> Val {
    if level.is_zero() {
        return Val::Top;
    }
    if !level.is_nonzero() {
        // A type whose sort may or may not be a proposition: neither rule
        // applies until the level is known.
        return stuck_eq(level, ty, a, b);
    }
    match &ty {
        Val::Pi(p) => {
            let p = p.clone();
            let (name, dom, dom_level) = (p.name.clone(), p.dom.clone(), p.dom_level.clone());
            proposition_pi(&name, dom, dom_level, move |sig, x| {
                let cod = p.cod.apply(sig, x.clone());
                eq(sig, p.cod_level.clone(), cod, apply(sig, a.clone(), x.clone()), apply(sig, b.clone(), x))
            })
        }
        Val::Sigma(p) => {
            let (a1, b1) = (fst(a.clone()), fst(b.clone()));
            let first = eq(sig, p.dom_level.clone(), p.dom.clone(), a1.clone(), b1.clone());
            let (ca, cb) = (p.cod.apply(sig, a1), p.cod.apply(sig, b1));
            let moved = cast(sig, ca, cb.clone(), Val::Irrelevant, snd(a));
            let second = eq(sig, p.cod_level.clone(), cb, moved, snd(b));
            and(first, second)
        }
        Val::Sort(k) => eq_types(sig, k, a, b),
        Val::Data(_) => match (&a, &b) {
            (Val::Constructor(x), Val::Constructor(y)) if x.index != y.index => Val::Bottom,
            (Val::Constructor(x), Val::Constructor(y)) => eq_fields(sig, x, y),
            _ => stuck_eq(level, ty, a, b),
        },
        Val::Neutral(_) => stuck_eq(level, ty, a, b),
        Val::Top | Val::Bottom => unreachable!("a proposition's level is zero"),
        Val::Lam(..) | Val::Pair(_) | Val::Tt | Val::Constructor(_) | Val::Irrelevant => {
            unreachable!("an equality at a non-type")
        }
    }
}

/// The fields of two values of one constructor are equal, each later field
/// cast along the earlier ones when its type depends on them.
fn eq_fields(sig: &Signature, x: &ConstructorVal, y: &ConstructorVal) -> Val {
    let decl = &sig.data(x.data).constructors[x.index];
    let mut props = Vec::new();
    for (j, field) in decl.fields.iter().enumerate() {
        if field.level.is_zero() {
            continue;
        }
        let ty_y = field_type(sig, x, j, &y.fields[..j]);
        let dependent = (0..j as u32).any(|i| mentions(&field.ty, i));
        let left = if dependent {
            let ty_x = field_type(sig, x, j, &x.fields[..j]);
            cast(sig, ty_x, ty_y.clone(), Val::Irrelevant, x.fields[j].clone())
        } else {
            x.fields[j].clone()
        };
        props.push(eq(sig, field.level.instantiate(&x.levels), ty_y, left, y.fields[j].clone()));
    }
    and_all(props)
}

/// How two components of the same position in two type formers compare:
/// at the level both live in, or not at all when one is a proposition and the
/// other data.
enum Component {
    At(Level),
    Mismatch,
    Unknown,
}

fn component(ambient: &Level, a: &Level, b: &Level) -> Component {
    if a.is_zero() && b.is_zero() {
        Component::At(Level::Zero)
    } else if a.is_nonzero() && b.is_nonzero() {
        // Both fit in the ambient universe by cumulativity.
        Component::At(ambient.clone())
    } else if (a.is_zero() && b.is_nonzero()) || (a.is_nonzero() && b.is_zero()) {
        Component::Mismatch
    } else {
        Component::Unknown
    }
}

/// `a ~ b` for two types in `@Type k`.
fn eq_types(sig: &Signature, k: &Level, a: Val, b: Val) -> Val {
    let universe = || Val::Sort(k.clone());
    if k.is_zero() {
        return and(implies(a.clone(), b.clone()), implies(b, a));
    }
    if !k.is_nonzero() {
        return stuck_eq(k.succ(), universe(), a, b);
    }
    match (&a, &b) {
        (Val::Sort(i), Val::Sort(j)) => {
            if i.equal(j) {
                Val::Top
            } else if i.succ().leq(j) || j.succ().leq(i) {
                Val::Bottom
            } else {
                stuck_eq(k.succ(), universe(), a, b)
            }
        }
        (Val::Pi(p), Val::Pi(q)) | (Val::Sigma(p), Val::Sigma(q)) => {
            let is_pi = matches!(a, Val::Pi(_));
            let (dom, cod) = match (component(k, &p.dom_level, &q.dom_level), component(k, &p.cod_level, &q.cod_level)) {
                (Component::At(d), Component::At(c)) => (d, c),
                (Component::Mismatch, _) | (_, Component::Mismatch) => return Val::Bottom,
                _ => return stuck_eq(k.succ(), universe(), a, b),
            };
            let (p, q) = (p.clone(), q.clone());
            if is_pi {
                // `(x: A) -> B ~ (x: A') -> B'` is `A' ~ A` and, for every
                // `a': A'`, `B[cast a'] ~ B'[a']`.
                let doms = eq_types(sig, &dom, q.dom.clone(), p.dom.clone());
                let (name, dom, dom_level) = (q.name.clone(), q.dom.clone(), q.dom_level.clone());
                let cods = proposition_pi(&name, dom, dom_level, move |sig, a2| {
                    let a1 = cast(sig, q.dom.clone(), p.dom.clone(), Val::Irrelevant, a2.clone());
                    eq_types(sig, &cod, p.cod.apply(sig, a1), q.cod.apply(sig, a2))
                });
                and(doms, cods)
            } else {
                // `(x: A) * B ~ (x: A') * B'` is `A ~ A'` and, for every
                // `a: A`, `B[a] ~ B'[cast a]`.
                let doms = eq_types(sig, &dom, p.dom.clone(), q.dom.clone());
                let (name, dom, dom_level) = (p.name.clone(), p.dom.clone(), p.dom_level.clone());
                let cods = proposition_pi(&name, dom, dom_level, move |sig, a1| {
                    let a2 = cast(sig, p.dom.clone(), q.dom.clone(), Val::Irrelevant, a1.clone());
                    eq_types(sig, &cod, p.cod.apply(sig, a1), q.cod.apply(sig, a2))
                });
                and(doms, cods)
            }
        }
        (Val::Data(x), Val::Data(y)) if x.data == y.data => {
            let decl = sig.data(x.data);
            let same_levels = x.levels.len() == y.levels.len() && x.levels.iter().zip(y.levels.iter()).all(|(l, m)| l.equal(m));
            let dependent = decl.params.iter().enumerate().any(|(i, p)| (0..i as u32).any(|j| mentions(&p.ty, j)));
            if !same_levels || dependent {
                return stuck_eq(k.succ(), universe(), a, b);
            }
            let mut props = Vec::new();
            for (i, param) in decl.params.iter().enumerate() {
                let level = param.level.instantiate(&x.levels);
                if !level.is_zero() {
                    props.push(eq(sig, level, param_type(sig, x, i), x.params[i].clone(), y.params[i].clone()));
                }
            }
            and_all(props)
        }
        (Val::Neutral(_), _) | (_, Val::Neutral(_)) => stuck_eq(k.succ(), universe(), a, b),
        _ => Val::Bottom,
    }
}

// ## Cast

/// `@cast a b e t`: `t : a` seen at `b`, along `e : a ~ b`.
pub fn cast(sig: &Signature, a: Val, b: Val, e: Val, t: Val) -> Val {
    if let Val::Irrelevant = t {
        return t;
    }
    match (&a, &b) {
        (Val::Sort(_), Val::Sort(_)) | (Val::Top, Val::Top) | (Val::Bottom, Val::Bottom) => t,
        (Val::Pi(p), Val::Pi(q)) => {
            let (p, q) = (p.clone(), q.clone());
            Val::Lam(
                q.name.clone(),
                Closure::Native(Rc::new(move |sig, a2| {
                    let a1 = cast(sig, q.dom.clone(), p.dom.clone(), Val::Irrelevant, a2.clone());
                    let (from, to) = (p.cod.apply(sig, a1.clone()), q.cod.apply(sig, a2));
                    cast(sig, from, to, Val::Irrelevant, apply(sig, t.clone(), a1))
                })),
            )
        }
        (Val::Sigma(p), Val::Sigma(q)) => {
            let a1 = fst(t.clone());
            let a2 = cast(sig, p.dom.clone(), q.dom.clone(), Val::Irrelevant, a1.clone());
            let (from, to) = (p.cod.apply(sig, a1), q.cod.apply(sig, a2.clone()));
            let b2 = cast(sig, from, to, Val::Irrelevant, snd(t));
            Val::Pair(Rc::new((a2, b2)))
        }
        (Val::Data(x), Val::Data(y)) if x.data == y.data => {
            if sig.data(x.data).params.is_empty() {
                return t;
            }
            let Val::Constructor(c) = &t else {
                return stuck_cast(a, b, e, t);
            };
            let target = ConstructorVal {
                data: c.data,
                index: c.index,
                levels: y.levels.clone(),
                params: y.params.clone(),
                fields: Vec::new(),
            };
            let mut fields = Vec::with_capacity(c.fields.len());
            for (j, old) in c.fields.iter().enumerate() {
                let from = field_type(sig, c, j, &c.fields[..j]);
                let to = field_type(sig, &target, j, &fields);
                fields.push(cast(sig, from, to, Val::Irrelevant, old.clone()));
            }
            Val::Constructor(Rc::new(ConstructorVal { fields, ..target }))
        }
        _ => stuck_cast(a, b, e, t),
    }
}

fn stuck_cast(a: Val, b: Val, e: Val, t: Val) -> Val {
    Val::neutral(Neutral::Cast { from: a, to: b, proof: e, value: t })
}

// Note [Proofs evaluate to an irrelevant witness]
//
// A proof's value never matters: conversion compares proofs by their type
// alone, and the computation rules of equality and cast only ever need *some* proof to pass
// along. So `@refl` and `@transp` evaluate to the one value
// `Val::Irrelevant`, and the rules above pass it wherever TT^obs builds a
// proof term (the components of `e : A ~ B` that a cast through a Pi or
// Sigma type needs, or the congruence proofs a fieldwise cast needs). Those
// proofs exist in the published system, which is what makes the shortcut
// sound; the checker still type-checks every proof the user writes.
// Eliminating the witness gives the witness again: applying or projecting
// it, and casting it to another proposition.
// See Note [Proof irrelevance is decided by the type].
//
// One rule of the paper's presentation is missing: `@cast A A e t` reduces to
// `t` only when `A` has a head the cast rules know (a universe, a Pi or Sigma
// type, an inductive type), not for a neutral `A`.

#[cfg(test)]
mod tests;
