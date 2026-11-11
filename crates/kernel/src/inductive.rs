//! Inductive types: the types of their constructors and recursor, and the
//! recursor's computation rule.
//!
//! An inductive declaration `D (params) (indices) : @Type l` lists
//! constructors whose fields may mention the parameters, earlier fields, and
//! `D params ys` itself as a whole field (strict positivity, in the
//! restricted form with no recursive occurrence under a Pi). A constructor of
//! an indexed family states its result, `: D params ys`. Indexed families are
//! not primitive: they are stored forded.
//! See Note [Indexed families are forded].
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
use crate::observational::cast;
use crate::signature::{ConstructorDecl, DataDecl, DataId, Param, Signature};
use crate::term::{rename, shift, Binding, Index, Name, Term};

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
    data_applied_from(decl, levels, depth, 0)
}

/// `D params`, with the parameters at levels `first..first + params`.
fn data_applied_from(decl: &DataDecl, levels: usize, depth: u32, first: u32) -> Term {
    let params = (0..decl.params.len() as u32).map(|p| var(depth, first + p));
    apply_all(Term::Global(decl.global, own_levels(levels)), params)
}

// Rule [Data-Ctor]
/// `(params) -> (fields) -> D params indices`, over the uniform parameters
/// and the fields as written.
pub fn constructor_type(decl: &DataDecl, levels: usize, index: usize) -> Term {
    let ctor = &decl.constructors[index];
    let u = decl.uniform as u32;
    let depth = u + ctor.surface.len() as u32;
    let params = (0..u).map(|p| var(depth, p));
    let indices = ctor.indices.iter().map(|y| (**y).clone());
    let result = apply_all(Term::Global(decl.global, own_levels(levels)), params.chain(indices));
    let (fields, sort) = pis(&telescope(&ctor.surface), result, decl.level.clone());
    pis(&telescope(&decl.params[..decl.uniform]), fields, sort).0
}

/// The arguments after the uniform parameters in the type of recursive
/// field `field`: the indices it is at.
fn recursive_indices(decl: &DataDecl, ctor: &ConstructorDecl, field: usize) -> Vec<Term> {
    let mut args = Vec::new();
    let mut t = &*ctor.surface[field].ty;
    while let Term::App(f, a) = t {
        args.push((**a).clone());
        t = f;
    }
    args.reverse();
    args.split_off(decl.uniform)
}

/// Move `t`, a term over the uniform parameters and the first `fields`
/// fields, to depth `depth`, where the fields start at level `base`.
fn relocate(t: &Term, uniform: u32, fields: u32, base: u32, depth: u32) -> Term {
    rename(t, 0, &|v| {
        let level = if v < fields { base + fields - 1 - v } else { uniform - 1 - (v - fields) };
        depth - 1 - level
    })
}

