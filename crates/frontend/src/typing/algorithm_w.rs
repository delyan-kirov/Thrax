//! Type inference for expressions and definitions.
//!
//! The algorithm is Damas-Milner's Algorithm W ("Principal type-schemes for
//! functional programs", POPL 1982), run bidirectionally in the style of Pierce
//! and Turner ("Local type inference", TOPLAS 2000). [`Checker::infer`]
//! synthesizes a type for an expression; [`Checker::check`] pushes an expected
//! type into it, which is how a signature reaches a lambda's parameters, a
//! literal reaches its construction hook, and a record argument is promoted.
//! Both walk the AST once and hand every equality to the [`Unifier`].
//!
//! A definition is checked against its signature: the signature's variables
//! are rigid while the body is checked, so the body may not specialize them.
//!
//! Input: one definition body and its signature. Output: its type, plus the
//! side tables desugaring reads (see [`super::resolved`]).
//!
//! Entry points: [`Checker::infer`], [`Checker::check`].

use super::*;

// ## Definitions against their signatures

impl<'a> Checker<'a> {
    pub(super) fn sig_has_row_tail(&self, sig: Aol<Ty>) -> bool {
        let (mut sites, mut tails) = (Vec::new(), Vec::new());
        sig_var_sites(self.ast, sig, Span::at(0), &mut sites, &mut tails);
        !tails.is_empty()
    }

    /// Unify a definition's variable with the type its signature exposes.
    pub(super) fn bind_signature(&mut self, def: &Def<'a>, decl: Type) -> Result<()> {
        let Some(sig) = def.sig else {
            return Ok(());
        };
        let mut tvars = HashMap::new();
        let sig_ty = self.ty_of_ast(sig, &mut tvars);
        let exposed = match (def.ctx, self.unifier.types.node(sig_ty)) {
            (Some(_), TypeNode::Arrow(_, to, _)) => to,
            _ => sig_ty,
        };
        self.unifier.unify(decl, exposed, &format!("against the signature of `{}`", def.name))
    }

