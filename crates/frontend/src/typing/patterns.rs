//! Pattern typing, and the bridge to the exhaustiveness check.
//!
//! [`Checker::type_pattern`] checks a pattern against the scrutinee's type and
//! binds its variables in the current scope. Once a match is typed,
//! [`Checker::check_coverage`] translates its arms into the constructor algebra
//! of [`super::usefulness`] and reports a missing case or an unreachable arm.
//!
//! Entry points: [`Checker::type_pattern`], [`Checker::check_coverage`].

use super::*;
use super::usefulness;

// ## Pattern typing

impl<'a> Checker<'a> {
    pub fn type_pattern(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<()> {
        let r = self.type_pattern_node(pat, expected);
        match self.ast.pat_span(pat) {
            Some(span) => r.map_err(|d| d.fill_span(span)),
            None => r,
        }
    }

    pub(super) fn type_pattern_node(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<()> {
        if matches!(self.pnode(pat), Pattern::Int(_) | Pattern::Range { .. }) {
            self.int_pattern_tys.insert(pat, expected);
        }
        if let Pattern::Int(n) = self.pnode(pat) {
            let span = self.ast.pat_span(pat).unwrap_or_else(|| Span::at(0));
            self.int_literals.push((expected, *n, span));
        }
        match self.pnode(pat) {
            Pattern::Wild => Ok(()),
            Pattern::Var(name) => {
                self.bind(*name, expected);
                Ok(())
            }
            Pattern::Int(_) if self.literal_pattern_hook_check(pat, expected)? => Ok(()),
            Pattern::Real(_) if self.literal_pattern_hook_check(pat, expected)? => Ok(()),
            Pattern::Str(_) if self.literal_pattern_hook_check(pat, expected)? => Ok(()),
            Pattern::Int(_) if self.is_int_scalar(expected) => Ok(()),
            Pattern::Int(_) => {
                self.unifier
                    .unify(expected, self.unifier.types.con(ty::INT), "in an integer pattern")
            }
            Pattern::Real(_) => self
                .unifier
                .unify(expected, self.unifier.types.con(ty::REAL), "in a real pattern"),
            Pattern::Str(_) => self
                .unifier
                .unify(expected, self.unifier.types.con(ty::STR), "in a string pattern"),
            Pattern::Bool(_) => {
                self.unifier
                    .unify(expected, self.unifier.types.con(ty::BOOL), "in a boolean pattern")
            }
            Pattern::Range { lo, hi } => {
                // Both bounds are typed against the scrutinee, so a range forces its
                // scalar (`1 ... 5` -> Int, `1.0 ... 5.0` -> Real) and rejects mixed
                // bounds. An open range `lo ...` types only its lower bound. Binds
                // nothing.
                let (lo, hi) = (*lo, *hi);
                self.type_pattern(lo, expected)?;
                if let Some(hi) = hi {
                    self.type_pattern(hi, expected)?;
                }
                Ok(())
            }
            Pattern::Tuple(pats) => {
                let pats = self.ast.slice(*pats);
                let vars: Vec<Type> = pats.iter().map(|_| self.unifier.fresh()).collect();
                let want = self.unifier.types.tuple(vars.clone());
                self.unifier.unify(expected, want, "in a tuple pattern")?;
                for (p, v) in pats.iter().zip(&vars) {
                    self.type_pattern(*p, *v)?;
                }
                Ok(())
            }
            Pattern::List(_) | Pattern::Concat { .. } if self.tensor_parts(expected).is_some() => {
                self.type_tensor_pattern(pat, expected)
            }
            Pattern::List(elems) => {
                // `[a, b]` / `[]` matches any sequence through its `@ISeqView` (the
                // default `@Vec` when the scrutinee is not yet known).
                let elem = self.sequence_pattern_elem(pat, expected, IF_SEQ_VIEW.into())?;
                for e in self.ast.slice(*elems).iter() {
                    self.type_pattern(*e, elem)?;
                }
                Ok(())
            }
            Pattern::Concat { pre, mid, suf } => {
                let (pre, mid, suf) = (self.ast.slice(*pre), *mid, self.ast.slice(*suf));
                for (pieces, view) in [(pre, IF_SEQ_VIEW.into()), (suf, IF_SEQ_VIEW_BACK.into())] {
                    for piece in pieces.iter() {
                        self.type_concat_piece(*piece, expected, view)?;
                    }
                }
                match mid {
                    Some(mid) => self.type_pattern(mid, expected),
                    None => Ok(()),
                }
            }
            Pattern::Struct { ty, fields } => {
                let ty = *ty;
                let fields = self.ast.slice(*fields);
                self.type_struct_pattern(ty, fields, expected)
            }
            Pattern::Record { fields, rest } => {
                let fields = self.ast.slice(*fields);
                self.type_record_pattern(fields, *rest, expected)
            }
            Pattern::Variant {
                module, ty, tag, fields,
            } => {
                let tag = *tag;
                if let Some(d) = self.unqualified_alias_ctor(*module, *ty, tag) {
                    let span = self.ast.pat_span(pat).unwrap_or(Span::at(0));
                    return Err(d.fill_span(span));
                }
                let fields = self.ast.slice(*fields);
                self.type_variant_pattern(pat, *ty, tag, fields, expected)
            }
        }
    }

    /// Type a record pattern by building an OPEN row from its fields and unifying
    /// with the scrutinee (so it matches any record/struct that has them). Binds
    /// each field's subpattern; the rest may be discarded (`.._`) or bound
    /// (`..name`), in which case the binder gets the leftover row as its record type.
    pub(super) fn type_record_pattern(
        &mut self,
        fields: &'a [FieldPat],
        rest: Option<Aol<Pattern>>,
        expected: Type,
    ) -> Result<()> {
        let tail = self.unifier.fresh();
        let mut entries: Vec<(StrId, Type, Option<Aol<Pattern>>)> = Vec::new();
        for f in fields {
            match f {
                FieldPat::Named { name, pat } => {
                    entries.push((*name, self.unifier.fresh(), Some(*pat)))
                }
                FieldPat::Shorthand(name) => entries.push((*name, self.unifier.fresh(), None)),
                FieldPat::Positional(_) => {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a record pattern's fields need names (`.field = pat`)"
                    ))
                }
            }
        }
        let row = entries
            .iter()
            .rev()
            .fold(tail, |rest, (n, t, _)| self.unifier.types.row_field(*n, *t, rest));
        let want = self.unifier.types.record(row);
        self.unifier.unify(expected, want, "in a record pattern")?;
        for (name, t, pat) in entries {
            match pat {
                Some(p) => self.type_pattern(p, t)?,
                None => self.bind(name, t),
            }
        }
        // `..name` binds the leftover fields as a record over the row tail (which
        // unification has bound to the remaining fields); `.._` is a discard.
        if let Some(r) = rest {
            let rec = self.unifier.types.record(tail);
            self.type_pattern(r, rec)?;
        }
        Ok(())
    }

