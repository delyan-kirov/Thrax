//! Core terms of the kernel, with de Bruijn indices.
//!
//! The checker elaborates the script syntax into these terms, normalisation
//! by evaluation reads them back from values, and nothing else builds them.
//! A bound variable is its de Bruijn index (0 is the innermost binder); a
//! global is its position in the [`Signature`] with the levels it is
//! instantiated at. Binders keep their source name for printing only.
//!
//! Children are `Rc`-shared: a value's closure captures the body it will
//! evaluate, so a term outlives the pass that built it.
//!
//! Entry point: [`Term`], printed by [`TermDisplay`].

use std::fmt;
use std::rc::Rc;

use crate::level::{Level, LevelDisplay};
use crate::signature::{GlobalId, Signature};

/// A de Bruijn index: the number of binders between a variable and its own.
pub type Index = u32;

/// A binder's source name.
pub type Name = Rc<str>;

/// A dependent binding form, `(name: dom) -> cod`, with the sorts of both
/// sides (the sort of the whole is computed from them).
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: Name,
    pub dom: Rc<Term>,
    pub cod: Rc<Term>,
    pub dom_level: Level,
    pub cod_level: Level,
}

#[derive(Clone, Debug)]
pub enum Term {
    Var(Index),
    Global(GlobalId, Rc<[Level]>),
    /// `@Type l`; `@Type 0` is the universe of propositions.
    Sort(Level),
    Pi(Rc<Binding>),
    Lam(Name, Rc<Term>),
    App(Rc<Term>, Rc<Term>),
    /// `let name: ty = val in body`, kept so `name` stays transparent.
    Let(Name, Rc<Term>, Rc<Term>, Rc<Term>),
    /// `(name: dom) * cod`, the dependent pair type.
    Sigma(Rc<Binding>),
    Pair(Rc<Term>, Rc<Term>),
    Fst(Rc<Term>),
    Snd(Rc<Term>),
    /// The true proposition and its proof.
    Top,
    Tt,
    /// The false proposition, and its elimination into any type.
    Bottom,
    Absurd(Rc<Term>, Rc<Term>),
    /// `a ~ b` at type `ty`, where `ty : @Type level`.
    Eq(Level, Rc<Term>, Rc<Term>, Rc<Term>),
    /// `@cast from to proof value`.
    Cast(Rc<Term>, Rc<Term>, Rc<Term>, Rc<Term>),
    /// `@Quot carrier relation`, at the carrier's sort; the proofs that the
    /// relation is an equivalence were checked and are not kept.
    Quot(Rc<Term>, Rc<Term>, Level),
    /// `@qin a`, the class of `a`.
    QIn(Rc<Term>),
    /// `@qelim motive method q`, with the motive's sort; the proof that the
    /// method respects the relation was checked and is not kept.
    QElim(Rc<Term>, Rc<Term>, Level, Rc<Term>),
    /// A proof, whose value is irrelevant.
    /// See Note [Proofs evaluate to an irrelevant witness].
    Irrelevant,
}

impl Term {
    pub fn app(f: Term, a: Term) -> Term {
        Term::App(Rc::new(f), Rc::new(a))
    }
}

// ## Printing

/// Prints a term, naming bound variables from `names` (outermost first).
pub struct TermDisplay<'a> {
    pub sig: &'a Signature,
    pub names: &'a [Name],
    pub level_names: &'a [Name],
    pub term: &'a Term,
}

/// How tightly a printed position binds, loosest first.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Precedence {
    Top,
    Arrow,
    Product,
    App,
    Atom,
}

struct Printer<'a> {
    sig: &'a Signature,
    names: Vec<Name>,
    level_names: &'a [Name],
}

