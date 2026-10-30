//! Context (`@ctx`) instance resolution.
//!
//! A definition whose signature opens with a `@ctx` parameter takes that
//! argument implicitly. At each use the checker plans the requirement, then
//! resolves it at the enclosing definition's boundary, once inference has
//! pinned the requirement's type variables. Resolution searches by type: first
//! the context binders in scope, then a flat index of the instances (globals
//! whose type's head is the requirement's nominal head). An instance may itself
//! take a context, so resolution recurses, up to [`CTX_DEPTH`]. One candidate
//! is chosen; none or several is an error naming the candidates.
//!
//! The blessed interfaces (`@IStrLit`, `@ISeqView`, ...) resolve the same way:
//! a literal, range or sequence pattern on a user type finds its instance by
//! type, and desugaring projects the interface's one field.
//!
//! See `documentation/interfaces.md` for the language-level rules.
//!
//! Entry points: [`Checker::plan_ctx`], [`Checker::resolve_pending_ctx`].

use super::*;

// ## Context signatures

impl<'a> Checker<'a> {
    /// Elaborate a signature into the type callers see and, when it opens with a
    /// `@ctx` parameter, that parameter's type. Both are generalized together (as
    /// one tuple) so a variable they share becomes the same `Generic` in each.
    pub(super) fn scheme_and_ctx(&mut self, sig: Aol<Ty>, has_ctx: bool) -> (Type, Option<Type>) {
        self.unifier.enter_level();
        let mut tvars = HashMap::new();
        let full = self.ty_of_ast(sig, &mut tvars);
        self.unifier.leave_level();
        let (ctx, exposed) = if has_ctx {
            match self.unifier.types.node(full) {
                TypeNode::Arrow(from, to, _) => (Some(from), to),
                // A `@ctx` signature is built as an arrow by the parser; an alias
                // could still hide one, in which case there is nothing to strip.
                _ => (None, full),
            }
        } else {
            (None, full)
        };
        let items: Vec<Type> = std::iter::once(exposed).chain(ctx).collect();
        let bundle = self.unifier.types.tuple(items);
        self.unifier.generalize(bundle);
        let zonked = self.unifier.zonk(bundle);
        let TypeNode::Tuple(parts) = self.unifier.types.node(zonked) else {
            unreachable!("packed bundle stays a tuple");
        };
        let parts = self.unifier.types.items(parts).to_vec();
        (parts[0], parts.get(1).copied())
    }

    /// Elaborate a context-bearing signature at a USE site: fresh variables for
    /// its type variables, split into the context requirement and the type the
    /// caller applies. One map for the whole signature keeps the two aligned, so
    /// `Ito_string t -> t -> @Str` ties the requirement to the argument.
    pub(super) fn ctx_use_type(&mut self, sig: Aol<Ty>) -> (Type, Type) {
        let mut tvars = HashMap::new();
        let full = self.ty_of_ast(sig, &mut tvars);
        match self.unifier.types.node(full) {
            TypeNode::Arrow(from, to, _) => (from, to),
            _ => (full, full),
        }
    }

    /// Pack an instance's exposed type with its own context requirement, so one
    /// instantiation keeps their variables aligned.
    pub(super) fn instance_bundle(&mut self, exposed: Type, ctx: Option<Type>) -> Type {
        let items: Vec<Type> = std::iter::once(exposed).chain(ctx).collect();
        self.unifier.types.tuple(items)
    }

    /// A context parameter's type must be a declared nominal type applied to its
    /// arguments. A bare variable or a base type would turn the search into "any
    /// value of this type in scope", which is never what is meant, and the error
    /// belongs at the declaration rather than at every call site.
    pub(super) fn check_ctx_head(&mut self, name: StrId, ctx: Aol<Ty>) -> Result<()> {
        let mut tvars = HashMap::new();
        let ty = self.ty_of_ast(ctx, &mut tvars);
        let zonked = self.unifier.zonk(ty);
        // A bundle of several contexts travels as a tuple; each component carries
        // the same requirement.
        let parts = match self.unifier.types.node(zonked) {
            TypeNode::Tuple(items) => self.unifier.types.items(items).to_vec(),
            _ => vec![zonked],
        };
        for part in parts {
            let ok = self.ctx_head(part).is_some_and(|h| self.is_nominal(h));
            if !ok {
                let span = self.ast.ty_span(ctx).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::TypeMismatch, span, 0,
                    "the `@ctx` parameter of `{name}` must be a declared type, not `{}`",
                    self.show(part)
                )
                .with_note("a context is found by its type, so it needs a named one")
                .with_help("wrap it in a `@struct` whose fields are the operations"));
            }
        }
        Ok(())
    }

    /// Whether a type-constructor name is a declared struct or union (as opposed to
    /// a base type like `@Int` or an arrow).
    pub(super) fn is_nominal(&self, head: utilities::StrId) -> bool {
        self.structs.contains_key(&head) || self.unions.contains_key(&head)
    }
}

