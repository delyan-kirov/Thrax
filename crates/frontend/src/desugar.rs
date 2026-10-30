//! Desugaring: the checked AST ([`crate::parser::data`]) to the [`crate::core`].
//!
//! This is the whole "remove the sugar" pass. Operators become applications of a
//! built-in variable (or, for `;`/`|>`/`<|`/`::`, a `let`/application/`List`
//! construction); `if` and `is` become a single [`Term::Case`]; list and cons
//! sugar become `List.Cons`/`List.Nil`; multi-parameter and pattern lambdas curry
//! into single-parameter lambdas over `Case`; leading record parameters of a
//! definition's signature are consumed. Positional struct and variant fields are
//! labelled and ordered against the declarations the checker resolved
//! ([`crate::typing::resolved::Declarations`]).
//!
//! The AST is read through a borrowed [`Ast`]: a handle is resolved with
//! `ast.expr`/`ast.ty`/`ast.pat`, a name with `ast.text`. Because `ast` is
//! shared, those resolve to `'a`-lived data independent of the `&mut self` used
//! to add Core nodes and generate fresh names.
//!
//! Input: one checked module and what the checker resolved across every module
//! ([`Resolved`]). Output: its [`CoreProgram`].
//!
//! Entry point: [`desugar_program`].

use std::collections::HashMap;

use crate::parser::data::{
    Ast, Binding, Expr, FieldInit, FieldPat, Item, LamParam, Pattern,
    Program, RecField, SliceSlot, Ty,
};
use crate::typing::{CtxVal, HookImpl, Resolved};
use utilities::{Aol, Slice, Span, StrId};

use crate::core::{
    CoreArm, CoreClause, CoreEffect, CoreHandler, CoreProgram, FreshNames, Pat, Term, Terms,
};

/// The built-in that suspends a value. Construction wraps a lazy slot in
/// `@delay (\_ = e)`; the machine forces it at the point the value is
/// scrutinised and patches the cell with the result.
pub const DELAY: &str = "@delay";

/// One surface match arm's unresolved handles: its patterns (an or-pattern has
/// several), an optional guard, and the body.
type ArmHandles = (Vec<Aol<Pattern>>, Option<Aol<Expr>>, Aol<Expr>);

/// Desugar one checked module's globals to Core.
pub fn desugar_program(
    ast: &Ast,
    program: &Program,
    resolved: &Resolved,
) -> CoreProgram {
    let dotted = |parts: Slice<StrId>| {
        StrId::intern(&ast.slice(parts).iter().map(|&p| ast.text(p)).collect::<Vec<_>>().join("."))
    };
    let mut imports = Vec::new();
    let mut aliases = HashMap::new();
    for item in ast.slice(program.items) {
        if let Item::Import { module, alias, .. } = item {
            let module = dotted(*module);
            if let Some(alias) = alias {
                aliases.insert(*alias, module);
            }
            imports.push(module);
        }
    }
    let mut lw = Desugarer {
        ast,
        resolved,
        module: program.module,
        imports,
        aliases,
        names: FreshNames::new("%", 1),
        terms: Terms::new(),
        meta_evals: Vec::new(),
        meta_type_evals: Vec::new(),
    };
    let mut effects = Vec::new();
    let mut globals = Vec::new();
    let mut ct_runs = Vec::new();
    for item in ast.slice(program.items).iter() {
        match item {
            Item::Def { body, .. } if matches!(ast.expr(*body), Expr::Builtin) => {}
            Item::Def {
                name, sig, body, ..
            } => {
                if let Some(sig) = sig {
                    lw.collect_meta_types(*sig);
                }
                let term = lw.def(*sig, *body);
                globals.push((*name, term));
            }
            // `$ @e <expr>`: back it with a synthetic global, forced at compile
            // time by the driver. The name is bare here (index-tagged so several
            // `@e`s in one module never collide); `lower_modules` prefixes the
            // module, and the driver qualifies the same way to force it.
            Item::Run(expr, span) => {
                let name = StrId::intern(&format!("@e#{}", ct_runs.len()));
                let term = lw.expr(*expr);
                globals.push((name, term));
                ct_runs.push((name, *span));
            }
            Item::Effect { name, ops, .. } => {
                for op in ast.slice(*ops).iter() {
                    effects.push(CoreEffect {
                        effect: *name,
                        op: op.name,
                    });
                }
            }
            _ => {}
        }
    }
    // Expression-position `@e X` synthetic globals accumulated while lowering the
    // bodies above; append them and record (name, source-span) for the driver to
    // fold/splice.
    let ct_evals: Vec<(StrId, Span)> =
        lw.meta_evals.iter().map(|(n, _, s)| (*n, *s)).collect();
    globals.extend(lw.meta_evals.drain(..).map(|(n, body, _)| (n, body)));
    // Type-position `@e X` synthetic globals, same treatment as `ct_evals` but the
    // driver splices the result as type source rather than a value literal.
    let ct_types: Vec<(StrId, Span)> =
        lw.meta_type_evals.iter().map(|(n, _, s)| (*n, *s)).collect();
    globals.extend(lw.meta_type_evals.drain(..).map(|(n, body, _)| (n, body)));
    // `$ @build : {} -> <@Meta, @IO> @Code`: back the call `@build {}` with a
    // synthetic global so the driver can force it exactly as it forces an `$ @e`.
    // The reference is module-qualified because several modules may each define
    // their own `@build`, and a bare global lookup resolves root-first.
    let ct_build = if globals.iter().any(|(n, _)| *n == crate::BUILD) {
        let name = StrId::intern("@build#call");
        let build = lw.add(Term::Var {
            module: Some(program.module),
            name: StrId::intern(crate::BUILD),
        });
        let unit = lw.add(Term::Unit);
        let call = lw.app(build, unit);
        globals.push((name, call));
        Some(name)
    } else {
        None
    };
    let mut core = CoreProgram::new(program.module);
    core.terms = lw.terms;
    core.effects = effects;
    core.globals = globals;
    core.crepr_layouts = resolved
        .crepr_layouts
        .iter()
        .map(|(n, l)| (*n, l.clone()))
        .collect();
    core.ct_runs = ct_runs;
    core.ct_evals = ct_evals;
    core.ct_types = ct_types;
    core.ct_build = ct_build;
    core
}

struct Desugarer<'a> {
    ast: &'a Ast,
    resolved: &'a Resolved,
    /// This lowering's own `@mod` name and the modules it imports (`$ with`),
    /// used to resolve a same-named type to the right module's declaration.
    module: StrId,
    imports: Vec<StrId>,
    /// Import aliases (`$ with V = VEC`), alias to module, so `V.name` lowers
    /// to `VEC.name`.
    aliases: HashMap<StrId, StrId>,
    names: FreshNames,
    /// The Core being built.
    terms: Terms,
    /// Synthetic globals for expression-position `@e X`: each `@e X` becomes a
    /// global `@e_expr#n = X` that the driver forces at compile time, with the
    /// source span of the whole `@e X` so the driver can substitute the result's
    /// source there and re-compile. Drained by [`desugar_program`] into
    /// `Program.globals` / `Program.ct_evals`.
    meta_evals: Vec<(StrId, Aol<Term>, Span)>,
    /// Synthetic globals for type-position `@e X` (`Foo : @e X = ...`), each a
    /// global `@e_type#n = X` forced at compile time; the driver splices the
    /// resulting `@Code`'s type source at the `@e X` span and re-compiles. Drained
    /// by [`desugar_program`] into `Program.globals` / `Program.ct_types`.
    meta_type_evals: Vec<(StrId, Aol<Term>, Span)>,
}

