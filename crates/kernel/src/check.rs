//! The kernel's type checker: bidirectional elaboration of script syntax into
//! core terms.
//!
//! The algorithm is bidirectional type checking (Pierce and Turner, "Local
//! type inference", TOPLAS 2000; Dunfield and Krishnaswami, "Bidirectional
//! typing", CSUR 2021) over normalisation-by-evaluation values, in the shape
//! of Coquand's checker for dependent types. `check` pushes an
//! expected type into lambdas; `infer` synthesises a type at
//! variables, applications and annotations, and a synthesised type fits an
//! expected one by cumulative subtyping. Names are resolved as they are
//! checked, so the output is a de Bruijn [`Term`].
//!
//! Each global is checked in order and added to the [`Signature`]: its level
//! parameters first, then its type, then its body against that type.
//!
//! Input: a script. Output: the signature of its globals, or the first error.
//!
//! Entry point: [`check_script`].

use std::rc::Rc;

use utilities::error::closest;
use utilities::{Code, Diagnostic, Result, Span};

use crate::conversion::ConversionChecker;
use crate::level::Level;
use crate::nbe::{eval, global_type, quote, Closure, Env, Lvl, PiVal, Val};
use crate::signature::{Global, GlobalKind, Signature};
use crate::syntax::{error, parse_script, Item, ItemBody, Raw, RawKind, RawLevel};
use crate::term::{Binding, Name, Term, TermDisplay};

/// Check every global of a script.
pub fn check_script(src: &str) -> Result<Signature> {
    let mut checker = Checker {
        src,
        sig: Signature::new(),
    };
    for item in parse_script(src)? {
        checker.check_item(&item)?;
    }
    Ok(checker.sig)
}

struct Checker<'s> {
    src: &'s str,
    sig: Signature,
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

    fn check_item(&mut self, item: &Item) -> Result<()> {
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
        let mut ctx = Context {
            names: Vec::new(),
            types: Vec::new(),
            env: Env::new(),
            level_names: level_names.clone(),
        };
        let (ty, _) = self.check_type(&mut ctx, &item.ty)?;
        let kind = match &item.body {
            ItemBody::Definition(body) => {
                let expected = eval(&self.sig, &ctx.env, &ty);
                GlobalKind::Definition(Rc::new(self.check(&mut ctx, body, &expected)?))
            }
            ItemBody::Axiom => GlobalKind::Axiom,
        };
        self.sig.add(Global {
            name: item.name.clone(),
            level_params: level_names,
            ty: Rc::new(ty),
            kind,
        });
        Ok(())
    }
}

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

    fn check(&self, ctx: &mut Context, raw: &Raw, expected: &Val) -> Result<Term> {
        match (&raw.kind, expected) {
            (RawKind::Lam(name, ann, body), Val::Pi(pi)) => {
                if let Some(ann) = ann {
                    let (t, _) = self.check_type(ctx, ann)?;
                    let given = self.eval(ctx, &t);
                    if !self.conversion(ctx).conv_type(&given, &pi.dom, false) {
                        return Err(self.mismatch(ctx, ann.span, &pi.dom, &given));
                    }
                }
                let body = self.under(ctx, name, pi.dom.clone(), |c, ctx| {
                    let cod = pi.cod.apply(&c.sig, Val::var(ctx.depth() - 1));
                    c.check(ctx, body, &cod)
                })?;
                Ok(Term::Lam(name.clone(), Rc::new(body)))
            }
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
                if self.conversion(ctx).subtype(&found, expected) {
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
            RawKind::Sort(l) => {
                let l = self.level(ctx, l)?;
                Ok((Term::Sort(l.clone()), Val::Sort(l.succ())))
            }
            RawKind::Pi(name, dom, cod) => {
                let (dom, dom_level) = self.check_type(ctx, dom)?;
                let dom_val = self.eval(ctx, &dom);
                let (cod, cod_level) = self.under(ctx, name, dom_val, |c, ctx| c.check_type(ctx, cod))?;
                let sort = Level::imax(&dom_level, &cod_level);
                let binding = Binding {
                    name: name.clone(),
                    dom: Rc::new(dom),
                    cod: Rc::new(cod),
                    dom_level,
                    cod_level,
                };
                Ok((Term::Pi(Rc::new(binding)), Val::Sort(sort)))
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
                let ty = Val::Pi(Rc::new(PiVal {
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
            RawKind::Prim(prim, _) => match *prim {},
        }
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

    fn infer_var(&self, ctx: &mut Context, span: Span, name: &Name, level_args: &[crate::syntax::LevelArg]) -> Result<(Term, Val)> {
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
        let levels: Rc<[Level]> = levels.into_iter().map(Option::unwrap).collect();
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

#[cfg(test)]
mod tests;