// Rule [Data-Eq-Projection]
/// The projection out of `D a ~ D a'` for field `field` of constructor
/// `index`: the equality of that field's types, over the earlier fields at
/// `a` and their casts to `a'`.
/// See Note [Equality of two instances of one inductive type is stuck].
///
/// ```text
/// (a: params) -> (a': params) -> D a ~ D a' -> (b_0: B_0[a]) -> ...
///   -> B_j[a, b] ~ (let b'_0: B_0[a'] = @cast B_0[a] B_0[a'] _ b_0 in ... in B_j[a', b'])
/// ```
pub fn projection_type(decl: &DataDecl, levels: usize, index: usize, field: usize) -> Term {
    let ctor = &decl.constructors[index];
    let np = decl.params.len() as u32;
    let j = field as u32;
    let fields = &ctor.fields;

    // `B_k[a, b_<k]` right under the binders `b_<k`, where its parameters
    // skip `e` and `a'`; then moved out by `extra` binders.
    let at_a = |k: usize, extra: u32| shift(&shift(&fields[k].ty, k as Index, np + 1), 0, extra);
    // `B_k[a', b'_<k]` under the `k` lets that bind `b'_<k`, where its
    // parameters skip the `b`s and `e`.
    let at_a2 = |k: usize| shift(&fields[k].ty, k as Index, j + 1);

    let mut primed = at_a2(field);
    for k in (0..field).rev() {
        // Under `k` lets, field `b_t` of the outer binders is `j` further
        // out than right under them, and `b_k` itself is at `j - 1`.
        let from = at_a(k, j);
        let value = Term::Var(j - 1);
        let moved = Term::Cast(Rc::new(from), Rc::new(at_a2(k)), Rc::new(Term::Irrelevant), Rc::new(value));
        primed = Term::Let(format!("{}'", fields[k].name).into(), Rc::new(at_a2(k)), Rc::new(moved), Rc::new(primed));
    }
    let level = fields[field].level.clone();
    let goal = Term::Eq(level.succ(), Rc::new(Term::Sort(level)), Rc::new(at_a(field, 0)), Rc::new(primed));

    let earlier: Vec<(Name, Term, Level)> =
        (0..field).map(|k| (fields[k].name.clone(), at_a(k, 0), fields[k].level.clone())).collect();
    let (body, sort) = pis(&earlier, goal, Level::Zero);
    let depth = 2 * np;
    let same = Term::Eq(
        decl.level.succ(),
        Rc::new(Term::Sort(decl.level.clone())),
        Rc::new(data_applied_from(decl, levels, depth, 0)),
        Rc::new(data_applied_from(decl, levels, depth, np)),
    );
    let (body, sort) = pis(&[("e".into(), same, Level::Zero)], body, sort);
    let mut params = telescope(&decl.params);
    for p in telescope(&decl.params) {
        params.push((format!("{}'", p.0).into(), p.1, p.2));
    }
    pis(&params, body, sort).0
}

// Algorithm [Recursor generation]
// Rule [Data-Elim]
/// The recursor's type; `levels` counts the declaration's own level
/// parameters, and the motive's sort is parameter `levels`.
///
/// ```text
/// (params) -> (P: (indices) -> D params indices -> @Type m) -> (methods)
///   -> (indices) -> (x: D params indices) -> P indices x
/// ```
pub fn recursor_type(decl: &DataDecl, levels: usize) -> Term {
    let u = decl.uniform as u32;
    let ni = (decl.params.len() - decl.uniform) as u32;
    let k = decl.constructors.len() as u32;
    let m = Level::Param(levels as u32);
    // The major premise, named apart from the parameters and indices so the
    // type prints unambiguously.
    let mut x: Name = "x".into();
    while decl.params.iter().any(|p| p.name == x) {
        x = format!("{x}'").into();
    }
    let indices = &decl.params[decl.uniform..];

    // `(indices) -> (x: D params indices) -> P indices x`, under the
    // parameters, the motive and the methods.
    let outside = 1 + k;
    let base = u + outside;
    let depth = base + ni + 1;
    let p_var = |depth: u32| var(depth, u);
    let index_vars = |depth: u32| (0..ni).map(move |i| var(depth, base + i));
    let applied = apply_all(Term::app(apply_all(p_var(depth), index_vars(depth)), Term::Var(0)), []);
    let data = apply_all(
        Term::Global(decl.global, own_levels(levels)),
        (0..u).map(|p| var(depth - 1, p)).chain(index_vars(depth - 1)),
    );
    let mut rest = (pi(&x, data, decl.level.clone(), applied, m.clone()), Level::imax(&decl.level, &m));
    for (i, index) in indices.iter().enumerate().rev() {
        let ty = shift(&index.ty, i as Index, outside);
        let sort = Level::imax(&index.level, &rest.1);
        rest = (pi(&index.name, ty, index.level.clone(), rest.0, rest.1), sort);
    }

    let methods: Vec<(Term, Level)> =
        (0..decl.constructors.len()).map(|i| method_type(decl, levels, i, &m)).collect();
    for (i, (method, method_level)) in methods.into_iter().enumerate().rev() {
        let name: Name = format!("case_{}", decl.constructors[i].name).into();
        let sort = Level::imax(&method_level, &rest.1);
        rest = (pi(&name, method, method_level, rest.0, rest.1), sort);
    }

    // `(P: (indices) -> D params indices -> @Type m) -> ...`.
    let motive_target = pi(&x, data_applied(decl, levels, u + ni), decl.level.clone(), Term::Sort(m.clone()), m.succ());
    let (motive_ty, motive_level) = pis(&telescope(indices), motive_target, Level::imax(&decl.level, &m.succ()));
    let sort = Level::imax(&motive_level, &rest.1);
    let with_motive = (pi(&"P".into(), motive_ty, motive_level, rest.0, rest.1), sort);
    pis(&telescope(&decl.params[..decl.uniform]), with_motive.0, with_motive.1).0
}