    pub(super) fn type_struct_pattern(
        &mut self,
        ty: StrId,
        fields: &'a [FieldPat],
        expected: Type,
    ) -> Result<()> {
        // No loose fallback: binding a pattern's fields to fresh variables without
        // unifying against the scrutinee would let the arm claim any type it liked,
        // so an unresolvable pattern is an error, not an unconstrained one.
        let Some(info) = self.structs.get(&ty).cloned() else {
            let what = if self.unions.contains_key(&ty) {
                "a union, so match its variants (`{ty}.Tag.{{ .. }}`)".to_string()
            } else if self.is_known_type(ty) {
                "not a struct".to_string()
            } else {
                "not a declared type".to_string()
            };
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "`{ty}.{{ .. }}` is not a struct pattern: `{ty}` is {what}"
            ));
        };
        let (args, mut subst) = self.instantiate_params(&info.params);
        self.unifier
            .unify(expected, applied(&self.unifier.types, ty, &args), "in a struct pattern")?;
        // A pattern may bind fewer fields than the struct has, but never more: a
        // binder with no field behind it names nothing and faults when read.
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let name = *name;
                    let want = self.struct_field_ty(&info, &mut subst, Some(name), i);
                    let want = want.ok_or_else(|| no_such_field("struct", ty, name))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = self.struct_field_ty(&info, &mut subst, None, i);
                    let n = info.fields.len();
                    let want = want.ok_or_else(|| no_field_at("struct", ty, i, n))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let name = *name;
                    let want = self.struct_field_ty(&info, &mut subst, Some(name), i);
                    let want = want.ok_or_else(|| no_such_field("struct", ty, name))?;
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    pub(super) fn type_variant_pattern(
        &mut self,
        pat: Aol<Pattern>,
        ty: Option<StrId>,
        tag: StrId,
        fields: &'a [FieldPat],
        expected: Type,
    ) -> Result<()> {
        let union = match ty {
            Some(n) => Some(n),
            None => self.find_union_by_tag(tag),
        };
        let resolved = union.and_then(|u| self.variant_sig(u, tag));
        // As in `type_struct_pattern`, an unresolvable tag is an error rather than
        // an arm that binds fresh variables and constrains nothing.
        let Some((result, payload)) = resolved else {
            return Err(match ty {
                Some(n) if !self.unions.contains_key(&n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "`{n}` is not a union, so it has no variant `{tag}`"
                ),
                Some(n) => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "union `{n}` has no variant `{tag}`"
                ),
                None => diag!(
                    Code::TypeMismatch, Span::at(0), 0,
                    "no union has a variant `{tag}`";
                    note: "a `.Tag` pattern names a variant of some declared `@union`"
                ),
            });
        };
        self.unifier.unify(expected, result, "in a variant pattern")?;
        if let Some(u) = union {
            self.variant_pattern_unions.insert(pat, u);
        }
        let label = variant_label(union, tag);
        // As for a struct pattern: fewer binders than slots is fine, more is not.
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let name = *name;
                    let want = variant_field_ty(&payload, Some(name), i);
                    let want =
                        want.ok_or_else(|| no_such_field("constructor", &label, name))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = variant_field_ty(&payload, None, i);
                    let n = payload.len();
                    let want = want.ok_or_else(|| no_field_at("constructor", &label, i, n))?;
                    self.type_pattern(*pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let name = *name;
                    let want = variant_field_ty(&payload, Some(name), i);
                    let want =
                        want.ok_or_else(|| no_such_field("constructor", &label, name))?;
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    /// One fixed piece of a `++` pattern, matched from the front (`view` is
    /// `@ISeqView`) or the back (`@ISeqViewBack`). A string literal strips a
    /// literal prefix or suffix of a `@Str`; a closed `[..]` steps the view.
    pub(super) fn type_concat_piece(&mut self, piece: Aol<Pattern>, expected: Type, view: StrId) -> Result<()> {
        let r = match self.pnode(piece) {
            Pattern::Str(_) => {
                let str_ty = self.unifier.types.con(ty::STR);
                self.unifier.unify(expected, str_ty, "in a string `++` pattern")
            }
            Pattern::List(elems) => {
                let elems = self.ast.slice(*elems);
                self.sequence_pattern_elem(piece, expected, view)
                    .and_then(|elem| elems.iter().try_for_each(|e| self.type_pattern(*e, elem)))
            }
            _ => unreachable!("the parser keeps only literal pieces around a `++` hole"),
        };
        match self.ast.pat_span(piece) {
            Some(span) => r.map_err(|d| d.fill_span(span)),
            None => r,
        }
    }

    /// A `[..]` or `++` pattern on a sized tensor `[n]T`. The size must be known,
    /// so every length is checked here: a closed `[a, b, c]` fixes `n = 3`, and a
    /// `pre ++ mid ++ suf` gives `mid` the exact type `[n - k]T`. Splits along the
    /// first axis, so on `[m, n]T` each element is a `[n]T` row.
    pub(super) fn type_tensor_pattern(&mut self, pat: Aol<Pattern>, expected: Type) -> Result<()> {
        let (variance, size, elem) = self.unifier.tensor_spine(expected).expect("guarded by tensor_parts");
        let (pre, mid, suf): (Vec<Aol<Pattern>>, Option<Aol<Pattern>>, Vec<Aol<Pattern>>) =
            match self.pnode(pat) {
                Pattern::List(_) => (vec![pat], None, Vec::new()),
                Pattern::Concat { pre, mid, suf } => {
                    (self.ast.slice(*pre).to_vec(), *mid, self.ast.slice(*suf).to_vec())
                }
                _ => unreachable!("guarded"),
            };
        let mut elems = Vec::new();
        for piece in pre.iter().chain(&suf) {
            match self.pnode(*piece) {
                Pattern::List(es) => elems.extend_from_slice(self.ast.slice(*es)),
                _ => {
                    return Err(diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "a tensor pattern's fixed parts are `[..]` element lists";
                        help: "a string literal matches a `@Str`, not a tensor"
                    ))
                }
            }
        }
        let k = elems.len() as u64;
        let Some(n) = self.unifier.nat_value(size) else {
            if mid.is_none() {
                let want = self.unifier.types.nat(k);
                self.unifier.unify(size, want, "in a tensor pattern (its length fixes the size)")?;
                return self.type_tensor_pattern(pat, expected);
            }
            let shown = self.unifier.show(expected);
            return Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "the size of `{shown}` is not known here, so a `++` pattern cannot split it";
                note: "a tensor pattern checks its lengths at compile time";
                help: "annotate the scrutinee with a concrete size, such as `[3] @Int`"
            ));
        };
        match mid {
            None if n != k => Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "this pattern has {k} elements, but the tensor has {n}";
                help: "list all {n} elements, or match the rest with `++`, as in `[a] ++ rest`"
            )),
            Some(_) if n < k => Err(diag!(
                Code::TypeMismatch, Span::at(0), 0,
                "this pattern needs at least {k} elements, but the tensor has {n}"
            )),
            _ => {
                for e in &elems {
                    self.type_pattern(*e, elem)?;
                }
                if let Some(mid) = mid {
                    let rest = self.unifier.types.nat(n - k);
                    let rest = tensor_type(&self.unifier.types, variance, rest, elem);
                    self.type_pattern(mid, rest)?;
                }
                self.tensor_patterns.insert(pat, n);
                Ok(())
            }
        }
    }
}