// ## Resolution

impl<'a> Checker<'a> {
    /// Plan the context argument of `fname` at use `site`. An explicit `(@ctx e)`
    /// is checked and used at once; otherwise resolution is deferred to the
    /// enclosing definition's boundary, where the requirement's type variables are
    /// pinned (so `max_of 3 7` searches for a context over `@Int` rather than over
    /// a bare variable). The binders that could satisfy it are captured now,
    /// because the scopes are gone by then.
    pub(super) fn plan_ctx(&mut self, site: Aol<Expr>, fname: StrId, req: Type) -> Result<()> {
        if let Some(value) = self.ctx_override.remove(&site) {
            self.check(value, req)?;
            self.ctx_args.insert(site, CtxVal::Expr(value));
            return Ok(());
        }
        let locals = self.ctx_locals(req);
        self.ctx_pending.push(PendingCtx {
            site,
            fname,
            req,
            sig: self.current_sig,
            locals,
        });
        Ok(())
    }

    /// The binders in scope that could satisfy `req`, innermost scope first. A
    /// binder qualifies only when its type's head constructor is already the
    /// requirement's: a binder whose type is still open is not a candidate, because
    /// matching it would decide its type rather than read it.
    pub(super) fn ctx_locals(&mut self, req: Type) -> Vec<CtxLocal> {
        let entries: Vec<(usize, StrId, Type)> = self
            .scopes
            .iter()
            .enumerate()
            .skip(1)
            .flat_map(|(d, s)| s.iter().map(move |(n, t)| (d, *n, *t)))
            .collect();
        let zonked = self.unifier.zonk(req);
        let want = self.ctx_head(zonked);
        if want.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (depth, name, ty) in entries {
            let z = self.unifier.zonk(ty);
            if self.ctx_head(z) == want {
                out.push(CtxLocal { depth, name, ty, index: None });
                continue;
            }
            // A binder holding a BUNDLE offers each of its components, so a function
            // that received several contexts at once can pass one of them onward.
            if let TypeNode::Tuple(items) = self.unifier.types.node(z) {
                let parts = self.unifier.types.items(items).to_vec();
                for (i, part) in parts.into_iter().enumerate() {
                    if self.ctx_head(part) == want {
                        out.push(CtxLocal { depth, name, ty: part, index: Some(i) });
                    }
                }
            }
        }
        out.sort_by(|a, b| b.depth.cmp(&a.depth));
        out
    }

    /// The head constructor of a zonked type: `Ito_string (List t)` has head
    /// `Ito_string`. `None` when the head is not a constructor.
    pub(super) fn ctx_head(&self, ty: Type) -> Option<utilities::StrId> {
        let mut cur = ty;
        loop {
            match self.unifier.types.node(cur) {
                TypeNode::App(h, _) => cur = h,
                TypeNode::Con(n) => return Some(n),
                _ => return None,
            }
        }
    }

    // Note [Ctx resolution fixpoint]
    //
    // A `@ctx` requirement is resolved by its type, but at the use site that
    // type is usually still a variable: in `max_of 3 7` the requirement is
    // `IOrd a` until something says what `a` is. So a use only plans the
    // requirement (`plan_ctx`), and resolution waits for the definition
    // boundary, after its body is inferred.
    //
    // Even there the order matters. Three things pin variables, and each can
    // unblock the others: an operation several effects declare picks its
    // candidate once its argument types are known; a pending result type can
    // flow back to literal operands; and an unconstrained integer literal
    // defaults to `@Int`. `solve_pending` runs those to a fixpoint, trying the
    // next only when the previous made no progress, so a default never wins
    // over information inference could still find. Only then does
    // `resolve_pending_ctx` search for instances, against types that are as
    // pinned as they will get. A requirement whose type is still open there is
    // ambiguous, and reported, rather than resolved by a guess.
    //
    // The variables of the definition's own signature are rigid throughout:
    // the caller chooses them, so resolution may not specialize them to the one
    // instance that happens to exist.

