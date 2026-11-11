//! Inductive types: the types of their constructors and recursor, and the
//! recursor's computation rule.
//!
//! An inductive declaration `D (params) : @Type l` lists constructors whose
//! fields may mention the parameters, earlier fields, and `D params` itself
//! as a whole field (strict positivity, in the restricted form with no
//! recursive occurrence under a Pi). Indexed families are not primitive:
//! TT^obs encodes them by "fording", a parameter plus an equality field.
//!
//! The recursor follows Dybjer's schema ("Inductive families", FAC 1994), in
//! Lean's argument order:
//!
//! ```text
//! D.elim : @ctx {..., m: @l} -> (params) -> (P: D params -> @Type m)
//!        -> (method for each constructor) -> (x: D params) -> P x
//! ```
//!
//! where the method for `c (f1: A1) ... (fn: An)` takes the fields, then one
//! induction hypothesis `P fi` for each recursive field, and returns
//! `P (c params f1 ... fn)`. Iota reduction applies the method of a
//! constructor value to its fields and to the recursor's results on the
//! recursive ones.
//!
//! Input: a checked [`DataDecl`]. Output: core types for its globals, and
//! values for their applications.
//!
//! Entry points: [`constructor_type`], [`recursor_type`], [`recursor_value`].

use std::rc::Rc;

use crate::level::Level;
use crate::nbe::{apply, eval, Closure, Env, Neutral, Val};
use crate::signature::{DataDecl, DataId, Param, Signature};
use crate::term::{shift, Binding, Index, Name, Term};

// ## Types of the generated globals

/// The variable bound at de Bruijn level `level`, seen from `depth`.
fn var(depth: u32, level: u32) -> Term {
    assert!(level < depth, "variable level {level} escapes depth {depth}");
    Term::Var(depth - 1 - level)
}

fn pi(name: &Name, dom: Term, dom_level: Level, cod: Term, cod_level: Level) -> Term {
    Term::Pi(Rc::new(Binding {
        name: name.clone(),
        dom: Rc::new(dom),
        cod: Rc::new(cod),
        dom_level,
        cod_level,
    }))
}

/// Wrap `body` (of sort `level`) in Pi binders for `params`, innermost last.
fn pis(params: &[(Name, Term, Level)], body: Term, level: Level) -> (Term, Level) {
    params.iter().rev().fold((body, level), |(body, level), (name, ty, dom_level)| {
        let sort = Level::imax(dom_level, &level);
        (pi(name, ty.clone(), dom_level.clone(), body, level), sort)
    })
}

/// The levels a declaration's globals refer to themselves at: its parameters.
fn own_levels(count: usize) -> Rc<[Level]> {
    (0..count as u32).map(Level::Param).collect()
}

fn apply_all(head: Term, args: impl IntoIterator<Item = Term>) -> Term {
    args.into_iter().fold(head, Term::app)
}

fn telescope(params: &[Param]) -> Vec<(Name, Term, Level)> {
    params.iter().map(|p| (p.name.clone(), (*p.ty).clone(), p.level.clone())).collect()
}

/// `D params`, with the parameters at levels `0..params` seen from `depth`.
fn data_applied(decl: &DataDecl, levels: usize, depth: u32) -> Term {
    let params = (0..decl.params.len() as u32).map(|p| var(depth, p));
    apply_all(Term::Global(decl.global, own_levels(levels)), params)
}

// Rule [Data-Ctor]
/// `(params) -> (fields) -> D params`.
pub fn constructor_type(decl: &DataDecl, levels: usize, index: usize) -> Term {
    let ctor = &decl.constructors[index];
    let depth = (decl.params.len() + ctor.fields.len()) as u32;
    let (fields, sort) = pis(&telescope(&ctor.fields), data_applied(decl, levels, depth), decl.level.clone());
    pis(&telescope(&decl.params), fields, sort).0
}