    /// Check a definition's body. For a signed one, also hand back the
    /// variables its signature names, for [`Self::check_sig_vars_rigid`] once the
    /// component is solved.
    pub(super) fn check_def_body(&mut self, def: &Def<'a>, decl: Type) -> Result<Option<SigVars<'a>>> {
        self.ambient = self.top_level_ambient();
        if let Some(sig) = def.sig {
            let mut tvars = HashMap::new();
            let sig_ty = self.ty_of_ast(sig, &mut tvars);
            // The body sees the context parameter (it binds it like any other
            // leading parameter); callers do not, so the definition's own type is
            // the signature with that parameter stripped.
            let exposed = match (def.ctx, self.unifier.types.node(sig_ty)) {
                (Some(_), TypeNode::Arrow(from, to, _)) => {
                    self.decl_ctx.insert(def.name, from);
                    to
                }
                _ => sig_ty,
            };
            self.unifier.unify(
                decl,
                exposed,
                &format!("against the signature of `{}`", def.name),
            )?;
            let saved_sig = self.current_sig.replace(sig_ty);
            let r = self.check_body_against_sig(def.body, sig, sig_ty);
            self.current_sig = saved_sig;
            r?;
            // A row tail stays open on purpose: `<| e>` lets the body perform
            // more, which is how an entry point performs any effect.
            let (mut sites, mut tails) = (Vec::new(), Vec::new());
            sig_var_sites(self.ast, sig, Span::at(0), &mut sites, &mut tails);
            let vars = sites
                .into_iter()
                .filter(|(name, _)| !tails.contains(name))
                .filter_map(|(name, span)| tvars.get(name).map(|&ty| (name, span, ty)))
                .collect();
            Ok(Some(SigVars { def: def.name, vars }))
        } else {
            let inferred = self.infer(def.body)?;
            self.unifier.unify(
                decl,
                inferred,
                &format!("in the definition of `{}`", def.name),
            )?;
            Ok(None)
        }
    }

    /// A signature's variables are the caller's to choose, so the body must
    /// leave each one a variable of its own: one the body fixed to a type, or
    /// merged with another, makes the signature claim more than the definition
    /// gives (#270).
    pub(super) fn check_sig_vars_rigid(&mut self, sig: &SigVars<'a>) -> Result<()> {
        let mut seen: Vec<(VarId, &'a str)> = Vec::new();
        for &(name, span, ty) in &sig.vars {
            let zonked = self.unifier.zonk(ty);
            match self.unifier.types.node(zonked) {
                TypeNode::Var(id) => match seen.iter().find(|(v, _)| *v == id) {
                    Some(&(_, other)) => {
                        return Err(diag!(
                            Code::TypeMismatch, span, 0,
                            "the body of `{}` needs `{name}` and `{other}` to be the same type",
                            sig.def;
                            note: "the caller chooses each of them, so they may differ";
                            help: "write the same variable for both, if that is what the body means"
                        ));
                    }
                    None => seen.push((id, name)),
                },
                _ => {
                    let found = self.show(zonked);
                    return Err(diag!(
                        Code::TypeMismatch, span, 0,
                        "the body of `{}` needs `{name}` to be `{found}`",
                        sig.def;
                        note: "`{name}` is a type variable the caller chooses, so the body must work \
                               for every one";
                        help: "write `{found}` in the signature, if that is what the body means"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Check a body against its signature, implicitly consuming leading record
    /// parameters (`{x: Int, y: Int}` binds its fields directly; a `with` field
    /// also scopes its struct's fields).
    pub(super) fn check_body_against_sig(
        &mut self,
        body: Aol<Expr>,
        sig: Aol<Ty>,
        sig_ty: Type,
    ) -> Result<()> {
        // A bare `@extern` takes its whole argument opaquely (a record parameter is
        // marshalled, not destructured into locals), so it is checked against the
        // FULL signature arrow rather than having its record/unit parameter stripped
        // by the sugar below. This keeps the extern node's type the complete arrow,
        // which `build_extern_specs` reads to plan the marshalling.
        if matches!(self.node(body), Expr::Extern { .. } | Expr::Builtin) {
            return self.check(body, sig_ty);
        }
        // The body's leading lambdas bind the last k of the m signature parameters,
        // so the record-parameter sugar only applies when the lambdas do not reach
        // this one (`k < m`). `\p = p.x` then names a record parameter itself.
        if self.leading_lam_params(body) >= self.arrow_arity_ty(sig) {
            return self.check(body, sig_ty);
        }
        let fields: &[RecField] = match self.tnode(sig) {
            Ty::Arrow { from, .. } => match self.tnode(*from) {
                // A CLOSED record parameter is the destructuring sugar; an open
                // `{ x | r }` is a real row-polymorphic record value, bound as-is.
                Ty::Record { fields, tail: None } => self.ast.slice(*fields),
                // A unit parameter takes no fields: the sugar just introduces the
                // (unused) thunk parameter, so `f : {} -> T = <body>` needs no `\u =`.
                Ty::Unit => &[],
                _ => return self.check(body, sig_ty),
            },
            _ => return self.check(body, sig_ty),
        };
        let to = match self.tnode(sig) {
            Ty::Arrow { to, .. } => *to,
            _ => unreachable!("guarded on an arrow above"),
        };
        let (param_ty, result_ty, eff) = self.arrow_parts(sig_ty)?;
        self.enter_scope();
        self.bind_record_param(fields, param_ty)?;
        // The body may perform this arrow's latent effect (the declared row).
        let saved = std::mem::replace(&mut self.ambient, eff);
        let out = self.check_body_against_sig(body, to, result_ty);
        self.ambient = saved;
        self.leave_scope();
        out
    }

    pub(super) fn bind_record_param(&mut self, fields: &'a [RecField], param_ty: Type) -> Result<()> {
        // A unit parameter (the thunk sugar) binds nothing, so it needs no row.
        if fields.is_empty() {
            return Ok(());
        }
        // The record parameter auto-binds each field name (the "define with an
        // implicit destructuring" sugar). The parameter is a real record type, so
        // look each field up by name in its row.
        let recfields = self.record_fields_of(param_ty)?;
        for f in fields {
            let name = self.text(f.name);
            let t = recfields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| *t)
                .unwrap_or_else(|| self.unifier.fresh());
            self.bind(name, t);
            if f.with {
                self.scope_struct_fields(t)?;
            }
        }
        Ok(())
    }

    /// Bring the struct's fields into scope, returning their names in declaration
    /// order (empty if `ty` is not a known struct). Lowering keys the `with`
    /// desugaring off these names.
    pub(super) fn scope_struct_fields(&mut self, ty: Type) -> Result<Vec<String>> {
        let (head, args) = self.spine(ty);
        let mut names = Vec::new();
        if let TypeNode::Con(name) = self.unifier.types.node(head) {
            if let Some(info) = self.structs.get(self.unifier.types.name(name).as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.unifier);
                for (fname, fty) in &info.fields {
                    let field_ty = self.ty_of_ast(*fty, &mut subst);
                    self.bind(fname, field_ty);
                    names.push(fname.to_string());
                }
            }
        }
        Ok(names)
    }

    pub(super) fn pending_vars(&self) -> HashSet<VarId> {
        let mut out = HashSet::new();
        for p in &self.pending {
            for a in &p.args {
                self.unifier.collect_vars(*a, &mut out);
            }
            self.unifier.collect_vars(p.result, &mut out);
        }
        for (t, _) in &self.numeric {
            self.unifier.collect_vars(*t, &mut out);
        }
        // A variable an unresolved context still mentions must stay monomorphic:
        // generalizing it would quantify the very type the search is waiting for,
        // and a `let square = \x = x * x` would then resolve `IArith` against a
        // quantified variable instead of the `@Int` its use site supplies.
        for p in &self.ctx_pending {
            self.unifier.collect_vars(p.req, &mut out);
        }
        out
    }
}

// ## Checking mode

impl<'a> Checker<'a> {
    /// Check an expression against an expected type (the checking direction).
    ///
    /// Fills in the expression's span the way [`Self::infer`] does, so a
    /// diagnostic raised down here points at the offending expression instead of
    /// the start of the file.
    pub(super) fn check(&mut self, e: Aol<Expr>, expected: Type) -> Result<()> {
        let r = self.check_node(e, expected);
        if let (Some(map), true) = (self.expr_types.as_mut(), r.is_ok()) {
            map.entry(e).or_insert(expected);
        }
        r.map_err(|d| match self.ast.expr_span(e) {
            Some(span) => d.fill_span(span),
            None => d,
        })
    }

    pub(super) fn check_node(&mut self, e: Aol<Expr>, expected: Type) -> Result<()> {
        // A literal aimed at a user type may build it through a blessed `@I*`
        // construction interface; when one applies, that supersedes the built-in default.
        if self.literal_hook_check(e, expected)? {
            return Ok(());
        }
        match self.node(e) {
            Expr::Lambda { params, body } => {
                let params = self.ast.slice(*params);
                self.enter_scope();
                let mut exp = expected;
                let mut body_eff = self.ambient;
                let mut tvars = HashMap::new();
                for p in params.iter() {
                    let (param_ty, rest, eff) = self.arrow_parts(exp)?;
                    if let Some(sig) = p.sig {
                        let sig_ty = self.ty_of_ast(sig, &mut tvars);
                        self.unifier.unify(
                            param_ty,
                            sig_ty,
                            "in a parameter's type annotation",
                        )?;
                    }
                    self.type_pattern(p.pat, param_ty)?;
                    self.check_irrefutable(p.pat, "a lambda parameter")?;
                    exp = rest;
                    body_eff = eff; // the innermost arrow's effect: the body's ambient
                }
                let saved = std::mem::replace(&mut self.ambient, body_eff);
                let out = self.check(*body, exp);
                self.ambient = saved;
                self.leave_scope();
                out
            }
            // `[..]` where a sized tensor `[n]T` is expected: the literal's length
            // fixes `n`, and every element is checked against `T`. Lowering builds
            // a vector.
            Expr::List(items) if self.tensor_parts(expected).is_some() => {
                let items = self.ast.slice(*items);
                let (size, elem) = self.tensor_parts(expected).expect("guarded");
                let n = self.unifier.types.nat(items.len() as u64);
                self.unifier.unify(
                    size,
                    n,
                    "in a tensor literal (its length fixes the size)",
                )?;
                for item in items.iter() {
                    self.check(*item, elem)?;
                }
                self.tensor_exprs.insert(e);
                Ok(())
            }
            // A range `[lo ... hi]` where a sized tensor `[n]T` is expected. The
            // length must be a COMPILE-TIME constant, so the bounds must be literals;
            // it fixes `n = hi - lo + 1` (0 when `hi < lo`). Bounds are checked
            // against the element type, so `[4]Nat = [1 ... 4]` types the ends as Nat.
            Expr::Range { lo, hi } if self.tensor_parts(expected).is_some() => {
                let lo = *lo;
                let Some(hi) = *hi else {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "an open range `[lo ...]` is infinite and cannot build a sized \
                         tensor `[n]T`; it builds a `Stream`"
                    ));
                };
                let (size, elem) = self.tensor_parts(expected).expect("guarded");
                let (Some(l), Some(h)) = (self.int_literal(lo), self.int_literal(hi)) else {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a range building a sized tensor `[n]T` needs literal bounds, \
                         so its length is known at compile time";
                        note: "a compile-time-constant bound (e.g. `let a = 4 in [1 ... a]`) \
                               is not resolved yet";
                        help: "use integer literals, or annotate the \
                               range as a `List` for a runtime-length sequence"
                    ));
                };
                let n = if h >= l { (h - l + 1) as u64 } else { 0 };
                let n = self.unifier.types.nat(n);
                self.unifier.unify(
                    size,
                    n,
                    "in a range tensor (its bounds fix the length)",
                )?;
                self.check(lo, elem)?;
                self.check(hi, elem)?;
                self.tensor_exprs.insert(e);
                Ok(())
            }
            // Any other range resolves its hook against the expected type, so a
            // user sequence's overload is picked by context.
            Expr::Range { lo, hi } => {
                let (lo, hi) = (*lo, *hi);
                let got = self.range_hook(e, lo, hi, Some(expected))?;
                self.unifier.unify_expected(got, expected, "against the expected type")
            }
            // A `.{ .. }` literal (bare or `Type.{ .. }`) checked against an expected
            // type: resolve the struct from the qualifier or the expected type, and
            // pass the expected type down so the parameters are pinned BEFORE fields
            // are checked (bidirectional). A bare positional literal, which carries no
            // field names, depends on this to resolve its struct at all.
            Expr::StructLit { ty, fields, spread } => {
                let (ty, spread) = (*ty, *spread);
                let fields = self.ast.slice(*fields);
                let save = self.unifier.save();
                let name = ty.map(|t| self.text(t)).or_else(|| self.struct_name_of(expected));
                let direct = self
                    .infer_struct_lit(e, name, fields, spread, Some(expected))
                    .and_then(|got| {
                        self.unifier.unify_expected(got, expected, "against the expected type")
                    });
                let Err(err) = direct else {
                    return Ok(());
                };
                // The literal is not that record itself, so try promoting it INTO the
                // record the way a struct-typed variable already promotes: `diag
                // Point.{ .. }` fills a `{with q: Point}` parameter. Only a closed row
                // has known fields to promote into; anything else keeps the direct
                // mismatch, which is the more useful error.
                if !self.record_is_closed(expected) {
                    return Err(err);
                }
                self.unifier.restore(save);
                let got = self.infer_struct_lit(e, ty.map(|t| self.text(t)), fields, spread, None)?;
                self.promote_to_record(e, &[got], expected).map_err(|_| err)
            }
            // A branch does not produce a value of its own, so the expectation
            // passes THROUGH it to each result. Without this, an arm body is only
            // ever inferred, which is why a construction that needs its type from
            // context had to be written `(e : T)` inside `is` or `if`.
            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                let tc = self.infer(cond)?;
                let b = self.unifier.types.con(ty::BOOL);
                self.unifier.unify(tc, b, "in an 'if' condition")?;
                self.check(then, expected)?;
                self.check(alt, expected)
            }
            Expr::Match { scrut, arms } => {
                let ts = self.infer(*scrut)?;
                for arm in self.ast.slice(*arms).iter() {
                    self.enter_scope();
                    let r = (|this: &mut Self| {
                        for pat in this.ast.slice(arm.patterns).iter() {
                            this.type_pattern(*pat, ts)?;
                        }
                        if let Some(guard) = arm.guard {
                            let tg = this.infer(guard)?;
                            let b = this.unifier.types.con(ty::BOOL);
                            this.unifier.unify(tg, b, "in a match guard")?;
                        }
                        this.check(arm.body, expected)
                    })(self);
                    self.leave_scope();
                    r?;
                }
                self.check_coverage(e, ts, *arms)
            }
            // A `{ .f = e, ... }` literal checked against a declared struct builds
            // THAT struct rather than an anonymous row record, so construction
            // consults the type's own slots. That is what gives a recursive field
            // its laziness however the value is written.
            Expr::Record {
                fields,
                with: None,
                update: None,
            } if self.struct_name_of(expected).is_some()
                && self.ast.slice(*fields).iter().all(|f| matches!(f, FieldInit::Named { .. })) =>
            {
                let name = self.struct_name_of(expected).expect("guarded just above");
                let fields = self.ast.slice(*fields);
                self.infer_struct_lit(e, Some(name), fields, None, Some(expected))?;
                Ok(())
            }
            // A bare `.Tag` takes its union from the expected type (type-directed), so
            // a constructor name shared by several unions resolves unambiguously.
            Expr::Variant { module: None, ty: None, tag, fields }
                if self
                    .union_head_with_tag(expected, self.text(*tag))
                    .is_some() =>
            {
                let tag = self.text(*tag);
                let uname = self
                    .union_head_with_tag(expected, tag)
                    .expect("guarded above");
                let fields = self.ast.slice(*fields);
                let got = self.infer_variant(Some(uname), tag, fields)?;
                self.unifier
                    .unify_expected(got, expected, "against the expected type")
            }
            // A `Real` literal takes the expected float width, like an integer
            // literal takes its width: `1.0` checks against `Real32` as well as
            // `Real`/`Real64`. The runtime value stays a `Real` and is narrowed to
            // the field/parameter width at the `@extern` boundary.
            Expr::Real(_) if self.is_float_type(expected) => Ok(()),
            // `@cast x` reinterprets an integer operand at the expected width. It is
            // type-directed: the target integer type comes from the checking context.
            Expr::App(f, arg) if self.is_cast_head(*f) => {
                let arg = *arg;
                self.check_cast(arg, expected)?;
                if let Some(width) = self.sized_width(expected) {
                    self.cast_widths.insert(e, width);
                }
                Ok(())
            }
            // A bare reference to an overloaded name, with no argument types to
            // dispatch on (`(+)` passed as a value, `foldl (+) 0 xs`). The expected
            // type selects the overload, and the choice is recorded at this site so
            // lowering emits the resolved global rather than the bare name. An
            // ambiguity here is deferred like any other, so an expected type that is
            // still an unpinned variable is settled once inference constrains it.
            Expr::Var { module: None, name }
                if self.overloads.contains_key(self.text(*name))
                    && self.lookup(self.text(*name)).is_none()
                    && !self.shadowed_locally(self.text(*name)) =>
            {
                let name = self.text(*name);
                let cands = self.overloads.get(name).cloned().expect("guarded above");
                let got = self.resolve_overload(name, &cands, &[], Some(e))?;
                self.unifier.unify_expected(got, expected, "against the expected type")
            }
            _ => {
                let got = self.infer(e)?;
                // Promotion at an argument position: a bare scalar or a positional
                // tuple passed where a record is expected is wrapped into that record
                // (`foo 1` -> `foo { .x = 1 }`, `foo {1,2}` -> `foo { .x=1, .y=2 }`).
                if let TypeNode::Record(_) = self.unifier.head(expected) {
                    // Try a direct unification first (a record value, or a nominal
                    // struct via the `Con ~ Record` bridge). Skip it for a numeric
                    // literal, whose undefaulted variable would wrongly unify with
                    // the record. If unification fails, promote a scalar / tuple /
                    // struct into a CLOSED record (an open row has no known fields to
                    // promote into, so a mismatch there is a real error).
                    if !self.is_numeric(got) {
                        let save = self.unifier.save();
                        if self.unifier.unify_expected(got, expected, "against the expected type").is_ok() {
                            return Ok(());
                        }
                        self.unifier.restore(save);
                    }
                    if self.record_is_closed(expected) {
                        let g = self.unifier.head(got);
                        let values: Vec<Type> = match g {
                            TypeNode::Tuple(items) => self.unifier.types.items(items).to_vec(),
                            _ => vec![got],
                        };
                        return self.promote_to_record(e, &values, expected);
                    }
                    if let TypeNode::Tuple(_) = self.unifier.head(got) {
                        let help = self.open_record_help(expected);
                        return self
                            .unifier
                            .unify_expected(got, expected, "against the expected type")
                            .map_err(|d| d.with_help(help));
                    }
                }
                self.unifier.unify_expected(got, expected, "against the expected type")
            }
        }
    }

    /// Promote a scalar or a positional tuple to the expected record type, unifying
    /// each value with the field in declaration order and recording the site so
    /// lowering wraps the value into a name-keyed record.
    pub(super) fn promote_to_record(
        &mut self,
        site: Aol<Expr>,
        values: &[Type],
        record_ty: Type,
    ) -> Result<()> {
        let fields = self.record_fields_of(record_ty)?;
        if fields.len() != values.len() {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "cannot pass {} value(s) as a record with {} field(s)",
                values.len(), fields.len()
            ));
        }
        for (val, (_, fty)) in values.to_vec().into_iter().zip(fields.clone()) {
            self.unifier.unify(val, fty, "promoting an argument to a record")?;
        }
        self.promotions
            .insert(site, fields.into_iter().map(|(n, _)| n).collect());
        Ok(())
    }

    /// Whether `ty` is a floating type (`Real`/`Real64`/`Real32`, either spelling),
    /// so a `Real` literal may take its width.
    pub(super) fn is_float_type(&self, ty: Type) -> bool {
        matches!(
            self.unifier.head(ty),
            TypeNode::Con(name)
                if matches!(self.unifier.types.name(name).as_str(), "@Float64" | "@Float32")
        )
    }

    /// Whether `ty` is a sized integer type (any width, signed or unsigned). `@cast`
    /// reinterprets between these; float and non-numeric types are rejected.
    pub(super) fn is_int_scalar(&self, ty: Type) -> bool {
        matches!(self.unifier.head(ty),
            TypeNode::Con(name) if matches!(self.unifier.types.name(name).as_str(),
                "@Int" | "@Nat"
                    | "@Int8" | "@Int16" | "@Int32" | "@Int64"
                    | "@Nat8" | "@Nat16" | "@Nat32" | "@Nat64"))
    }

    /// Whether `f` is the `@cast` intrinsic in head position.
    pub(super) fn is_cast_head(&self, f: Aol<Expr>) -> bool {
        matches!(self.node(f), Expr::Var { module: None, name } if self.text(*name) == "@cast")
    }

    /// Check `@cast x` against the expected integer type. Both engines hold every
    /// integer as one 64-bit value, so lowering wraps `x` into a narrower target's
    /// range (see `cast_widths`) and emits a 64-bit target's operand unchanged. A
    /// numeric literal operand is accepted (it defaults to `@Int`).
    pub(super) fn check_cast(&mut self, arg: Aol<Expr>, expected: Type) -> Result<()> {
        if !self.is_int_scalar(expected) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`@cast` converts between integer widths, but the target type here is `{}`, \
                 not a sized integer",
                self.show(expected)
            ));
        }
        let src = self.infer(arg)?;
        // A still-unresolved operand, e.g. the result of a deferred overload (`a +
        // b` whose resolution waits on numeric defaulting) or a bare numeric
        // literal, gets pinned by later solving. Only the target's width matters,
        // so accept it here rather than reject on an incomplete type.
        if matches!(self.unifier.head(src), TypeNode::Var(_)) {
            return Ok(());
        }
        if !self.is_int_scalar(src) {
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`@cast` expects an integer operand, but got `{}`",
                self.show(src)
            ));
        }
        Ok(())
    }

    /// Decompose a function type into (parameter, result, latent effect). If it is
    /// not yet known to be an arrow, force it to one with fresh parts.
    pub(super) fn arrow_parts(&mut self, ty: Type) -> Result<(Type, Type, Type)> {
        match self.unifier.head(ty) {
            TypeNode::Arrow(from, to, eff) => Ok((from, to, eff)),
            _ => {
                let from = self.unifier.fresh();
                let to = self.unifier.fresh();
                let eff = self.unifier.fresh();
                let want = self.unifier.types.arrow_eff(from, to, eff);
                self.unifier.unify(ty, want, "expected a function")?;
                Ok((from, to, eff))
            }
        }
    }
}

