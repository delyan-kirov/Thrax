//! Normalisation by evaluation: terms evaluate to values in weak head normal
//! form, and values read back to terms.
//!
//! The algorithm is normalisation by evaluation (Berger and Schwichtenberg,
//! "An inverse of the evaluation functional for typed lambda-calculus", LICS
//! 1991), in the untyped, environment-machine form of Coquand's type checker
//! ("An algorithm for type-checking dependent types", SCP 1996). A binder's
//! body is a [`Closure`]: the term and the environment it was evaluated in,
//! or a Rust function when a computation rule builds the binder itself. A
//! variable that is not bound to a value is a de Bruijn *level* (0 is the
//! outermost binder), so values never need shifting. Elimination of a
//! variable is a [`Neutral`].
//!
//! [`eval`] substitutes the level arguments of the global being evaluated as
//! it goes, so a value's levels are always closed over the parameters of the
//! global being checked.
//!
//! Input: a term and an environment. Output: its value; [`quote`] reads a
//! value back to a term at a context depth.
//!
//! Entry points: [`eval`], [`apply`], [`quote`].

use std::rc::Rc;

use crate::level::Level;
use crate::signature::{GlobalId, GlobalKind, Signature};
use crate::term::{Binding, Index, Name, Term};

/// A de Bruijn level: the number of binders outside a variable's own.
pub type Lvl = u32;

// ## Values

#[derive(Clone)]
pub enum Val {
    Neutral(Rc<Neutral>),
    Sort(Level),
    Pi(Rc<PiVal>),
    Lam(Name, Closure),
}

/// A Pi type's value: `(name: dom) -> cod`, with the sorts of both sides.
pub struct PiVal {
    pub name: Name,
    pub dom: Val,
    pub cod: Closure,
    pub dom_level: Level,
    pub cod_level: Level,
}

/// An elimination stuck on a variable or an axiom.
pub enum Neutral {
    Var(Lvl),
    Axiom(GlobalId, Rc<[Level]>),
    App(Rc<Neutral>, Val),
}

/// A binder's body, waiting for the value of its variable.
#[derive(Clone)]
pub enum Closure {
    Term(Env, Rc<Term>),
    Native(Rc<dyn Fn(&Signature, Val) -> Val>),
}

impl Closure {
    pub fn apply(&self, sig: &Signature, arg: Val) -> Val {
        match self {
            Closure::Term(env, body) => eval(sig, &env.extend(arg), body),
            Closure::Native(f) => f(sig, arg),
        }
    }
}

impl Val {
    pub fn var(level: Lvl) -> Val {
        Val::Neutral(Rc::new(Neutral::Var(level)))
    }

    pub fn neutral(n: Neutral) -> Val {
        Val::Neutral(Rc::new(n))
    }
}

// ## Environments

/// The values of the bound variables, innermost first, and the level
/// arguments of the global being evaluated (`None` while checking a global,
/// whose parameters stay parameters).
#[derive(Clone, Default)]
pub struct Env {
    values: Option<Rc<EnvNode>>,
    levels: Option<Rc<[Level]>>,
}

struct EnvNode {
    head: Val,
    tail: Option<Rc<EnvNode>>,
}

impl Env {
    pub fn new() -> Env {
        Env::default()
    }

    fn with_levels(levels: Rc<[Level]>) -> Env {
        Env {
            values: None,
            levels: Some(levels),
        }
    }

    pub fn extend(&self, v: Val) -> Env {
        Env {
            values: Some(Rc::new(EnvNode {
                head: v,
                tail: self.values.clone(),
            })),
            levels: self.levels.clone(),
        }
    }

    fn lookup(&self, ix: Index) -> Val {
        let mut node = self.values.as_deref();
        for _ in 0..ix {
            node = node.expect("de Bruijn index out of scope").tail.as_deref();
        }
        node.expect("de Bruijn index out of scope").head.clone()
    }