// ## Coverage

impl<'a> Checker<'a> {
    /// Check that the typed match `e` covers every value of its scrutinee (an
    /// error otherwise) and record a warning for each arm no value can reach. A
    /// handler's value arms are parsed into such a match on `%ret`; its
    /// diagnostics speak of the value arms, never of that name.
    pub(super) fn check_coverage(
        &mut self,
        e: Aol<Expr>,
        scrut: Type,
        arms: utilities::Slice<crate::parser::data::Arm>,
    ) -> Result<()> {
        let arms = self.ast.slice(arms);
        // The checker reads a column's constructors off its rows, so with no arms
        // it cannot see that a variantless union has no values to cover.
        if arms.is_empty() && self.is_variantless_union(scrut) {
            return Ok(());
        }
        let rows: Vec<(Vec<usefulness::DPat>, bool)> = arms
            .iter()
            .map(|arm| {
                let alts = self.ast.slice(arm.patterns).iter().map(|p| self.dpat(*p)).collect();
                (alts, arm.guard.is_some())
            })
            .collect();
        let report = usefulness::check(&rows);
        let handler = matches!(
            self.node(e),
            Expr::Match { scrut, .. }
                if matches!(self.node(*scrut), Expr::Var { module: None, name } if *name == "%ret")
        );
        for dead in report.unreachable {
            let (msg, pats) = match dead {
                usefulness::Dead::Arm(i) => (
                    "unreachable arm: the arms before it already match every value it matches",
                    self.ast.slice(arms[i].patterns),
                ),
                usefulness::Dead::Alt(i, j) => (
                    "unreachable alternative: the patterns before it already match every value it matches",
                    &self.ast.slice(arms[i].patterns)[j..=j],
                ),
            };
            let first = pats.first().and_then(|p| self.ast.pat_span(*p));
            let last = pats.last().and_then(|p| self.ast.pat_span(*p));
            let span = match (first, last) {
                (Some(a), Some(b)) => Span::new(a.start, b.end),
                _ => Span::at(0),
            };
            if span != Span::at(0) && self.warnings.iter().any(|w| w.root().span == span) {
                continue;
            }
            self.warnings.push(diag!(Code::UnreachableArm, span, 0, "{msg}"));
        }
        let Some(missing) = report.missing else {
            return Ok(());
        };
        let span = self.ast.expr_span(e).unwrap_or(Span::at(0));
        if let Some(union) = report.mixed {
            return Err(diag!(
                Code::NonExhaustiveMatch, span, 0,
                "cannot check coverage: the arms match `{union}` with both its variants and \
                 sequence patterns, which go through its `@ISeqView` instead";
                help: "match with only one kind of pattern, or add a catch-all `| _ => ...`"
            ));
        }
        let what = if handler { "the handler's value arms do" } else { "this `is` does" };
        Err(diag!(
            Code::NonExhaustiveMatch, span, 0,
            "non-exhaustive match: {what} not cover `{}`", usefulness::show(&missing);
            help: "add an arm for it, or a catch-all `| _ => ...`"
        ))
    }