// ## Inference mode

impl<'a> Checker<'a> {
    pub fn infer(&mut self, e: Aol<Expr>) -> Result<Type> {
        let r = self.infer_node(e);
        if let (Some(map), Ok(t)) = (self.expr_types.as_mut(), &r) {
            map.insert(e, *t);
        }
        r.map_err(|d| match self.ast.expr_span(e) {
            Some(span) => d.fill_span(span),
            None => d,
        })
    }

    /// Whether an expression is a literal whose type its construction hook decides
    /// (so checking it against an expected type can build a user type instead).
    pub(super) fn is_literal(&self, e: Aol<Expr>) -> bool {
        matches!(
            self.node(e),
            Expr::Int(_) | Expr::Real(_) | Expr::Str(_) | Expr::List(_)
        )
    }

    pub(super) fn infer_node(&mut self, e: Aol<Expr>) -> Result<Type> {
        match self.node(e) {
            Expr::Int(n) => {
                let t = self.unifier.fresh();
                let span = self.ast.expr_span(e).unwrap_or_else(|| Span::at(0));
                self.numeric.push((t, span));
                self.int_literals.push((t, *n, span));
                self.unifier.mark_int_literal(t);
                Ok(t)
            }
            Expr::Real(_) => Ok(self.unifier.types.con(ty::REAL)),
            Expr::Str(_) => Ok(self.unifier.types.con(ty::STR)),
            Expr::Bool(_) => Ok(self.unifier.types.con(ty::BOOL)),
            Expr::Unit => Ok(self.unifier.types.con(ty::UNIT)),

            Expr::Var { module, name } => {
                let module = module.map(|m| self.text(m));
                let name = self.text(*name);
                self.infer_var(module, name, e)
            }

            Expr::App(..) => self.infer_app(e),

            Expr::Slice { recv, slots } => {
                let (recv, slots) = (*recv, *slots);
                let slots = self.ast.slice(slots);
                self.infer_slice(e, recv, slots)
            }

            Expr::BinOp { op, lhs, rhs } => {
                let (op, lhs, rhs) = (self.text(*op), *lhs, *rhs);
                match op {
                    "<|" if self.is_meta_e(lhs) => return self.infer_meta_e(rhs),
                    "|>" if self.is_meta_e(rhs) => return self.infer_meta_e(lhs),
                    _ => {}
                }
                // An operator that takes a context (`(+)` over `IArith t`) plans it
                // at this site, which is where lowering injects it. Its operands
                // share one type, so the side that is a literal is CHECKED against
                // the other's type rather than inferred on its own: that is what
                // lets a literal reach its type-directed construction hook, as in
                // `3.4 + 1.2i`, where the real literal is built as a `Cpx`.
                let ctx_sig = self.global_ctx.get(op).copied();
                if let Some(sig) = ctx_sig {
                    let (req, exposed) = self.ctx_use_type(sig);
                    self.plan_ctx(e, op, req)?;
                    let owner = self.owner_of(op);
                    self.record_call(Some(e), owner);
                    // Take the operand types from the OPERATOR's signature rather
                    // than from each other: that way a literal is checked against
                    // the parameter the operator declares, which is what lets it
                    // reach its type-directed construction hook, and it stays right
                    // for an operator whose operands differ (`x :: xs`).
                    let (tl, tr) = (self.unifier.fresh(), self.unifier.fresh());
                    let result = self.unifier.fresh();
                    let eff = self.unifier.fresh();
                    let inner = self.unifier.types.arrow_eff(tr, result, eff);
                    let want = self.unifier.types.arrow(tl, inner);
                    self.unifier
                        .unify(exposed, want, &format!("in operator `{op}`"))?;
                    // The operands that FIX types go first, so a literal sees as
                    // much of the signature as possible.
                    for (operand, param) in [(lhs, tl), (rhs, tr)] {
                        if !self.is_literal(operand) {
                            let t = self.infer(operand)?;
                            self.unifier
                                .unify(t, param, &format!("in operator `{op}`"))?;
                        }
                    }
                    for (operand, param) in [(lhs, tl), (rhs, tr)] {
                        if self.is_literal(operand) {
                            self.check(operand, param)?;
                        }
                    }
                    let amb = self.ambient;
                    self.unifier
                        .subrow(eff, amb, &format!("in operator `{op}`"))?;
                    return Ok(result);
                }
                let tl = self.infer(lhs)?;
                let tr = self.infer(rhs)?;
                let op_ty = {
                    let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                    self.unifier.instantiate(scheme)
                };
                let result = self.unifier.fresh();
                // The operator's result arrow may carry a latent effect: `<|` and
                // `|>` pass one through from their function argument (`f <| x` is a
                // call to `f`). Force it into the ambient the way a plain
                // application would. For every other operator this row is empty and
                // the subrow is a no-op.
                let eff = self.unifier.fresh();
                let inner = self.unifier.types.arrow_eff(tr, result, eff);
                let want = self.unifier.types.arrow(tl, inner);
                self.unifier
                    .unify(op_ty, want, &format!("in operator `{op}`"))?;
                let amb = self.ambient;
                self.unifier
                    .subrow(eff, amb, &format!("in operator `{op}`"))?;
                Ok(result)
            }

            Expr::UnOp { op, operand } => {
                let (op, operand) = (self.text(*op), *operand);
                let t = self.infer(operand)?;
                // `-128` is `neg 128`: range-check the literal as the value it denotes.
                if op == "neg" && matches!(self.node(operand), Expr::Int(_)) {
                    let span = self.ast.expr_span(e);
                    if let Some(lit) = self.int_literals.last_mut() {
                        lit.1 = lit.1.wrapping_neg();
                        lit.2 = span.unwrap_or(lit.2);
                    }
                }
                let op_ty = match self.global_ctx.get(op).copied() {
                    Some(sig) => {
                        let (req, exposed) = self.ctx_use_type(sig);
                        self.plan_ctx(e, op, req)?;
                        let owner = self.owner_of(op);
                        self.record_call(Some(e), owner);
                        exposed
                    }
                    None => {
                        let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                        self.unifier.instantiate(scheme)
                    }
                };
                let result = self.unifier.fresh();
                let want = self.unifier.types.arrow(t, result);
                self.unifier.unify(
                    op_ty,
                    want,
                    &format!("in unary `{op}`"),
                )?;
                Ok(result)
            }

            Expr::Tuple(items) => {
                let items = self.ast.slice(*items);
                let mut tys = Vec::with_capacity(items.len());
                for item in items.iter() {
                    tys.push(self.infer(*item)?);
                }
                Ok(self.unifier.types.tuple(tys))
            }

            // An unconstrained `[...]` DEFAULTS to a `@Vec`, then builds it through
            // the same `@ISeqLit` instance a user sequence uses (CORE's `@Vec` one is
            // the identity on the payload).
            Expr::List(items) => {
                let items = self.ast.slice(*items);
                let elem = self.unifier.fresh();
                for item in items.iter() {
                    let t = self.infer(*item)?;
                    self.unifier.unify(elem, t, "in a sequence literal")?;
                }
                let vec_con = self.unifier.types.con(ty::VEC);
                let vec = self.unifier.types.app(vec_con, elem);
                if let Some((hook, _)) = self.blessed(IF_SEQ_LIT, &[elem, vec]) {
                    self.literal_hooks.insert(e, hook);
                }
                Ok(vec)
            }

            // A range builds whatever its `@IRange` / `@IRangeFrom`
            // instance returns (CORE: a `@Vec` for `[lo ... hi]`, a
            // `Stream` for the unbounded `[lo ...]`). The sized-tensor target is
            // reached only through `check` against a `[n]T`.
            Expr::Range { lo, hi } => {
                let (lo, hi) = (*lo, *hi);
                self.range_hook(e, lo, hi, None)
            }

            Expr::If { cond, then, alt } => {
                let (cond, then, alt) = (*cond, *then, *alt);
                let tc = self.infer(cond)?;
                let b = self.unifier.types.con(ty::BOOL);
                self.unifier.unify(tc, b, "in an 'if' condition")?;
                let tt = self.infer(then)?;
                let ta = self.infer(alt)?;
                self.unifier
                    .unify(tt, ta, "between the branches of an 'if'")?;
                Ok(tt)
            }

            Expr::Let { bindings, body } => {
                let (bindings, body) = (self.ast.slice(*bindings), *body);
                self.enter_scope();
                self.infer_let_group(bindings)?;
                let t = self.infer(body);
                self.leave_scope();
                t
            }

            Expr::Lambda { params, body } => {
                let (params, body) = (self.ast.slice(*params), *body);
                self.enter_scope();
                let mut param_tys = Vec::with_capacity(params.len());
                let mut tvars = HashMap::new();
                for p in params.iter() {
                    let pv = match p.sig {
                        Some(sig) => self.ty_of_ast(sig, &mut tvars),
                        None => self.unifier.fresh(),
                    };
                    self.type_pattern(p.pat, pv)?;
                    self.check_irrefutable(p.pat, "a lambda parameter")?;
                    param_tys.push(pv);
                }
                // Constructing the closure performs nothing under the current
                // ambient; the body runs under its own fresh ambient, which becomes
                // the innermost arrow's latent effect. Outer (curried, partial-
                // application) arrows stay pure.
                let e_body = self.unifier.fresh();
                let saved = std::mem::replace(&mut self.ambient, e_body);
                let body_ty = self.infer(body);
                self.ambient = saved;
                self.leave_scope();
                let body_ty = body_ty?;
                let last = param_tys.len() - 1;
                let ty = param_tys.into_iter().enumerate().rev().fold(
                    body_ty,
                    |acc, (i, p)| {
                        if i == last {
                            self.unifier.types.arrow_eff(p, acc, e_body)
                        } else {
                            self.unifier.types.arrow(p, acc)
                        }
                    },
                );
                Ok(ty)
            }

            Expr::Match { scrut, arms } => {
                let ts = self.infer(*scrut)?;
                let result = self.unifier.fresh();
                for arm in self.ast.slice(*arms).iter() {
                    self.enter_scope();
                    for pat in self.ast.slice(arm.patterns).iter() {
                        self.type_pattern(*pat, ts)?;
                    }
                    if let Some(guard) = arm.guard {
                        let tg = self.infer(guard)?;
                        let b = self.unifier.types.con(ty::BOOL);
                        self.unifier.unify(tg, b, "in a match guard")?;
                    }
                    let tb = self.infer(arm.body)?;
                    self.unifier.unify(result, tb, "between match arms")?;
                    self.leave_scope();
                }
                self.check_coverage(e, ts, *arms)?;
                Ok(result)
            }

            Expr::Field { record, name } => {
                let (record, name) = (*record, self.text(*name));
                let rec_ty = self.infer(record)?;
                self.infer_field(rec_ty, name)
            }
            Expr::StructLit { ty, fields, spread } => {
                let (ty, fields, spread) = (ty.map(|t| self.text(t)), self.ast.slice(*fields), *spread);
                self.infer_struct_lit(e, ty, fields, spread, None)
            }
            Expr::Record {
                fields,
                with,
                update,
            } => {
                let fields = self.ast.slice(*fields);
                self.infer_record(fields, *with, *update)
            }
            Expr::Variant {
                module, ty, tag, fields,
            } => {
                let ty = ty.map(|t| self.text(t));
                let tag = self.text(*tag);
                if let Some(d) = self.unqualified_alias_ctor(*module, ty, tag) {
                    let span = self.ast.expr_span(e).unwrap_or(Span::at(0));
                    return Err(d.fill_span(span));
                }
                let fields = self.ast.slice(*fields);
                self.infer_variant(ty, tag, fields)
            }

            Expr::Array { size } => {
                let size = *size;
                let ts = self.infer(size)?;
                let i = self.unifier.types.con(ty::INT);
                self.unifier.unify(ts, i, "in an array size")?;
                Ok(self.unifier.types.con(ty::ARRAY))
            }
            Expr::With { subject, body } => {
                let (subject, body) = (*subject, *body);
                let subject_ty = self.infer(subject)?;
                self.enter_scope();
                let names = self.scope_struct_fields(subject_ty)?;
                self.with_fields.insert(e, names);
                let t = self.infer(body);
                self.leave_scope();
                t
            }
            Expr::Handle { body, handler } => match handler {
                None => self.infer(*body),
                Some(handler) => self.infer_handle(*body, handler),
            },
            Expr::Defer { cleanup, body } => {
                let (cleanup, body) = (*cleanup, *body);
                self.infer(cleanup)?;
                self.infer(body)
            }
            Expr::Extern { .. } => {
                let v = self.unifier.fresh();
                self.extern_tys.insert(e, v);
                Ok(v)
            }
            Expr::Builtin => {
                if self.module_name != "CORE" {
                    let span = self.ast.expr_span(e).unwrap_or_else(|| Span::at(0));
                    return Err(diag!(
                        Code::TypeMismatch, span, 0,
                        "only CORE may declare a builtin with `@extern \"thrax\"`";
                        note: "a builtin is implemented by the compiler and runtime, \
                               not by a library"
                    ));
                }
                Ok(self.unifier.fresh())
            }

            // `(inner : T)`: check `inner` against `T` and take `T` as the type. Any
            // type variable written in `T` is fresh (a local, monomorphic annotation).
            Expr::Ascribe { expr, ty } => {
                let (expr, ty) = (*expr, *ty);
                let mut tvars = HashMap::new();
                let t = self.ty_of_ast(ty, &mut tvars);
                self.check(expr, t).map_err(|d| match self.ast.ty_span(ty) {
                    Some(span) if d.root().code == Code::TypeMismatch => {
                        d.with_label(span, "expected because of this annotation")
                    }
                    _ => d,
                })?;
                Ok(t)
            }

            // A `(@ctx e)` reaching inference on its own is misplaced: it is only
            // meaningful as the first argument of an application, which `infer_app`
            // peels off before the head is inferred.
            Expr::CtxArg(_) => Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`(@ctx ...)` must be the FIRST argument of a call to a function that \
                 declares a `@ctx` parameter"
            )),
        }
    }

    pub(super) fn infer_field_inits(&mut self, fields: &'a [FieldInit]) -> Result<()> {
        for f in fields {
            match f {
                FieldInit::Named { value, .. } => {
                    self.infer(*value)?;
                }
                FieldInit::Positional(v) => {
                    self.infer(*v)?;
                }
            }
        }
        Ok(())
    }

    /// Type a `do body ctl k <clauses> <value arms>` handler. `R` is the result
    /// of the whole handled computation: every clause body and the value arms have
    /// type `R`, and with no value arm the body's own value passes through (`R` is
    /// the body type). In a clause handling `op : Arg -> Res`, the payload `arg` has
    /// type `Arg` and the continuation `k` has type `Res -> R` (a deep handler:
    /// resuming yields the final result).
    pub(super) fn infer_handle(
        &mut self,
        body: Aol<Expr>,
        handler: &'a crate::parser::data::Handler,
    ) -> Result<Type> {
        let result = self.unifier.fresh();

        // The handler discharges the effects its clauses name: the body runs under
        // the ambient extended with each DISTINCT handled effect (several clauses
        // may handle one effect, e.g. get/put both belong to State), the handle
        // expression itself under the outer ambient.
        // Each handled effect gets one instance of its type parameters, shared
        // by every clause and every operation the body performs on it.
        let mut inner = self.ambient;
        let mut instances: HashMap<String, Type> = HashMap::new();
        for clause in self.ast.slice(handler.clauses).iter() {
            let effect = clause.effect.map(|e| self.text(e));
            let op = self.text(clause.op);
            if let Some(eff_name) = self.op_owner(effect, op) {
                if !instances.contains_key(&eff_name) {
                    let n = self.effect_arity.get(eff_name.as_str()).copied().unwrap_or(0);
                    let args = if n == 0 {
                        self.unifier.types.row_empty()
                    } else {
                        let vars: Vec<Type> = (0..n).map(|_| self.unifier.fresh()).collect();
                        self.unifier.types.tuple(vars)
                    };
                    inner = self.unifier.types.row_extend_args(&eff_name, args, inner);
                    instances.insert(eff_name, args);
                }
            }
        }
        let saved = std::mem::replace(&mut self.ambient, inner);
        let body_ty = self.infer(body);
        self.ambient = saved;
        let body_ty = body_ty?;

        for clause in self.ast.slice(handler.clauses).iter() {
            let effect = clause.effect.map(|e| self.text(e));
            let op = self.text(clause.op);
            let owner = self.op_owner(effect, op);
            let shown = match &owner {
                Some(o) => format!("{o}.{op}"),
                None => op.to_string(),
            };
            let (arg_ty, res_ty) = match self.resolve_op_ty(effect, op) {
                Some(scheme) => {
                    let inst = self.unifier.instantiate_for_handler(scheme, &shown);
                    let (a, r, eff) = self.arrow_parts(inst)?;
                    if let (TypeNode::RowExtend(_, op_args, _), Some(args)) =
                        (self.unifier.types.node(eff), owner.and_then(|o| instances.get(&o)))
                    {
                        let args = *args;
                        if !matches!(self.unifier.types.node(op_args), TypeNode::RowEmpty) {
                            self.unifier.unify(op_args, args, "in a handler clause")?;
                        }
                    }
                    (a, r)
                }
                None => (self.unifier.fresh(), self.unifier.fresh()),
            };
            self.enter_scope();
            self.bind(self.text(clause.arg), arg_ty);
            // Deep handler: resuming continues the computation under the outer
            // ambient, so `k : Res -[amb]-> R`.
            let amb = self.ambient;
            let k_ty = self.unifier.types.arrow_eff(res_ty, result, amb);
            self.bind(self.text(handler.continuation), k_ty);
            let cb = self.infer(clause.body).map_err(|d| {
                if !d.root().msg.contains(&format!(" of `{shown}`")) {
                    return d;
                }
                d.with_note(format!(
                    "each use of `{shown}` chooses its own types, so the handler sees them as \
                     opaque: it can pass such a value along, but not make one up or inspect it"
                ))
                .with_help(format!(
                    "make the type a parameter of the effect, which one handler fixes for every \
                     use, or do not resume `{shown}`"
                ))
            })?;
            self.unifier.unify(cb, result, "in a handler clause")?;
            self.leave_scope();
        }
        match &handler.value {
            Some((name, value_body)) => {
                self.enter_scope();
                self.bind(self.text(*name), body_ty);
                let vb = self.infer(*value_body)?;
                self.unifier.unify(vb, result, "in a handler value arm")?;
                self.leave_scope();
            }
            // With no value arm the body's result becomes the handler's result.
            // When they differ the handler needs a value arm to convert it.
            None => {
                if self.unifier.unify(body_ty, result, "in a handled body").is_err() {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "the body produces {}, but the handler's clauses produce {}",
                        self.unifier.show(body_ty), self.unifier.show(result);
                        note: "with no value arm the body's result is returned unchanged";
                        help: "add a `| x => ...` arm to convert it"
                    ));
                }
            }
        }
        Ok(result)
    }

    /// A multi-axis tensor slice `recv.[s0, ...]`. The receiver must be a tensor of
    /// rank >= the slot count; each `Index` slot reduces its axis, each `Range`/`Full`
    /// slot keeps it (a `Range` gets a fresh existential size, modular indexing making
    /// an unknown static size fine). The result wraps the kept axes around whatever
    /// remains below the sliced axes.
    pub(super) fn infer_slice(
        &mut self,
        site: Aol<Expr>,
        recv: Aol<Expr>,
        slots: &'a [SliceSlot],
    ) -> Result<Type> {
        let rt = self.infer(recv)?;
        // A lone `lo ... hi` over a non-tensor receiver is the `@ISlice` interface,
        // which `@Vec` / `@Array` / `@Str` and any user sequence share. A tensor keeps
        // the path below: its result SHAPE is computed from the slots, which no
        // interface method's type can express.
        if let [SliceSlot::Range(lo, hi)] = slots {
            if self.tensor_parts(rt).is_none() {
                let (lo, hi) = (*lo, *hi);
                let result = self.unifier.fresh();
                if let Some((hook, _)) = self.blessed(IF_SLICE, &[rt, result]) {
                    let int = self.unifier.types.con(ty::INT);
                    self.check(lo, int)?;
                    self.check(hi, int)?;
                    self.literal_hooks.insert(site, hook);
                    return Ok(result);
                }
            }
        }
        let elem = self.unifier.fresh();
        let dims: Vec<Type> = (0..slots.len()).map(|_| self.unifier.fresh_nat()).collect();
        // A fresh variance var per sliced axis, so the receiver may have any
        // variance and each kept axis carries its own variance through unchanged.
        let vars: Vec<Type> = (0..slots.len()).map(|_| self.unifier.fresh()).collect();
        let mut expected = elem;
        for i in (0..slots.len()).rev() {
            expected = tensor_type(&self.unifier.types, vars[i], dims[i], expected);
        }
        self.unifier.unify(rt, expected, "slicing a tensor")?;

        let int = self.unifier.types.con(ty::INT);
        let mut kept: Vec<(Type, Type)> = Vec::new();
        for (i, s) in slots.iter().enumerate() {
            match s {
                SliceSlot::Index(x) => self.check(*x, int)?,
                SliceSlot::Range(lo, hi) => {
                    self.check(*lo, int)?;
                    self.check(*hi, int)?;
                    kept.push((vars[i], self.unifier.fresh_nat()));
                }
                SliceSlot::Full => kept.push((vars[i], dims[i])),
            }
        }
        let mut result = elem;
        for (v, d) in kept.iter().rev() {
            result = tensor_type(&self.unifier.types, *v, *d, result);
        }
        Ok(result)
    }

    /// An application whose callee (`head` through `last`, everything left of
    /// `arg`) has the non-function type `ty`.
    pub(super) fn not_a_function(&self, head: Aol<Expr>, last: Aol<Expr>, arg: Aol<Expr>, ty: Type) -> Diagnostic {
        let mut d = diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "this is applied to an argument, but its type `{}` is not a function",
            self.show(ty);
            help: "if the argument is meant as a separate step, put `;` between the two"
        );
        if let (Some(h), Some(l)) = (self.ast.expr_span(head), self.ast.expr_span(last)) {
            d = d.fill_span(Span::new(h.start, l.end.max(h.start)));
        }
        if let Some(x) = self.ast.expr_span(arg) {
            d = d.with_label(x, "the argument");
        }
        d
    }

    pub(super) fn infer_app(&mut self, e: Aol<Expr>) -> Result<Type> {
        let mut args_rev = Vec::new();
        let mut head = e;
        while let Expr::App(f, x) = self.node(head) {
            args_rev.push(*x);
            head = *f;
        }
        args_rev.reverse();
        let mut args = args_rev;

        // An explicit `(@ctx e)` is the first argument. Peel it off and register it
        // against the head's reference site, which is where the context is planned,
        // then let the rest of the call proceed as an ordinary application.
        if let Some(first) = args.first().copied() {
            if let Expr::CtxArg(value) = self.node(first) {
                let value = *value;
                args.remove(0);
                self.ctx_override.insert(head, value);
            }
        }
        if let Some(stray) = args.iter().find(|a| matches!(self.node(**a), Expr::CtxArg(_))) {
            let span = self.ast.expr_span(*stray).unwrap_or_else(|| Span::at(0));
            return Err(diag!(
                Code::TypeMismatch, span, 0,
                "`(@ctx ...)` must be the FIRST argument of the call"
            ));
        }
        let args = args;

        // `@cast` is type-directed: its result width comes from the checking context,
        // handled in `check`. Reaching it here means it has no expected type.
        if self.is_cast_head(head) {
            if args.len() != 1 {
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`@cast` takes exactly one argument"
                ));
            }
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "the target type of `@cast` must be known from context";
                help: "annotate it (e.g. `let n : @Int = @cast x`) or use it where a specific \
                       integer type is expected"
            ));
        }

        if self.is_meta_e(head) {
            if args.len() != 1 {
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`@e` takes exactly one argument"
                ));
            }
            return self.infer_meta_e(args[0]);
        }

        if let Expr::Var { module: None, name } = self.node(head) {
            let name = self.text(*name);
            if is_blessed_interface(name) {
                return self.infer_blessed_app(head, name, &args);
            }
        }

        // An effect operation whose name several effects declare still needs the
        // argument types to pick one (`ask {}` under a `<Reader>` ambient); nothing
        // else dispatches by argument type any more.
        if let Expr::Var { module: None, name } = self.node(head) {
            let name = self.text(*name);
            if !self.shadowed_locally(name) {
                if let Some(cands) = self.overloads.get(name).cloned() {
                    let arg_tys = args
                        .iter()
                        .map(|a| self.infer(*a))
                        .collect::<Result<Vec<_>>>()?;
                    return self.resolve_overload(name, &cands, &arg_tys, Some(head));
                }
            }
        }

        let mut tf = self.infer(head)?;
        let mut callee = head;
        for a in &args {
            if self.unifier.resolve(tf) == self.unifier.types.error() {
                for a in &args {
                    self.infer(*a)?;
                }
                return Ok(tf);
            }
            if !matches!(self.unifier.head(tf), TypeNode::Var(_) | TypeNode::Arrow(..)) {
                return Err(self.not_a_function(head, callee, *a, tf));
            }
            callee = *a;
            let (param, result, eff) = self.arrow_parts(tf)?;
            // The callee may perform at most what the ambient allows; performing
            // an operation (whose latent row is `<Effect | mu>`) forces its effect
            // into the ambient here.
            let amb = self.ambient;
            self.unifier.subrow(eff, amb, "in a function application")?;
            if let Err(d) = self.check(*a, param) {
                return Err(self.explain_task_effect(d, result));
            }
            tf = result;
        }
        Ok(tf)
    }

    /// An argument whose effect is not handled, where the call returns a
    /// `@Future`, is a parallel task performing an effect that cannot cross to
    /// the worker thread: say so, rather than the bare row mismatch.
    pub(super) fn explain_task_effect(&mut self, d: Diagnostic, result: Type) -> Diagnostic {
        let msg = &d.root().msg;
        let Some(label) = msg
            .strip_prefix("effect `")
            .and_then(|m| m.split_once('`'))
            .filter(|(_, rest)| rest.contains("is performed but not handled"))
            .map(|(l, _)| l.to_string())
        else {
            return d;
        };
        let is_future = match self.unifier.head(result) {
            TypeNode::App(head, _) => {
                matches!(self.unifier.head(head), TypeNode::Con(c) if self.unifier.types.name(c) == "@Future")
            }
            _ => false,
        };
        if !is_future {
            return d;
        }
        let io = label == ty::IO;
        let what = if io {
            format!("a pure parallel task performs effect `{label}`")
        } else {
            format!("a parallel task performs effect `{label}`, which cannot cross to another thread")
        };
        d.retitle(what)
        .with_note(
            "a task runs on a worker thread, away from the handlers of the thread that \
             spawned it; `par` takes a pure task, and `spawn` also allows `@IO`",
        )
        .with_help(if io {
            "start it with `spawn`, which allows `@IO`".to_string()
        } else {
            format!(
                "handle `{label}` inside the task, around the code that performs it \
                 (`PAR.spawn_with handler body` and `PAR.par_with` do this)"
            )
        })
    }

    pub(super) fn infer_let_group(&mut self, bindings: &'a [Binding]) -> Result<()> {
        for b in bindings {
            self.infer_binding(b)?;
        }
        Ok(())
    }

    pub(super) fn infer_binding(&mut self, b: &Binding) -> Result<()> {
        self.unifier.enter_level();
        let declared = match self.pnode(b.pat) {
            Pattern::Var(name) => {
                let name = self.text(*name);
                let v = self.unifier.fresh();
                self.bind(name, v);
                Some(v)
            }
            _ => None,
        };
        // With a signature, CHECK the value against it (bidirectional), so the
        // expected type reaches constructs that need it -- e.g. a positional
        // `.{ .. }` literal resolves its struct from the annotation.
        let value_ty = match b.sig {
            Some(sig) => {
                let mut tvars = HashMap::new();
                let sig_ty = self.ty_of_ast(sig, &mut tvars);
                self.check(b.value, sig_ty)?;
                sig_ty
            }
            None => self.infer(b.value)?,
        };
        if let Some(decl) = &declared {
            self.unifier
                .unify(*decl, value_ty, "in a recursive 'let' binding")?;
        }
        self.unifier.leave_level();
        let mono = self.pending_vars();
        match declared {
            Some(decl) => self.unifier.generalize_except(decl, &mono),
            None => {
                self.unifier.generalize_except(value_ty, &mono);
                self.type_pattern(b.pat, value_ty)?;
                self.check_irrefutable(b.pat, "a `let` binding")?;
            }
        }
        Ok(())
    }
}