/// The method for constructor `index`, in the context of the parameters,
/// the motive and the methods before it:
/// `(fields) -> (P ys f for each recursive field f: D params ys) -> P indices (c params fields)`.
fn method_type(decl: &DataDecl, levels: usize, index: usize, m: &Level) -> (Term, Level) {
    let ctor = &decl.constructors[index];
    let u = decl.uniform as u32;
    let outside = 1 + index as u32;
    let base = u + outside;
    let p_level = u;
    let mut binders: Vec<(Name, Term, Level)> = Vec::new();
    for (f, field) in ctor.surface.iter().enumerate() {
        // A field's type was checked over the parameters and earlier fields;
        // here the motive and earlier methods sit between them.
        binders.push((field.name.clone(), shift(&field.ty, f as Index, outside), field.level.clone()));
    }
    let nf = ctor.surface.len() as u32;
    for (f, _) in ctor.recursive.iter().take(ctor.surface.len()).enumerate().filter(|(_, r)| **r) {
        let depth = base + binders.len() as u32;
        let ys = recursive_indices(decl, ctor, f).into_iter().map(|y| relocate(&y, u, f as u32, base, depth));
        let hypothesis = Term::app(apply_all(var(depth, p_level), ys), var(depth, base + f as u32));
        binders.push((format!("ih_{}", ctor.surface[f].name).into(), hypothesis, m.clone()));
    }
    let depth = base + binders.len() as u32;
    let params = (0..u).map(|p| var(depth, p));
    let fields = (0..nf).map(|f| var(depth, base + f));
    let value = apply_all(Term::Global(ctor.global, own_levels(levels)), params.chain(fields));
    let ys = ctor.indices.iter().map(|y| relocate(y, u, nf, base, depth));
    let result = Term::app(apply_all(var(depth, p_level), ys), value);
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
        &self.args[sig.data(self.data).uniform]
    }

    /// The indices the major premise is at.
    pub fn indices(&self, sig: &Signature) -> &[Val] {
        let decl = sig.data(self.data);
        &self.args[decl.uniform + 1 + decl.constructors.len()..]
    }

    /// `P indices major`, the type of the recursor's result.
    pub fn result_type(&self, sig: &Signature, major: Val) -> Val {
        let at = self.indices(sig).iter().fold(self.motive(sig).clone(), |f, i| apply(sig, f, i.clone()));
        apply(sig, at, major)
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
    let u = decl.uniform;
    let fields = decl.constructors[index].surface.iter();
    let names: Rc<[Name]> = decl.params[..u].iter().chain(fields).map(|p| p.name.clone()).collect();
    let done = Rc::new(move |sig: &Signature, mut args: Vec<Val>| {
        let fields = args.split_off(u);
        Val::Constructor(Rc::new(canonical(sig, data, index, levels.clone(), args, fields)))
    });
    curry(names, Vec::new(), done, sig)
}

/// Constructor `index` at the uniform parameters `params` and its written
/// `fields`, at the indices its result states: the forded value, with the
/// irrelevant witness for each index equality.
pub fn canonical(sig: &Signature, data: DataId, index: usize, levels: Rc<[Level]>, params: Vec<Val>, fields: Vec<Val>) -> ConstructorVal {
    let decl = sig.data(data);
    let ctor = &decl.constructors[index];
    let env = params.iter().chain(&fields).fold(Env::with_levels(levels.clone()), |env, v| env.extend(v.clone()));
    let mut all_params = params;
    all_params.extend(ctor.indices.iter().map(|y| eval(sig, &env, y)));
    let mut all_fields = fields;
    all_fields.extend(ctor.indices.iter().map(|_| Val::Irrelevant));
    ConstructorVal {
        data,
        index,
        levels,
        params: all_params,
        fields: all_fields,
    }
}