impl Printer<'_> {
    fn name_at(&self, ix: Index) -> String {
        let depth = self.names.len();
        match depth.checked_sub(ix as usize + 1) {
            Some(level) => self.names[level].to_string(),
            None => format!("#{ix}"),
        }
    }

    fn level(&self, l: &Level) -> String {
        LevelDisplay { level: l, names: self.level_names }.to_string()
    }

    fn bind<R>(&mut self, name: &Name, k: impl FnOnce(&mut Self) -> R) -> R {
        self.names.push(name.clone());
        let r = k(self);
        self.names.pop();
        r
    }

    fn print(&mut self, out: &mut String, prec: Precedence, t: &Term) {
        let open = |out: &mut String, needs: Precedence| {
            if prec > needs {
                out.push('(');
            }
        };
        let close = |out: &mut String, needs: Precedence| {
            if prec > needs {
                out.push(')');
            }
        };
        match t {
            Term::Var(ix) => out.push_str(&self.name_at(*ix)),
            Term::Global(id, levels) => {
                let global = self.sig.global(*id);
                out.push_str(&global.name);
                if !levels.is_empty() {
                    out.push_str(" @ctx {");
                    for (i, (param, l)) in global.level_params.iter().zip(levels.iter()).enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&format!(".{param} = {}", self.level(l)));
                    }
                    out.push('}');
                }
            }
            Term::Sort(l) => {
                open(out, Precedence::App);
                out.push_str("@Type ");
                let shown = self.level(l);
                if shown.contains(' ') {
                    out.push_str(&format!("({shown})"));
                } else {
                    out.push_str(&shown);
                }
                close(out, Precedence::App);
            }
            Term::Pi(b) => self.binding(out, prec, b, " -> ", Precedence::Arrow),
            Term::Sigma(b) => self.binding(out, prec, b, " * ", Precedence::Product),
            Term::Lam(name, body) => {
                open(out, Precedence::Arrow);
                out.push_str(&format!("\\{name} => "));
                self.bind(name, |p| p.print(out, Precedence::Top, body));
                close(out, Precedence::Arrow);
            }
            Term::App(f, a) => {
                open(out, Precedence::App);
                self.print(out, Precedence::App, f);
                out.push(' ');
                self.print(out, Precedence::Atom, a);
                close(out, Precedence::App);
            }
            Term::Let(name, ty, val, body) => {
                open(out, Precedence::Arrow);
                out.push_str(&format!("let {name}: "));
                self.print(out, Precedence::Top, ty);
                out.push_str(" = ");
                self.print(out, Precedence::Top, val);
                out.push_str(" in ");
                self.bind(name, |p| p.print(out, Precedence::Top, body));
                close(out, Precedence::Arrow);
            }
            Term::Pair(a, b) => {
                out.push('(');
                self.print(out, Precedence::Top, a);
                out.push_str(", ");
                self.print(out, Precedence::Top, b);
                out.push(')');
            }
            Term::Fst(e) | Term::Snd(e) => {
                self.print(out, Precedence::Atom, e);
                out.push_str(if matches!(t, Term::Fst(_)) { ".1" } else { ".2" });
            }
            Term::Eq(_, _, a, b) => {
                open(out, Precedence::Product);
                self.print(out, Precedence::App, a);
                out.push_str(" ~ ");
                self.print(out, Precedence::App, b);
                close(out, Precedence::Product);
            }
            Term::Cast(from, to, proof, value) => {
                open(out, Precedence::App);
                out.push_str("@cast");
                for t in [from, to, proof, value] {
                    out.push(' ');
                    self.print(out, Precedence::Atom, t);
                }
                close(out, Precedence::App);
            }
            Term::Irrelevant => out.push_str("@irrelevant"),
            Term::Quot(a, r, _) => {
                open(out, Precedence::App);
                out.push_str("@Quot ");
                self.print(out, Precedence::Atom, a);
                out.push(' ');
                self.print(out, Precedence::Atom, r);
                close(out, Precedence::App);
            }
            Term::QIn(a) => {
                open(out, Precedence::App);
                out.push_str("@qin ");
                self.print(out, Precedence::Atom, a);
                close(out, Precedence::App);
            }
            Term::QElim(p, f, _, q) => {
                open(out, Precedence::App);
                out.push_str("@qelim");
                for t in [p, f, q] {
                    out.push(' ');
                    self.print(out, Precedence::Atom, t);
                }
                close(out, Precedence::App);
            }
            Term::Top => out.push_str("@Top"),
            Term::Tt => out.push_str("@tt"),
            Term::Bottom => out.push_str("@Bottom"),
            Term::Absurd(ty, e) => {
                open(out, Precedence::App);
                out.push_str("@absurd ");
                self.print(out, Precedence::Atom, ty);
                out.push(' ');
                self.print(out, Precedence::Atom, e);
                close(out, Precedence::App);
            }
        }
    }

    /// A Pi or Sigma type, written `(x: A) op B`, or `A op B` when `B` does
    /// not mention `x`.
    fn binding(&mut self, out: &mut String, prec: Precedence, b: &Binding, op: &str, own: Precedence) {
        if prec > own {
            out.push('(');
        }
        if mentions(&b.cod, 0) {
            out.push_str(&format!("({}: ", b.name));
            self.print(out, Precedence::Top, &b.dom);
            out.push(')');
        } else {
            let next = if own == Precedence::Arrow { Precedence::Product } else { Precedence::App };
            self.print(out, next, &b.dom);
        }
        out.push_str(op);
        self.bind(&b.name, |p| p.print(out, own, &b.cod));
        if prec > own {
            out.push(')');
        }
    }
}