// ## Variables

impl<'a> Checker<'a> {
    pub(super) fn infer_var(
        &mut self,
        module: Option<&'a str>,
        name: &'a str,
        site: Aol<Expr>,
    ) -> Result<Type> {
        if module.is_none() && name == crate::lexer::HOLE {
            let t = self.unifier.fresh();
            let locals = self.scopes[1..].iter().flat_map(|s| s.iter().map(|(n, t)| (*n, *t))).collect();
            self.holes.push((site, t, locals));
            return Ok(t);
        }
        // A blessed interface in VALUE position is a desugar's reference to the single
        // method of the instance the compiler resolves here (`.[i]` emits `@IIndex`,
        // `1.2i` emits `@IImagLit`). Resolution is deferred like any other context, so
        // the surrounding types pin the instance before the search runs.
        if module.is_none() && is_blessed_interface(name) {
            let arity = self.structs.get(name).map(|i| i.params.len()).unwrap_or(0);
            let args: Vec<Type> = (0..arity).map(|_| self.unifier.fresh()).collect();
            let Some((req, field, field_ty)) = self.blessed_req(name, &args) else {
                let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::TypeUnbound, span, 0,
                    "`{name}` is not declared";
                    note: "it is a blessed interface the compiler resolves at this site; \
                           CORE declares it as a one-field `@struct`"
                ));
            };
            self.plan_ctx(site, name, req)?;
            self.blessed_sites.insert(site, field);
            return Ok(field_ty);
        }
        if let Some(m) = module {
            if self.imported_private.get(m).is_some_and(|s| s.contains(name)) {
                let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::TypeUnbound, span, 0,
                    "`{m}.{name}` is private to module `{m}` and cannot be used from another module"
                ));
            }
            // A qualified reference to a `@ctx`-bearing function plans its context
            // just like a bare one, so `LA.dot u v` injects its dictionary rather
            // than staying an under-applied function.
            if let Some(sig) = self.qualified_ctx.get(&(m, name)).copied() {
                let (req, exposed) = self.ctx_use_type(sig);
                self.plan_ctx(site, name, req)?;
                return Ok(exposed);
            }
            return match self.qualified_candidates(m, name) {
                Some(cands) if cands.len() == 1 => Ok(self.unifier.instantiate(cands[0])),
                Some(_) => Ok(self.unifier.fresh()),
                None if self.lenient || m == self.module_name => Ok(self.unifier.fresh()),
                None => {
                    let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                    Err(self.unknown_qualified(m, name).fill_span(span))
                }
            };
        }
        // Qualify a reference to a global so the interpreter reaches the intended
        // definition rather than a same-named global from another loaded module.
        // A local definition resolves to this module; a single imported value to
        // its owner. Skip names an inner binder shadows, and overloaded names
        // (resolved instead at the application site).
        if !self.shadowed_locally(name) && !self.overloads.contains_key(name) {
            if self.local_defs.contains(name) {
                self.resolved_calls.insert(site, self.module_name);
            } else if let Some(m) = self.value_module.get(name).copied() {
                self.resolved_calls.insert(site, m);
            }
        }
        // A reference to a `@ctx`-bearing global (not shadowed by a local of the
        // same name): instantiate its signature and requirement type with one shared
        // variable map and plan the context. The returned type is the plain arrow, so
        // callers apply only the explicit parameters.
        if !self.shadowed_locally(name) {
            if let Some(sig) = self.global_ctx.get(name).copied() {
                let (req, exposed) = self.ctx_use_type(sig);
                self.plan_ctx(site, name, req)?;
                return Ok(exposed);
            }
        }
        // Several imports bring this name in. Global scope is flat, so the bare form
        // names nothing; say which modules have it and let the user qualify.
        if !self.shadowed_locally(name) && !self.local_defs.contains(name) {
            if let Some(owners) = self.ambiguous_imports.get(name) {
                let list = owners
                    .iter()
                    .map(|m| format!("`{m}.{name}`"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                let span = self.ast.expr_span(site).unwrap_or_else(|| Span::at(0));
                return Err(diag!(
                    Code::AmbiguousName, span, 0,
                    "`{name}` is imported from more than one module";
                    help: "qualify it as {list}"
                ));
            }
        }
        if let Some(scheme) = self.lookup(name) {
            Ok(self.unifier.instantiate(scheme))
        } else if self.overloads.contains_key(name) {
            Ok(self.unifier.fresh())
        } else if self.broken.contains(name) {
            Ok(self.unifier.types.error())
        } else if self.lenient {
            // A metaprogram-expansion round: the name may be injected by a
            // generator that has not run yet. Defer it as a fresh variable; the
            // final strict round rejects it if it is still unbound.
            Ok(self.unifier.fresh())
        } else {
            Err(self.unbound_name(name))
        }
    }

    /// The module that owns a bare global name, so lowering emits a qualified
    /// reference instead of a bare one (which the runtime would read as a builtin).
    pub(super) fn owner_of(&self, name: &str) -> Option<&'a str> {
        if self.local_defs.contains(name) {
            return Some(self.module_name);
        }
        self.value_module.get(name).copied()
    }

    /// Whether `name` is bound by an inner scope (a lambda/`let`/pattern binder),
    /// as opposed to the global scope where imports and top-level defs live.
    pub(super) fn shadowed_locally(&self, name: &str) -> bool {
        self.scopes[1..].iter().any(|s| s.contains_key(name))
    }

    pub(super) fn is_meta_e(&self, e: Aol<Expr>) -> bool {
        matches!(self.node(e), Expr::Var { module: None, name } if self.text(*name) == "@e")
    }

    /// `@e X` runs the `@Code`-valued X at compile time and splices the code in
    /// at this site. What the code denotes is only known once the driver has
    /// spliced it and re-checked, so the site is a fresh variable here (which
    /// the context binds).
    pub(super) fn infer_meta_e(&mut self, arg: Aol<Expr>) -> Result<Type> {
        let ambient = self.meta_ambient();
        let saved = std::mem::replace(&mut self.ambient, ambient);
        let arg_ty = self.infer(arg);
        self.ambient = saved;
        self.require_code(arg_ty?)?;
        Ok(self.unifier.fresh())
    }

    /// The ambient an `@e` operand runs under: a closed `<@Meta>`, so meta ops
    /// discharge there and any other effect is rejected.
    pub(super) fn meta_ambient(&mut self) -> Type {
        let empty = self.unifier.types.row_empty();
        self.unifier.types.row_extend("@Meta", empty)
    }

    /// Require an `@e` operand to be `@Code`: `@e` inserts code and nothing else.
    pub(super) fn require_code(&mut self, found: Type) -> Result<()> {
        let code = self.unifier.types.con("@Code");
        if self.unifier.unify(found, code, "in an `@e` operand").is_ok() {
            return Ok(());
        }
        let shown = self.unifier.show(found);
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "`@e` inserts code, but its operand is `{shown}`, not `@Code`";
            help: "build the code with `@parse_str`/`@parse_items`; a site run only for \
                   its effect returns empty code (`@parse_items \"\"`)"
        ))
    }
}

