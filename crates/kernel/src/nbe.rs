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

use crate::inductive::{canonical, constructor_value, data_value, recursor_value, ConstructorVal, DataVal, RecursorArgs};
use crate::level::Level;
use crate::observational::{cast, eq};
use crate::signature::{GlobalId, GlobalKind, Signature};
use crate::term::{Binding, Index, Name, Term};

/// A de Bruijn level: the number of binders outside a variable's own.
pub type Lvl = u32;

// ## Values

#[derive(Clone)]
pub enum Val {
    Neutral(Rc<Neutral>),
    Sort(Level),
    Pi(Rc<BindingVal>),
    Lam(Name, Closure),
    Sigma(Rc<BindingVal>),
    Pair(Rc<(Val, Val)>),
    Top,
    Tt,
    Bottom,
    Data(Rc<DataVal>),
    Constructor(Rc<ConstructorVal>),
    /// Any proof. See Note [Proofs evaluate to an irrelevant witness].
    Irrelevant,
    Quot(Rc<QuotVal>),
    QIn(Rc<Val>),
}

/// A quotient type: a carrier in `@Type level` and an equivalence relation.
pub struct QuotVal {
    pub carrier: Val,
    pub relation: Val,
    pub level: Level,
}

/// A Pi or Sigma type's value, with the sorts of both sides.
pub struct BindingVal {
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
    Fst(Rc<Neutral>),
    Snd(Rc<Neutral>),
    /// `@absurd ty proof`, stuck because no proof of `@Bottom` is canonical.
    Absurd(Val, Val),
    /// A recursor stuck on its major premise.
    Recursor(Rc<RecursorArgs>, Rc<Neutral>),
    /// An equality whose type or sides have no head its rules know.
    Eq { level: Level, ty: Val, a: Val, b: Val },
    /// A cast between types whose heads differ or are neutral.
    Cast { from: Val, to: Val, proof: Val, value: Val },
    /// A quotient eliminator stuck on its argument.
    QElim { motive: Val, method: Val, level: Level, quotient: Rc<Neutral> },
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

    pub fn with_levels(levels: Rc<[Level]>) -> Env {
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

// Algorithm [Normalisation by evaluation]
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
        // Rule [Let]
        Term::Let(_, _, val, body) => eval(sig, &env.extend(eval(sig, env, val)), body),
        Term::Sigma(b) => Val::Sigma(Rc::new(eval_binding(sig, env, b))),
        Term::Pair(a, b) => Val::Pair(Rc::new((eval(sig, env, a), eval(sig, env, b)))),
        Term::Fst(e) => fst(eval(sig, env, e)),
        Term::Snd(e) => snd(eval(sig, env, e)),
        Term::Top => Val::Top,
        Term::Tt => Val::Tt,
        Term::Bottom => Val::Bottom,
        Term::Absurd(ty, e) => Val::neutral(Neutral::Absurd(eval(sig, env, ty), eval(sig, env, e))),
        Term::Eq(level, ty, a, b) => eq(sig, env.level(level), eval(sig, env, ty), eval(sig, env, a), eval(sig, env, b)),
        Term::Cast(from, to, proof, value) => {
            let (from, to) = (eval(sig, env, from), eval(sig, env, to));
            cast(sig, from, to, eval(sig, env, proof), eval(sig, env, value))
        }
        // Rule [Proof-Witness]
        Term::Irrelevant => Val::Irrelevant,
        Term::Quot(a, r, level) => Val::Quot(Rc::new(QuotVal {
            carrier: eval(sig, env, a),
            relation: eval(sig, env, r),
            level: env.level(level),
        })),
        Term::QIn(a) => Val::QIn(Rc::new(eval(sig, env, a))),
        Term::QElim(p, f, level, q) => {
            let (motive, method) = (eval(sig, env, p), eval(sig, env, f));
            qelim(sig, motive, method, env.level(level), eval(sig, env, q))
        }
    }
}

fn eval_binding(sig: &Signature, env: &Env, b: &Binding) -> BindingVal {
    BindingVal {
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
        GlobalKind::Data(data) => data_value(sig, *data, levels),
        GlobalKind::Constructor(data, index) => constructor_value(sig, *data, *index, levels),
        GlobalKind::Recursor(data) => recursor_value(sig, *data, levels),
        GlobalKind::Projection(..) => Val::Irrelevant,
    }
}

/// A global's type at the given levels.
pub fn global_type(sig: &Signature, id: GlobalId, levels: Rc<[Level]>) -> Val {
    eval(sig, &Env::with_levels(levels), &sig.global(id).ty)
}

pub fn apply(sig: &Signature, f: Val, a: Val) -> Val {
    match f {
        // Rule [Beta]
        Val::Lam(_, body) => body.apply(sig, a),
        Val::Neutral(n) => Val::neutral(Neutral::App(n, a)),
        Val::Irrelevant => Val::Irrelevant,
        _ => unreachable!("applied a non-function: the checker let an ill-typed term through"),
    }
}

/// `@qelim motive method q`: the method on a class's representative.
pub fn qelim(sig: &Signature, motive: Val, method: Val, level: Level, q: Val) -> Val {
    match q {
        // Rule [Quot-Beta]
        Val::QIn(a) => apply(sig, method, (*a).clone()),
        Val::Neutral(n) => Val::neutral(Neutral::QElim { motive, method, level, quotient: n }),
        _ => unreachable!("eliminated a non-quotient: the checker let an ill-typed term through"),
    }
}