    fn level(&self, l: &Level) -> Level {
        match &self.levels {
            Some(args) => l.instantiate(args),
            None => l.clone(),
        }
    }
}

// ## Evaluation

pub fn eval(sig: &Signature, env: &Env, t: &Term) -> Val {
    match t {
        Term::Var(ix) => env.lookup(*ix),
        Term::Global(id, levels) => {
            let levels: Rc<[Level]> = levels.iter().map(|l| env.level(l)).collect();
            eval_global(sig, *id, levels)
        }
        Term::Sort(l) => Val::Sort(env.level(l)),
        Term::Pi(b) => Val::Pi(Rc::new(eval_binding(sig, env, b))),
        Term::Lam(name, body) => Val::Lam(name.clone(), Closure::Term(env.clone(), body.clone())),
        Term::App(f, a) => apply(sig, eval(sig, env, f), eval(sig, env, a)),
        Term::Let(_, _, val, body) => eval(sig, &env.extend(eval(sig, env, val)), body),
    }
}

fn eval_binding(sig: &Signature, env: &Env, b: &Binding) -> PiVal {
    PiVal {
        name: b.name.clone(),
        dom: eval(sig, env, &b.dom),
        cod: Closure::Term(env.clone(), b.cod.clone()),
        dom_level: env.level(&b.dom_level),
        cod_level: env.level(&b.cod_level),
    }
}

/// A global's value at the given levels.
pub fn eval_global(sig: &Signature, id: GlobalId, levels: Rc<[Level]>) -> Val {
    let global = sig.global(id);
    assert_eq!(global.level_params.len(), levels.len(), "`{}` instantiated at the wrong number of levels", global.name);
    match &global.kind {
        GlobalKind::Definition(body) => eval(sig, &Env::with_levels(levels), body),
        GlobalKind::Axiom => Val::neutral(Neutral::Axiom(id, levels)),
    }
}

/// A global's type at the given levels.
pub fn global_type(sig: &Signature, id: GlobalId, levels: Rc<[Level]>) -> Val {
    eval(sig, &Env::with_levels(levels), &sig.global(id).ty)
}

pub fn apply(sig: &Signature, f: Val, a: Val) -> Val {
    match f {
        Val::Lam(_, body) => body.apply(sig, a),
        Val::Neutral(n) => Val::neutral(Neutral::App(n, a)),
        Val::Sort(_) | Val::Pi(_) => unreachable!("applied a type: the checker let an ill-typed term through"),
    }
}

// ## Read-back

/// Read a value back to a term in a context of `depth` variables.
pub fn quote(sig: &Signature, depth: Lvl, v: &Val) -> Term {
    match v {
        Val::Neutral(n) => quote_neutral(sig, depth, n),
        Val::Sort(l) => Term::Sort(l.clone()),
        Val::Pi(p) => Term::Pi(Rc::new(Binding {
            name: p.name.clone(),
            dom: Rc::new(quote(sig, depth, &p.dom)),
            cod: Rc::new(quote_closure(sig, depth, &p.cod)),
            dom_level: p.dom_level.clone(),
            cod_level: p.cod_level.clone(),
        })),
        Val::Lam(name, body) => Term::Lam(name.clone(), Rc::new(quote_closure(sig, depth, body))),
    }
}

/// Read a closure back to its body, under one more binder.
pub fn quote_closure(sig: &Signature, depth: Lvl, c: &Closure) -> Term {
    quote(sig, depth + 1, &c.apply(sig, Val::var(depth)))
}

fn quote_neutral(sig: &Signature, depth: Lvl, n: &Neutral) -> Term {
    match n {
        Neutral::Var(level) => {
            assert!(*level < depth, "variable level {level} escapes a context of depth {depth}");
            Term::Var(depth - 1 - level)
        }
        Neutral::Axiom(id, levels) => Term::Global(*id, levels.clone()),
        Neutral::App(f, a) => Term::app(quote_neutral(sig, depth, f), quote(sig, depth, a)),
    }
}
