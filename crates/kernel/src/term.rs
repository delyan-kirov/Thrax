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
            Term::Pi(b) => {
                open(out, Precedence::Arrow);
                if mentions(&b.cod, 0) {
                    out.push_str(&format!("({}: ", b.name));
                    self.print(out, Precedence::Top, &b.dom);
                    out.push_str(") -> ");
                } else {
                    self.print(out, Precedence::App, &b.dom);
                    out.push_str(" -> ");
                }
                self.bind(&b.name, |p| p.print(out, Precedence::Arrow, &b.cod));
                close(out, Precedence::Arrow);
            }
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
        }
    }
}

/// Whether variable `ix` occurs free in `t`.
pub fn mentions(t: &Term, ix: Index) -> bool {
    match t {
        Term::Var(i) => *i == ix,
        Term::Global(..) | Term::Sort(_) => false,
        Term::Pi(b) => mentions(&b.dom, ix) || mentions(&b.cod, ix + 1),
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