// Algorithm [Recursor generation]
// Rule [Data-Elim]
/// The recursor's type; `levels` counts the declaration's own level
/// parameters, and the motive's sort is parameter `levels`.
pub fn recursor_type(decl: &DataDecl, levels: usize) -> Term {
    let np = decl.params.len() as u32;
    let k = decl.constructors.len() as u32;
    let m = Level::Param(levels as u32);
    let motive_name: Name = "P".into();
    let p_level = np;

    // `(x: D params) -> P x`, under the parameters, the motive and the methods.
    let target_depth = np + 1 + k;
    let x: Name = "x".into();
    let target = pi(
        &x,
        data_applied(decl, levels, target_depth),
        decl.level.clone(),
        Term::app(var(target_depth + 1, p_level), Term::Var(0)),
        m.clone(),
    );
    let mut rest = (target, Level::imax(&decl.level, &m));

    let methods: Vec<(Term, Level)> =
        (0..decl.constructors.len()).map(|i| method_type(decl, levels, i, &m)).collect();
    for (i, (method, method_level)) in methods.into_iter().enumerate().rev() {
        let name: Name = format!("case_{}", decl.constructors[i].name).into();
        let sort = Level::imax(&method_level, &rest.1);
        rest = (pi(&name, method, method_level, rest.0, rest.1), sort);
    }

    // `(P: D params -> @Type m) -> ...`.
    let motive_ty = pi(&x, data_applied(decl, levels, np), decl.level.clone(), Term::Sort(m.clone()), m.succ());
    let motive_level = Level::imax(&decl.level, &m.succ());
    let sort = Level::imax(&motive_level, &rest.1);
    let with_motive = (pi(&motive_name, motive_ty, motive_level, rest.0, rest.1), sort);
    pis(&telescope(&decl.params), with_motive.0, with_motive.1).0
}

/// The method for constructor `index`, in the context of the parameters,
/// the motive and the methods before it.
fn method_type(decl: &DataDecl, levels: usize, index: usize, m: &Level) -> (Term, Level) {
    let ctor = &decl.constructors[index];
    let np = decl.params.len() as u32;
    let outside = 1 + index as u32;
    let base = np + outside;
    let p_level = np;
    let mut binders: Vec<(Name, Term, Level)> = Vec::new();
    for (f, field) in ctor.fields.iter().enumerate() {
        // A field's type was checked over the parameters and earlier fields;
        // here the motive and earlier methods sit between them.
        binders.push((field.name.clone(), shift(&field.ty, f as Index, outside), field.level.clone()));
    }
    let nf = ctor.fields.len() as u32;
    for (f, _) in ctor.recursive.iter().enumerate().filter(|(_, r)| **r) {
        let depth = base + binders.len() as u32;
        let hypothesis = Term::app(var(depth, p_level), var(depth, base + f as u32));
        binders.push((format!("ih_{}", ctor.fields[f].name).into(), hypothesis, m.clone()));
    }
    let depth = base + binders.len() as u32;
    let params = (0..np).map(|p| var(depth, p));
    let fields = (0..nf).map(|f| var(depth, base + f));
    let value = apply_all(Term::Global(ctor.global, own_levels(levels)), params.chain(fields));
    let result = Term::app(var(depth, p_level), value);
    pis(&binders, result, m.clone())
}

// ## Values of the generated globals

/// A function of `names.len()` arguments that calls `done` once it has them
/// all, so a partly applied type former, constructor or recursor is an
/// ordinary lambda.
pub fn curry(names: Rc<[Name]>, collected: Vec<Val>, done: Rc<dyn Fn(&Signature, Vec<Val>) -> Val>, sig: &Signature) -> Val {
    if collected.len() == names.len() {
        return done(sig, collected);
    }
    let name = names[collected.len()].clone();
    Val::Lam(
        name,
        Closure::Native(Rc::new(move |sig, arg| {
            let mut collected = collected.clone();
            collected.push(arg);
            curry(names.clone(), collected, done.clone(), sig)
        })),
    )
}

/// An inductive type applied to its parameters.
pub struct DataVal {
    pub data: DataId,
    pub levels: Rc<[Level]>,
    pub params: Vec<Val>,
}

/// A constructor applied to the parameters and all its fields.
pub struct ConstructorVal {
    pub data: DataId,
    pub index: usize,
    pub levels: Rc<[Level]>,
    pub params: Vec<Val>,
    pub fields: Vec<Val>,
}

/// The recursor's arguments before the major premise: the parameters, the
/// motive and the methods.
pub struct RecursorArgs {
    pub data: DataId,
    pub levels: Rc<[Level]>,
    pub args: Vec<Val>,
}

