//! The kernel's type checker: bidirectional elaboration of script syntax into
//! core terms.
//!
//! The algorithm is bidirectional type checking (Pierce and Turner, "Local
//! type inference", TOPLAS 2000; Dunfield and Krishnaswami, "Bidirectional
//! typing", CSUR 2021) over normalisation-by-evaluation values, in the shape
//! of Coquand's checker for dependent types. `check` pushes an
//! expected type into lambdas; `infer` synthesises a type at
//! variables, applications and annotations, and a synthesised type fits an
//! expected one when the two are convertible. Names are resolved as they are
//! checked, so the output is a de Bruijn [`Term`].
//!
//! Each global is checked in order and added to the [`Signature`]: its level
//! parameters first, then its type, then its body against that type.
//!
//! Input: a script. Output: the signature of its globals, or the first error.
//!
//! Entry point: [`check_script`].

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use utilities::error::closest;
use utilities::{Code, Diagnostic, Result, Span};

use crate::conversion::ConversionChecker;
use crate::level::Level;
use crate::nbe::{apply, eval, fst, global_type, quote, BindingVal, Closure, Env, Lvl, Neutral, QuotVal, Val};
use crate::observational::{cast, eq, native_pi, relation_type};
use crate::inductive::{constructor_type, recursor_type};
use crate::signature::{ConstructorDecl, DataDecl, DataId, Global, GlobalId, GlobalKind, Param, Signature};
use crate::syntax::{error, parse_script, Item, ItemBody, LevelArg, Primitive, Raw, RawConstructor, RawKind, RawLevel};
use crate::term::{mentions_global, Binding, Name, Term, TermDisplay};

/// Check every global of a script.
pub fn check_script(src: &str) -> Result<Signature> {
    let mut checker = Checker {
        src,
        sig: Signature::new(),
        uses: RefCell::default(),
    };
    for item in parse_script(src)? {
        checker.check_item(&item)?;
    }
    Ok(checker.sig)
}

struct Checker<'s> {
    src: &'s str,
    sig: Signature,
    /// The globals resolved since the last [`Checker::take_uses`].
    /// See Note [Axioms are collected before proofs are erased].
    uses: RefCell<BTreeSet<GlobalId>>,
}

/// The variables in scope, outermost first: their names and types, and the
/// environment that evaluates a term in that scope.
struct Context {
    names: Vec<Name>,
    types: Vec<Val>,
    env: Env,
    level_names: Vec<Name>,
}

impl Context {
    fn new(level_names: Vec<Name>) -> Context {
        Context {
            names: Vec::new(),
            types: Vec::new(),
            env: Env::new(),
            level_names,
        }
    }

    fn depth(&self) -> Lvl {
        self.types.len() as Lvl
    }

    /// Bind a variable of type `ty` with no value.
    fn bind(&mut self, name: Name, ty: Val) {
        self.env = self.env.extend(Val::var(self.depth()));
        self.names.push(name);
        self.types.push(ty);
    }

    /// Bind a variable of type `ty` that stands for `val`.
    fn define(&mut self, name: Name, ty: Val, val: Val) {
        self.env = self.env.extend(val);
        self.names.push(name);
        self.types.push(ty);
    }
}

// ## Globals

impl Checker<'_> {
    fn error(&self, code: Code, span: Span, msg: impl Into<String>) -> Diagnostic {
        error(self.src, code, span, msg)
    }

    fn take_uses(&self) -> Vec<GlobalId> {
        std::mem::take(&mut *self.uses.borrow_mut()).into_iter().collect()
    }

    fn check_item(&mut self, item: &Item) -> Result<()> {
        self.take_uses();
        if self.sig.lookup(&item.name).is_some() {
            return Err(self.error(Code::DuplicateSymbol, item.span, format!("`{}` is already defined", item.name)));
        }
        let mut level_names: Vec<Name> = Vec::new();
        for (name, span) in &item.level_params {
            if level_names.contains(name) {
                return Err(self.error(Code::DuplicateSymbol, *span, format!("level `{name}` is bound twice")));
            }
            level_names.push(name.clone());
        }
        if let ItemBody::Data(ctors) = &item.body {
            return self.check_data(item, level_names, ctors);
        }
        let mut ctx = Context::new(level_names.clone());
        let (ty, _) = self.check_type(&mut ctx, &item.ty)?;
        let kind = match &item.body {
            ItemBody::Definition(body) => {
                let expected = eval(&self.sig, &ctx.env, &ty);
                GlobalKind::Definition(Rc::new(self.check(&mut ctx, body, &expected)?))
            }
            ItemBody::Axiom => GlobalKind::Axiom,
            ItemBody::Data(_) => unreachable!("handled above"),
        };
        self.sig.add(Global {
            name: item.name.clone(),
            level_params: level_names,
            ty: Rc::new(ty),
            kind,
            uses: self.take_uses(),
        });
        Ok(())
    }
}