    /// Solve every deferred context, at a definition boundary after overload
    /// solving and numeric defaulting have pinned the types.
    /// See Note [Ctx resolution fixpoint].
    pub(super) fn resolve_pending_ctx(&mut self) -> Result<()> {
        for p in std::mem::take(&mut self.ctx_pending) {
            let req = self.unifier.zonk(p.req);
            let rigid: HashSet<VarId> = match p.sig {
                Some(sig) => {
                    let mut vs = HashSet::new();
                    self.unifier.collect_vars(self.unifier.zonk(sig), &mut vs);
                    vs
                }
                None => HashSet::new(),
            };
            let resolved = self.resolve_ctx(p.fname, req, &p.locals, &rigid, 0);
            // A metaprogram-expansion round: the instance may be injected by a
            // generator that has not run yet, so leave the site unresolved rather
            // than failing. The final strict round rejects it if it is still missing.
            let val = match resolved {
                Ok(val) => val,
                Err(_) if self.lenient => continue,
                Err(d) => {
                    return Err(match self.ast.expr_span(p.site) {
                        Some(span) => d.fill_span(span),
                        None => d,
                    })
                }
            };
            self.ctx_args.insert(p.site, val);
        }
        Ok(())
    }

    /// Resolve one context requirement. A tuple requirement splits into its
    /// components (several contexts travel as one bundle, built here so no global
    /// of the bundled type has to exist). Otherwise the innermost scope offering
    /// exactly one binder of the type wins, then the instances in scope, of which
    /// exactly one must match because global scope is flat. An instance that itself
    /// takes a context has it resolved here too, bounded by [`CTX_DEPTH`].
    pub(super) fn resolve_ctx(
        &mut self,
        fname: StrId,
        req: Type,
        locals: &[CtxLocal],
        rigid: &HashSet<VarId>,
        depth: usize,
    ) -> Result<CtxVal> {
        if depth > CTX_DEPTH {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "resolving the context of `{fname}` did not terminate within {CTX_DEPTH} steps"
            )
            .with_note("an instance whose own context leads back to itself cannot be built")
            .with_help("pass the context explicitly instead"));
        }
        if let TypeNode::Tuple(parts) = self.unifier.types.node(req) {
            let parts = self.unifier.types.items(parts).to_vec();
            let mut vals = Vec::with_capacity(parts.len());
            for part in parts {
                let part = self.unifier.zonk(part);
                vals.push(self.resolve_ctx(fname, part, locals, rigid, depth + 1)?);
            }
            return Ok(CtxVal::Tuple(vals));
        }
        let Some(head) = self.ctx_head(req) else {
            return Err(self.ctx_unresolved(fname, req));
        };
        // The requirement's own rigid variables: a candidate may not bind these, so
        // only something that mentions the same variables (the enclosing
        // definition's context parameter) can satisfy a still-open requirement.
        // Which of the requirement's variables a candidate may not bind. For a LOCAL
        // binder that is all of them: a dictionary in scope must satisfy the
        // requirement as written, never specialize it, or an unrelated operator
        // whose type is not pinned yet (`i + 1` inside a function that takes a
        // dictionary) would silently adopt it. The instance search is allowed to
        // pin a flexible variable, which is how a unique instance decides the type
        // of a literal; it may still never touch a variable of the enclosing
        // signature, which belongs to the caller.
        let all: HashSet<VarId> = {
            let mut vs = HashSet::new();
            self.unifier.collect_vars(req, &mut vs);
            vs
        };
        let open: HashSet<VarId> = all.intersection(rigid).copied().collect();
        let req_vars = all.len();
        let local_closed: HashSet<VarId> = all.union(rigid).copied().collect();
        let depths: Vec<usize> = {
            let mut ds: Vec<usize> = locals.iter().map(|l| l.depth).collect();
            ds.dedup();
            ds
        };
        for d in depths {
            let hits: Vec<CtxLocal> = locals
                .iter()
                .filter(|l| l.depth == d)
                .filter(|l| self.ctx_matches(l.ty, req, &local_closed))
                .cloned()
                .collect();
            if hits.len() > 1 {
                let names: Vec<StrId> = hits.iter().map(|l| l.name).collect();
                return Err(self.ctx_ambiguous(fname, req, &names));
            }
            if let Some(l) = hits.first() {
                let (name, ty, index) = (l.name, l.ty, l.index);
                let inst = self.unifier.instantiate(ty);
                self.unifier.unify(
                    inst,
                    req,
                    &format!("resolving the context of `{fname}`"),
                )?;
                let base = CtxVal::Bare(name);
                return Ok(match index {
                    Some(i) => CtxVal::Proj(Box::new(base), i),
                    None => base,
                });
            }
        }
        let cands = self.instances.get(&head).cloned().unwrap_or_default();
        let mut hits: Vec<Inst> = Vec::new();
        for c in &cands {
            let save = self.unifier.save();
            let inst = self.unifier.instantiate(c.bundle);
            let exposed = self.bundle_parts(inst).0;
            let mut ok = self.unifier.unify(exposed, req, "resolving a context").is_ok();
            if ok {
                ok = self.keeps_open(&open);
            }
            self.unifier.restore(save);
            if ok {
                hits.push(c.clone());
            }
        }
        // With the requirement still open, neither "none in scope" nor "several in
        // scope" is the user's problem: the type is not pinned, so say that instead.
        if req_vars > 0 && hits.len() != 1 {
            return Err(self.ctx_unresolved(fname, req));
        }
        if hits.len() > 1 {
            let names: Vec<StrId> = hits.iter().map(|c| c.name).collect();
            return Err(self.ctx_ambiguous(fname, req, &names));
        }
        let Some(c) = hits.first().cloned() else {
            let d = self.ctx_missing(fname, req);
            return Err(self.explain_near_miss(d, fname, &cands, req));
        };
        let inst = self.unifier.instantiate(c.bundle);
        let (exposed, own) = self.bundle_parts(inst);
        self.unifier.unify(
            exposed,
            req,
            &format!("resolving the context of `{fname}`"),
        )?;
        let base = match c.module {
            Some(m) => CtxVal::Qualified {
                module: m,
                name: c.name,
            },
            None => CtxVal::Bare(c.name),
        };
        if !c.has_ctx {
            return Ok(base);
        }
        let own = self.unifier.zonk(own.expect("an instance with a context packs it"));
        let arg = self.resolve_ctx(c.name, own, locals, rigid, depth + 1)?;
        Ok(CtxVal::App(Box::new(base), Box::new(arg)))
    }

    /// Whether a candidate of type `ty` satisfies `req` without binding any variable
    /// in `closed`. The trial unification is rolled back either way.
    pub(super) fn ctx_matches(&mut self, ty: Type, req: Type, closed: &HashSet<VarId>) -> bool {
        let save = self.unifier.save();
        let inst = self.unifier.instantiate(ty);
        let mut ok = self.unifier.unify(inst, req, "resolving a context").is_ok();
        if ok {
            ok = self.keeps_open(closed);
        }
        self.unifier.restore(save);
        ok
    }

    /// Whether every variable in `closed` is still unbound after a trial
    /// unification. One that has been bound was specialized by the candidate, which
    /// is not this resolution's business: the variable belongs to the caller (a
    /// signature's) or to the requirement itself.
    pub(super) fn keeps_open(&self, closed: &HashSet<VarId>) -> bool {
        closed.iter().all(|v| self.unifier.is_free(*v))
    }

    /// Split an instantiated instance bundle into its exposed type and, when the
    /// instance takes a context of its own, that requirement.
    pub(super) fn bundle_parts(&self, bundle: Type) -> (Type, Option<Type>) {
        match self.unifier.types.node(bundle) {
            TypeNode::Tuple(parts) => {
                let items = self.unifier.types.items(parts);
                (items[0], items.get(1).copied())
            }
            _ => (bundle, None),
        }
    }
}