// Rule [Sigma-Pair]
pub fn fst(p: Val) -> Val {
    match p {
        Val::Pair(pair) => pair.0.clone(),
        Val::Neutral(n) => Val::neutral(Neutral::Fst(n)),
        Val::Irrelevant => Val::Irrelevant,
        _ => unreachable!("projected from a non-pair: the checker let an ill-typed term through"),
    }
}

pub fn snd(p: Val) -> Val {
    match p {
        Val::Pair(pair) => pair.1.clone(),
        Val::Neutral(n) => Val::neutral(Neutral::Snd(n)),
        Val::Irrelevant => Val::Irrelevant,
        _ => unreachable!("projected from a non-pair: the checker let an ill-typed term through"),
    }
}

// ## Read-back

/// Read a value back to a term in a context of `depth` variables.
pub fn quote(sig: &Signature, depth: Lvl, v: &Val) -> Term {
    match v {
        Val::Neutral(n) => quote_neutral(sig, depth, n),
        Val::Sort(l) => Term::Sort(l.clone()),
        Val::Pi(p) => Term::Pi(Rc::new(quote_binding(sig, depth, p))),
        Val::Lam(name, body) => Term::Lam(name.clone(), Rc::new(quote_closure(sig, depth, body))),
        Val::Sigma(p) => Term::Sigma(Rc::new(quote_binding(sig, depth, p))),
        Val::Pair(pair) => Term::Pair(Rc::new(quote(sig, depth, &pair.0)), Rc::new(quote(sig, depth, &pair.1))),
        Val::Top => Term::Top,
        Val::Tt => Term::Tt,
        Val::Bottom => Term::Bottom,
        Val::Irrelevant => Term::Irrelevant,
        Val::Quot(q) => Term::Quot(Rc::new(quote(sig, depth, &q.carrier)), Rc::new(quote(sig, depth, &q.relation)), q.level.clone()),
        Val::QIn(a) => Term::QIn(Rc::new(quote(sig, depth, a))),
        Val::Data(d) => {
            let head = Term::Global(sig.data(d.data).global, d.levels.clone());
            quote_spine(sig, depth, head, &d.params)
        }
        Val::Constructor(c) => {
            // The constructor as written: the uniform parameters and its
            // fields, at the indices it states, cast to the value's own
            // indices when they differ.
            // See Note [Indexed families are forded].
            let decl = sig.data(c.data);
            let ctor = &decl.constructors[c.index];
            let (u, nf) = (decl.uniform, ctor.surface.len());
            let head = Term::Global(ctor.global, c.levels.clone());
            let written = quote_spine(sig, depth, quote_spine(sig, depth, head, &c.params[..u]), &c.fields[..nf]);
            if u == decl.params.len() {
                return written;
            }
            let own = canonical(sig, c.data, c.index, c.levels.clone(), c.params[..u].to_vec(), c.fields[..nf].to_vec());
            let at = |params: &[Val]| quote_spine(sig, depth, Term::Global(decl.global, c.levels.clone()), params);
            let (from, to) = (at(&own.params), at(&c.params));
            if from == to {
                written
            } else {
                Term::Cast(Rc::new(from), Rc::new(to), Rc::new(Term::Irrelevant), Rc::new(written))
            }
        }
    }
}

fn quote_spine(sig: &Signature, depth: Lvl, head: Term, args: &[Val]) -> Term {
    args.iter().fold(head, |f, a| Term::app(f, quote(sig, depth, a)))
}

fn quote_binding(sig: &Signature, depth: Lvl, p: &BindingVal) -> Binding {
    Binding {
        name: p.name.clone(),
        dom: Rc::new(quote(sig, depth, &p.dom)),
        cod: Rc::new(quote_closure(sig, depth, &p.cod)),
        dom_level: p.dom_level.clone(),
        cod_level: p.cod_level.clone(),
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
        Neutral::Fst(p) => Term::Fst(Rc::new(quote_neutral(sig, depth, p))),
        Neutral::Snd(p) => Term::Snd(Rc::new(quote_neutral(sig, depth, p))),
        Neutral::Absurd(ty, e) => Term::Absurd(Rc::new(quote(sig, depth, ty)), Rc::new(quote(sig, depth, e))),
        Neutral::Recursor(rec, major) => {
            let head = Term::Global(sig.data(rec.data).recursor, rec.levels.clone());
            Term::app(quote_spine(sig, depth, head, &rec.args), quote_neutral(sig, depth, major))
        }
        Neutral::Eq { level, ty, a, b } => {
            let q = |v| Rc::new(quote(sig, depth, v));
            Term::Eq(level.clone(), q(ty), q(a), q(b))
        }
        Neutral::Cast { from, to, proof, value } => {
            let q = |v| Rc::new(quote(sig, depth, v));
            Term::Cast(q(from), q(to), q(proof), q(value))
        }
        Neutral::QElim { motive, method, level, quotient } => {
            let q = |v| Rc::new(quote(sig, depth, v));
            Term::QElim(q(motive), q(method), level.clone(), Rc::new(quote_neutral(sig, depth, quotient)))
        }
    }
}