// ## Inductive declarations

impl Checker<'_> {
    /// Reject a name a declaration would introduce twice.
    fn fresh_global(&self, name: &str, span: Span, taken: &[Name]) -> Result<()> {
        if self.sig.lookup(name).is_some() || taken.iter().any(|t| &**t == name) {
            return Err(self.error(Code::DuplicateSymbol, span, format!("`{name}` is already defined")));
        }
        Ok(())
    }

    // Rule [Data-Form]
    fn check_data(&mut self, item: &Item, level_names: Vec<Name>, ctors: &[RawConstructor]) -> Result<()> {
        let mut ctx = Context::new(level_names.clone());
        let (ty, _) = self.check_type(&mut ctx, &item.ty)?;
        let mut params = Vec::new();
        let mut result = &ty;
        while let Term::Pi(b) = result {
            params.push(Param {
                name: b.name.clone(),
                ty: b.dom.clone(),
                level: b.dom_level.clone(),
            });
            result = &b.cod;
        }
        let Term::Sort(level) = result.clone() else {
            return Err(self.error(
                Code::Unsupported,
                item.ty.span,
                "an inductive type's signature must end in `@Type l` after its parameters",
            )
            .with_note("indexed families are not primitive: give the index as a parameter and an equality field"));
        };
        if !level.is_nonzero() {
            return Err(self.error(
                Code::Unsupported,
                item.ty.span,
                "inductive propositions are not supported: the type must be in `@Type l` with `l >= 1`",
            ));
        }
        let recursor_name: Name = format!("{}.elim", item.name).into();
        let mut taken = vec![item.name.clone(), recursor_name.clone()];
        for c in ctors {
            self.fresh_global(&c.name, c.span, &taken)?;
            taken.push(c.name.clone());
        }
        self.fresh_global(&recursor_name, item.span, &[])?;

        let data = self.sig.add_data(DataDecl {
            global: self.sig.next_global(),
            params,
            level: level.clone(),
            constructors: Vec::new(),
            recursor: GlobalId::MAX,
        });
        let former = self.sig.add(Global {
            name: item.name.clone(),
            level_params: level_names.clone(),
            ty: Rc::new(ty),
            kind: GlobalKind::Data(data),
            uses: self.take_uses(),
        });
        let mut constructors = Vec::new();
        for (index, c) in ctors.iter().enumerate() {
            let mut decl = self.check_constructor(data, &level_names, &level, c)?;
            decl.global = self.sig.next_global();
            self.sig.data_mut(data).constructors.push(decl);
            let ty = constructor_type(self.sig.data(data), level_names.len(), index);
            let mut uses = self.take_uses();
            uses.retain(|g| *g != former);
            uses.push(former);
            constructors.push(self.sig.add(Global {
                name: c.name.clone(),
                level_params: level_names.clone(),
                ty: Rc::new(ty),
                kind: GlobalKind::Constructor(data, index),
                uses,
            }));
        }

        let mut recursor_levels = level_names.clone();
        let mut motive: Name = "m".into();
        while recursor_levels.contains(&motive) {
            motive = format!("{motive}'").into();
        }
        recursor_levels.push(motive);
        let ty = recursor_type(self.sig.data(data), level_names.len());
        self.sig.data_mut(data).recursor = self.sig.next_global();
        self.sig.add(Global {
            name: recursor_name,
            level_params: recursor_levels,
            ty: Rc::new(ty),
            kind: GlobalKind::Recursor(data),
            uses: constructors.into_iter().chain([former]).collect(),
        });
        Ok(())
    }

    fn check_constructor(&self, data: DataId, level_names: &[Name], level: &Level, c: &RawConstructor) -> Result<ConstructorDecl> {
        let decl = self.sig.data(data);
        let mut ctx = Context::new(level_names.to_vec());
        for p in &decl.params {
            let ty = self.eval(&ctx, &p.ty);
            ctx.bind(p.name.clone(), ty);
        }
        let np = decl.params.len();
        let mut fields = Vec::new();
        let mut recursive = Vec::new();
        for (i, (name, raw)) in c.fields.iter().enumerate() {
            // An unnamed field gets a positional name, for printing the
            // recursor's type.
            let name: Name = if &**name == "_" { format!("f{i}").into() } else { name.clone() };
            let (ty, field_level) = self.check_type(&mut ctx, raw)?;
            if !field_level.leq(level) {
                return Err(self.error(
                    Code::TypeMismatch,
                    raw.span,
                    format!("this field is too large for the type's universe `{}`", self.show(&ctx, &Val::Sort(level.clone()))),
                ));
            }
            let ty_val = self.eval(&ctx, &ty);
            // Rule [Positivity-Restricted]
            // See Note [Strict positivity, restricted].
            let is_self = is_data_at_own_params(&ty_val, data, np);
            if !is_self && mentions_global(&ty, decl.global) {
                let name = &self.sig.global(decl.global).name;
                return Err(self.error(
                    Code::Unsupported,
                    raw.span,
                    format!("`{name}` may only occur as a whole field, applied to its own parameters"),
                ));
            }
            fields.push(Param {
                name: name.clone(),
                ty: Rc::new(ty),
                level: field_level,
            });
            recursive.push(is_self);
            ctx.bind(name, ty_val);
        }
        Ok(ConstructorDecl {
            name: c.name.clone(),
            global: GlobalId::MAX,
            fields,
            recursive,
        })
    }
}