    /// Reject a typed `let` or lambda-parameter pattern that some value of its
    /// type fails to match: there is no next arm to fall through to.
    pub(super) fn check_irrefutable(&mut self, pat: Aol<Pattern>, site: &str) -> Result<()> {
        let report = usefulness::check(&[(vec![self.dpat(pat)], false)]);
        let Some(missing) = report.missing else {
            return Ok(());
        };
        let span = self.ast.pat_span(pat).unwrap_or(Span::at(0));
        Err(diag!(
            Code::NonExhaustiveMatch, span, 0,
            "refutable pattern in {site}: it does not match `{}`", usefulness::show(&missing);
            help: "bind a variable and match it with `is`, which can handle every case"
        ))
    }

    /// The value range of an integer pattern's type, or `None` when it is not a
    /// built-in integer. `@Int` and `@Nat` take the widest target's range, which
    /// is conservative on a narrower one: covering it covers the narrower range.
    pub(super) fn int_domain(&self, pat: Aol<Pattern>) -> Option<(i128, i128)> {
        self.int_type_domain(*self.int_pattern_tys.get(&pat)?)
    }

    /// The value range of a built-in integer type (see [`Self::int_domain`]).
    pub(super) fn int_type_domain(&self, ty: Type) -> Option<(i128, i128)> {
        let ty = self.unifier.zonk(ty);
        let TypeNode::Con(name) = self.unifier.head(ty) else {
            return None;
        };
        let signed = |bits: u32| (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1);
        let unsigned = |bits: u32| (0, (1i128 << bits) - 1);
        Some(match self.unifier.types.name(name) {
            "@Int8" => signed(8),
            "@Int16" => signed(16),
            "@Int32" => signed(32),
            "@Int64" | "@Int" => signed(64),
            "@Nat8" => unsigned(8),
            "@Nat16" => unsigned(16),
            "@Nat32" => unsigned(32),
            "@Nat64" | "@Nat" => unsigned(64),
            _ => return None,
        })
    }