// ## Building Core

impl<'a> Desugarer<'a> {
    fn add(&mut self, t: Term) -> Aol<Term> {
        self.terms.add(t)
    }
    fn app(&mut self, f: Aol<Term>, x: Aol<Term>) -> Aol<Term> {
        self.terms.app(f, x)
    }
    fn var(&mut self, name: impl Into<StrId>) -> Aol<Term> {
        self.terms.var(name)
    }
    fn field(&mut self, record: Aol<Term>, field: impl Into<StrId>) -> Aol<Term> {
        self.terms.field(record, field)
    }
    fn int(&mut self, n: i64) -> Aol<Term> {
        self.add(Term::Int(n))
    }
    fn unit(&mut self) -> Aol<Term> {
        self.add(Term::Unit)
    }
    fn fault(&mut self, msg: impl Into<String>) -> Aol<Term> {
        self.add(Term::Fault(msg.into()))
    }
    /// A one-armed case with no default, the shape of a destructuring binder.
    fn case1(&mut self, scrut: Aol<Term>, pat: Pat, body: Aol<Term>) -> Aol<Term> {
        self.add(Term::Case {
            scrut,
            arms: vec![CoreArm { pat, guard: None, body }],
            default: None,
        })
    }
}

impl<'a> Desugarer<'a> {
    fn node(&self, e: Aol<Expr>) -> &'a Expr {
        self.ast.expr(e)
    }
    fn tnode(&self, t: Aol<Ty>) -> &'a Ty {
        self.ast.ty(t)
    }
    fn pnode(&self, p: Aol<Pattern>) -> &'a Pattern {
        self.ast.pat(p)
    }
    fn text(&self, id: utilities::StrId) -> &'a str {
        self.ast.text(id)
    }

    fn fresh(&mut self) -> StrId {
        self.names.name()
    }

    /// Suspend `body` for a lazy constructor slot. A lambda is an atom, so ANF
    /// and closure conversion leave it in place instead of hoisting it into a
    /// `let` the way they would a bare call.
    /// See Note [Lazy constructor slots].
    fn delay(&mut self, body: Aol<Term>) -> Aol<Term> {
        let param = self.fresh();
        let delay = self.var(DELAY);
        let thunk = self.terms.lam(param, body);
        self.app(delay, thunk)
    }

    /// The lazy flags for a declared type's slots, once the name is resolved the
    /// way construction resolves it.
    fn lazy_flags(&self, ty: StrId, tag: Option<StrId>) -> Option<&[bool]> {
        self.resolved.lazy_slots.get(&(ty, tag)).map(Vec::as_slice)
    }

    /// The head type constructor's name, if `t` is a (possibly applied) `Con`.
    fn ty_head_con(&self, t: Aol<Ty>) -> Option<StrId> {
        match self.tnode(t) {
            Ty::Con { name, .. } => Some(*name),
            Ty::App(head, _) => self.ty_head_con(*head),
            _ => None,
        }
    }

    /// Desugar `with subject in body` into a `let` per field: bind the subject to
    /// a fresh name (so it is forced once) and each of its `fields` to a field
    /// access on it. This removes the name-binding-by-type `with` node from the
    /// Core, keeping it De-Bruijn indexable.
    fn desugar_with(&mut self, subject: Aol<Term>, fields: &[StrId], body: Aol<Term>) -> Aol<Term> {
        let s = self.fresh();
        let mut inner = body;
        for &f in fields.iter().rev() {
            let sv = self.var(s);
            let val = self.field(sv, f);
            inner = self.terms.let_(f, val, inner);
        }
        self.terms.let_(s, subject, inner)
    }

    /// Lower a definition, consuming leading record parameters of its signature. A
    /// `@ctx` parameter needs nothing here: the body binds it like any other first
    /// parameter, and each call site injects the resolved value as a leading
    /// argument (see [`Self::apply_ctx`]).
    fn def(&mut self, sig: Option<Aol<Ty>>, body: Aol<Expr>) -> Aol<Term> {
        let term = self.expr(body);
        // A bare `@extern` already declares every parameter in its `arg_types`
        // spine (a `{}` parameter included), so it must NOT be wrapped in the
        // record/thunk-parameter sugar: a wrapper lambda would swallow the unit
        // argument and leave the extern under-applied, so it never fires.
        if matches!(self.terms.get(term), Term::Extern { .. }) {
            term
        } else {
            self.record_params(sig, term)
        }
    }

    fn record_params(&mut self, sig: Option<Aol<Ty>>, body: Aol<Term>) -> Aol<Term> {
        let Some(sig) = sig else { return body };
        let Ty::Arrow { from, to, .. } = self.tnode(sig) else {
            return body;
        };
        let (from, to) = (*from, *to);
        // The body's leading lambdas bind the LAST k of the m signature parameters,
        // so a parameter is only auto-bound when the lambdas do not reach it
        // (`k < m`). This lets `\p = p.x` name a record parameter explicitly, while
        // still auto-binding a record ahead of a later explicit lambda parameter.
        let m = 1 + self.arrow_arity(to);
        if leading_lams(&self.terms, body) >= m {
            return body;
        }
        // A CLOSED record parameter is the destructuring sugar; an open `{ x | r }`
        // is a real record value (plain parameter); a unit parameter takes no
        // fields, so the sugar just introduces the thunk parameter (`f : {} -> T`).
        let fields: &[RecField] = match self.tnode(from) {
            Ty::Record { fields, tail: None } => self.ast.slice(*fields),
            Ty::Unit => &[],
            _ => return body,
        };
        let inner = self.record_params(Some(to), body);
        self.record_param(fields, inner)
    }

    /// The number of parameters in a signature's arrow spine.
    fn arrow_arity(&self, ty: Aol<Ty>) -> usize {
        let mut n = 0;
        let mut cur = ty;
        while let Ty::Arrow { to, .. } = self.tnode(cur) {
            n += 1;
            cur = *to;
        }
        n
    }

    /// Wrap `body` in the lambda binding a record parameter's fields by name. The
    /// parameter is a name-keyed record value, so each field is bound by field
    /// access (`param.field`); the field bindings are outermost so a `with` field's
    /// scoping can reference them.
    fn record_param(&mut self, fields: &[RecField], body: Aol<Term>) -> Aol<Term> {
        let param = self.fresh();
        let mut inner = body;
        for f in fields.iter().rev() {
            if f.with {
                let names: Vec<StrId> = self
                    .ty_head_con(f.ty)
                    .and_then(|s| self.resolved.declarations.fields_of(self.module, &self.imports, s))
                    .map(<[StrId]>::to_vec)
                    .unwrap_or_default();
                let subject = self.var(f.name);
                inner = self.desugar_with(subject, &names, inner);
            }
        }
        for f in fields.iter().rev() {
            let pv = self.var(param);
            let val = self.field(pv, f.name);
            inner = self.terms.let_(f.name, val, inner);
        }
        self.terms.lam(param, inner)
    }

    fn expr(&mut self, e: Aol<Expr>) -> Aol<Term> {
        // A promoted argument (scalar or positional tuple passed where a record is
        // expected) is wrapped into a name-keyed record here.
        if let Some(names) = self.resolved.promotions.get(&e) {
            let names = names.clone();
            return self.promote_to_record(e, &names);
        }
        self.expr_core(e)
    }

    /// An anonymous (name-keyed) record literal.
    fn record(&mut self, fields: Vec<(StrId, Aol<Term>)>) -> Aol<Term> {
        self.add(Term::Struct {
            name: StrId::intern(""),
            base: None,
            fields,
        })
    }

    /// Wrap a promoted argument into a record with `names`: a positional tuple
    /// literal maps its elements to the names; a scalar becomes a one-field record;
    /// a tuple-typed value is bound once and projected by index.
    fn promote_to_record(&mut self, e: Aol<Expr>, names: &[StrId]) -> Aol<Term> {
        if let Expr::Tuple(items) = self.node(e) {
            let items: Vec<Aol<Expr>> = self.ast.slice(*items).to_vec();
            let mut fields = Vec::new();
            for (&name, it) in names.iter().zip(items) {
                fields.push((name, self.expr(it)));
            }
            return self.record(fields);
        }
        let val = self.expr_core(e);
        if names.len() == 1 {
            return self.record(vec![(names[0], val)]);
        }
        let t = self.fresh();
        let mut fields = Vec::new();
        for (i, &n) in names.iter().enumerate() {
            let tv = self.var(t);
            fields.push((n, self.field(tv, i.to_string())));
        }
        let body = self.record(fields);
        self.terms.let_(t, val, body)
    }

    /// Wrap a raw literal payload in its blessed construction interface when the
    /// checker recorded one at `site`; otherwise return the payload as-is (the folded
    /// built-in constant, so the default case builds nothing extra).
    fn literal_hook_wrap(&mut self, site: Aol<Expr>, payload: Aol<Term>) -> Aol<Term> {
        match self.hook_fn(site) {
            Some(f) => self.app(f, payload),
            None => payload,
        }
    }

    /// The interface method the checker resolved at `site`, as a callable term: the
    /// single field of the resolved instance.
    fn hook_fn(&mut self, site: Aol<Expr>) -> Option<Aol<Term>> {
        let hook = self.resolved.literal_hooks.get(&site).cloned()?;
        Some(self.hook_term(&hook))
    }

    /// A resolved blessed interface as a callable term: project its single field off
    /// the instance value.
    fn hook_term(&mut self, hook: &HookImpl) -> Aol<Term> {
        let instance = self.ctx_term(&hook.ctx);
        self.field(instance, hook.field)
    }

    fn expr_core(&mut self, e: Aol<Expr>) -> Aol<Term> {
        match self.node(e) {
            Expr::Int(n) => {
                let raw = self.int(*n);
                self.literal_hook_wrap(e, raw)
            }
            Expr::Real(r) => {
                let raw = self.add(Term::Real(*r));
                self.literal_hook_wrap(e, raw)
            }
            Expr::Str(s) => {
                let raw = self.add(Term::Str(*s));
                self.literal_hook_wrap(e, raw)
            }
            Expr::Bool(b) => self.add(Term::Bool(*b)),
            Expr::Unit => self.unit(),

            Expr::Var { .. } => {
                let base = self.var_head(e);
                // A multi-argument foreign function referenced first-class (not the
                // head of an application) presents to Thrax as a record-taking
                // function, but its runtime value is N-ary. Eta-expand so the record
                // is destructured into the positional C arguments when the wrapper is
                // finally called.
                if let Some(params) = self.extern_params_of(e) {
                    if extern_needs_eta(&params) {
                        return self.eta_extern(base, &params);
                    }
                }
                base
            }

            Expr::App(f, x) => {
                let (f, x) = (*f, *x);
                // An explicit `(@ctx e)` argument is not applied here: the checker
                // took it off the spine and recorded it as the head's resolved
                // context, which `apply_ctx` injects at the head reference.
                if matches!(self.node(x), Expr::CtxArg(_)) {
                    return self.expr(f);
                }
                // Integers are one 64-bit value at runtime, so `@cast x` to a narrower
                // type wraps `x` into its range (`@swrap`/`@uwrap`, engine builtins the
                // checker does not expose); a 64-bit target is the operand itself.
                if self.is_cast_head(f) {
                    let x = self.expr(x);
                    return match self.resolved.cast_widths.get(&e) {
                        Some(&(signed, bits)) => {
                            let wrap = if signed { "@swrap" } else { "@uwrap" };
                            let bits = self.int(bits as i64);
                            self.terms.bin(wrap, bits, x)
                        }
                        None => x,
                    };
                }
                // `@e X` in expression position: run X at compile time and embed
                // its value here (see `lower_meta_e`).
                if self.is_meta_e_head(f) {
                    let span = self.ast.expr_span(e).unwrap_or(Span::at(0));
                    return self.lower_meta_e(x, span);
                }
                // A foreign call flattens the record that groups its C parameters
                // into positional arguments here, so the record is never built and
                // the extern node stays a plain N-ary function.
                if let Some(params) = self.extern_params_of(f) {
                    return self.flatten_extern_app(f, &params, x);
                }
                let f = self.expr(f);
                let x = self.expr(x);
                self.app(f, x)
            }

            // A multi-axis slice: apply each slot to the CURRENT axis. An `Index` slot
            // reduces its axis (so later axis numbers shift down by `dropped`); a
            // `Range`/`Full` slot keeps it. All ops are O(1) strided views.
            Expr::Slice { recv, slots } => {
                // A single-axis `lo ... hi` slice the checker resolved to the
                // `@ISlice` instance: apply it to the bounds as written
                // (the instance decides what an inclusive slice means for its type).
                if let Some(f) = self.hook_fn(e) {
                    let (lo, hi) = match self.ast.slice(*slots) {
                        [SliceSlot::Range(lo, hi)] => (*lo, *hi),
                        _ => unreachable!("only a lone range slot resolves the slice hook"),
                    };
                    let recv = self.expr(*recv);
                    let lo = self.expr(lo);
                    let hi = self.expr(hi);
                    let partial = self.app(f, recv);
                    let partial = self.app(partial, lo);
                    return self.app(partial, hi);
                }
                let mut t = self.expr(*recv);
                let mut dropped = 0usize;
                for (pos, s) in self.ast.slice(*slots).iter().enumerate() {
                    let axis = self.int((pos - dropped) as i64);
                    match s {
                        SliceSlot::Index(x) => {
                            let idx = self.expr(*x);
                            let op = self.var("@tensor_index_axis");
                            let partial = self.app(op, t);
                            let partial = self.app(partial, axis);
                            t = self.app(partial, idx);
                            dropped += 1;
                        }
                        SliceSlot::Range(lo, hi) => {
                            let lo = self.expr(*lo);
                            // `@iadd`, not `+`: `+` is now a CORE function, but this
                            // is compiler-internal Int arithmetic (inclusive end + 1),
                            // so it calls the intrinsic directly.
                            let hi = self.expr(*hi);
                            let one = self.int(1);
                            let hi1 = self.terms.bin("@iadd", hi, one);
                            let op = self.var("@tensor_slice_axis");
                            let partial = self.app(op, t);
                            let partial = self.app(partial, axis);
                            let partial = self.app(partial, lo);
                            t = self.app(partial, hi1);
                        }
                        SliceSlot::Full => {}
                    }
                }
                t
            }

            Expr::BinOp { op, lhs, rhs } => {
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                self.binop(e, op, lhs, rhs)
            }
            Expr::UnOp { op, operand } => {
                let (op, operand) = (*op, *operand);
                let head = self.operator_head(e, op);
                let operand = self.expr(operand);
                self.app(head, operand)
            }

            Expr::Tuple(items) => {
                let items: Vec<Aol<Expr>> = self.ast.slice(*items).to_vec();
                let items = items.into_iter().map(|e| self.expr(e)).collect();
                self.add(Term::Tuple(items))
            }

            Expr::List(items) => {
                let items: Vec<Aol<Expr>> = self.ast.slice(*items).to_vec();
                // Every other `[...]` collects its elements into the `@Vec` payload
                // (push left to right) that its construction hook consumes. A sized
                // tensor is the exception the type system forces: its static length
                // cannot ride along in the payload, so `@tensor_stack` flattens the
                // vector into the strided buffer instead.
                let mut acc = self.vec_new();
                for it in items {
                    let x = self.expr(it);
                    acc = self.vec_push(acc, x);
                }
                if self.resolved.tensor_exprs.contains(&e) {
                    let stack = self.var("@tensor_stack");
                    self.app(stack, acc)
                } else {
                    self.literal_hook_wrap(e, acc)
                }
            }

            Expr::Range { lo, hi } => {
                let (lo, hi) = (*lo, *hi);
                match hi {
                    // An open range `[lo ...]` applies its `_range_from` hook.
                    None => {
                        let f = self.hook_fn(e).expect("an open range resolved its hook");
                        let lo = self.expr(lo);
                        self.app(f, lo)
                    }
                    Some(hi) if self.resolved.tensor_exprs.contains(&e) => {
                        // Resolved to a sized tensor: the checker proved the bounds are
                        // literals, so the elements are known here. Push `lo..=hi` into
                        // a vector, then `@tensor_stack` builds the flat strided tensor.
                        let lit = |ast: &Ast, x| match ast.expr(x) {
                            Expr::Int(n) => *n,
                            _ => unreachable!("a range tensor has literal bounds"),
                        };
                        let (l, h) = (lit(self.ast, lo), lit(self.ast, hi));
                        let mut acc = self.vec_new();
                        if h >= l {
                            for v in l..=h {
                                let v = self.int(v);
                                acc = self.vec_push(acc, v);
                            }
                        }
                        let stack = self.var("@tensor_stack");
                        self.app(stack, acc)
                    }
                    // Any other closed range applies its `_range` hook to the bounds.
                    Some(hi) => {
                        let f = self.hook_fn(e).expect("a range resolved its hook");
                        let lo = self.expr(lo);
                        let hi = self.expr(hi);
                        let partial = self.app(f, lo);
                        self.app(partial, hi)
                    }
                }
            }

            Expr::Array { size } => {
                let alloc = self.var("@array_alloc");
                let size = self.expr(*size);
                self.app(alloc, size)
            }

            Expr::Field { record, name } => {
                let (record, name) = (*record, *name);
                let record = self.expr(record);
                self.field(record, name)
            }

            Expr::StructLit { ty, fields, spread } => {
                // The nominal struct name comes from the AST (`Type.{..}`) or, for a
                // bare `.{..}`, from the checker's resolution -- so a positional
                // literal gets the right field-name layout.
                let ty = ty.or_else(|| self.resolved.struct_lit_names.get(&e).copied());
                let spread = *spread;
                self.struct_lit(ty, self.ast.slice(*fields), spread)
            }
            Expr::Record {
                fields,
                with,
                update,
            } => {
                // Checked against a declared struct, the literal builds that
                // struct (the checker recorded which), so lazy slots apply.
                if let Some(&name) = self.resolved.struct_lit_names.get(&e) {
                    return self.struct_lit(Some(name), self.ast.slice(*fields), None);
                }
                // A name-keyed record value. Update (`| base`) and stack (`with
                // base`) build over that base with the listed fields overriding /
                // adding; an anonymous record has no nominal name (access is
                // name-keyed, so the empty name is fine).
                let base = (*update).or(*with).map(|b| self.expr(b));
                let mut out = Vec::new();
                for fi in self.ast.slice(*fields).iter() {
                    if let FieldInit::Named { name, value } = fi {
                        out.push((*name, self.expr(*value)));
                    }
                }
                self.add(Term::Struct {
                    name: StrId::intern(""),
                    base,
                    fields: out,
                })
            }
            Expr::Variant {
                ty, tag, fields, ..
            } => {
                self.variant(*ty, *tag, self.ast.slice(*fields))
            }

            // Note [Case is never lowered to application]
            //
            // Thrax evaluates arguments before a call. A branching form written
            // as an application (`@if c t e`, or `&&` as a function) would
            // evaluate both branches before choosing one, which breaks every
            // recursive definition and every effect in a branch. So `if` (and
            // `&&`/`||`, which the parser rewrites to `if`) and `is` each become
            // one `Term::Case`, a node the engines evaluate lazily: the
            // scrutinee first, then only the chosen arm. No later pass may turn a `Case` back into a call;
            // match compilation keeps the arms behind `Case`s and thunks.
            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                let scrut = self.expr(cond);
                let body = self.expr(then);
                let default = self.expr(alt);
                self.add(Term::Case {
                    scrut,
                    arms: vec![CoreArm {
                        pat: Pat::Bool(true),
                        guard: None,
                        body,
                    }],
                    default: Some(default),
                })
            }

            Expr::Match { scrut, arms } => {
                let scrut = *scrut;
                let mut lowered = Vec::new();
                // Collect handles first so the node borrow does not span the
                // recursive lowering of children.
                let arm_data: Vec<ArmHandles> = self.ast.slice(*arms)
                    .iter()
                    .map(|arm| (self.ast.slice(arm.patterns).to_vec(), arm.guard, arm.body))
                    .collect();
                for (patterns, guard, body) in arm_data {
                    let guard = guard.map(|g| self.expr(g));
                    let body = self.expr(body);
                    for pat in patterns {
                        lowered.push(CoreArm {
                            pat: self.pat(pat),
                            guard,
                            body,
                        });
                    }
                }
                let scrut = self.expr(scrut);
                self.add(Term::Case {
                    scrut,
                    arms: lowered,
                    default: None,
                })
            }

            Expr::Lambda { params, body } => {
                let params: Vec<LamParam> = self.ast.slice(*params).to_vec();
                let body = *body;
                let mut term = self.expr(body);
                for p in params.into_iter().rev() {
                    term = self.lambda_param(p.pat, term);
                }
                term
            }

            Expr::Let { bindings, body } => {
                let bindings: Vec<Binding> = self.ast.slice(*bindings).to_vec();
                let body = *body;
                let mut term = self.expr(body);
                for b in bindings.into_iter().rev() {
                    term = self.binding(&b, term);
                }
                term
            }

            Expr::With { subject, body } => {
                let (subject, body) = (*subject, *body);
                let subject_t = self.expr(subject);
                let body_t = self.expr(body);
                let fields = self.resolved.with_fields.get(&e).cloned().unwrap_or_default();
                self.desugar_with(subject_t, &fields, body_t)
            }

            Expr::Handle { body, handler } => {
                let body = *body;
                let Some(handler) = handler.as_deref() else {
                    return self.expr(body);
                };
                let continuation = handler.continuation;
                let clauses: Vec<(Option<StrId>, StrId, StrId, Aol<Expr>)> = self.ast.slice(handler
                    .clauses)
                    .iter()
                    .map(|clause| (clause.effect, clause.op, clause.arg, clause.body))
                    .collect();
                let default = handler.value;
                let body = self.expr(body);
                let clauses = clauses
                    .into_iter()
                    .map(|(effect, op, arg, body)| CoreClause {
                        effect,
                        op,
                        arg,
                        body: self.expr(body),
                    })
                    .collect();
                let default = default.map(|(name, body)| (name, self.expr(body)));
                self.add(Term::Handle {
                    body,
                    handler: Box::new(CoreHandler {
                        continuation,
                        clauses,
                        default,
                    }),
                })
            }
            Expr::Defer { cleanup, body } => {
                let (cleanup, body) = (*cleanup, *body);
                let cleanup = self.expr(cleanup);
                let body = self.expr(body);
                self.add(Term::Defer { cleanup, body })
            }
            Expr::Builtin => self.fault("`@extern \"thrax\"` is only a definition's whole body"),
            Expr::Extern { abi, symbol, lib } => {
                let (abi, symbol, lib) = (*abi, *symbol, *lib);
                match self.resolved.extern_sigs.get(&e) {
                    Some((_params, arg_types, ret_type)) => {
                        let (arg_types, ret_type) = (arg_types.clone(), *ret_type);
                        self.add(Term::Extern {
                            abi,
                            symbol,
                            lib,
                            arg_types,
                            ret_type,
                        })
                    }
                    // No checker resolution (e.g. an untyped standalone lowering):
                    // an extern with an unknown signature cannot be marshalled.
                    None => self.fault(format!("foreign function `{symbol}` has no resolved type")),
                }
            }

            // A `(@ctx e)` argument is consumed at the head reference it applies to
            // (`ctx_args` holds it as the resolved value), so reaching it here means
            // the checker let a stray one through.
            Expr::CtxArg(_) => self.fault("a `(@ctx ...)` argument out of place"),

            Expr::Ascribe { expr, .. } => self.expr(*expr),
        }
    }

    /// `@vec_new {}`.
    fn vec_new(&mut self) -> Aol<Term> {
        let new = self.var("@vec_new");
        let unit = self.unit();
        self.app(new, unit)
    }

    /// `@vec_push acc x`.
    fn vec_push(&mut self, acc: Aol<Term>, x: Aol<Term>) -> Aol<Term> {
        let push = self.var("@vec_push");
        let partial = self.app(push, acc);
        self.app(partial, x)
    }

    /// Wrap a context-bearing function reference in the application of its resolved
    /// context, which leads the explicit arguments.
    fn apply_ctx(&mut self, site: Aol<Expr>, base: Aol<Term>) -> Aol<Term> {
        let Some(val) = self.resolved.ctx_args.get(&site).cloned() else {
            return base;
        };
        let arg = self.ctx_term(&val);
        self.app(base, arg)
    }

    /// Build the term for a resolved context value.
    fn ctx_term(&mut self, val: &CtxVal) -> Aol<Term> {
        match val {
            CtxVal::Bare(name) => self.var(*name),
            CtxVal::Qualified { module, name } => self.add(Term::Var {
                module: Some(*module),
                name: *name,
            }),
            CtxVal::App(f, x) => {
                let f = self.ctx_term(f);
                let x = self.ctx_term(x);
                self.app(f, x)
            }
            CtxVal::Tuple(parts) => {
                let items = parts.iter().map(|p| self.ctx_term(p)).collect();
                self.add(Term::Tuple(items))
            }
            CtxVal::Proj(base, index) => {
                let base = self.ctx_term(base);
                self.field(base, index.to_string())
            }
            CtxVal::Expr(e) => self.expr(*e),
        }
    }

    /// Resolve an `Expr::Var` to its lowered `(module, name)`: the explicit
    /// qualifier, else the checker's `call_modules` owner.
    fn resolved_var_id(&self, site: Aol<Expr>) -> (Option<StrId>, StrId) {
        let (module, name) = match self.node(site) {
            Expr::Var { module, name } => (*module, *name),
            _ => unreachable!("resolved_var_id on a non-variable"),
        };
        let module = match module {
            Some(m) => Some(self.aliases.get(&m).copied().unwrap_or(m)),
            None => self.resolved.call_modules.get(&site).copied(),
        };
        (self.unless_builtin(module, name), name)
    }

    /// `module`, or `None` when `module.name` is an `@extern "thrax"` declaration:
    /// a bare name is how the runtime reaches a builtin.
    fn unless_builtin(&self, module: Option<StrId>, name: StrId) -> Option<StrId> {
        match module {
            Some(m) if self.resolved.builtins.contains(&(m, name)) => None,
            m => m,
        }
    }

    /// A global reference, qualified when `module` is set.
    fn global(&mut self, module: Option<StrId>, name: StrId) -> Aol<Term> {
        self.add(Term::Var { module, name })
    }

    /// The lowered head term for an `Expr::Var`: the resolved global reference
    /// with any `@ctx` context applied, but WITHOUT the first-class extern
    /// eta-wrapper (a call site applies it directly).
    fn var_head(&mut self, site: Aol<Expr>) -> Aol<Term> {
        // A blessed interface in head position is not a global at all: the checker
        // resolved an instance for this site, and the reference IS that instance's
        // single field.
        // Only a lenient expansion round leaves the instance unresolved, and the
        // strict round rejects the site, so the bare reference below never runs.
        if let Some(&field) = self.resolved.blessed_sites.get(&site) {
            if let Some(ctx) = self.resolved.ctx_args.get(&site).cloned() {
                let instance = self.ctx_term(&ctx);
                return self.field(instance, field);
            }
        }
        let (module, name) = self.resolved_var_id(site);
        if let Some(t) = self.wrapper_projection(site, module, name) {
            return t;
        }
        let base = self.global(module, name);
        self.apply_ctx(site, base)
    }

    /// An interface wrapper applied to its resolved instance IS that instance's
    /// field, so emit the projection and skip the wrapper's three closure
    /// applications. `None` when the global is not a wrapper or the site resolved no
    /// context (a wrapper referenced where its context is passed in explicitly still
    /// goes through `apply_ctx`, which handles every `CtxVal` shape).
    fn wrapper_projection(
        &mut self,
        site: Aol<Expr>,
        module: Option<StrId>,
        name: StrId,
    ) -> Option<Aol<Term>> {
        let field = self.resolved.declarations.wrapper_field_of(module?, name)?;
        let ctx = self.resolved.ctx_args.get(&site).cloned()?;
        let base = self.ctx_term(&ctx);
        Some(self.field(base, field))
    }

    /// Whether `f` is the `@e` compile-time splice in head position.
    fn is_meta_e_head(&self, f: Aol<Expr>) -> bool {
        matches!(self.node(f), Expr::Var { module: None, name } if self.text(*name) == "@e")
    }

    /// Lower an expression-position `@e arg`: `arg` becomes a synthetic global the
    /// driver forces at compile time and splices the returned code in for, and
    /// this site references that global. Shared by direct application and the
    /// `|>` / `<|` pipes (so `x |> @e` and `@e <| x` splice too).
    fn lower_meta_e(&mut self, arg: Aol<Expr>, span: Span) -> Aol<Term> {
        let body = self.expr(arg);
        let name = StrId::intern(&format!("@e_expr#{}", self.meta_evals.len()));
        self.meta_evals.push((name, body, span));
        self.var(name)
    }

    fn is_cast_head(&self, f: Aol<Expr>) -> bool {
        matches!(self.node(f), Expr::Var { module: None, name } if self.text(*name) == "@cast")
    }

    /// Walk a signature type, turning each type-position `@e X` into a synthetic
    /// global (`@e_type#n = X`) forced at compile time; the node's span is recorded
    /// so the driver splices the resulting type source there. Recurses through
    /// every type sub-position an annotation can nest a `@e` in.
    fn collect_meta_types(&mut self, ty: Aol<Ty>) {
        match self.tnode(ty) {
            Ty::MetaE(expr) => {
                let expr = *expr;
                let span = self.ast.ty_span(ty).unwrap_or_else(|| Span::at(0));
                let name = StrId::intern(&format!("@e_type#{}", self.meta_type_evals.len()));
                let term = self.expr(expr);
                self.meta_type_evals.push((name, term, span));
            }
            Ty::App(a, b) | Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
                let (a, b) = (*a, *b);
                self.collect_meta_types(a);
                self.collect_meta_types(b);
            }
            Ty::Sized { size, elem, .. } => {
                let (size, elem) = (*size, *elem);
                self.collect_meta_types(size);
                self.collect_meta_types(elem);
            }
            Ty::Arrow { from, to, .. } => {
                let (from, to) = (*from, *to);
                self.collect_meta_types(from);
                self.collect_meta_types(to);
            }
            Ty::Tuple(items) => {
                for t in self.ast.slice(*items).to_vec() {
                    self.collect_meta_types(t);
                }
            }
            Ty::Record { fields, .. } => {
                for f in self.ast.slice(*fields).to_vec() {
                    self.collect_meta_types(f.ty);
                }
            }
            Ty::Con { .. } | Ty::Var(_) | Ty::Nat(_) | Ty::Unit | Ty::Row(_) => {}
        }
    }

    /// The marshalling plan of the foreign function `site` refers to, if it is a
    /// variable naming one.
    fn extern_params_of(&self, site: Aol<Expr>) -> Option<Vec<utilities::ExternArg>> {
        if !matches!(self.node(site), Expr::Var { .. }) {
            return None;
        }
        let (module, name) = self.resolved_var_id(site);
        self.resolved
            .externs
            .get(&(module?, name))
            .map(|(params, _, _)| params.clone())
    }

    /// Flatten a foreign call `App(ext, arg)`: the record (or tuple) grouping the
    /// C parameters becomes positional arguments, so it is never allocated.
    fn flatten_extern_app(
        &mut self,
        head: Aol<Expr>,
        params: &[utilities::ExternArg],
        arg: Aol<Expr>,
    ) -> Aol<Term> {
        let ext = self.var_head(head);
        // Unit (a single unit argument) or one whole value: a single application.
        if params.is_empty() || matches!(params, [utilities::ExternArg::Whole]) {
            let arg = self.expr(arg);
            return self.app(ext, arg);
        }
        let arg = self.expr(arg);
        self.spread_extern(ext, params, arg)
    }

    /// Apply `ext` to each positional C argument pulled from `arg` per `params`. A
    /// record/tuple literal is deconstructed in place (its fields are already
    /// lowered terms); any other value is bound once and projected.
    fn spread_extern(&mut self, ext: Aol<Term>, params: &[utilities::ExternArg], arg: Aol<Term>) -> Aol<Term> {
        use utilities::ExternArg;
        if let Term::Struct { fields, base: None, .. } = self.terms.get(arg) {
            if params.iter().all(|p| matches!(p, ExternArg::Field(_))) {
                let fields = fields.clone();
                let mut acc = ext;
                for p in params {
                    let ExternArg::Field(name) = p else {
                        unreachable!()
                    };
                    match fields.iter().find(|(n, _)| n == name) {
                        Some((_, t)) => acc = self.app(acc, *t),
                        None => {
                            return self.fault(format!(
                                "foreign call is missing the C argument `{name}`"
                            ))
                        }
                    }
                }
                return acc;
            }
        }
        if let Term::Tuple(items) = self.terms.get(arg) {
            if params.iter().all(|p| matches!(p, ExternArg::Elem(_))) {
                let items = items.clone();
                let mut acc = ext;
                for p in params {
                    let ExternArg::Elem(i) = p else { unreachable!() };
                    match items.get(*i) {
                        Some(t) => acc = self.app(acc, *t),
                        None => {
                            return self.fault(format!("foreign call has no tuple element {i}"))
                        }
                    }
                }
                return acc;
            }
        }
        // A record/tuple variable or expression: bind once, then project each C
        // argument out of the binding.
        let t = self.fresh();
        let mut acc = ext;
        let subject = self.var(t);
        for a in self.project_extern(subject, params) {
            acc = self.app(acc, a);
        }
        self.terms.let_(t, arg, acc)
    }

    /// The positional-argument terms projected from `subject` (a bound
    /// record/tuple value) per `params`.
    fn project_extern(&mut self, subject: Aol<Term>, params: &[utilities::ExternArg]) -> Vec<Aol<Term>> {
        use utilities::ExternArg;
        params
            .iter()
            .map(|p| match p {
                ExternArg::Whole => subject,
                ExternArg::Field(name) => self.field(subject, *name),
                ExternArg::Elem(i) => self.field(subject, i.to_string()),
            })
            .collect()
    }

    /// Eta-expand a first-class multi-argument extern into `\r = ext r.f0 r.f1
    /// ...`, so it keeps its record-taking Thrax type while feeding the wrapper
    /// positional arguments.
    fn eta_extern(&mut self, ext: Aol<Term>, params: &[utilities::ExternArg]) -> Aol<Term> {
        let r = self.fresh();
        let mut acc = ext;
        let subject = self.var(r);
        for a in self.project_extern(subject, params) {
            acc = self.app(acc, a);
        }
        self.terms.lam(r, acc)
    }

    fn binop(&mut self, site: Aol<Expr>, op: StrId, lhs: Aol<Expr>, rhs: Aol<Expr>) -> Aol<Term> {
        match op.as_str() {
            ";" => {
                let name = self.fresh();
                let val = self.expr(lhs);
                let body = self.expr(rhs);
                self.terms.let_(name, val, body)
            }
            // Pipes are application, so they compose with `@e`: `x |> @e` and
            // `@e <| x` fold `x` at compile time just like `@e x`.
            "|>" if self.is_meta_e_head(rhs) => {
                self.lower_meta_e(lhs, self.ast.expr_span(site).unwrap_or(Span::at(0)))
            }
            "<|" if self.is_meta_e_head(lhs) => {
                self.lower_meta_e(rhs, self.ast.expr_span(site).unwrap_or(Span::at(0)))
            }
            "|>" => {
                let f = self.expr(rhs);
                let x = self.expr(lhs);
                self.app(f, x)
            }
            "<|" => {
                let f = self.expr(lhs);
                let x = self.expr(rhs);
                self.app(f, x)
            }
            // The operator resolves like a named call: a built-in use keeps the bare
            // name (a runtime builtin), a CORE or user one carries the module the
            // checker recorded at this site.
            _ => {
                let head = self.operator_head(site, op);
                let l = self.expr(lhs);
                let r = self.expr(rhs);
                let partial = self.app(head, l);
                self.app(partial, r)
            }
        }
    }

    /// The lowered head for an operator at `site`: the module the checker resolved
    /// it to (`None` for a builtin) and the operator's name.
    fn operator_head(&mut self, site: Aol<Expr>, op: StrId) -> Aol<Term> {
        let module = self.unless_builtin(self.resolved.call_modules.get(&site).copied(), op);
        if let Some(t) = self.wrapper_projection(site, module, op) {
            return t;
        }
        let head = self.global(module, op);
        // An operator that takes a context is applied to it first, exactly as a
        // named call is.
        self.apply_ctx(site, head)
    }

    fn lambda_param(&mut self, pat: Aol<Pattern>, body: Aol<Term>) -> Aol<Term> {
        match self.pnode(pat) {
            Pattern::Var(name) => self.terms.lam(*name, body),
            Pattern::Wild => {
                let param = self.fresh();
                self.terms.lam(param, body)
            }
            _ => {
                let param = self.fresh();
                let scrut = self.var(param);
                let arm_pat = self.pat(pat);
                let case = self.case1(scrut, arm_pat, body);
                self.terms.lam(param, case)
            }
        }
    }

    fn binding(&mut self, b: &Binding, body: Aol<Term>) -> Aol<Term> {
        match self.pnode(b.pat) {
            Pattern::Var(name) => {
                let val = self.expr(b.value);
                self.add(Term::Let {
                    name: *name,
                    rec: true,
                    val,
                    body,
                })
            }
            Pattern::Wild => {
                let name = self.fresh();
                let val = self.expr(b.value);
                self.terms.let_(name, val, body)
            }
            _ => {
                let pat = self.pat(b.pat);
                let scrut = self.expr(b.value);
                self.case1(scrut, pat, body)
            }
        }
    }

    fn struct_lit(
        &mut self,
        ty: Option<StrId>,
        fields: &[FieldInit],
        spread: Option<Aol<Expr>>,
    ) -> Aol<Term> {
        let base = spread.map(|s| self.expr(s));

        let named: Option<Vec<StrId>> = fields
            .iter()
            .map(|f| match f {
                FieldInit::Named { name, .. } => Some(*name),
                FieldInit::Positional(_) => None,
            })
            .collect();

        let field_names: Option<Vec<StrId>> = ty
            .and_then(|n| self.resolved.declarations.fields_of(self.module, &self.imports, n))
            .map(<[StrId]>::to_vec)
            .or_else(|| {
                named.as_ref().and_then(|ns| {
                    self.resolved
                        .declarations
                        .struct_by_fields(self.module, &self.imports, ns)
                        .map(|_| ns.clone())
                })
            });

        let name = ty
            .or_else(|| {
                named.and_then(|ns| {
                    self.resolved
                        .declarations
                        .struct_by_fields(self.module, &self.imports, &ns)
                })
            })
            .unwrap_or_else(|| StrId::intern(""));

        // A field the declaration made lazy is suspended here, whatever expression
        // fills it, so the property belongs to the type rather than to this site.
        let lazy: Vec<bool> = self
            .lazy_flags(name, None)
            .map_or_else(Vec::new, <[bool]>::to_vec);
        let slot_of = |fname: StrId, i: usize| {
            field_names
                .as_ref()
                .and_then(|ns| ns.iter().position(|n| *n == fname))
                .unwrap_or(i)
        };
        let mut out = Vec::with_capacity(fields.len());
        for (i, fi) in fields.iter().enumerate() {
            let (fname, value) = match fi {
                FieldInit::Named { name, value } => (*name, *value),
                FieldInit::Positional(value) => (
                    field_names
                        .as_ref()
                        .and_then(|ns| ns.get(i))
                        .copied()
                        .unwrap_or_else(|| StrId::intern(&i.to_string())),
                    *value,
                ),
            };
            let slot = slot_of(fname, i);
            let term = self.expr(value);
            let term = if lazy.get(slot).copied().unwrap_or(false) {
                self.delay(term)
            } else {
                term
            };
            out.push((fname, term));
        }
        self.add(Term::Struct {
            name,
            base,
            fields: out,
        })
    }

    fn variant(&mut self, ty: Option<StrId>, tag: StrId, fields: &[FieldInit]) -> Aol<Term> {
        let none = StrId::intern("");
        let (union, names) = match self.resolved.declarations.variant(self.module, &self.imports, tag) {
            Some((u, ns)) => (u, ns.to_vec()),
            None => (ty.unwrap_or(none), Vec::new()),
        };

        let arity = names.len().max(fields.len());
        let mut slots: Vec<Option<Aol<Term>>> = (0..arity).map(|_| None).collect();
        let mut next = 0;
        for fi in fields {
            match fi {
                FieldInit::Named { name, value } => {
                    let idx = names
                        .iter()
                        .position(|n| *n == Some(*name))
                        .unwrap_or(next);
                    slots[idx] = Some(self.expr(*value));
                }
                FieldInit::Positional(value) => {
                    slots[next] = Some(self.expr(*value));
                    next += 1;
                }
            }
        }
        let ty = if union == none { ty.unwrap_or(none) } else { union };
        // A recursive slot is suspended, so building a cell does not build the
        // whole structure; the machine forces it when an arm scrutinises it.
        let lazy: Vec<bool> = self
            .lazy_flags(ty, Some(tag))
            .map_or_else(Vec::new, <[bool]>::to_vec);
        let mut out = Vec::with_capacity(slots.len());
        for (i, s) in slots.into_iter().enumerate() {
            let t = match s {
                Some(t) => t,
                None => self.unit(),
            };
            out.push(if lazy.get(i).copied().unwrap_or(false) { self.delay(t) } else { t });
        }
        self.add(Term::Variant { ty, tag, fields: out })
    }

    // -- patterns -----------------------------------------------------------

    /// A range pattern's bound as a literal term (the parser guarantees numeric).
    fn range_bound(&mut self, p: Aol<Pattern>) -> Aol<Term> {
        match self.pnode(p) {
            Pattern::Int(n) => self.int(*n),
            Pattern::Real(r) => self.add(Term::Real(*r)),
            _ => self.fault("range bound is not a numeric literal"),
        }
    }

    /// A literal pattern on a user type: build the literal into the type (its
    /// construction hook applied to the raw payload) and compare with the equality
    /// hook. `None` when the checker recorded no hook for this site (the built-in
    /// literal pattern applies instead).
    fn pat_hook_eq(&mut self, p: Aol<Pattern>, raw: Term) -> Option<Pat> {
        let (build, eq) = self.resolved.literal_pattern_hooks.get(&p).cloned()?;
        let build = self.hook_term(&build);
        let eq = self.hook_term(&eq);
        let raw = self.add(raw);
        Some(Pat::HookEq {
            eq,
            value: self.app(build, raw),
        })
    }

    /// One fixed piece of a sequence pattern, matched from the front or (`back`)
    /// the back, with `rest` matching what is left; `None` requires nothing left.
    fn seq_piece(&mut self, piece: Aol<Pattern>, back: bool, rest: Option<Pat>) -> Pat {
        match self.pnode(piece) {
            Pattern::Str(s) => Pat::StrPrefix {
                prefix: *s,
                back,
                rest: Box::new(rest.unwrap_or_else(|| Pat::Str(StrId::intern("")))),
            },
            Pattern::List(elems) => {
                let elems: Vec<Aol<Pattern>> = self.ast.slice(*elems).to_vec();
                let hook = self
                    .resolved
                    .sequence_pattern_hooks
                    .get(&piece)
                    .cloned()
                    .expect("a list pattern resolves a sequence view");
                let view = self.hook_term(&hook);
                Pat::SeqView {
                    view,
                    back,
                    elems: elems.into_iter().map(|e| self.pat(e)).collect(),
                    rest: rest.map(Box::new),
                }
            }
            _ => unreachable!("the parser keeps only literal pieces around a `++` hole"),
        }
    }

    /// A `[..]` or `++` pattern on a sized tensor, its lengths already proven.
    fn tensor_pat(&mut self, p: Aol<Pattern>) -> Pat {
        let len = self.resolved.tensor_patterns[&p];
        let (pre, mid, suf) = match self.pnode(p) {
            Pattern::List(_) => (vec![p], None, Vec::new()),
            Pattern::Concat { pre, mid, suf } => {
                (self.ast.slice(*pre).to_vec(), *mid, self.ast.slice(*suf).to_vec())
            }
            _ => unreachable!("guarded"),
        };
        let mut elems = |pieces: Vec<Aol<Pattern>>| -> Vec<Pat> {
            let mut out = Vec::new();
            for piece in pieces {
                if let Pattern::List(es) = self.pnode(piece) {
                    let es: Vec<Aol<Pattern>> = self.ast.slice(*es).to_vec();
                    out.extend(es.into_iter().map(|e| self.pat(e)));
                }
            }
            out
        };
        let front = elems(pre);
        let back = elems(suf);
        Pat::Tensor {
            len,
            front,
            mid: mid.map(|m| Box::new(self.pat(m))),
            back,
        }
    }

    fn pat(&mut self, p: Aol<Pattern>) -> Pat {
        match self.pnode(p) {
            Pattern::Wild => Pat::Wild,
            Pattern::Var(name) => Pat::Var(*name),
            Pattern::Int(n) => self.pat_hook_eq(p, Term::Int(*n)).unwrap_or(Pat::Int(*n)),
            Pattern::Real(r) => self.pat_hook_eq(p, Term::Real(*r)).unwrap_or(Pat::Real(*r)),
            Pattern::Str(s) => self.pat_hook_eq(p, Term::Str(*s)).unwrap_or(Pat::Str(*s)),
            Pattern::Bool(b) => Pat::Bool(*b),
            Pattern::Range { lo, hi } => Pat::Range {
                lo: self.range_bound(*lo),
                hi: hi.map(|h| self.range_bound(h)),
            },
            Pattern::Tuple(pats) => {
                let pats: Vec<Aol<Pattern>> = self.ast.slice(*pats).to_vec();
                Pat::Tuple(pats.into_iter().map(|p| self.pat(p)).collect())
            }
            Pattern::List(_) | Pattern::Concat { .. }
                if self.resolved.tensor_patterns.contains_key(&p) =>
            {
                self.tensor_pat(p)
            }
            Pattern::List(_) => self.seq_piece(p, false, None),
            // `pre ++ mid ++ suf` nests: the front pieces strip from the outside in,
            // then the back pieces from the last one inward, leaving `mid`.
            Pattern::Concat { pre, mid, suf } => {
                let (pre, mid, suf) = (self.ast.slice(*pre).to_vec(), *mid, self.ast.slice(*suf).to_vec());
                let mut inner = mid.map(|m| self.pat(m));
                for piece in suf.iter() {
                    let rest = inner.take().expect("a back piece has a middle part inside it");
                    inner = Some(self.seq_piece(*piece, true, Some(rest)));
                }
                for piece in pre.iter().rev() {
                    inner = Some(self.seq_piece(*piece, false, inner));
                }
                inner.expect("a `++` pattern has a piece")
            }
            Pattern::Struct { ty, fields } => {
                let names = self
                    .resolved
                    .declarations
                    .fields_of(self.module, &self.imports, *ty)
                    .map(<[StrId]>::to_vec);
                let fields: Vec<FieldPat> = self.ast.slice(*fields).to_vec();
                Pat::Struct {
                    fields: self.field_pats(&fields, names.as_deref()),
                    rest: None,
                }
            }
            // A record pattern matches a name-keyed record/struct by field name.
            // `..name` binds the leftover fields; `.._` (a wild rest) discards them.
            Pattern::Record { fields, rest } => {
                let rest = rest.and_then(|r| match self.pnode(r) {
                    Pattern::Var(name) => Some(*name),
                    _ => None,
                });
                let fields: Vec<FieldPat> = self.ast.slice(*fields).to_vec();
                Pat::Struct {
                    fields: self.field_pats(&fields, None),
                    rest,
                }
            }
            Pattern::Variant {
                ty, tag, fields, ..
            } => {
                let (tag, ty) = (*tag, *ty);
                let names = self
                    .resolved
                    .declarations
                    .variant(self.module, &self.imports, tag)
                    .map(|(_, ns)| ns.to_vec())
                    .or_else(|| {
                        ty.and_then(|t| self.resolved.declarations.fields_of(self.module, &self.imports, t))
                            .map(|ns| ns.iter().copied().map(Some).collect())
                    });
                let fields: Vec<FieldPat> = self.ast.slice(*fields).to_vec();
                let positional = self.variant_field_pats(&fields, names.as_deref());
                Pat::Variant {
                    tag,
                    fields: positional,
                }
            }
        }
    }

    fn field_pats(&mut self, fields: &[FieldPat], names: Option<&[StrId]>) -> Vec<(StrId, Pat)> {
        let mut out = Vec::new();
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => out.push((*name, self.pat(*pat))),
                FieldPat::Shorthand(name) => out.push((*name, Pat::Var(*name))),
                FieldPat::Positional(pat) => {
                    let fname = names
                        .and_then(|ns| ns.get(i))
                        .copied()
                        .unwrap_or_else(|| StrId::intern(&i.to_string()));
                    out.push((fname, self.pat(*pat)));
                }
            }
        }
        out
    }

    fn variant_field_pats(
        &mut self,
        fields: &[FieldPat],
        names: Option<&[Option<StrId>]>,
    ) -> Vec<Pat> {
        let arity = names
            .map(<[_]>::len)
            .unwrap_or(fields.len())
            .max(fields.len());
        let mut slots: Vec<Pat> = (0..arity).map(|_| Pat::Wild).collect();
        let mut next = 0;
        for f in fields {
            match f {
                FieldPat::Named { name, pat } => {
                    let idx = names
                        .and_then(|ns| ns.iter().position(|n| *n == Some(*name)))
                        .unwrap_or(next);
                    slots[idx] = self.pat(*pat);
                }
                FieldPat::Shorthand(name) => {
                    let idx = names
                        .and_then(|ns| ns.iter().position(|n| *n == Some(*name)))
                        .unwrap_or(next);
                    slots[idx] = Pat::Var(*name);
                    next += 1;
                }
                FieldPat::Positional(pat) => {
                    slots[next] = self.pat(*pat);
                    next += 1;
                }
            }
        }
        slots
    }
}

/// Whether a foreign function's parameter is a record/tuple whose fields were
/// flattened into several positional C arguments. Such an extern is N-ary at
/// runtime but record-taking to Thrax, so a first-class reference must eta-expand
/// (a `Whole`/unit extern is already a 1-ary function matching its Thrax type).
fn extern_needs_eta(params: &[utilities::ExternArg]) -> bool {
    matches!(
        params.first(),
        Some(utilities::ExternArg::Field(_) | utilities::ExternArg::Elem(_))
    )
}

/// The number of leading lambdas a body opens with (the explicit parameters the
/// user wrote), so the record-parameter sugar knows which parameters are already
/// bound by hand.
fn leading_lams(terms: &Terms, mut t: Aol<Term>) -> usize {
    let mut n = 0;
    while let Term::Lam { body, .. } = terms.get(t) {
        n += 1;
        t = *body;
    }
    n
}

#[cfg(test)]
mod tests;