/// `(x y: A) -> R x y -> @cast (P (@qin x)) (P (@qin y)) _ (f x) ~ f y`: the
/// method `f` sends related representatives to equal results.
fn respect_type(quot: Rc<QuotVal>, p: Val, f: Val, level: Level) -> Val {
    let outer = quot.clone();
    native_pi("x", outer.carrier.clone(), outer.level.clone(), Level::Zero, move |_, x| {
        let (quot, p, f, level) = (quot.clone(), p.clone(), f.clone(), level.clone());
        native_pi("y", quot.carrier.clone(), quot.level.clone(), Level::Zero, move |sig, y| {
            let related = apply(sig, apply(sig, quot.relation.clone(), x.clone()), y.clone());
            let px = apply(sig, p.clone(), Val::QIn(Rc::new(x.clone())));
            let py = apply(sig, p.clone(), Val::QIn(Rc::new(y.clone())));
            let moved = cast(sig, px, py.clone(), Val::Irrelevant, apply(sig, f.clone(), x.clone()));
            let goal = eq(sig, level.clone(), py, moved, apply(sig, f.clone(), y));
            native_pi("_", related, Level::Zero, Level::Zero, move |_, _| goal.clone())
        })
    })
}

/// `ty` is `D p0 ... pn` with each `pi` the parameter variable at level `i`.
fn is_data_at_own_params(ty: &Val, data: DataId, params: usize) -> bool {
    let Val::Data(d) = ty else { return false };
    let own = |(i, p): (usize, &Val)| matches!(p, Val::Neutral(n) if matches!(**n, Neutral::Var(l) if l as usize == i));
    d.data == data && d.params.len() == params && d.params.iter().enumerate().all(own)
}

// Note [Strict positivity, restricted]
//
// An inductive type may appear in its own constructors only where its
// recursor can recurse: as a whole field `D params`, with exactly the
// declaration's parameters. Strict positivity proper also allows
// `A -> D params` (infinitely branching trees) and nested occurrences such as
// `List (D params)`; both need more of the recursor's machinery and are
// rejected for now. Anything that is not positive must be rejected for
// consistency: a negative occurrence gives a fixpoint combinator.

// ## Bidirectional checking