    /// Translate a typed pattern for [`usefulness::check`].
    /// `[e1, .., en]` followed by `end`, as cons steps.
    pub(super) fn seq_dpat(&self, elems: &[Aol<Pattern>], end: usefulness::DPat) -> usefulness::DPat {
        use usefulness::{Ctor, DPat};
        elems
            .iter()
            .rev()
            .fold(end, |tail, e| DPat::Ctor(Ctor::SeqMore, vec![self.dpat(*e), tail]))
    }

    /// A tensor pattern as a product over element positions: its length is static,
    /// so `[x, y, z]` on a `[3]T` covers it. A refutable middle part is not modeled.
    pub(super) fn tensor_dpat(&self, pat: Aol<Pattern>) -> usefulness::DPat {
        use usefulness::{DPat, ProductKind};
        let n = self.tensor_patterns[&pat];
        let (pre, mid, suf): (&[Aol<Pattern>], Option<Aol<Pattern>>, &[Aol<Pattern>]) =
            match self.pnode(pat) {
                Pattern::List(_) => (std::slice::from_ref(&pat), None, &[]),
                Pattern::Concat { pre, mid, suf } => (self.ast.slice(*pre), *mid, self.ast.slice(*suf)),
                _ => return DPat::Opaque,
            };
        if mid.is_some_and(|m| !matches!(self.pnode(m), Pattern::Var(_) | Pattern::Wild)) {
            return DPat::Opaque;
        }
        let pieces = |ps: &[Aol<Pattern>]| -> Vec<Aol<Pattern>> {
            ps.iter()
                .flat_map(|p| match self.pnode(*p) {
                    Pattern::List(es) => self.ast.slice(*es).to_vec(),
                    _ => Vec::new(),
                })
                .collect()
        };
        let (front, back) = (pieces(pre), pieces(suf));
        let back_start = n - back.len() as u64;
        let fields = front
            .iter()
            .enumerate()
            .map(|(i, p)| (i as u64, *p))
            .chain(back.iter().enumerate().map(|(j, p)| (back_start + j as u64, *p)))
            .map(|(i, p)| (StrId::intern(&i.to_string()), self.dpat(p)))
            .collect();
        DPat::Product(ProductKind::Tensor(n), fields)
    }