// ## Resolution failures

impl<'a> Checker<'a> {
    /// No instance matched `req`, but one may agree on its first parameter (the
    /// type the interface is "for") and disagree on another, typically one the
    /// call fixed indirectly, e.g. `for xs \\i = i` pins `IFor`'s element type
    /// through the body's result. Then the context IS determined and simply does
    /// not fit, so say that, naming the instance and the parameter that differs,
    /// rather than claim none is in scope.
    pub(super) fn explain_near_miss(&mut self, d: Diagnostic, fname: StrId, cands: &[Inst], req: Type) -> Diagnostic {
        let (_, want) = self.spine(req);
        let Some(&first) = want.first() else {
            return d;
        };
        for c in cands {
            let save = self.unifier.save();
            let inst = self.unifier.instantiate(c.bundle);
            let exposed = self.bundle_parts(inst).0;
            let (_, has) = self.spine(exposed);
            let mut differs = Vec::new();
            if has.len() == want.len() && self.unifier.unify(has[0], first, "").is_ok() {
                for i in 1..has.len() {
                    let probe = self.unifier.save();
                    if self.unifier.unify(has[i], want[i], "").is_err() {
                        differs.push(i);
                    }
                    self.unifier.restore(probe);
                }
            }
            if differs.is_empty() {
                self.unifier.restore(save);
                continue;
            }
            let mut out = diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`{}` does not fit this use of `{fname}`", c.name;
                note: "`{fname}` needs `{}` here, and `{}` has type `{}`",
                    self.show(req), c.name, self.show(self.unifier.zonk(exposed))
            );
            for i in differs {
                let (h, w) = (self.show(self.unifier.zonk(has[i])), self.show(self.unifier.zonk(want[i])));
                out = out.with_note(format!(
                    "its {} type parameter is `{h}`, but this use needs `{w}`",
                    ordinal(i + 1)
                ));
            }
            self.unifier.restore(save);
            if let Some(sig) = self.show_name(fname) {
                out = out.with_note(format!("`{fname} : {sig}`"));
            }
            return out.with_help(format!(
                "make the arguments of `{fname}` agree with `{}`",
                c.name
            ));
        }
        d
    }

    /// No value of the requirement's type is in scope.
    pub(super) fn ctx_missing(&self, fname: StrId, req: Type) -> Diagnostic {
        // A blessed interface comes from a desugar (`.[i]`, `1.2i`), so the site
        // has no call to pass a context to.
        if is_blessed_interface(fname.as_str()) {
            return diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "no `{fname}` instance of type `{}` is in scope",
                self.show(req);
                help: "define or import a value of type `{}`", self.show(req)
            );
        }
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "no value of type `{}` in scope to satisfy the context of `{fname}`",
            self.show(req)
        )
        .with_help(format!(
            "define or import a value of type `{}`, or pass one explicitly as \
             `{fname} (@ctx value) ...`",
            self.show(req)
        ))
    }

    /// The requirement is still open, so there is nothing to search for. Reported
    /// with both escapes, because neither is obvious from the call alone.
    pub(super) fn ctx_unresolved(&self, fname: StrId, req: Type) -> Diagnostic {
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "the context `{}` of `{fname}` is not determined here",
            self.show(req)
        )
        .with_help(
            "annotate the call so its type is known, or declare the same `@ctx` parameter on \
             the enclosing definition so the context is passed in",
        )
    }

    /// Several values of the requirement's type are in scope. An annotation cannot
    /// break this tie, since the candidates share a type, so the note offers the
    /// explicit form instead.
    pub(super) fn ctx_ambiguous(&self, fname: StrId, req: Type, names: &[StrId]) -> Diagnostic {
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "several values of type `{}` satisfy the context of `{fname}`: {}",
            self.show(req),
            names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
        )
        .with_help(format!(
            "pass the one you mean explicitly as `{fname} (@ctx {}) ...`",
            names.first().map_or("value", |n| n.as_str())
        ))
    }
}