impl Checker<'_> {
    fn conversion<'a>(&'a self, ctx: &'a mut Context) -> ConversionChecker<'a> {
        ConversionChecker {
            sig: &self.sig,
            types: &mut ctx.types,
        }
    }

    fn eval(&self, ctx: &Context, t: &Term) -> Val {
        eval(&self.sig, &ctx.env, t)
    }

    fn show(&self, ctx: &Context, v: &Val) -> String {
        let term = quote(&self.sig, ctx.depth(), v);
        TermDisplay {
            sig: &self.sig,
            names: &ctx.names,
            level_names: &ctx.level_names,
            term: &term,
        }
        .to_string()
    }

    fn mismatch(&self, ctx: &Context, span: Span, expected: &Val, found: &Val) -> Diagnostic {
        self.error(
            Code::TypeMismatch,
            span,
            format!("expected `{}`, found `{}`", self.show(ctx, expected), self.show(ctx, found)),
        )
    }

    /// Run `k` with `name: ty` in scope.
    fn under<R>(&self, ctx: &mut Context, name: &Name, ty: Val, k: impl FnOnce(&Self, &mut Context) -> R) -> R {
        let env = ctx.env.clone();
        ctx.bind(name.clone(), ty);
        let r = k(self, ctx);
        ctx.names.pop();
        ctx.types.pop();
        ctx.env = env;
        r
    }

    /// Elaborate `raw` as a type, returning its sort.
    fn check_type(&self, ctx: &mut Context, raw: &Raw) -> Result<(Term, Level)> {
        let (t, ty) = self.infer(ctx, raw)?;
        match ty {
            Val::Sort(l) => Ok((t, l)),
            other => Err(self.error(
                Code::TypeMismatch,
                raw.span,
                format!("expected a type, found a value of type `{}`", self.show(ctx, &other)),
            )),
        }
    }

    // Algorithm [Bidirectional checking]
    fn check(&self, ctx: &mut Context, raw: &Raw, expected: &Val) -> Result<Term> {
        let term = self.check_relevant(ctx, raw, expected)?;
        // Rule [Proof-Witness]
        // See Note [Proofs evaluate to an irrelevant witness].
        if self.conversion(ctx).is_proposition(expected) {
            return Ok(Term::Irrelevant);
        }
        Ok(term)
    }

    fn check_relevant(&self, ctx: &mut Context, raw: &Raw, expected: &Val) -> Result<Term> {
        match (&raw.kind, expected) {
            // Rule [Fun]
            (RawKind::Lam(name, ann, body), Val::Pi(pi)) => {
                if let Some(ann) = ann {
                    let (t, _) = self.check_type(ctx, ann)?;
                    let given = self.eval(ctx, &t);
                    if !self.conversion(ctx).conv_type(&given, &pi.dom) {
                        return Err(self.mismatch(ctx, ann.span, &pi.dom, &given));
                    }
                }
                let body = self.under(ctx, name, pi.dom.clone(), |c, ctx| {
                    let cod = pi.cod.apply(&c.sig, Val::var(ctx.depth() - 1));
                    c.check(ctx, body, &cod)
                })?;
                Ok(Term::Lam(name.clone(), Rc::new(body)))
            }
            // Rule [Sigma-Pair]
            (RawKind::Pair(a, b), Val::Sigma(sigma)) => {
                let a = self.check(ctx, a, &sigma.dom)?;
                let cod = sigma.cod.apply(&self.sig, self.eval(ctx, &a));
                let b = self.check(ctx, b, &cod)?;
                Ok(Term::Pair(Rc::new(a), Rc::new(b)))
            }
            // Rule [Quot-Intro]
            (RawKind::Prim(Primitive::QIn, args), Val::Quot(q)) => {
                let a = self.check(ctx, &args[0], &q.carrier)?;
                Ok(Term::QIn(Rc::new(a)))
            }
            // Rule [Let]
            (RawKind::Let(name, ann, val, body), _) => {
                let (ty, val) = self.let_binding(ctx, ann.as_deref(), val)?;
                let (ty_val, val_val) = (self.eval(ctx, &ty), self.eval(ctx, &val));
                let env = ctx.env.clone();
                ctx.define(name.clone(), ty_val, val_val);
                let body = self.check(ctx, body, expected);
                ctx.names.pop();
                ctx.types.pop();
                ctx.env = env;
                Ok(Term::Let(name.clone(), Rc::new(ty), Rc::new(val), Rc::new(body?)))
            }
            _ => {
                let (t, found) = self.infer(ctx, raw)?;
                if self.conversion(ctx).conv_type(&found, expected) {
                    Ok(t)
                } else {
                    Err(self.mismatch(ctx, raw.span, expected, &found))
                }
            }
        }
    }

    fn infer(&self, ctx: &mut Context, raw: &Raw) -> Result<(Term, Val)> {
        match &raw.kind {
            RawKind::Var(name, level_args) => self.infer_var(ctx, raw.span, name, level_args),
            // Rule [Univ]
            RawKind::Sort(l) => {
                let l = self.level(ctx, l)?;
                Ok((Term::Sort(l.clone()), Val::Sort(l.succ())))
            }
            // Rule [Pi-Form]
            RawKind::Pi(name, dom, cod) => {
                let binding = self.binding(ctx, name, dom, cod)?;
                let sort = Level::imax(&binding.dom_level, &binding.cod_level);
                Ok((Term::Pi(Rc::new(binding)), Val::Sort(sort)))
            }
            // Rule [Sigma-Form]
            RawKind::Sigma(name, dom, cod) => {
                let binding = self.binding(ctx, name, dom, cod)?;
                let sort = Level::max(&binding.dom_level, &binding.cod_level);
                Ok((Term::Sigma(Rc::new(binding)), Val::Sort(sort)))
            }
            RawKind::Pair(..) => Err(self.error(
                Code::TypeAnnotationRequired,
                raw.span,
                "cannot infer the type of a pair; annotate it as `(e : (x: A) * B)`",
            )),
            RawKind::Fst(e) | RawKind::Snd(e) => {
                let (t, ty) = self.infer(ctx, e)?;
                let Val::Sigma(sigma) = &ty else {
                    return Err(self.error(
                        Code::TypeMismatch,
                        e.span,
                        format!("projected from a value of type `{}`, which is not a pair", self.show(ctx, &ty)),
                    ));
                };
                let t = Rc::new(t);
                if matches!(raw.kind, RawKind::Fst(_)) {
                    Ok((Term::Fst(t), sigma.dom.clone()))
                } else {
                    let first = fst(self.eval(ctx, &t));
                    Ok((Term::Snd(t), sigma.cod.apply(&self.sig, first)))
                }
            }
            RawKind::Lam(name, Some(ann), body) => {
                let (dom, dom_level) = self.check_type(ctx, ann)?;
                let dom_val = self.eval(ctx, &dom);
                let (body, cod, cod_level) = self.under(ctx, name, dom_val.clone(), |c, ctx| {
                    let (body, cod) = c.infer(ctx, body)?;
                    let level = c.conversion(ctx).sort_of(&cod);
                    Ok::<_, Diagnostic>((body, quote(&c.sig, ctx.depth(), &cod), level))
                })?;
                let Some(cod_level) = cod_level else {
                    return Err(self.error(Code::TypeAnnotationRequired, raw.span, "cannot infer the sort of this lambda's result"));
                };
                let ty = Val::Pi(Rc::new(BindingVal {
                    name: name.clone(),
                    dom: dom_val,
                    cod: Closure::Term(ctx.env.clone(), Rc::new(cod)),
                    dom_level,
                    cod_level,
                }));
                Ok((Term::Lam(name.clone(), Rc::new(body)), ty))
            }
            RawKind::Lam(name, None, _) => Err(self.error(
                Code::TypeAnnotationRequired,
                raw.span,
                format!("cannot infer the type of `{name}`; annotate it as `\\({name}: T) => ...`"),
            )),
            // Rule [App]
            RawKind::App(f, a) => {
                let (f_term, f_ty) = self.infer(ctx, f)?;
                let Val::Pi(pi) = &f_ty else {
                    return Err(self.error(
                        Code::TypeMismatch,
                        f.span,
                        format!("applied a value of type `{}`, which is not a function", self.show(ctx, &f_ty)),
                    ));
                };
                let a_term = self.check(ctx, a, &pi.dom)?;
                let ty = pi.cod.apply(&self.sig, self.eval(ctx, &a_term));
                Ok((Term::app(f_term, a_term), ty))
            }
            RawKind::Let(name, ann, val, body) => {
                let (ty, val) = self.let_binding(ctx, ann.as_deref(), val)?;
                let (ty_val, val_val) = (self.eval(ctx, &ty), self.eval(ctx, &val));
                let env = ctx.env.clone();
                ctx.define(name.clone(), ty_val, val_val);
                let body = self.infer(ctx, body);
                ctx.names.pop();
                ctx.types.pop();
                ctx.env = env;
                let (body, body_ty) = body?;
                Ok((Term::Let(name.clone(), Rc::new(ty), Rc::new(val), Rc::new(body)), body_ty))
            }
            RawKind::Ann(e, ty) => {
                let (ty, _) = self.check_type(ctx, ty)?;
                let ty_val = self.eval(ctx, &ty);
                Ok((self.check(ctx, e, &ty_val)?, ty_val))
            }
            RawKind::Prim(prim, args) => self.infer_primitive(ctx, *prim, args),
            // Rule [Eq-Form]
            RawKind::Eq(a, b) => {
                let (a, ty) = self.infer(ctx, a)?;
                let level = self.sort_of(ctx, &ty, raw.span)?;
                self.relevant(ctx, &ty, &level, raw.span)?;
                let b = self.check(ctx, b, &ty)?;
                let ty = quote(&self.sig, ctx.depth(), &ty);
                Ok((Term::Eq(level, Rc::new(ty), Rc::new(a), Rc::new(b)), Val::Sort(Level::Zero)))
            }
        }
    }

    /// The sort of a type a term was inferred to have.
    fn sort_of(&self, ctx: &mut Context, ty: &Val, span: Span) -> Result<Level> {
        match self.conversion(ctx).sort_of(ty) {
            Some(level) => Ok(level),
            None => Err(self.error(
                Code::TypeAnnotationRequired,
                span,
                format!("cannot tell which universe `{}` lives in", self.show(ctx, ty)),
            )),
        }
    }

    /// Equality is formed only at a type that is not a proposition: two
    /// proofs are already equal by conversion.
    /// See Note [Equality is not formed at a proposition].
    fn relevant(&self, ctx: &mut Context, ty: &Val, level: &Level, span: Span) -> Result<()> {
        if !level.is_zero() {
            return Ok(());
        }
        Err(self.error(
            Code::TypeMismatch,
            span,
            format!("equality is not formed at the proposition `{}`", self.show(ctx, ty)),
        ))
    }

    /// The domain and codomain of a Pi or Sigma type.
    fn binding(&self, ctx: &mut Context, name: &Name, dom: &Raw, cod: &Raw) -> Result<Binding> {
        let (dom, dom_level) = self.check_type(ctx, dom)?;
        let dom_val = self.eval(ctx, &dom);
        let (cod, cod_level) = self.under(ctx, name, dom_val, |c, ctx| c.check_type(ctx, cod))?;
        Ok(Binding {
            name: name.clone(),
            dom: Rc::new(dom),
            cod: Rc::new(cod),
            dom_level,
            cod_level,
        })
    }

    fn infer_primitive(&self, ctx: &mut Context, prim: Primitive, args: &[Raw]) -> Result<(Term, Val)> {
        let proposition = Val::Sort(Level::Zero);
        Ok(match prim {
            // Rule [Top]
            Primitive::Top => (Term::Top, proposition),
            Primitive::Tt => (Term::Tt, Val::Top),
            // Rule [Bottom]
            Primitive::Bottom => (Term::Bottom, proposition),
            Primitive::Absurd => {
                let (ty, _) = self.check_type(ctx, &args[0])?;
                let proof = self.check(ctx, &args[1], &Val::Bottom)?;
                let ty_val = self.eval(ctx, &ty);
                (Term::Absurd(Rc::new(ty), Rc::new(proof)), ty_val)
            }
            // Rule [Eq-Form]
            Primitive::Eq => {
                let (ty, level) = self.check_type(ctx, &args[0])?;
                let ty_val = self.eval(ctx, &ty);
                self.relevant(ctx, &ty_val, &level, args[0].span)?;
                let a = self.check(ctx, &args[1], &ty_val)?;
                let b = self.check(ctx, &args[2], &ty_val)?;
                (Term::Eq(level, Rc::new(ty), Rc::new(a), Rc::new(b)), proposition)
            }
            // Rule [Refl]
            Primitive::Refl => {
                let (a, ty) = self.infer(ctx, &args[0])?;
                let level = self.sort_of(ctx, &ty, args[0].span)?;
                self.relevant(ctx, &ty, &level, args[0].span)?;
                let a = self.eval(ctx, &a);
                (Term::Irrelevant, eq(&self.sig, level, ty, a.clone(), a))
            }
            // Rule [Transport]
            Primitive::Transp => {
                let (t, ty) = self.infer(ctx, &args[0])?;
                let level = self.sort_of(ctx, &ty, args[0].span)?;
                self.relevant(ctx, &ty, &level, args[0].span)?;
                let motive_ty = Val::Pi(Rc::new(BindingVal {
                    name: "_".into(),
                    dom: ty.clone(),
                    cod: Closure::Native(Rc::new(|_, _| Val::Sort(Level::Zero))),
                    dom_level: level.clone(),
                    cod_level: Level::constant(1),
                }));
                let motive = self.check(ctx, &args[1], &motive_ty)?;
                let motive = self.eval(ctx, &motive);
                let t = self.eval(ctx, &t);
                self.check(ctx, &args[2], &apply(&self.sig, motive.clone(), t.clone()))?;
                let t2 = self.check(ctx, &args[3], &ty)?;
                let t2 = self.eval(ctx, &t2);
                self.check(ctx, &args[4], &eq(&self.sig, level, ty, t, t2.clone()))?;
                (Term::Irrelevant, apply(&self.sig, motive, t2))
            }
            // Rule [Cast]
            Primitive::Cast => {
                let (from, level) = self.check_type(ctx, &args[0])?;
                let universe = Val::Sort(level.clone());
                let to = self.check(ctx, &args[1], &universe)?;
                let (from_val, to_val) = (self.eval(ctx, &from), self.eval(ctx, &to));
                let along = eq(&self.sig, level.succ(), universe, from_val.clone(), to_val.clone());
                let proof = self.check(ctx, &args[2], &along)?;
                let value = self.check(ctx, &args[3], &from_val)?;
                let term = Term::Cast(Rc::new(from), Rc::new(to), Rc::new(proof), Rc::new(value));
                (term, to_val)
            }
            Primitive::Quot => self.infer_quotient(ctx, args)?,
            Primitive::QIn => {
                return Err(self.error(
                    Code::TypeAnnotationRequired,
                    args[0].span,
                    "cannot infer which quotient this class belongs to; annotate it as `(@qin a : Q)`",
                ))
            }
            Primitive::QElim => self.infer_quotient_elim(ctx, args)?,
        })
    }

    // Rule [Quot-Form]
    fn infer_quotient(&self, ctx: &mut Context, args: &[Raw]) -> Result<(Term, Val)> {
        let (carrier, level) = self.check_type(ctx, &args[0])?;
        if !level.is_nonzero() {
            return Err(self.error(Code::Unsupported, args[0].span, "only data can be quotiented, not a proposition"));
        }
        let a = self.eval(ctx, &carrier);
        let relation = self.check(ctx, &args[1], &relation_type(a.clone(), level.clone()))?;
        let r = self.eval(ctx, &relation);
        let r0 = r.clone();
        let rel = Rc::new(move |sig: &Signature, x: Val, y: Val| apply(sig, apply(sig, r0.clone(), x), y));

        // (x: A) -> R x x
        let (r1, l1) = (r.clone(), level.clone());
        let reflexive = native_pi("x", a.clone(), l1, Level::Zero, move |sig, x| apply(sig, apply(sig, r1.clone(), x.clone()), x));
        self.check(ctx, &args[2], &reflexive)?;

        // (x y: A) -> R x y -> R y x
        let (a2, l2, r2) = (a.clone(), level.clone(), r.clone());
        let symmetric = native_pi("x", a.clone(), level.clone(), Level::Zero, move |_, x| {
            let r2 = r2.clone();
            native_pi("y", a2.clone(), l2.clone(), Level::Zero, move |sig, y| {
                let forward = apply(sig, apply(sig, r2.clone(), x.clone()), y.clone());
                let back = apply(sig, apply(sig, r2.clone(), y), x.clone());
                native_pi("_", forward, Level::Zero, Level::Zero, move |_, _| back.clone())
            })
        });
        self.check(ctx, &args[3], &symmetric)?;

        // (x y z: A) -> R x y -> R y z -> R x z
        let (a3, l3) = (a.clone(), level.clone());
        let rel3 = rel.clone();
        let transitive = native_pi("x", a.clone(), level.clone(), Level::Zero, move |_, x| {
            let (a3, l3, rel3) = (a3.clone(), l3.clone(), rel3.clone());
            native_pi("y", a3.clone(), l3.clone(), Level::Zero, move |_, y| {
                let (x, rel3) = (x.clone(), rel3.clone());
                native_pi("z", a3.clone(), l3.clone(), Level::Zero, move |sig, z| {
                    let (xy, yz, xz) = (rel3(sig, x.clone(), y.clone()), rel3(sig, y.clone(), z.clone()), rel3(sig, x.clone(), z));
                    native_pi("_", xy, Level::Zero, Level::Zero, move |_, _| {
                        let xz = xz.clone();
                        native_pi("_", yz.clone(), Level::Zero, Level::Zero, move |_, _| xz.clone())
                    })
                })
            })
        });
        self.check(ctx, &args[4], &transitive)?;

        let term = Term::Quot(Rc::new(carrier), Rc::new(relation), level.clone());
        Ok((term, Val::Sort(level)))
    }

    // Rule [Quot-Elim]
    fn infer_quotient_elim(&self, ctx: &mut Context, args: &[Raw]) -> Result<(Term, Val)> {
        let (q, q_ty) = self.infer(ctx, &args[3])?;
        let Val::Quot(quot) = &q_ty else {
            return Err(self.error(
                Code::TypeMismatch,
                args[3].span,
                format!("eliminated a value of type `{}`, which is not a quotient", self.show(ctx, &q_ty)),
            ));
        };
        let (motive, level) = self.infer_motive(ctx, &args[0], &q_ty, &quot.level)?;
        let p = self.eval(ctx, &motive);

        // (x: A) -> P (@qin x)
        let p1 = p.clone();
        let method_ty = native_pi("x", quot.carrier.clone(), quot.level.clone(), level.clone(), move |sig, x| {
            apply(sig, p1.clone(), Val::QIn(Rc::new(x)))
        });
        let method = self.check(ctx, &args[1], &method_ty)?;
        let f = self.eval(ctx, &method);

        // (x y: A) -> R x y -> @cast (P (@qin x)) (P (@qin y)) _ (f x) ~ f y
        let respects = respect_type(quot.clone(), p.clone(), f.clone(), level.clone());
        self.check(ctx, &args[2], &respects)?;

        let ty = apply(&self.sig, p, self.eval(ctx, &q));
        Ok((Term::QElim(Rc::new(motive), Rc::new(method), level, Rc::new(q)), ty))
    }

    /// A motive `P : dom -> @Type m`, with its sort `m`. A lambda is checked
    /// against the domain and its body's sort read off; anything else must
    /// have a Pi type over `dom`.
    fn infer_motive(&self, ctx: &mut Context, raw: &Raw, dom: &Val, dom_level: &Level) -> Result<(Term, Level)> {
        if let RawKind::Lam(name, ann, body) = &raw.kind {
            if let Some(ann) = ann {
                let (t, _) = self.check_type(ctx, ann)?;
                let given = self.eval(ctx, &t);
                if !self.conversion(ctx).conv_type(&given, dom) {
                    return Err(self.mismatch(ctx, ann.span, dom, &given));
                }
            }
            let (body, level) = self.under(ctx, name, dom.clone(), |c, ctx| c.check_type(ctx, body))?;
            return Ok((Term::Lam(name.clone(), Rc::new(body)), level));
        }
        let (t, ty) = self.infer(ctx, raw)?;
        if let Val::Pi(pi) = &ty {
            let cod = pi.cod.apply(&self.sig, Val::var(ctx.depth()));
            if let Val::Sort(level) = cod {
                if self.conversion(ctx).conv_type(&pi.dom, dom) {
                    return Ok((t, level));
                }
            }
        }
        let expected = native_pi("_", dom.clone(), dom_level.clone(), Level::constant(1), |_, _| Val::Sort(Level::Zero));
        Err(self.error(
            Code::TypeMismatch,
            raw.span,
            format!("expected a motive over `{}`, found `{}`", self.show(ctx, dom), self.show(ctx, &ty)),
        )
        .with_note(format!("a motive has a type like `{}`", self.show(ctx, &expected))))
    }

    /// The type and value of `let x: ann = val`.
    fn let_binding(&self, ctx: &mut Context, ann: Option<&Raw>, val: &Raw) -> Result<(Term, Term)> {
        match ann {
            Some(ann) => {
                let (ty, _) = self.check_type(ctx, ann)?;
                let ty_val = self.eval(ctx, &ty);
                Ok((ty, self.check(ctx, val, &ty_val)?))
            }
            None => {
                let (val, ty) = self.infer(ctx, val)?;
                Ok((quote(&self.sig, ctx.depth(), &ty), val))
            }
        }
    }

    fn infer_var(&self, ctx: &mut Context, span: Span, name: &Name, level_args: &[LevelArg]) -> Result<(Term, Val)> {
        if let Some(level) = ctx.names.iter().rposition(|n| n == name) {
            if let Some(arg) = level_args.first() {
                return Err(self.error(Code::TypeMismatch, arg.span, format!("`{name}` is a local variable and takes no levels")));
            }
            let ix = ctx.depth() - 1 - level as Lvl;
            return Ok((Term::Var(ix), ctx.types[level].clone()));
        }
        let Some(id) = self.sig.lookup(name) else {
            let locals = ctx.names.iter().map(|n| &**n);
            let mut err = self.error(Code::UnknownSymbol, span, format!("unknown name `{name}`"));
            if let Some(near) = closest(name, locals.chain(self.sig.names())) {
                err = err.with_help(format!("did you mean `{near}`?"));
            }
            return Err(err);
        };
        let params = &self.sig.global(id).level_params;
        let mut levels: Vec<Option<Level>> = vec![None; params.len()];
        for arg in level_args {
            let Some(i) = params.iter().position(|p| *p == arg.name) else {
                return Err(self.error(Code::UnknownSymbol, arg.span, format!("`{name}` has no level `{}`", arg.name)));
            };
            if levels[i].is_some() {
                return Err(self.error(Code::DuplicateSymbol, arg.span, format!("level `{}` is given twice", arg.name)));
            }
            levels[i] = Some(self.level(ctx, &arg.level)?);
        }
        if let Some(i) = levels.iter().position(Option::is_none) {
            return Err(self.error(
                Code::TypeAnnotationRequired,
                span,
                format!("`{name}` needs its level `{}`: write `{name} @ctx {{.{} = ...}}`", params[i], params[i]),
            ));
        }
        // Rule [Level-Polymorphism]
        let levels: Rc<[Level]> = levels.into_iter().map(Option::unwrap).collect();
        self.uses.borrow_mut().insert(id);
        let ty = global_type(&self.sig, id, levels.clone());
        Ok((Term::Global(id, levels), ty))
    }

    fn level(&self, ctx: &Context, raw: &RawLevel) -> Result<Level> {
        Ok(match raw {
            RawLevel::Num(n) => Level::constant(*n),
            RawLevel::Name(name, span) => match ctx.level_names.iter().position(|n| n == name) {
                Some(i) => Level::Param(i as u32),
                None => {
                    let mut err = self.error(Code::UnknownSymbol, *span, format!("unknown level `{name}`"));
                    if let Some(near) = closest(name, ctx.level_names.iter().map(|n| &**n)) {
                        err = err.with_help(format!("did you mean `{near}`?"));
                    }
                    return Err(err);
                }
            },
            RawLevel::Plus(l, n) => (0..*n).fold(self.level(ctx, l)?, |l, _| l.succ()),
            RawLevel::Max(a, b) => Level::max(&self.level(ctx, a)?, &self.level(ctx, b)?),
            RawLevel::IMax(a, b) => Level::imax(&self.level(ctx, a)?, &self.level(ctx, b)?),
        })
    }
}

// Note [Equality is not formed at a proposition]
//
// TT^obs and CC^obs form `t ~ u` only at a type in a relevant universe (PT22
// and PT23 Fig. 2, Eq-Form), and so do `@refl` and `@transp`: two proofs are
// equal by conversion, so the equality would say nothing. A type whose
// universe is a level parameter that may be `0` is allowed, and its equality
// stays stuck until the level is known. Equality still meets proofs inside
// data, as the second component of a pair or a field of a constructor; there
// it is `{}`, as for a Box type.
// See Rule [Eq-Box].

#[cfg(test)]
pub(crate) mod tests;
