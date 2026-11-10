//! Universe levels and the decision procedure for `<=` between them.
//!
//! A level is built from `0`, successor, `max`, `imax` and level parameters
//! (the prenex `u: @l` fields of a global's `@ctx`). Level `0` is the universe
//! of propositions, so the sort of a Pi type is `imax a b`: `0` when the
//! codomain is a proposition, `max a b` otherwise (Lean's rule, from the
//! impredicative system of Pujet and Tabareau, "Impredicative Observational
//! Equality", POPL 2023).
//!
//! Deciding `a <= b` for every assignment of the parameters uses Lean 4's
//! case split: while some `imax x y` has a `y` that may or may not be zero, pick
//! a parameter `p` of `y` that keeps it undecided and check both `p := 0` and
//! `p := p + 1`. Without undecided `imax`, a level is a maximum of `p + k` and
//! one constant (its max-plus normal form), where `<=` is read off directly.
//!
//! Kernel parameters range over every level, `0` included. The surface default
//! of `n >= 1` is a kernel `k + 1`.
//!
//! Entry points: [`Level::leq`], [`Level::equal`].

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

/// A level parameter, by position in its global's level telescope.
pub type LevelParam = u32;

/// A universe level expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Level {
    Zero,
    Succ(Rc<Level>),
    Max(Rc<Level>, Rc<Level>),
    /// `imax a b` is `0` when `b` is `0`, and `max a b` otherwise.
    IMax(Rc<Level>, Rc<Level>),
    Param(LevelParam),
}

// ## Construction

impl Level {
    pub fn constant(n: u32) -> Level {
        (0..n).fold(Level::Zero, |l, _| l.succ())
    }

    pub fn succ(&self) -> Level {
        Level::Succ(Rc::new(self.clone()))
    }

    /// `max a b`, folding the cases that are decided syntactically.
    pub fn max(a: &Level, b: &Level) -> Level {
        match (a, b) {
            (Level::Zero, _) => b.clone(),
            (_, Level::Zero) => a.clone(),
            _ if a == b => a.clone(),
            _ => Level::Max(Rc::new(a.clone()), Rc::new(b.clone())),
        }
    }

    /// `imax a b`, folding the cases that are decided syntactically.
    pub fn imax(a: &Level, b: &Level) -> Level {
        match b {
            Level::Zero => Level::Zero,
            Level::Succ(_) => Level::max(a, b),
            _ if *a == Level::Zero => b.clone(),
            _ => Level::IMax(Rc::new(a.clone()), Rc::new(b.clone())),
        }
    }

    /// Replace every parameter `i` by `args[i]`.
    pub fn instantiate(&self, args: &[Level]) -> Level {
        match self {
            Level::Zero => Level::Zero,
            Level::Succ(l) => l.instantiate(args).succ(),
            Level::Max(a, b) => Level::max(&a.instantiate(args), &b.instantiate(args)),
            Level::IMax(a, b) => Level::imax(&a.instantiate(args), &b.instantiate(args)),
            Level::Param(p) => {
                assert!((*p as usize) < args.len(), "level parameter {p} has no argument");
                args[*p as usize].clone()
            }
        }
    }

    /// Replace parameter `p` by `by`, leaving the others alone.
    fn substitute(&self, p: LevelParam, by: &Level) -> Level {
        match self {
            Level::Zero => Level::Zero,
            Level::Succ(l) => l.substitute(p, by).succ(),
            Level::Max(a, b) => Level::max(&a.substitute(p, by), &b.substitute(p, by)),
            Level::IMax(a, b) => Level::imax(&a.substitute(p, by), &b.substitute(p, by)),
            Level::Param(q) if *q == p => by.clone(),
            Level::Param(_) => self.clone(),
        }
    }
}

// ## Decision procedure

/// A level without undecided `imax`: `max(constant, p + k for (p, k) in params)`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct MaxPlus {
    constant: u32,
    params: BTreeMap<LevelParam, u32>,
}