// ## Blessed interfaces

impl<'a> Checker<'a> {
    /// The requirement type, single field name, and field type of blessed interface
    /// `name` applied to `args`. `None` when the interface is not declared in scope,
    /// so a desugar site keeps its built-in meaning.
    pub(super) fn blessed_req(&mut self, name: StrId, args: &[Type]) -> Option<(Type, StrId, Type)> {
        let info = self.structs.get(&name)?.clone();
        if info.params.len() != args.len() || info.fields.len() != 1 {
            return None;
        }
        let mut tvars: HashMap<StrId, Type> = info
            .params
            .iter()
            .copied()
            .zip(args.iter().copied())
            .collect();
        let (field, field_ty) = info.fields[0];
        let field_ty = self.ty_of_ast(field_ty, &mut tvars);
        let mut req = self.unifier.types.con(name);
        for a in args {
            req = self.unifier.types.app(req, *a);
        }
        Some((req, field, field_ty))
    }

    /// Resolve blessed interface `name` applied to `args` at a desugar site: the
    /// instance found by type, plus the type of its single field (what the site's
    /// operands are checked against). A failure leaves the unifier untouched and
    /// returns `None`, so the site falls back to its built-in meaning.
    pub(super) fn blessed(&mut self, name: StrId, args: &[Type]) -> Option<(HookImpl, Type)> {
        let save = self.unifier.save();
        let Some((req, field, field_ty)) = self.blessed_req(name, args) else {
            self.unifier.restore(save);
            return None;
        };
        match self.resolve_blessed(name, req, field) {
            Some(hook) => Some((hook, field_ty)),
            None => {
                self.unifier.restore(save);
                None
            }
        }
    }

    /// The instance satisfying an already-built blessed requirement. Unlike
    /// [`Self::blessed`] a failure is NOT rolled back, so callers that have already
    /// committed unifications probe with their own `save`/`restore` first.
    pub(super) fn resolve_blessed(&mut self, name: StrId, req: Type, field: StrId) -> Option<HookImpl> {
        let locals = self.ctx_locals(req);
        let rigid = HashSet::new();
        self.resolve_ctx(name, req, &locals, &rigid, 0)
            .ok()
            .map(|ctx| HookImpl { ctx, field })
    }

