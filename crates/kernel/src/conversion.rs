//! Definitional equality of values, and cumulative subtyping of types.
//!
//! The algorithm is typed conversion checking by evaluation, as in Abel and
//! Coquand's "Untyped algorithmic equality for Martin-Löf's logical framework
//! with surjective pairs" (TLCA 2005) extended with the type-directed rules
//! that need the type: two values of a proposition are always equal (proof
//! irrelevance), and a function is compared by applying both sides to a fresh
//! variable (eta). Types are compared structurally, and neutrals by their
//! heads and then their arguments at the argument types.
//!
//! Subtyping is equality except at universes: `@Type a <= @Type b` when
//! `a <= b` and `a` is not `0` (cumulativity starts at level 1, so a
//! proposition is never data), covariantly in a Pi type's codomain.
//!
//! The checker hands its context's variable types to a [`ConversionChecker`],
//! which pushes the types of the fresh variables it introduces and pops them
//! on the way out.
//!
//! Entry points: [`ConversionChecker::conv`], [`ConversionChecker::subtype`].

use crate::level::Level;
use crate::nbe::{apply, global_type, Lvl, Neutral, Val};
use crate::signature::Signature;

pub struct ConversionChecker<'a> {
    pub sig: &'a Signature,
    /// The type of every variable in scope, by de Bruijn level.
    pub types: &'a mut Vec<Val>,
}

// ## Types of neutrals and sorts of types

impl ConversionChecker<'_> {
    fn depth(&self) -> Lvl {
        self.types.len() as Lvl
    }

    fn under<R>(&mut self, ty: Val, k: impl FnOnce(&mut Self, Val) -> R) -> R {
        let x = Val::var(self.depth());
        self.types.push(ty);
        let r = k(self, x);
        self.types.pop();
        r
    }

    /// The type of a neutral, rebuilt from its head's type.
    pub fn infer_neutral(&mut self, n: &Neutral) -> Val {
        match n {
            Neutral::Var(level) => self.types[*level as usize].clone(),
            Neutral::Axiom(id, levels) => global_type(self.sig, *id, levels.clone()),
            Neutral::App(f, a) => match self.infer_neutral(f) {
                Val::Pi(p) => p.cod.apply(self.sig, a.clone()),
                _ => unreachable!("applied a neutral that is not a function"),
            },
        }
    }

    /// The level `l` with `ty : @Type l`, when it can be read off.
    pub fn sort_of(&mut self, ty: &Val) -> Option<Level> {
        match ty {
            Val::Sort(l) => Some(l.succ()),
            Val::Pi(p) => Some(Level::imax(&p.dom_level, &p.cod_level)),
            Val::Neutral(n) => match self.infer_neutral(n) {
                Val::Sort(l) => Some(l),
                _ => None,
            },
            Val::Lam(..) => None,
        }
    }

    /// Whether every value of `ty` is a proof.
    pub fn is_proposition(&mut self, ty: &Val) -> bool {
        self.sort_of(ty).is_some_and(|l| l.is_zero())
    }
}

// ## Conversion

impl ConversionChecker<'_> {
    /// `a` and `b` are equal values of type `ty`.
    pub fn conv(&mut self, ty: &Val, a: &Val, b: &Val) -> bool {
        // See Note [Proof irrelevance is decided by the type].
        if self.is_proposition(ty) {
            return true;
        }
        match ty {
            Val::Pi(p) => self.under(p.dom.clone(), |c, x| {
                let cod = p.cod.apply(c.sig, x.clone());
                let fa = apply(c.sig, a.clone(), x.clone());
                let fb = apply(c.sig, b.clone(), x);
                c.conv(&cod, &fa, &fb)
            }),
            Val::Sort(_) => self.conv_type(a, b, false),
            Val::Neutral(_) => self.conv_by_structure(a, b),
            Val::Lam(..) => unreachable!("a lambda is not a type"),
        }
    }

    /// Values of a type with no extensionality rule: canonical values by
    /// constructor, neutrals by head and spine.
    fn conv_by_structure(&mut self, a: &Val, b: &Val) -> bool {
        match (a, b) {
            (Val::Neutral(x), Val::Neutral(y)) => self.conv_neutral(x, y).is_some(),
            _ => false,
        }
    }

    /// Neutrals with the same head and equal arguments, returning their type.
    fn conv_neutral(&mut self, a: &Neutral, b: &Neutral) -> Option<Val> {
        match (a, b) {
            (Neutral::Var(x), Neutral::Var(y)) if x == y => Some(self.types[*x as usize].clone()),
            (Neutral::Axiom(x, xl), Neutral::Axiom(y, yl))
                if x == y && xl.iter().zip(yl.iter()).all(|(l, m)| l.equal(m)) =>
            {
                Some(global_type(self.sig, *x, xl.clone()))
            }
            (Neutral::App(f, x), Neutral::App(g, y)) => match self.conv_neutral(f, g)? {
                Val::Pi(p) => self.conv(&p.dom, x, y).then(|| p.cod.apply(self.sig, x.clone())),
                _ => unreachable!("applied a neutral that is not a function"),
            },
            _ => None,
        }
    }

    pub fn conv_type(&mut self, a: &Val, b: &Val, cumulative: bool) -> bool {
        match (a, b) {
            (Val::Sort(l), Val::Sort(m)) => {
                // See Note [Cumulativity starts at data].
                l.equal(m) || (cumulative && l.is_nonzero() && l.leq(m))
            }
            (Val::Pi(p), Val::Pi(q)) => {
                self.conv_type(&p.dom, &q.dom, false)
                    && self.under(p.dom.clone(), |c, x| {
                        let pc = p.cod.apply(c.sig, x.clone());
                        let qc = q.cod.apply(c.sig, x);
                        c.conv_type(&pc, &qc, cumulative)
                    })
            }
            (Val::Neutral(x), Val::Neutral(y)) => self.conv_neutral(x, y).is_some(),
            _ => false,
        }
    }

    /// `a` fits where `b` is expected.
    pub fn subtype(&mut self, a: &Val, b: &Val) -> bool {
        self.conv_type(a, b, true)
    }
}

// Note [Proof irrelevance is decided by the type]
//
// Any two proofs of a proposition are definitionally equal (the
// proof-irrelevant universe of TT^obs). Whether two values are proofs is a
// property of their type, not of the values: a variable `p : P` looks like
// any other neutral. So conversion is typed, and the first thing it does is
// ask whether the type lives in `@Type 0`. A type whose sort is a level
// parameter that may be zero is not treated as a proposition: its values are
// compared structurally, which is sound and only loses equations.

// Note [Cumulativity starts at data]
//
// `@Type 1 <= @Type 2 <= ...`, but `@Type 0` is not below `@Type 1`: a
// proposition used as a data type would let a proof (all of which are equal)
// stand for data (which are not). So `@Type a <= @Type b` needs `a` to be at
// least one under every assignment, as well as `a <= b`.