// ## Records, structs and variants

impl<'a> Checker<'a> {
    pub(super) fn infer_field(&mut self, rec_ty: Type, field: &str) -> Result<Type> {
        let (head, args) = self.spine(rec_ty);
        if let TypeNode::Con(name) = self.unifier.types.node(head) {
            if let Some(info) = self.structs.get(self.unifier.types.name(name).as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.unifier);
                if let Some((_, ty)) = info.fields.iter().find(|(n, _)| *n == field) {
                    return Ok(self.ty_of_ast(*ty, &mut subst));
                }
            }
        }
        if let TypeNode::Tuple(items) = self.unifier.types.node(head) {
            if let Ok(idx) = field.parse::<usize>() {
                if let Some(t) = self.unifier.types.items(items).get(idx) {
                    return Ok(*t);
                }
            }
        }
        // A structural record (an open-row parameter, say): look the field up in
        // the row, growing an open tail to include it.
        if let TypeNode::Record(_) = self.unifier.head(rec_ty) {
            return self
                .unifier
                .record_field(rec_ty, field, &format!("accessing field `{field}`"));
        }
        // The record's type is not known yet, so CONSTRAIN it to have this field
        // rather than guessing: it becomes an open row, which unification solves
        // (and the `Con ~ Record` bridge still lets a nominal struct satisfy it).
        if let TypeNode::Var(_) = self.unifier.head(rec_ty) {
            // A NUMERIC label is ambiguous between a tuple index and a record
            // field, and nothing here can yet say which, so it stays unconstrained
            // until the type is known some other way. This is the one place a
            // field access still fabricates a type.
            if field.parse::<usize>().is_ok() {
                return Ok(self.unifier.fresh());
            }
            let fty = self.unifier.fresh();
            let tail = self.unifier.fresh();
            let row = self.unifier.types.row_field(field, fty, tail);
            let rec = self.unifier.types.record(row);
            self.unifier.unify(rec_ty, rec, &format!("accessing field `{field}`"))?;
            return Ok(fty);
        }
        // Otherwise the type is known and has no such field. Returning a fresh
        // variable here would unify with anything, which is how a typo used to
        // type-check and fault at run time.
        let shown = self.show(rec_ty);
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "`{shown}` has no field `{field}`"
        ))
    }

    pub(super) fn infer_struct_lit(
        &mut self,
        site: Aol<Expr>,
        ty: Option<&'a str>,
        fields: &'a [FieldInit],
        spread: Option<Aol<Expr>>,
        expected: Option<Type>,
    ) -> Result<Type> {
        let mut struct_name = ty.unwrap_or("");
        let (info, result, mut subst) = if let Some(base) = spread {
            let base_ty = self.infer(base)?;
            let (head, args) = self.spine(base_ty);
            match self.unifier.types.node(head) {
                TypeNode::Con(n) if self.structs.contains_key(self.unifier.types.name(n).as_str()) => {
                    let (name, info) = self
                        .structs
                        .get_key_value(self.unifier.types.name(n).as_str())
                        .map(|(k, v)| (*k, v.clone()))
                        .expect("struct present");
                    self.struct_lit_names.insert(site, name.to_string());
                    let subst = subst_from_args(&info.params, &args, &mut self.unifier);
                    (info, base_ty, subst)
                }
                _ => {
                    self.infer_field_inits(fields)?;
                    return Ok(base_ty);
                }
            }
        } else {
            let resolved = match ty {
                Some(n) => self.structs.get_key_value(n).map(|(k, v)| (*k, v.clone())),
                None => self.resolve_struct_by_fields(fields),
            };
            match resolved {
                Some((name, info)) => {
                    self.struct_lit_names.insert(site, name.to_string());
                    struct_name = name;
                    let (args, subst) = self.instantiate_params(&info.params);
                    (info, applied(&self.unifier.types, name, &args), subst)
                }
                // No fresh() escape hatch: a struct literal whose type cannot be
                // determined is a compile error, not a silent runtime fault. The
                // checking direction (an annotation / expected type / qualified
                // `Type.{..}`) resolves it; a bare positional literal cannot.
                None if ty.is_some_and(|n| !self.is_known_type(n)) => {
                    return Err(self.unknown_type(struct_name));
                }
                None if ty.is_some() => {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "`{struct_name}` is not a struct, so `{struct_name}.{{ .. }}` cannot build it"
                    ));
                }
                None => {
                    self.infer_field_inits(fields)?;
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "cannot infer which struct this `.{{ .. }}` builds"
                    )
                    .with_help(
                        "qualify it (`Type.{ .. }`), annotate the binding, or use named fields that match a struct",
                    ));
                }
            }
        };

        // Pin the instantiated parameters to the expected type BEFORE checking the
        // fields, so a field's declared type (e.g. `a`) is the actual parameter
        // variable rather than a fresh placeholder. This lets a value-position `@ctx`
        // dictionary in a field resolve by type (`.fst = blank` picks `blank : a`).
        if let Some(exp) = expected {
            self.unifier.unify_expected(result, exp, "against the expected type")?;
        }
        let mut covered = vec![false; info.fields.len()];
        for (i, fi) in fields.iter().enumerate() {
            let (slot, value) = match fi {
                // A clause the struct has no slot for is an error: skipping it would
                // let a literal claim a type whose shape it does not have.
                FieldInit::Named { name, value } => {
                    let name = self.text(*name);
                    match info.fields.iter().position(|(n, _)| *n == name) {
                        Some(slot) => (slot, *value),
                        None => {
                            self.infer(*value)?;
                            return Err(diag!(
                                Code::TypeMismatch, Span::at(0), 0,
                                "struct `{struct_name}` has no field `{name}`"
                            ));
                        }
                    }
                }
                FieldInit::Positional(value) if i < info.fields.len() => (i, *value),
                FieldInit::Positional(value) => {
                    self.infer(*value)?;
                    let n = info.fields.len();
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "struct `{struct_name}` has {n} fields, so there is no field {i}"
                    ));
                }
            };
            covered[slot] = true;
            // Check (not infer) so a `[..]` literal field takes its element/size or
            // Array-ness from the declared field type (bidirectional), like a call arg.
            let want = self.ty_of_ast(info.fields[slot].1, &mut subst);
            self.check(value, want)?;
        }
        // A C union's members share offset 0, so a literal picks exactly one of
        // them; every other literal stands for a whole value and must give every
        // field. An update (`.{ .. | base }`) takes the rest from its base.
        if spread.is_none() {
            if info.c_union {
                let n = fields.len();
                if n != 1 {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "C union `{struct_name}` is built from exactly one member, not {n}"
                    ));
                }
            } else if let Some(slot) = covered.iter().position(|c| !c) {
                let missing = info.fields[slot].0;
                return Err(diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "struct `{struct_name}` literal is missing field `{missing}`";
                    note: "a literal gives every field";
                    help: "to change only some, update another value with \
                           `.{{ .field = v | base }}`"
                ));
            }
        }
        Ok(result)
    }

    pub(super) fn infer_variant(
        &mut self,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &'a [FieldInit],
    ) -> Result<Type> {
        let union = match ty {
            Some(n) => Some(n),
            None => self.find_union_by_tag(tag),
        };
        let resolved = union.and_then(|u| self.variant_sig(u, tag));
        // No fresh() escape hatch, for the same reason `infer_struct_lit` has none:
        // a fresh variable unifies with anything, so an unresolvable constructor
        // would type-check here and fault at run time.
        let Some((result, payload)) = resolved else {
            self.infer_field_inits(fields)?;
            return Err(match ty {
                Some(n) if !self.unions.contains_key(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`{n}` is not a union, so it has no constructor `{tag}`"
                ),
                Some(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "union `{n}` has no constructor `{tag}`"
                ),
                None => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "no union has a constructor `{tag}`";
                    note: "a `.Tag` builds a variant of some declared `@union`"
                ),
            });
        };
        let label = variant_label(union, tag);
        let mut covered = vec![false; payload.len()];
        for (i, fi) in fields.iter().enumerate() {
            let (slot, value) = match fi {
                FieldInit::Named { name, value } => {
                    let name = self.text(*name);
                    match payload.iter().position(|(n, _)| *n == Some(name)) {
                        Some(slot) => (slot, *value),
                        None => {
                            self.infer(*value)?;
                            return Err(diag!(
                                Code::TypeMismatch, Span::at(0), 0,
                                "constructor `{label}` has no field `{name}`"
                            ));
                        }
                    }
                }
                FieldInit::Positional(value) if i < payload.len() => (i, *value),
                FieldInit::Positional(value) => {
                    self.infer(*value)?;
                    let n = payload.len();
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "constructor `{label}` takes {n} field(s), so there is no field {i}"
                    ));
                }
            };
            covered[slot] = true;
            // Check (not infer) against the declared payload type, so the
            // expectation flows into a nested value: a bare `.Tag` payload resolves
            // type-directedly, and a literal takes its construction hook / element type.
            self.check(value, payload[slot].1)?;
        }
        // Every payload slot must be given: a constructor builds a whole value, and
        // a missing slot would leave the variant holding something its type denies.
        if let Some(slot) = covered.iter().position(|c| !c) {
            let n = payload.len();
            let which = match payload[slot].0 {
                Some(name) => format!("`{name}`"),
                None => format!("at index {slot}"),
            };
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "constructor `{label}` takes {n} field(s) and is missing the one {which}"
            ));
        }
        Ok(result)
    }

    /// Infer an anonymous record value. Plain `{ .x = e }` builds a closed row from
    /// the fields' inferred types; `{ .x = e, with base }` stacks its fields on
    /// `base`'s (row concat); `{ .x = e | base }` updates `base` (its shape is
    /// preserved, each listed field must already exist).
    pub(super) fn infer_record(
        &mut self,
        fields: &'a [FieldInit],
        with: Option<Aol<Expr>>,
        update: Option<Aol<Expr>>,
    ) -> Result<Type> {
        let mut explicit: Vec<(String, Type)> = Vec::new();
        for fi in fields {
            match fi {
                FieldInit::Named { name, value } => {
                    let t = self.infer(*value)?;
                    explicit.push((self.text(*name).to_string(), t));
                }
                FieldInit::Positional(value) => {
                    self.infer(*value)?;
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a record field needs a name (`.field = value`)"
                    ));
                }
            }
        }
        if let Some(base) = update {
            // Update preserves the base's shape (open or closed): each listed field
            // must resolve in the base, and the result type is the base's.
            let base_ty = self.infer(base)?;
            for (n, got) in &explicit {
                let want = self.field_type_of(base_ty, n)?;
                self.unifier.unify(*got, want, "in a record update")?;
            }
            return Ok(base_ty);
        }
        if let Some(w) = with {
            // Stack: prepend the explicit fields onto the base's row, keeping the
            // base's tail (so stacking onto an open row stays open).
            let wty = self.infer(w)?;
            let row = self.record_row_of(wty)?;
            let full = explicit
                .into_iter()
                .rev()
                .fold(row, |rest, (n, t)| self.unifier.types.row_field(&n, t, rest));
            return Ok(self.unifier.types.record(full));
        }
        Ok(self.unifier.types.record_of(explicit.into_iter()))
    }

    /// Whether `ty` is a record whose row is closed (ends in `RowEmpty`, so its
    /// fields are fully known) rather than open (a tail variable).
    /// How to pass a value where the open record `ty` is expected: by naming its
    /// fields, since only a closed record takes values by position.
    pub(super) fn open_record_help(&self, ty: Type) -> String {
        let mut labels = Vec::new();
        if let TypeNode::Record(row) = self.unifier.head(ty) {
            let mut cur = row;
            while let TypeNode::RowField(label, _, rest) = self.unifier.head(cur) {
                labels.push(format!(".{} = ...", self.unifier.types.name(label)));
                cur = rest;
            }
        }
        format!(
            "a record type with an open row (`| r`) takes its fields by name, as in `{{ {} }}`; \
             only a closed record takes values by position",
            labels.join(", ")
        )
    }

    pub(super) fn record_is_closed(&self, ty: Type) -> bool {
        let TypeNode::Record(row) = self.unifier.head(ty) else {
            return false;
        };
        let mut cur = row;
        loop {
            match self.unifier.head(cur) {
                TypeNode::RowField(_, _, rest) => cur = rest,
                TypeNode::RowEmpty => return true,
                _ => return false,
            }
        }
    }
}