    /// A desugar's blessed-interface call (`.[i]` emits `@IIndex`, `1.2i` emits
    /// `@IImagLit`). The arguments pin the interface's parameters through its
    /// method's declared type; the instance resolves here when that is enough, and
    /// otherwise at the enclosing definition's boundary, once the surrounding types
    /// pin what is left.
    pub(super) fn infer_blessed_app(
        &mut self,
        head: Aol<Expr>,
        name: StrId,
        args: &[Aol<Expr>],
    ) -> Result<Type> {
        let arity = self.structs.get(&name).map(|i| i.params.len()).unwrap_or(0);
        let vars: Vec<Type> = (0..arity).map(|_| self.unifier.fresh()).collect();
        let Some((req, field, method)) = self.blessed_req(name, &vars) else {
            let span = self.ast.expr_span(head).unwrap_or_else(|| Span::at(0));
            return Err(diag!(
                Code::TypeUnbound, span, 0,
                "`{name}` is not declared";
                note: "it is a blessed interface the compiler resolves at this site; \
                       CORE declares it as a one-field `@struct`"
            ));
        };
        let mut rest = method;
        for a in args {
            let (param, next, eff) = self.arrow_parts(rest)?;
            let amb = self.ambient;
            self.unifier.subrow(eff, amb, "in a function application")?;
            self.check(*a, param)?;
            rest = next;
        }
        let probe = self.unifier.save();
        let zonked = self.unifier.zonk(req);
        let resolvable = self
            .resolve_blessed(name, zonked, field)
            .is_some();
        self.unifier.restore(probe);
        if resolvable {
            let zonked = self.unifier.zonk(req);
            let hook = self
                .resolve_blessed(name, zonked, field)
                .expect("the probe above resolved it");
            self.ctx_args.insert(head, hook.ctx);
            self.blessed_sites.insert(head, hook.field);
            return Ok(self.unifier.zonk(rest));
        }
        self.plan_ctx(head, name, req)?;
        self.blessed_sites.insert(head, field);
        Ok(rest)
    }

    /// Every instance of blessed interface `name` in scope, as the diagnostic that
    /// lists what a range can build needs them.
    pub(super) fn blessed_instances(&self, name: StrId) -> Vec<Inst> {
        self.instances.get(&name).cloned().unwrap_or_default()
    }

    /// The head constructor of `ty` if it is a user-declared type (struct / union /
    /// alias), following an application spine (`MyVec Int` -> `MyVec`). A
    /// builtin, a variable, a tuple, or a function returns `None`, so a construction
    /// hook only ever intercepts a literal aimed at a user type.
    pub(super) fn user_type_head(&self, ty: Type) -> Option<StrId> {
        let mut cur = self.unifier.resolve(ty);
        loop {
            match self.unifier.types.node(cur) {
                TypeNode::App(head, _) => cur = self.unifier.resolve(head),
                TypeNode::Con(name) => {
                    return (self.structs.contains_key(&name)
                        || self.unions.contains_key(&name)
                        || self.aliases.contains_key(&name))
                    .then_some(name);
                }
                _ => return None,
            }
        }
    }

    /// Route a literal (`"..."`, an int, a real, `[..]`) through the
    /// blessed `@I*` construction interface whose result type is the expected
    /// one. On a unique match it unifies, checks any element types, records the hook
    /// at `site` for lowering, and returns `true`; otherwise it leaves the unifier
    /// untouched and returns `false`, so the literal falls back to its built-in
    /// default (which lowering folds to a plain constant).
    pub(super) fn literal_hook_check(&mut self, e: Aol<Expr>, expected: Type) -> Result<bool> {
        // The target type must already have a constructor head: with it still open,
        // resolving would DECIDE what the literal is rather than read it.
        let zonked = self.unifier.zonk(expected);
        if self.ctx_head(zonked).is_none() {
            return Ok(false);
        }
        let (name, args, elem): (StrId, _, _) = match self.node(e) {
            Expr::Str(_) => (IF_STR_LIT.into(), vec![expected], None),
            Expr::Int(_) => (IF_INT_LIT.into(), vec![expected], None),
            Expr::Real(_) => (IF_REAL_LIT.into(), vec![expected], None),
            Expr::List(_) => {
                let elem = self.unifier.fresh();
                (IF_SEQ_LIT.into(), vec![elem, expected], Some(elem))
            }
            _ => return Ok(false),
        };
        let Some((hook, _)) = self.blessed(name, &args) else {
            return Ok(false);
        };
        // A sequence literal checks each element against the instance's element type.
        if let (Expr::List(items), Some(elem)) = (self.node(e), elem) {
            let items = self.ast.slice(*items).to_vec();
            for it in items {
                self.check(it, elem)?;
            }
        }
        self.literal_hooks.insert(e, hook);
        Ok(true)
    }