impl RecursorArgs {
    pub fn motive(&self, sig: &Signature) -> &Val {
        &self.args[sig.data(self.data).params.len()]
    }
}

pub fn data_value(sig: &Signature, data: DataId, levels: Rc<[Level]>) -> Val {
    let names: Rc<[Name]> = sig.data(data).params.iter().map(|p| p.name.clone()).collect();
    let done = Rc::new(move |_: &Signature, params: Vec<Val>| {
        Val::Data(Rc::new(DataVal {
            data,
            levels: levels.clone(),
            params,
        }))
    });
    curry(names, Vec::new(), done, sig)
}

pub fn constructor_value(sig: &Signature, data: DataId, index: usize, levels: Rc<[Level]>) -> Val {
    let decl = sig.data(data);
    let np = decl.params.len();
    let fields = decl.constructors[index].fields.iter();
    let names: Rc<[Name]> = decl.params.iter().chain(fields).map(|p| p.name.clone()).collect();
    let done = Rc::new(move |_: &Signature, mut args: Vec<Val>| {
        let fields = args.split_off(np);
        Val::Constructor(Rc::new(ConstructorVal {
            data,
            index,
            levels: levels.clone(),
            params: args,
            fields,
        }))
    });
    curry(names, Vec::new(), done, sig)
}

pub fn recursor_value(sig: &Signature, data: DataId, levels: Rc<[Level]>) -> Val {
    let decl = sig.data(data);
    let mut names: Vec<Name> = decl.params.iter().map(|p| p.name.clone()).collect();
    names.push("P".into());
    names.extend(decl.constructors.iter().map(|c| Name::from(format!("case_{}", c.name))));
    names.push("x".into());
    let done = Rc::new(move |sig: &Signature, mut args: Vec<Val>| {
        let major = args.pop().expect("the recursor has a major premise");
        let rec = RecursorArgs {
            data,
            levels: levels.clone(),
            args,
        };
        iota(sig, Rc::new(rec), major)
    });
    curry(names.into(), Vec::new(), done, sig)
}

// Rule [Data-Iota]
/// The recursor applied to a major premise: the method of a constructor, or
/// stuck on a neutral.
pub fn iota(sig: &Signature, rec: Rc<RecursorArgs>, major: Val) -> Val {
    match major {
        Val::Constructor(c) => {
            assert_eq!(c.data, rec.data, "the recursor of one type met a constructor of another");
            let decl = sig.data(rec.data);
            let method = rec.args[decl.params.len() + 1 + c.index].clone();
            let applied = c.fields.iter().fold(method, |f, a| apply(sig, f, a.clone()));
            let recursive = decl.constructors[c.index].recursive.iter();
            c.fields.iter().zip(recursive).filter(|(_, r)| **r).fold(applied, |f, (field, _)| {
                let hypothesis = iota(sig, rec.clone(), field.clone());
                apply(sig, f, hypothesis)
            })
        }
        Val::Neutral(n) => Val::neutral(Neutral::Recursor(rec, n)),
        _ => unreachable!("the recursor's major premise is not a value of its type"),
    }
}

/// The type of field `index` of a constructor, given the parameters and the
/// fields before it.
pub fn field_type(sig: &Signature, c: &ConstructorVal, index: usize, earlier: &[Val]) -> Val {
    let decl = sig.data(c.data);
    let field = &decl.constructors[c.index].fields[index];
    let env = c.params.iter().chain(earlier).fold(Env::with_levels(c.levels.clone()), |env, v| env.extend(v.clone()));
    eval(sig, &env, &field.ty)
}

/// The type of parameter `index` of an inductive type, given the earlier ones.
pub fn param_type(sig: &Signature, d: &DataVal, index: usize) -> Val {
    let decl = sig.data(d.data);
    let env = d.params[..index].iter().fold(Env::with_levels(d.levels.clone()), |env, v| env.extend(v.clone()));
    eval(sig, &env, &decl.params[index].ty)
}

/// The sort of an inductive type at its levels.
pub fn data_level(sig: &Signature, d: &DataVal) -> Level {
    sig.data(d.data).level.instantiate(&d.levels)
}