// ## Operations several effects declare

impl<'a> Checker<'a> {
    pub(super) fn resolve_overload(
        &mut self,
        name: &str,
        candidates: &[Cand<'a>],
        args: &[Type],
        site: Option<Aol<Expr>>,
    ) -> Result<Type> {
        let result = self.unifier.fresh();
        match self.match_overload(candidates, args, result) {
            Match::Unique(idx) => {
                let cand = candidates[idx].clone();
                self.apply_overload(cand.ty, args, result)?;
                let module = candidates[idx].module;
                self.record_call(site, module);
                Ok(result)
            }
            // In a lenient expansion round the matching overload may be injected
            // by a generator that has not run yet (a derived `to_string` for a new
            // type); leave the result an unresolved variable rather than erroring.
            Match::None if self.lenient => Ok(result),
            Match::None => Err(self.no_overload(name, args, site)),
            Match::Ambiguous => {
                self.pending.push(Pending {
                    name: name.to_string(),
                    candidates: candidates.to_vec(),
                    args: args.to_vec(),
                    result: result,
                    site,
                });
                Ok(result)
            }
        }
    }

    /// Note that a bare call `site` resolved to `module`, so lowering can qualify
    /// it. A builtin/local candidate (`module` is `None`) needs no annotation.
    pub(super) fn record_call(&mut self, site: Option<Aol<Expr>>, module: Option<&'a str>) {
        if let (Some(site), Some(module)) = (site, module) {
            self.resolved_calls.insert(site, module);
        }
    }

    /// Which candidate the argument types select: exactly one is a match, none is a
    /// miss, several is an ambiguity the caller defers.
    pub(super) fn match_overload(&mut self, candidates: &[Cand<'a>], args: &[Type], result: Type) -> Match {
        let mut matched = None;
        let mut count = 0;
        for (idx, cand) in candidates.iter().enumerate() {
            let save = self.unifier.save();
            let ok = self.apply_overload(cand.ty, args, result).is_ok();
            self.unifier.restore(save);
            if ok {
                count += 1;
                matched = Some(idx);
            }
        }
        match count {
            1 => Match::Unique(matched.expect("a match was recorded")),
            0 => Match::None,
            _ => Match::Ambiguous,
        }
    }

    /// Solve the pending effect-operation uses to a fixpoint, defaulting numeric
    /// literals only when nothing else makes progress.
    /// See Note [Ctx resolution fixpoint].
    pub(super) fn solve_pending(&mut self) -> Result<()> {
        loop {
            let batch = std::mem::take(&mut self.pending);
            let mut progress = false;
            let mut still = Vec::new();
            for p in batch {
                match self.match_overload(&p.candidates, &p.args, p.result) {
                    Match::Unique(idx) => {
                        let cand = p.candidates[idx].clone();
                        self.apply_overload(cand.ty, &p.args, p.result)?;
                        let module = p.candidates[idx].module;
                        self.record_call(p.site, module);
                        progress = true;
                    }
                    // Lenient round: a still-unmatched overload may be satisfied by
                    // an about-to-be-injected definition; drop it rather than error.
                    Match::None if self.lenient => {}
                    Match::None => return Err(self.no_overload(&p.name, &p.args, p.site)),
                    Match::Ambiguous => still.push(p),
                }
            }
            self.pending = still;
            if progress {
                continue;
            }
            if self.propagate_result_to_operands()? {
                continue;
            }
            if self.default_numerics()? {
                continue;
            }
            if let Some(p) = self.pending.first() {
                let mut mods: Vec<&str> = p.candidates.iter().filter_map(|c| c.module).collect();
                mods.sort_unstable();
                mods.dedup();
                let err = diag!(
                    Code::AmbiguousName, Span::at(0), 0,
                    "ambiguous overloaded use of `{}`", p.name
                );
                let err = match mods.as_slice() {
                    [a, b, ..] => err
                        .with_note(format!(
                            "several imported modules define `{name}` with a matching type",
                            name = p.name
                        ))
                        .with_help(format!(
                        "qualify just this reference to pick one, e.g. `{a}.{name}` or `{b}.{name}` \
                         (the rest of the module keeps using the bare name)",
                        name = p.name
                    )),
                    _ => err,
                };
                let err = match p.site.and_then(|s| self.ast.expr_span(s)) {
                    Some(span) => err.fill_span(span),
                    None => err,
                };
                return Err(err);
            }
            return Ok(());
        }
    }

    /// Break an ambiguity where a pending overload's RESULT is already a concrete
    /// integer type but its operands are still unconstrained numeric literals. For
    /// homogeneous arithmetic (`@Int32 + @Int32 -> @Int32`), the operands should
    /// take the result type, not default to `@Int`; e.g. `let x : @Int32 = 2 + 3`
    /// resolves to the `@Int32` overload rather than failing because both literals
    /// became `@Int`. Each candidate is tried under that constraint and kept only if
    /// it makes the overload unique, so a genuinely wrong guess is rolled back.
    pub(super) fn propagate_result_to_operands(&mut self) -> Result<bool> {
        let batch = std::mem::take(&mut self.pending);
        let mut progress = false;
        let mut still = Vec::new();
        for p in batch {
            let result = self.unifier.resolve(p.result);
            if !self.is_int_scalar(result) {
                still.push(p);
                continue;
            }
            let save = self.unifier.save();
            let mut pinned = true;
            for a in &p.args {
                if matches!(self.unifier.head(*a), TypeNode::Var(_))
                    && self
                        .unifier
                        .unify(*a, result, "numeric operand adopting result type")
                        .is_err()
                {
                    pinned = false;
                    break;
                }
            }
            if pinned {
                if let Match::Unique(idx) = self.match_overload(&p.candidates, &p.args, p.result) {
                    let cand_ty = p.candidates[idx].ty;
                    self.apply_overload(cand_ty, &p.args, p.result)?;
                    let module = p.candidates[idx].module;
                    self.record_call(p.site, module);
                    progress = true;
                    continue;
                }
            }
            self.unifier.restore(save);
            still.push(p);
        }
        self.pending = still;
        Ok(progress)
    }

    pub(super) fn no_overload(&self, name: &str, args: &[Type], site: Option<Aol<Expr>>) -> Diagnostic {
        let shown: Vec<String> = args.iter().map(|a| self.show(*a)).collect();
        let mut d = diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "no viable overload of `{name}` for argument types ({})",
            shown.join(", ")
        );
        if let Some(span) = site.and_then(|s| self.ast.expr_span(s)) {
            d = d.fill_span(span);
        }
        d
    }

    pub(super) fn apply_overload(&mut self, candidate: Type, args: &[Type], result: Type) -> Result<()> {
        let mut f = self.unifier.instantiate(candidate);
        for a in args {
            let next = self.unifier.fresh();
            let eff = self.unifier.fresh();
            let want = self.unifier.types.arrow_eff(*a, next, eff);
            self.unifier.unify(f, want, "in an overloaded application")?;
            // An effectful operation resolved by overload still injects its effect
            // into the ambient (same as a plain call; see `infer_app`).
            let amb = self.ambient;
            self.unifier.subrow(eff, amb, "in an overloaded application")?;
            f = next;
        }
        self.unifier.unify(f, result, "in an overloaded application")
    }
}

// ## Numeric literals

impl<'a> Checker<'a> {
    /// Whether `ty` resolves to a not-yet-defaulted numeric-literal variable.
    pub(super) fn is_numeric(&self, ty: Type) -> bool {
        let r = self.unifier.resolve(ty);
        matches!(self.unifier.types.node(r), TypeNode::Var(_))
            && self.numeric.iter().any(|(n, _)| self.unifier.resolve(*n) == r)
    }