    /// Resolve a range (`[lo ... hi]` / `[lo ...]`) through its `@IRange` /
    /// `@IRangeFrom` instance: the bounds are the instance method's arguments, and
    /// what it returns is the range's type. Records the instance at `site` so
    /// lowering projects and applies it. `expected`, when known, takes part in the
    /// resolution, so a second instance is picked by context.
    pub(super) fn range_hook(
        &mut self,
        site: Aol<Expr>,
        lo: Aol<Expr>,
        hi: Option<Aol<Expr>>,
        expected: Option<Type>,
    ) -> Result<Type> {
        let name = StrId::intern(if hi.is_some() { IF_RANGE } else { IF_RANGE_FROM });
        let bound = self.infer(lo)?;
        if let Some(hi) = hi {
            let t = self.infer(hi)?;
            self.unifier.unify(bound, t, "between a range's bounds")?;
        }
        let result = self.unifier.fresh();
        if let Some(exp) = expected {
            let save = self.unifier.save();
            if self.unifier.unify(result, exp, "in a range").is_err() {
                self.unifier.restore(save);
            }
        }
        // With nothing to pin the result, several instances fit. Rather than call that
        // ambiguous, the FIRST one declared is the default, so CORE's declaration
        // order sets what a bare range builds: a `@Vec` for `[lo ... hi]`, a `Stream`
        // for `[lo ...]`. An annotation or a known expected type still picks another.
        let hook = self
            .blessed(name, &[bound, result])
            .or_else(|| self.blessed_first(name, &[bound, result]));
        let Some((hook, field_ty)) = hook else {
            let offered = self
                .blessed_instances(name)
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let want = self.show(result);
            let note = if offered.is_empty() {
                format!("`{name}` is declared in CORE; a range builds whatever its instance returns")
            } else {
                format!("the `{name}` instances in scope are: {offered}")
            };
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "no `{name}` instance builds a `{want}` from this range";
                note: "{note}"
            ));
        };
        // The method's own type decides how the bounds are typed, so a range over a
        // non-`@Int` bound works exactly as the instance declares it.
        let mut rest = field_ty;
        for arg in std::iter::once(lo).chain(hi) {
            let (param, next, _) = self.arrow_parts(rest)?;
            self.check(arg, param)?;
            rest = next;
        }
        self.unifier.unify(rest, result, "in a range")?;
        self.literal_hooks.insert(site, hook);
        Ok(self.unifier.zonk(result))
    }

    /// The FIRST blessed instance of `name` that satisfies the requirement,
    /// committing it. Used as the default when nothing constrains the result and
    /// several instances fit; the strict unique-match rule is [`Self::blessed`].
    pub(super) fn blessed_first(&mut self, name: StrId, args: &[Type]) -> Option<(HookImpl, Type)> {
        for inst in self.blessed_instances(name) {
            let save = self.unifier.save();
            let Some((req, field, field_ty)) = self.blessed_req(name, args) else {
                self.unifier.restore(save);
                return None;
            };
            let packed = self.unifier.instantiate(inst.bundle);
            let (exposed, own) = self.bundle_parts(packed);
            if self.unifier.unify(exposed, req, "resolving a range").is_err() {
                self.unifier.restore(save);
                continue;
            }
            let base = match inst.module {
                Some(m) => CtxVal::Qualified {
                    module: m,
                    name: inst.name,
                },
                None => CtxVal::Bare(inst.name),
            };
            let ctx = if inst.has_ctx {
                let own = self.unifier.zonk(own.expect("an instance with a context packs it"));
                let locals = self.ctx_locals(own);
                match self.resolve_ctx(inst.name, own, &locals, &HashSet::new(), 0) {
                    Ok(arg) => CtxVal::App(Box::new(base), Box::new(arg)),
                    Err(_) => {
                        self.unifier.restore(save);
                        continue;
                    }
                }
            } else {
                base
            };
            return Some((HookImpl { ctx, field }, field_ty));
        }
        None
    }
}

// ## Pattern hooks

impl<'a> Checker<'a> {
    /// The blessed construction interface for a literal PATTERN kind
    /// (`Str`/`Int`/`Real`), so a literal pattern reuses the same builder a literal
    /// expression does.
    pub(super) fn pattern_literal_hook(pat: &Pattern) -> Option<StrId> {
        Some(match pat {
            Pattern::Str(_) => IF_STR_LIT,
            Pattern::Int(_) => IF_INT_LIT,
            Pattern::Real(_) => IF_REAL_LIT,
            _ => return None,
        }
        .into())
    }

    /// Resolve the sequence view `view` (`@ISeqView` or `@ISeqViewBack`) for a
    /// pattern piece's scrutinee. On success records the hook at `pat` and returns
    /// `Some(element type)`; otherwise `None`, with the unifier untouched.
    pub(super) fn sequence_pattern_hook_check(
        &mut self,
        pat: Aol<Pattern>,
        expected: Type,
        view: StrId,
    ) -> Result<Option<Type>> {
        let elem = self.unifier.fresh();
        let Some((hook, _)) = self.blessed(view, &[expected, elem]) else {
            return Ok(None);
        };
        self.sequence_pattern_hooks.insert(pat, hook);
        Ok(Some(elem))
    }