impl MaxPlus {
    fn join(mut self, other: MaxPlus) -> MaxPlus {
        self.constant = self.constant.max(other.constant);
        for (p, k) in other.params {
            let slot = self.params.entry(p).or_insert(k);
            *slot = (*slot).max(k);
        }
        self
    }

    fn plus_one(mut self) -> MaxPlus {
        // `max(c, p + k) + 1` is `max(c + 1, p + k + 1)`. A bare `0` constant
        // becomes `1`, which is right: the level is now at least one.
        self.constant += 1;
        for k in self.params.values_mut() {
            *k += 1;
        }
        self
    }

    fn is_zero(&self) -> bool {
        self.constant == 0 && self.params.is_empty()
    }

    /// At least one under every assignment.
    fn is_nonzero(&self) -> bool {
        self.constant >= 1 || self.params.values().any(|k| *k >= 1)
    }

    /// `self <= other` under every assignment.
    fn leq(&self, other: &MaxPlus) -> bool {
        let floor = other.params.values().copied().fold(other.constant, u32::max);
        self.constant <= floor
            && self.params.iter().all(|(p, k)| other.params.get(p).is_some_and(|j| j >= k))
    }
}

impl Level {
    /// The normal form, or the parameter to case-split on when an `imax`
    /// is undecided.
    fn max_plus(&self) -> Result<MaxPlus, LevelParam> {
        Ok(match self {
            Level::Zero => MaxPlus::default(),
            Level::Succ(l) => l.max_plus()?.plus_one(),
            Level::Max(a, b) => a.max_plus()?.join(b.max_plus()?),
            Level::IMax(a, b) => {
                let b = b.max_plus()?;
                if b.is_zero() {
                    MaxPlus::default()
                } else if b.is_nonzero() {
                    a.max_plus()?.join(b)
                } else {
                    // Undecided: the constant is 0 and every parameter has
                    // offset 0, so each one can make `b` zero.
                    return Err(*b.params.keys().next().expect("an undecided level has a parameter"));
                }
            }
            Level::Param(p) => MaxPlus {
                constant: 0,
                params: BTreeMap::from([(*p, 0)]),
            },
        })
    }

    /// `self <= other` for every assignment of the parameters.
    pub fn leq(&self, other: &Level) -> bool {
        match self.max_plus().and_then(|a| other.max_plus().map(|b| (a, b))) {
            Ok((a, b)) => a.leq(&b),
            Err(p) => {
                let succ = Level::Param(p).succ();
                self.substitute(p, &Level::Zero).leq(&other.substitute(p, &Level::Zero))
                    && self.substitute(p, &succ).leq(&other.substitute(p, &succ))
            }
        }
    }

    pub fn equal(&self, other: &Level) -> bool {
        self.leq(other) && other.leq(self)
    }

    /// Zero under every assignment: the level of propositions.
    pub fn is_zero(&self) -> bool {
        self.leq(&Level::Zero)
    }

    /// At least one under every assignment: the level of data.
    pub fn is_nonzero(&self) -> bool {
        Level::constant(1).leq(self)
    }
}

// ## Display

/// Prints a level with its parameters' names.
pub struct LevelDisplay<'a> {
    pub level: &'a Level,
    pub names: &'a [Rc<str>],
}

impl<'a> fmt::Display for LevelDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut offset = 0;
        let mut base: &'a Level = self.level;
        while let Level::Succ(l) = base {
            offset += 1;
            base = l;
        }
        let names = self.names;
        let show = move |l: &'a Level| LevelDisplay { level: l, names };
        match base {
            Level::Zero => return write!(f, "{offset}"),
            Level::Param(p) => match self.names.get(*p as usize) {
                Some(name) => write!(f, "{name}")?,
                None => write!(f, "?u{p}")?,
            },
            Level::Max(a, b) => write!(f, "(@max {} {})", show(a), show(b))?,
            Level::IMax(a, b) => write!(f, "(@imax {} {})", show(a), show(b))?,
            Level::Succ(_) => unreachable!("successors were stripped above"),
        }
        if offset > 0 {
            write!(f, " + {offset}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