    /// Default each still-unconstrained numeric literal to `@Int`. Whether any
    /// changed. See Note [Integer literals never become floats].
    pub(super) fn default_numerics(&mut self) -> Result<bool> {
        let vars = std::mem::take(&mut self.numeric);
        let mut changed = false;
        for (t, span) in &vars {
            match self.unifier.head(*t) {
                // Still unconstrained: default to `Int`.
                TypeNode::Var(_) => {
                    self.unifier
                        .unify(*t, self.unifier.types.con(ty::INT), "defaulting an integer literal")?;
                    changed = true;
                }
                // Pinned to a numeric type by use: fine.
                TypeNode::Con(name) if is_numeric_type(&self.unifier.types.name(name)) => {}
                // Pinned to a genuinely UNKNOWN con (a typo'd type): let that type's
                // own "unknown type" diagnostic surface instead of the numeric error.
                TypeNode::Con(name) if !self.is_known_type(&self.unifier.types.name(name)) => {}
                // Pinned to a known non-numeric type (e.g. `if 1` wants `@Bool`): a
                // bare literal is a number, so this is a type error, not a coercion.
                _ => {
                    let shown = self.show(*t);
                    let d = diag!(
                        Code::TypeMismatch, *span, 0,
                        "a numeric literal cannot be used where `{}` is expected",
                        shown
                    );
                    // A closed record would have taken the literal as its one field,
                    // so a record here has an open row.
                    return Err(match self.unifier.head(*t) {
                        TypeNode::Record(_) => d.with_help(self.open_record_help(*t)),
                        _ => d,
                    });
                }
            }
        }
        Ok(changed)
    }