    /// The element type of a sequence pattern piece's scrutinee, through `view`.
    /// A scrutinee not yet known DEFAULTS to `@Vec elem`. Records the hook at
    /// `pat` either way.
    pub(super) fn sequence_pattern_elem(&mut self, pat: Aol<Pattern>, expected: Type, view: StrId) -> Result<Type> {
        if let Some(elem) = self.sequence_pattern_hook_check(pat, expected, view)? {
            return Ok(elem);
        }
        if self.user_type_head(expected).is_some() || matches!(self.unifier.head(expected), TypeNode::Con(_)) {
            return Err(self.no_sequence_view(expected, view));
        }
        let elem = self.unifier.fresh();
        let vec_con = self.unifier.types.con(ty::VEC);
        let want = self.unifier.types.app(vec_con, elem);
        self.unifier.unify(expected, want, "in a sequence pattern")?;
        Ok(self.sequence_pattern_hook_check(pat, expected, view)?.unwrap_or(elem))
    }

    pub(super) fn no_sequence_view(&self, expected: Type, view: StrId) -> Diagnostic {
        let shown = self.unifier.show(expected);
        let end = if view == IF_SEQ_VIEW { "front" } else { "back" };
        diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "`{shown}` cannot be matched from the {end} by a sequence pattern";
            note: "a sequence pattern steps the scrutinee through its `{view}` instance";
            help: "give `{shown}` an `{view}` instance, or match it another way"
        )
    }

    /// Route a literal pattern (`is "foo"`, `is 42`) whose scrutinee is a user type
    /// through that type's construction + equality hooks: it matches by building the
    /// literal into the user type and comparing with its `IEq` instance.
    /// On success records both hooks at `pat` for lowering and returns `true`;
    /// otherwise leaves the unifier untouched and returns `false` (the caller applies
    /// the built-in literal-pattern typing).
    pub(super) fn literal_pattern_hook_check(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<bool> {
        if self.user_type_head(expected).is_none() {
            return Ok(false);
        }
        let Some(build_name) = Self::pattern_literal_hook(self.pnode(pat)) else {
            return Ok(false);
        };
        let save = self.unifier.save();
        let build = self.blessed(build_name, &[expected]);
        let eq = self.blessed(IF_EQ.into(), &[expected]);
        match (build, eq) {
            (Some((build, _)), Some((eq, _))) => {
                self.literal_pattern_hooks.insert(pat, (build, eq));
                Ok(true)
            }
            _ => {
                self.unifier.restore(save);
                Ok(false)
            }
        }
    }
}

/// `1` -> `first`, for a diagnostic naming a position.
pub(super) fn ordinal(n: usize) -> String {
    match n {
        1 => "first".to_string(),
        2 => "second".to_string(),
        3 => "third".to_string(),
        4 => "fourth".to_string(),
        _ => format!("#{n}"),
    }
}

/// How deep context resolution may chain instances (an instance whose own context
/// is satisfied by another instance) before it is reported as non-terminating.
pub(super) const CTX_DEPTH: usize = 32;

// ## Interface wrappers

/// The field an interface wrapper projects: `body` must be exactly
/// `\d p1 .. pn = d.field p1 .. pn`, with `d` and the parameters distinct variable
/// patterns applied in order. Anything else (`neg`'s `ds.0.sub ds.1.zero x`, a
/// wrapper that reorders or transforms) returns `None`.
pub(super) fn wrapper_field(ast: &Ast, body: Aol<Expr>) -> Option<StrId> {
    let Expr::Lambda { params, body } = ast.expr(body) else {
        return None;
    };
    let params = ast.slice(*params);
    let names: Vec<StrId> = params
        .iter()
        .map(|p| match ast.pat(p.pat) {
            Pattern::Var(n) => Some(*n),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let (ctx, rest) = names.split_first()?;
    // Peel the applications off the spine; they must be the parameters in order.
    let mut args = Vec::new();
    let mut head = *body;
    while let Expr::App(f, x) = ast.expr(head) {
        args.push(*x);
        head = *f;
    }
    args.reverse();
    if args.len() != rest.len() {
        return None;
    }
    for (arg, want) in args.iter().zip(rest) {
        match ast.expr(*arg) {
            Expr::Var { module: None, name } if *name == *want => {}
            _ => return None,
        }
    }
    let Expr::Field { record, name } = ast.expr(head) else {
        return None;
    };
    match ast.expr(*record) {
        Expr::Var { module: None, name: r } if *r == *ctx => Some(*name),
        _ => None,
    }
}