pub fn recursor_value(sig: &Signature, data: DataId, levels: Rc<[Level]>) -> Val {
    let decl = sig.data(data);
    let mut names: Vec<Name> = decl.params[..decl.uniform].iter().map(|p| p.name.clone()).collect();
    names.push("P".into());
    names.extend(decl.constructors.iter().map(|c| Name::from(format!("case_{}", c.name))));
    names.extend(decl.params[decl.uniform..].iter().map(|p| p.name.clone()));
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
/// stuck on a neutral. For an indexed family the method's result is at the
/// constructor's own indices, and is cast to the major premise's.
/// See Note [Indexed families are forded].
pub fn iota(sig: &Signature, rec: Rc<RecursorArgs>, major: Val) -> Val {
    match major {
        Val::Constructor(c) => {
            assert_eq!(c.data, rec.data, "the recursor of one type met a constructor of another");
            let decl = sig.data(rec.data);
            let ctor = &decl.constructors[c.index];
            let (u, nf) = (decl.uniform, ctor.surface.len());
            let fields = &c.fields[..nf];
            let method = rec.args[u + 1 + c.index].clone();
            let applied = fields.iter().fold(method, |f, a| apply(sig, f, a.clone()));
            let methods = &rec.args[..u + 1 + decl.constructors.len()];
            let result = (0..nf).filter(|f| ctor.recursive[*f]).fold(applied, |f, r| {
                let env = c.params[..u].iter().chain(&fields[..r]).fold(Env::with_levels(c.levels.clone()), |env, v| env.extend(v.clone()));
                let mut args = methods.to_vec();
                args.extend(recursive_indices(decl, ctor, r).iter().map(|y| eval(sig, &env, y)));
                let inner = RecursorArgs { data: rec.data, levels: rec.levels.clone(), args };
                apply(sig, f, iota(sig, Rc::new(inner), fields[r].clone()))
            });
            if decl.uniform == decl.params.len() {
                return result;
            }
            // Rule [Data-Ford]
            let own = canonical(sig, c.data, c.index, c.levels.clone(), c.params[..u].to_vec(), fields.to_vec());
            let own_rec = RecursorArgs { data: rec.data, levels: rec.levels.clone(), args: methods.iter().cloned().chain(own.params[u..].iter().cloned()).collect() };
            let from = own_rec.result_type(sig, Val::Constructor(Rc::new(own)));
            let to = rec.result_type(sig, Val::Constructor(c.clone()));
            cast(sig, from, to, Val::Irrelevant, result)
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

// Note [Indexed families are forded]
//
// An indexed family is elaborated as CIC^obs does it (Pujet, Leray and
// Tabareau, TOPLAS 2025, Section 4.2): its indices become parameters that
// the constructors do not fix, and each constructor gets one more field per
// index, the equality of the index its result states with the actual one.
// `cons (m: Nat) (h: a) (t: Vec a m) : Vec a (succ m)` is stored as
// `cons (m: Nat) (h: a) (t: Vec a m) (n_eq: succ m ~ n)` of `Vec a n`.
//
// The user sees the family as written. The constructor global takes the
// fields as written and fills in the index its result states, with the
// irrelevant witness for the equality (`canonical`). The recursor has the
// indexed motive `(indices) -> D params indices -> @Type m`, and its method
// for a constructor returns `P ys (c params fields)` at the constructor's own
// indices `ys`; iota casts that to `P xs v` at the value's indices `xs`.
// For a canonical value the two types are convertible, so Cast-Refl makes
// the cast invisible, which is why CIC^obs needs Cast-Refl in conversion
// before fording computes. A value cast to indices that are only
// propositionally equal is a constructor at other indices; quote prints it as
// that cast of the constructor.
//
// Index types may depend on the parameters but not on earlier indices, so
// each equality field is homogeneous; and an index must be data, since
// equality is not formed at a proposition.
// See Rule [Data-Ford].