    /// Reject an integer literal outside its resolved type's range, such as `300`
    /// as a `@Nat8`. A value that does not fit is not one of the type's values.
    pub(super) fn check_int_literals(&mut self) -> Result<()> {
        for (t, n, span) in std::mem::take(&mut self.int_literals) {
            let Some(dom) = self.int_type_domain(t) else {
                continue;
            };
            let v = n as i128;
            if v < dom.0 || dom.1 < v {
                let ty = self.show(t);
                return Err(diag!(
                    Code::IntLiteralRange, span, 0,
                    "integer literal {v} is out of range for `{ty}` ({} to {})", dom.0, dom.1;
                    help: "use a wider type, or `@cast` to wrap a wider value into `{ty}`"
                ));
            }
        }
        Ok(())
    }

    /// Whether a built-in integer type under 64 bits is signed, and its width.
    pub(super) fn sized_width(&self, ty: Type) -> Option<(bool, u32)> {
        let TypeNode::Con(name) = self.unifier.head(ty) else {
            return None;
        };
        Some(match self.unifier.types.name(name).as_str() {
            "@Int8" => (true, 8),
            "@Int16" => (true, 16),
            "@Int32" => (true, 32),
            "@Nat8" => (false, 8),
            "@Nat16" => (false, 16),
            "@Nat32" => (false, 32),
            _ => return None,
        })
    }

    /// Each sub-64-bit `@cast` site's target signedness and width.
    pub fn cast_widths(&self) -> &HashMap<Aol<Expr>, (bool, u32)> {
        &self.cast_widths
    }
}

/// The variables a definition's signature names, each with the span of its
/// first occurrence and the type it elaborated to.
pub(super) struct SigVars<'a> {
    def: &'a str,
    vars: Vec<(&'a str, Span, Type)>,
}

/// Collect the type variables appearing in `ty`, in order of first appearance.
/// Every variable a signature names, in order of first occurrence, with the
/// span of the type it occurs in (the nearest one with a span, falling back to
/// `outer`). The names of effect-row and record tails go to `tails` instead.
pub(super) fn sig_var_sites<'a>(
    ast: &'a Ast,
    ty: Aol<Ty>,
    outer: Span,
    out: &mut Vec<(&'a str, Span)>,
    tails: &mut Vec<&'a str>,
) {
    let span = ast.ty_span(ty).unwrap_or(outer);
    let row = |row: &EffectRow, out: &mut Vec<(&'a str, Span)>, tails: &mut Vec<&'a str>| {
        for entry in ast.slice(row.entries) {
            ast.slice(entry.args).iter().for_each(|a| sig_var_sites(ast, *a, span, out, tails));
        }
        tails.extend(row.tail.map(|t| ast.text(t)));
    };
    match ast.ty(ty) {
        Ty::Var(name) => {
            let name = ast.text(*name);
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, span));
            }
        }
        Ty::App(a, b) | Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            sig_var_sites(ast, *a, span, out, tails);
            sig_var_sites(ast, *b, span, out, tails);
        }
        Ty::Arrow { from, effect, to } => {
            sig_var_sites(ast, *from, span, out, tails);
            if let Some(effect) = effect {
                row(effect, out, tails);
            }
            sig_var_sites(ast, *to, span, out, tails);
        }
        Ty::Tuple(items) => {
            ast.slice(*items).iter().for_each(|t| sig_var_sites(ast, *t, span, out, tails))
        }
        Ty::Record { fields, tail } => {
            ast.slice(*fields).iter().for_each(|f| sig_var_sites(ast, f.ty, span, out, tails));
            tails.extend(tail.map(|t| ast.text(t)));
        }
        Ty::Sized { size, elem, .. } => {
            sig_var_sites(ast, *size, span, out, tails);
            sig_var_sites(ast, *elem, span, out, tails);
        }
        Ty::Row(r) => row(r, out, tails),
        Ty::Con { .. } | Ty::Nat(_) | Ty::Unit | Ty::MetaE(..) => {}
    }
}

/// Select a payload field's type by name (if named) or by position.
/// A pattern names a field the type does not declare. `what` is "struct" or
/// "constructor"; `owner` is its name (`Union.Tag` for a constructor).
pub(super) fn no_such_field(what: &str, owner: &str, field: &str) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "{what} `{owner}` has no field `{field}`"
    )
}

/// A pattern binds more positional fields than the type has.
pub(super) fn no_field_at(what: &str, owner: &str, index: usize, n: usize) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "{what} `{owner}` has {n} field(s), so there is no field {index}"
    )
}

/// How a constructor is named in a diagnostic: `Union.Tag` when the union is
/// known, otherwise the bare tag (a `.Tag` whose union never resolved).
pub(super) fn variant_label(union: Option<&str>, tag: &str) -> String {
    match union {
        Some(u) => format!("{u}.{tag}"),
        None => tag.to_string(),
    }
}

pub(super) fn variant_field_ty(payload: &VariantPayload, name: Option<&str>, index: usize) -> Option<Type> {
    match name {
        Some(name) => payload
            .iter()
            .find(|(n, _)| *n == Some(name))
            .map(|(_, t)| *t),
        None => payload.get(index).map(|(_, t)| *t),
    }
}

pub(super) fn unbound(name: &str) -> Diagnostic {
    diag!(Code::TypeUnbound, Span::at(0), 0, "unbound name `{name}`")
}

/// The numeric types a bare integer/real literal may take: the friendly word-size
/// `Int`/`Nat`/`Real` and every sized `@`-form. A literal used anywhere else (a
/// `@Bool` condition, a `@Ptr`, a `Str`) is a type error.
pub(super) fn is_numeric_type(name: &str) -> bool {
    matches!(
        name,
        "@Int" | "@Nat" | "@Float64"
            | "@Int8" | "@Int16" | "@Int32" | "@Int64"
            | "@Nat8" | "@Nat16" | "@Nat32" | "@Nat64"
            | "@Float32"
    )
}