    pub(super) fn dpat(&self, pat: Aol<Pattern>) -> usefulness::DPat {
        use usefulness::{Ctor, DPat, ProductKind};
        match self.pnode(pat) {
            Pattern::Wild | Pattern::Var(_) => DPat::Wild,
            Pattern::Int(_) | Pattern::Real(_) | Pattern::Str(_)
                if self.literal_pattern_hooks.contains_key(&pat) =>
            {
                DPat::Opaque
            }
            Pattern::Int(n) => match self.int_domain(pat) {
                Some(dom) => {
                    let n = *n as i128;
                    if n < dom.0 || dom.1 < n {
                        // Rejected by `check_int_literals` once the module is solved.
                        return DPat::Opaque;
                    }
                    DPat::Ctor(Ctor::Int { lo: n, hi: n, dom }, Vec::new())
                }
                None => DPat::Ctor(Ctor::Lit(n.to_string()), Vec::new()),
            },
            Pattern::Real(x) => DPat::Ctor(Ctor::Lit(format!("{x:?}")), Vec::new()),
            Pattern::Str(s) => {
                let text = String::from_utf8_lossy(self.ast.bytes(*s));
                DPat::Ctor(Ctor::Lit(format!("{text:?}")), Vec::new())
            }
            Pattern::Bool(b) => DPat::Ctor(Ctor::Bool(*b), Vec::new()),
            Pattern::Range { lo, hi } => {
                let Some(dom) = self.int_domain(pat) else {
                    return DPat::Opaque;
                };
                let bound = |p: Aol<Pattern>| match self.pnode(p) {
                    Pattern::Int(n) => Some(*n as i128),
                    _ => None,
                };
                let lo = bound(*lo);
                let hi = match hi {
                    Some(h) => bound(*h),
                    None => Some(dom.1),
                };
                match (lo, hi) {
                    (Some(lo), Some(hi)) if lo <= hi => {
                        DPat::Ctor(Ctor::Int { lo: lo.max(dom.0), hi: hi.min(dom.1), dom }, Vec::new())
                    }
                    _ => DPat::Opaque,
                }
            }
            Pattern::List(_) | Pattern::Concat { .. } if self.tensor_patterns.contains_key(&pat) => {
                self.tensor_dpat(pat)
            }
            Pattern::List(elems) => self.seq_dpat(self.ast.slice(*elems), DPat::Ctor(Ctor::SeqEmpty, Vec::new())),
            // A chain matched from the front only, through list pieces, is the same
            // cons steps a closed list takes. Anything else (a string piece, a piece
            // matched from the back) never covers the whole domain.
            Pattern::Concat { pre, mid, suf } => {
                let pre = self.ast.slice(*pre);
                if !self.ast.slice(*suf).is_empty() {
                    return DPat::Opaque;
                }
                let mut elems = Vec::new();
                for piece in pre.iter() {
                    match self.pnode(*piece) {
                        Pattern::List(es) => elems.extend_from_slice(self.ast.slice(*es)),
                        _ => return DPat::Opaque,
                    }
                }
                let end = match mid {
                    Some(m) => self.dpat(*m),
                    None => DPat::Ctor(Ctor::SeqEmpty, Vec::new()),
                };
                self.seq_dpat(&elems, end)
            }
            Pattern::Tuple(pats) => {
                let pats = self.ast.slice(*pats);
                if pats.is_empty() {
                    return DPat::Wild;
                }
                let fields = pats.iter().enumerate().map(|(i, p)| (StrId::intern(&i.to_string()), self.dpat(*p)));
                DPat::Product(ProductKind::Tuple, fields.collect())
            }
            Pattern::Struct { ty, fields } => {
                let ty = *ty;
                let Some(info) = self.structs.get(&ty) else {
                    return DPat::Opaque;
                };
                let mut out = Vec::new();
                for (i, f) in self.ast.slice(*fields).iter().enumerate() {
                    match f {
                        FieldPat::Named { name, pat } => {
                            out.push((*name, self.dpat(*pat)))
                        }
                        FieldPat::Positional(pat) => match info.fields.get(i) {
                            Some((name, _)) => out.push((*name, self.dpat(*pat))),
                            None => return DPat::Opaque,
                        },
                        FieldPat::Shorthand(_) => {}
                    }
                }
                DPat::Product(ProductKind::Struct(ty), out)
            }
            Pattern::Record { fields, .. } => {
                let fields = self.ast.slice(*fields).iter().filter_map(|f| match f {
                    FieldPat::Named { name, pat } => {
                        Some((*name, self.dpat(*pat)))
                    }
                    _ => None,
                });
                DPat::Product(ProductKind::Record, fields.collect())
            }
            Pattern::Variant { ty, tag, fields, .. } => {
                let tag = *tag;
                let union = self
                    .variant_pattern_unions
                    .get(&pat)
                    .copied()
                    .or(*ty)
                    .or_else(|| self.find_union_by_tag(tag));
                let Some((union, info)) = union.and_then(|u| Some((u, self.unions.get(&u)?))) else {
                    return DPat::Opaque;
                };
                let Some(index) = info.variants.iter().position(|v| v.tag == tag) else {
                    return DPat::Opaque;
                };
                let payload = &info.variants[index].payload;
                let mut args = vec![DPat::Wild; payload.len()];
                for (i, f) in self.ast.slice(*fields).iter().enumerate() {
                    let (slot, p) = match f {
                        FieldPat::Named { name, pat } => {
                            (payload.iter().position(|(n, _)| *n == Some(*name)), *pat)
                        }
                        FieldPat::Positional(pat) => (Some(i), *pat),
                        FieldPat::Shorthand(_) => continue,
                    };
                    match slot.and_then(|s| args.get_mut(s)) {
                        Some(a) => *a = self.dpat(p),
                        None => return DPat::Opaque,
                    }
                }
                let siblings: Vec<(StrId, usize)> = info
                    .variants
                    .iter()
                    .map(|v| (v.tag, v.payload.len()))
                    .collect();
                DPat::Ctor(
                    Ctor::Variant { union, index, siblings: siblings.into() },
                    args,
                )
            }
        }
    }
}