/// `t` with every variable at index `cutoff` or above moved out by `by`:
/// the de Bruijn shift that makes room for `by` new binders below `cutoff`
/// bound variables.
pub fn shift(t: &Term, cutoff: Index, by: Index) -> Term {
    let go = |t: &Rc<Term>, cutoff| Rc::new(shift(t, cutoff, by));
    let binding = |b: &Binding| {
        Rc::new(Binding {
            name: b.name.clone(),
            dom: go(&b.dom, cutoff),
            cod: go(&b.cod, cutoff + 1),
            dom_level: b.dom_level.clone(),
            cod_level: b.cod_level.clone(),
        })
    };
    match t {
        Term::Var(i) if *i >= cutoff => Term::Var(i + by),
        Term::Var(_) | Term::Global(..) | Term::Sort(_) | Term::Top | Term::Tt | Term::Bottom | Term::Irrelevant => t.clone(),
        Term::Eq(l, ty, a, b) => Term::Eq(l.clone(), go(ty, cutoff), go(a, cutoff), go(b, cutoff)),
        Term::Cast(x, y, e, v) => Term::Cast(go(x, cutoff), go(y, cutoff), go(e, cutoff), go(v, cutoff)),
        Term::Quot(a, r, l) => Term::Quot(go(a, cutoff), go(r, cutoff), l.clone()),
        Term::QIn(a) => Term::QIn(go(a, cutoff)),
        Term::QElim(p, f, l, q) => Term::QElim(go(p, cutoff), go(f, cutoff), l.clone(), go(q, cutoff)),
        Term::Pi(b) => Term::Pi(binding(b)),
        Term::Sigma(b) => Term::Sigma(binding(b)),
        Term::Lam(name, body) => Term::Lam(name.clone(), go(body, cutoff + 1)),
        Term::App(f, a) => Term::App(go(f, cutoff), go(a, cutoff)),
        Term::Let(name, ty, val, body) => Term::Let(name.clone(), go(ty, cutoff), go(val, cutoff), go(body, cutoff + 1)),
        Term::Pair(a, b) => Term::Pair(go(a, cutoff), go(b, cutoff)),
        Term::Fst(e) => Term::Fst(go(e, cutoff)),
        Term::Snd(e) => Term::Snd(go(e, cutoff)),
        Term::Absurd(ty, e) => Term::Absurd(go(ty, cutoff), go(e, cutoff)),
    }
}

/// Whether global `id` occurs in `t`.
pub fn mentions_global(t: &Term, id: GlobalId) -> bool {
    match t {
        Term::Global(g, _) => *g == id,
        Term::Var(_) | Term::Sort(_) | Term::Top | Term::Tt | Term::Bottom | Term::Irrelevant => false,
        Term::Eq(_, ty, a, b) => [ty, a, b].iter().any(|t| mentions_global(t, id)),
        Term::Cast(x, y, e, v) => [x, y, e, v].iter().any(|t| mentions_global(t, id)),
        Term::Quot(a, r, _) => mentions_global(a, id) || mentions_global(r, id),
        Term::QIn(a) => mentions_global(a, id),
        Term::QElim(p, f, _, q) => [p, f, q].iter().any(|t| mentions_global(t, id)),
        Term::Pi(b) | Term::Sigma(b) => mentions_global(&b.dom, id) || mentions_global(&b.cod, id),
        Term::Lam(_, e) | Term::Fst(e) | Term::Snd(e) => mentions_global(e, id),
        Term::App(a, b) | Term::Pair(a, b) | Term::Absurd(a, b) => mentions_global(a, id) || mentions_global(b, id),
        Term::Let(_, ty, val, body) => mentions_global(ty, id) || mentions_global(val, id) || mentions_global(body, id),
    }
}

/// Whether variable `ix` occurs free in `t`.
pub fn mentions(t: &Term, ix: Index) -> bool {
    match t {
        Term::Var(i) => *i == ix,
        Term::Global(..) | Term::Sort(_) | Term::Top | Term::Tt | Term::Bottom | Term::Irrelevant => false,
        Term::Eq(_, ty, a, b) => [ty, a, b].iter().any(|t| mentions(t, ix)),
        Term::Cast(x, y, e, v) => [x, y, e, v].iter().any(|t| mentions(t, ix)),
        Term::Quot(a, r, _) => mentions(a, ix) || mentions(r, ix),
        Term::QIn(a) => mentions(a, ix),
        Term::QElim(p, f, _, q) => [p, f, q].iter().any(|t| mentions(t, ix)),
        Term::Pi(b) | Term::Sigma(b) => mentions(&b.dom, ix) || mentions(&b.cod, ix + 1),
        Term::Pair(a, b) | Term::Absurd(a, b) => mentions(a, ix) || mentions(b, ix),
        Term::Fst(e) | Term::Snd(e) => mentions(e, ix),
        Term::Lam(_, body) => mentions(body, ix + 1),
        Term::App(f, a) => mentions(f, ix) || mentions(a, ix),
        Term::Let(_, ty, val, body) => mentions(ty, ix) || mentions(val, ix) || mentions(body, ix + 1),
    }
}

impl fmt::Display for TermDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut printer = Printer {
            sig: self.sig,
            names: self.names.to_vec(),
            level_names: self.level_names,
        };
        let mut out = String::new();
        printer.print(&mut out, Precedence::Top, self.term);
        f.write_str(&out)
    }
}
