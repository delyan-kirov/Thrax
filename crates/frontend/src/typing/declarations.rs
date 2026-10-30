//! Type declarations: structs, unions, aliases and effects.
//!
//! [`Checker::register_types`] records every declaration before any body is
//! checked, so definitions may refer to types declared later. It applies the
//! `with Other` splices, rejects alias cycles, computes which constructor slots
//! are lazy, and registers each struct's record row for the nominal-to-row
//! bridge. The lookups below answer the questions inference asks of a
//! declaration: a struct's fields, a variant's payload, a tag's union.
//!
//! Entry points: [`Checker::register_types`], [`Checker::register_effects`].

use super::*;

// ## Type parameters

impl<'a> Checker<'a> {
    /// A type's parameter list is exactly what it declares after the keyword. Every
    /// type variable used in the body must be declared (an undeclared one is an
    /// error: parameters are mandatory, never inferred). A declared parameter that
    /// appears nowhere is allowed (a phantom). `kind` is the keyword, for the error.
    pub(super) fn resolve_type_params(
        &self,
        kind: &str,
        name: StrId,
        declared: &[utilities::StrId],
        collected: Vec<(StrId, Span)>,
    ) -> Result<Vec<StrId>> {
        let declared: Vec<StrId> = declared.to_vec();
        for (v, span) in &collected {
            if !declared.contains(v) {
                return Err(undeclared_param(kind, name, v, &declared, false).fill_span(*span));
            }
        }
        Ok(declared)
    }

    /// A declared type's recorded span, or the no-location sentinel.
    pub(super) fn ty_span_or_none(&self, ty: Aol<Ty>) -> Span {
        self.ast.ty_span(ty).unwrap_or(Span::at(0))
    }

    /// Every type variable used across `tys`, each paired with the span of the
    /// type it was found in, so an undeclared one carets that field or payload
    /// rather than the whole declaration.
    pub(super) fn tyvars_with_spans(&self, tys: impl IntoIterator<Item = Aol<Ty>>) -> Vec<(StrId, Span)> {
        let mut out: Vec<(StrId, Span)> = Vec::new();
        for ty in tys {
            let mut vs = Vec::new();
            collect_tyvars(self.ast, ty, &mut vs);
            let span = self.ty_span_or_none(ty);
            for v in vs {
                if !out.iter().any(|(n, _)| *n == v) {
                    out.push((v, span));
                }
            }
        }
        out
    }

    /// Re-check a struct/union's parameters after its `with` splices are copied in:
    /// the parameters stay as declared, but every type variable in the now-complete
    /// field/variant set, including spliced-in ones, must still be covered.
    pub(super) fn splice_params(
        &self,
        kind: &str,
        name: StrId,
        collected: Vec<(StrId, Span)>,
    ) -> Result<Vec<StrId>> {
        let declared = self
            .structs
            .get(&name)
            .map(|i| i.params.clone())
            .or_else(|| self.unions.get(&name).map(|i| i.params.clone()))
            .expect("registered");
        for (v, span) in &collected {
            if !declared.contains(v) {
                return Err(undeclared_param(kind, name, v, &declared, true).fill_span(*span));
            }
        }
        Ok(declared)
    }
}

// ## Registration

impl<'a> Checker<'a> {
    pub(super) fn register_types(&mut self, program: &Program) -> Result<()> {
        for item in self.ast.slice(program.items).iter() {
            match item {
                Item::Struct {
                    name,
                    params,
                    includes,
                    fields,
                    abi,
                    c_union,
                } => {
                    let (params, includes, fields) = (
                        self.ast.slice(*params),
                        self.ast.slice(*includes),
                        self.ast.slice(*fields),
                    );
                    let collected = self.tyvars_with_spans(fields.iter().map(|f| f.ty));
                    let name = *name;
                    // A blessed interface is the compiler's own: CORE declares it, and
                    // exactly one field, because a desugar site projects that field
                    // without knowing its name.
                    if is_blessed_interface(name.as_str()) {
                        let at = fields
                            .first()
                            .map_or(Span::at(0), |f| self.ty_span_or_none(f.ty));
                        if self.module_name != "CORE" {
                            return Err(diag!(
                                Code::TypeMismatch, at, 0,
                                "`{name}` is a blessed interface and only CORE may declare it";
                                help: "implement it instead: a value of the applied type \
                                       (`$ r : @IRange @Int Span = .{{ .range = ... }}`)"
                            ));
                        }
                        if fields.len() != 1 {
                            return Err(diag!(
                                Code::TypeMismatch, at, 0,
                                "the blessed interface `{name}` must have exactly one field, \
                                 not {}", fields.len()
                            ));
                        }
                    }
                    let params = self.resolve_type_params("struct", name, params, collected)?;
                    let fields = fields.iter().map(|f| (f.name, f.ty)).collect();
                    let crepr = abi.is_some();
                    self.claim_type_name(name);
                    self.structs.insert(
                        name,
                        StructInfo {
                            params,
                            fields,
                            crepr,
                            c_union: *c_union,
                        },
                    );
                    self.own_type_names.push(name);
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| (p.name, p.span)).collect();
                        self.pending_includes.insert(name, (true, ps));
                    }
                }
                Item::Union {
                    name,
                    params,
                    includes,
                    variants,
                } => {
                    let (params, includes, variants) = (
                        self.ast.slice(*params),
                        self.ast.slice(*includes),
                        self.ast.slice(*variants),
                    );
                    let mut payload_tys = Vec::new();
                    let mut vs = Vec::with_capacity(variants.len());
                    for v in variants.iter() {
                        let payload = payload_fields(self.ast, &v.payload);
                        payload_tys.extend(payload.iter().map(|(_, ty)| *ty));
                        vs.push(VariantSig {
                            tag: v.tag,
                            payload,
                        });
                    }
                    let collected = self.tyvars_with_spans(payload_tys);
                    let name = *name;
                    let params = self.resolve_type_params("union", name, params, collected)?;
                    self.claim_type_name(name);
                    self.unions.insert(
                        name,
                        UnionInfo {
                            params,
                            variants: vs,
                        },
                    );
                    self.own_type_names.push(name);
                    if !includes.is_empty() {
                        let ps = includes.iter().map(|p| (p.name, p.span)).collect();
                        self.pending_includes.insert(name, (false, ps));
                    }
                }
                Item::Alias { name, params, ty } => {
                    let params = self.ast.slice(*params);
                    let collected = self.tyvars_with_spans([*ty]);
                    let name = *name;
                    let params = self.resolve_type_params("alias", name, params, collected)?;
                    self.claim_type_name(name);
                    self.aliases.insert(name, (params, *ty));
                    self.own_type_names.push(name);
                }
                _ => {}
            }
        }
        self.check_alias_cycles()?;
        // Copy in each `with Other` type's members once every type is registered,
        // so an included type may be declared after (or imported by) the one that
        // names it.
        let pending: Vec<StrId> = self.pending_includes.keys().copied().collect();
        for name in pending {
            let mut visiting = HashSet::new();
            self.splice_includes(name, &mut visiting)?;
        }
        self.compute_lazy_slots();
        Ok(())
    }

    /// Reject an alias that reaches itself through aliases alone (`T = T`, or
    /// `A = B` with `B = A`). A struct or union in the chain is nominal and stops
    /// the expansion, but an alias is expanded in place and would never finish.
    pub(super) fn check_alias_cycles(&self) -> Result<()> {
        fn visit<'a>(
            checker: &Checker<'a>,
            name: StrId,
            path: &mut Vec<StrId>,
            done: &mut HashSet<StrId>,
        ) -> Result<()> {
            if done.contains(&name) {
                return Ok(());
            }
            let Some((_, body)) = checker.aliases.get(&name) else {
                return Ok(());
            };
            if let Some(at) = path.iter().position(|n| *n == name) {
                let mut chain: Vec<&str> = path[at..].iter().map(|n| n.as_str()).collect();
                chain.push(name.as_str());
                let span = checker.ast.ty_span(*body).unwrap_or_else(|| Span::at(0));
                if matches!(checker.ast.ty(*body), Ty::Row(_)) {
                    return Err(diag!(
                        Code::TypeCycle, span, 0,
                        "row alias `{name}` expands to itself";
                        note: "{}", chain.join(" -> ");
                        help: "a row alias names a set of effects; list the effects in one of them"
                    ));
                }
                return Err(diag!(
                    Code::TypeCycle, span, 0,
                    "type alias `{name}` expands to itself";
                    note: "{}", chain.join(" -> ");
                    help: "an alias only renames a type; make one of them a `@struct` or `@union`"
                ));
            }
            path.push(name);
            let mut cons = Vec::new();
            match checker.ast.ty(*body) {
                Ty::Row(row) => cons.extend(
                    checker.ast.slice(row.entries).iter().map(|e| e.name),
                ),
                _ => collect_tycons(checker.ast, *body, &mut cons),
            }
            for c in cons {
                visit(checker, c, path, done)?;
            }
            path.pop();
            done.insert(name);
            Ok(())
        }
        let mut names: Vec<StrId> = self.aliases.keys().copied().collect();
        names.sort_unstable_by_key(|n| n.as_str());
        let mut done = HashSet::new();
        for name in names {
            visit(self, name, &mut Vec::new(), &mut done)?;
        }
        Ok(())
    }

    /// If `name` is a parameterless alias whose body is a bare type constructor,
    /// the constructor it aliases (`Quaternion` -> `Vector4`), else None. Used to
    /// see through aliases when deciding whether a C-repr field is representable.
    pub(super) fn nullary_alias_target(&self, name: StrId) -> Option<StrId> {
        let (params, body) = self.aliases.get(&name)?;
        if !params.is_empty() {
            return None;
        }
        match self.tnode(*body) {
            Ty::Con { name, .. } => Some(*name),
            _ => None,
        }
    }

    /// Copy each included type's fields (struct) or variants (union) into `name`,
    /// ahead of its own, resolving includes recursively (an included type may
    /// itself splice). Detects cycles, kind mismatches, and duplicate members.
    /// This copies members; it records no subtype/relationship in the type system.
    pub(super) fn splice_includes(&mut self, name: StrId, visiting: &mut HashSet<StrId>) -> Result<()> {
        let (is_struct, includes) = match self.pending_includes.get(&name) {
            Some(entry) => entry.clone(),
            None => return Ok(()), // already spliced (or never used `with`)
        };
        // Every failure below is about what a `with` dragged in, so the first
        // include is the fallback caret when the member itself has no span.
        let at_with = includes.first().map_or(Span::at(0), |(_, s)| *s);
        if !visiting.insert(name) {
            let span = at_with;
            return Err(diag!(
                Code::TypeMismatch, span, 0,
                "type `{name}` includes itself (a `with` cycle)"
            ));
        }
        if is_struct {
            let mut fields: Vec<(StrId, Aol<Ty>)> = Vec::new();
            for &(p, span) in &includes {
                self.splice_includes(p, visiting)?;
                let pinfo = self.structs.get(&p).cloned().ok_or_else(|| {
                    diag!(Code::TypeMismatch, span, 0,
                        "`{name}` does `with {p}`, which is not a known struct")
                })?;
                for f in &pinfo.fields {
                    if fields.iter().any(|(n, _)| n == &f.0) {
                        return Err(dup_member(name, f.0, "field").fill_span(span));
                    }
                    fields.push(*f);
                }
            }
            let own = self.structs.get(&name).expect("registered").fields.clone();
            for f in own {
                if fields.iter().any(|(n, _)| n == &f.0) {
                    return Err(dup_member(name, f.0, "field").fill_span(self.ty_span_or_none(f.1)));
                }
                fields.push(f);
            }
            let collected = self.tyvars_with_spans(fields.iter().map(|(_, ty)| *ty));
            let params = self.splice_params("struct", name, collected)?;
            let prev = self.structs.get(&name).expect("registered");
            let (crepr, c_union) = (prev.crepr, prev.c_union);
            self.structs.insert(
                name,
                StructInfo {
                    params,
                    fields,
                    crepr,
                    c_union,
                },
            );
        } else {
            let mut variants: Vec<VariantSig> = Vec::new();
            for &(p, span) in &includes {
                self.splice_includes(p, visiting)?;
                let pinfo = self.unions.get(&p).cloned().ok_or_else(|| {
                    diag!(Code::TypeMismatch, span, 0,
                        "`{name}` does `with {p}`, which is not a known union")
                })?;
                for v in &pinfo.variants {
                    if variants.iter().any(|w| w.tag == v.tag) {
                        return Err(dup_member(name, v.tag, "variant").fill_span(span));
                    }
                    variants.push(v.clone());
                }
            }
            let own = self.unions.get(&name).expect("registered").variants.clone();
            for v in own {
                if variants.iter().any(|w| w.tag == v.tag) {
                    let span = v
                        .payload
                        .first()
                        .map_or(at_with, |(_, ty)| self.ty_span_or_none(*ty));
                    return Err(dup_member(name, v.tag, "variant").fill_span(span));
                }
                variants.push(v);
            }
            let payload_tys: Vec<Aol<Ty>> = variants
                .iter()
                .flat_map(|v| v.payload.iter().map(|(_, ty)| *ty))
                .collect();
            let collected = self.tyvars_with_spans(payload_tys);
            let params = self.splice_params("union", name, collected)?;
            self.unions.insert(name, UnionInfo { params, variants });
        }
        self.pending_includes.remove(&name);
        visiting.remove(&name);
        Ok(())
    }

    /// Register every declared effect's operations. An operation `op : Arg -> Res`
    /// becomes a value: bound unqualified when a single effect declares that name,
    /// or an overload when several do (resolved by result type at the use site);
    /// always reachable qualified as `Effect.op`. Its `Arg -> Res` scheme is also
    /// kept per effect for handler-clause typing.
    /// Build the record row of every struct and hand them to the unifier, so a
    /// nominal struct can unify with a structural record row (the hybrid bridge).
    /// Each row is a scheme: its parameters become fresh vars (recorded in
    /// declaration order), so a generic struct instance `Box Int` bridges by
    /// substituting its type arguments for those parameter vars per use.
    pub(super) fn register_struct_rows(&mut self) {
        let decls: Vec<(StrId, Vec<StrId>, Vec<(StrId, Aol<Ty>)>)> = self
            .structs
            .iter()
            .map(|(name, info)| (*name, info.params.clone(), info.fields.clone()))
            .collect();
        let mut rows = HashMap::new();
        for (name, params, fields) in decls {
            let mut tvars = HashMap::new();
            // A fresh var per parameter, in order, so App arguments line up with
            // them at bridge time (a phantom param still gets a slot, unused).
            let param_ids: Vec<VarId> = params
                .iter()
                .map(|p| {
                    let v = self.unifier.fresh();
                    let id = match self.unifier.types.node(v) {
                        TypeNode::Var(id) => id,
                        _ => unreachable!("fresh() returns a variable"),
                    };
                    tvars.insert(*p, v);
                    id
                })
                .collect();
            let mut row = self.unifier.types.row_empty();
            for (fname, fty) in fields.iter().rev() {
                let f = self.ty_of_ast(*fty, &mut tvars);
                row = self.unifier.types.row_field(*fname, f, row);
            }
            rows.insert(name, (param_ids, row));
        }
        self.unifier.set_struct_rows(rows);
    }

    // Note [Lazy constructor slots]
    //
    // Thrax is call-by-value, but a recursive data type built strictly could
    // never describe an infinite or very long structure without building all of
    // it first. So a constructor slot is lazy exactly when its type can lead
    // back to the type that owns it: `Cons`'s tail, a tree's children. Every
    // other slot is strict.
    //
    // Laziness belongs to the slot, not to the expression that fills it, so it
    // holds however the value is built. The checker decides it per declaration
    // (`compute_lazy_slots`, into `Resolved::lazy_slots`), and desugaring wraps
    // each value placed in a lazy slot in a lambda passed to `@delay`. The
    // engines force the suspension when the value is scrutinised and overwrite
    // the cell with the result, so the work happens once and only if it is
    // needed.
    //
    // A C-repr struct is excluded: its runtime value is a flat C struct, and a
    // suspension cannot cross the `@extern` boundary.

    /// Decide which slots hold their value lazily.
    /// See Note [Lazy constructor slots].
    pub(super) fn compute_lazy_slots(&mut self) {
        let mut mentions: HashMap<StrId, Vec<StrId>> = HashMap::new();
        let mut slot_types: Vec<(StrId, Option<StrId>, Vec<Aol<Ty>>)> = Vec::new();
        for (name, info) in &self.structs {
            if info.crepr {
                continue;
            }
            slot_types.push((*name, None, info.fields.iter().map(|(_, t)| *t).collect()));
        }
        for (name, info) in &self.unions {
            for v in &info.variants {
                slot_types.push((*name, Some(v.tag), v.payload.iter().map(|(_, t)| *t).collect()));
            }
        }
        // An alias is transparent here: a slot typed through one still reaches
        // whatever the alias expands to.
        for (name, (_, ty)) in &self.aliases {
            let mut cons = Vec::new();
            collect_tycons(self.ast, *ty, &mut cons);
            mentions.entry(*name).or_default().extend(cons);
        }
        for (owner, _, tys) in &slot_types {
            let entry = mentions.entry(*owner).or_default();
            for ty in tys {
                let mut cons = Vec::new();
                collect_tycons(self.ast, *ty, &mut cons);
                for c in cons {
                    if !entry.contains(&c) {
                        entry.push(c);
                    }
                }
            }
        }
        // Transitive closure of "mentions", so a slot typed `B` counts as
        // recursive when `B` leads back to the owner through any chain.
        let mut reach: HashMap<StrId, HashSet<StrId>> =
            mentions.iter().map(|(k, v)| (*k, v.iter().copied().collect())).collect();
        loop {
            let mut changed = false;
            let keys: Vec<StrId> = reach.keys().copied().collect();
            for k in keys {
                let grown: HashSet<StrId> = reach[&k]
                    .iter()
                    .filter_map(|t| reach.get(t))
                    .flatten()
                    .copied()
                    .collect();
                let entry = reach.get_mut(&k).expect("key came from this map");
                let before = entry.len();
                entry.extend(grown);
                changed |= entry.len() != before;
            }
            if !changed {
                break;
            }
        }
        let leads_back = |c: StrId, owner: StrId| {
            c == owner || reach.get(&c).is_some_and(|r| r.contains(&owner))
        };
        for (owner, tag, tys) in &slot_types {
            let flags: Vec<bool> = tys
                .iter()
                .map(|ty| {
                    let mut cons = Vec::new();
                    collect_tycons(self.ast, *ty, &mut cons);
                    cons.into_iter().any(|c| leads_back(c, *owner))
                })
                .collect();
            if flags.iter().any(|f| *f) {
                self.lazy_slots
                    .insert((*owner, *tag), flags);
            }
        }
    }
}

// ## Effects

impl<'a> Checker<'a> {
    pub(super) fn register_effects(&mut self, program: &Program) -> Result<()> {
        let items = self.ast.slice(program.items);
        for item in items.iter() {
            if let Item::Effect { name, params, .. } = item {
                let n = self.ast.slice(*params).len();
                self.effect_arity.insert(*name, n);
            }
        }
        let mut per_op: HashMap<StrId, Vec<Type>> = HashMap::new();
        for item in items.iter() {
            let Item::Effect { name, params, ops } = item else {
                continue;
            };
            let effect = *name;
            let params: Vec<StrId> = self.ast.slice(*params).to_vec();
            let mut op_schemes = HashMap::new();
            for op in self.ast.slice(*ops).iter() {
                let op_name = op.name;
                let scheme = self.op_scheme(effect, &params, op.ty)?;
                op_schemes.insert(op_name, scheme);
                per_op.entry(op_name).or_default().push(scheme);
                self.qualified
                    .entry(effect)
                    .or_default()
                    .insert(op_name, vec![scheme]);
            }
            self.effect_ops.insert(effect, op_schemes);
            self.own_effects.push(effect);
        }
        for (op_name, mut schemes) in per_op {
            if schemes.len() == 1 {
                self.bind(op_name, schemes.pop().expect("one scheme"));
            } else {
                self.overloads
                    .entry(op_name)
                    .or_default()
                    .extend(schemes.into_iter().map(Cand::local));
            }
        }
        Ok(())
    }

    /// An operation's scheme: its written type, with the outermost arrow's
    /// latent effect `<effect params | mu>` (mu a fresh row variable), so
    /// performing it forces `effect` into the ambient, fits any ambient already
    /// containing more effects, and ties `params` to the handler's instance. The
    /// type may also use variables of its own, chosen anew at each use, like
    /// `throw : @Str -> a`. One handler serves every use, so its clause sees
    /// each of them as an opaque type (see `instantiate_for_handler`): it can
    /// pass such a value along, but cannot make one up or inspect it, which
    /// keeps it from answering one use's type to another (#290).
    pub(super) fn op_scheme(
        &mut self,
        effect: StrId,
        params: &[StrId],
        sig: Aol<Ty>,
    ) -> Result<Type> {
        self.unifier.enter_level();
        let mut tvars: HashMap<StrId, Type> = HashMap::new();
        let param_tys: Vec<Type> = params
            .iter()
            .map(|p| {
                let v = self.unifier.fresh();
                tvars.insert(*p, v);
                v
            })
            .collect();
        let args = if param_tys.is_empty() {
            self.unifier.types.row_empty()
        } else {
            self.unifier.types.tuple(param_tys)
        };
        let saved = self.effect_self.replace((effect, args));
        let ty = self.ty_of_ast(sig, &mut tvars);
        self.effect_self = saved;
        let ty = match self.unifier.types.node(ty) {
            TypeNode::Arrow(from, to, _) => {
                let mu = self.unifier.fresh();
                let eff = self.unifier.types.row_extend_args(effect, args, mu);
                self.unifier.types.arrow_eff(from, to, eff)
            }
            _ => ty,
        };
        self.unifier.leave_level();
        self.unifier.generalize(ty);
        Ok(self.unifier.zonk(ty))
    }

    /// The effect a handler clause discharges: its explicit qualifier, or the
    /// unique effect declaring a bare operation (ambiguous / unknown -> `None`,
    /// left to dynamic dispatch).
    pub(super) fn op_owner(&self, effect: Option<StrId>, op: StrId) -> Option<StrId> {
        if let Some(e) = effect {
            return Some(e);
        }
        for table in [&self.effect_ops, &self.imported_effect_ops] {
            let mut found = None;
            for (eff, ops) in table {
                if ops.contains_key(&op) {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(*eff);
                }
            }
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// The `Arg -> Res` scheme of an operation named in a handler clause, resolved
    /// by its explicit effect or, for a bare name, by the unique effect declaring
    /// it.
    pub(super) fn resolve_op_ty(&self, effect: Option<StrId>, op: StrId) -> Option<Type> {
        let owner = self.op_owner(effect, op)?;
        [&self.effect_ops, &self.imported_effect_ops]
            .into_iter()
            .find_map(|table| table.get(&owner).and_then(|ops| ops.get(&op)).copied())
    }
}

// ## Lookups

impl<'a> Checker<'a> {
    pub(super) fn variant_sig(&mut self, union: StrId, tag: StrId) -> Option<(Type, VariantPayload)> {
        let info = self.unions.get(&union)?.clone();
        let pos = info.variants.iter().position(|v| v.tag == tag)?;
        let (args, mut subst) = self.instantiate_params(&info.params);
        let result = applied(&self.unifier.types, union, &args);
        let variant = &info.variants[pos];
        let payload = variant
            .payload
            .clone()
            .into_iter()
            .map(|(name, ast_ty)| (name, self.ty_of_ast(ast_ty, &mut subst)))
            .collect();
        Some((result, payload))
    }

    pub(super) fn find_union_by_tag(&self, tag: StrId) -> Option<StrId> {
        self.unions
            .iter()
            .find_map(|(name, info)| info.variants.iter().any(|v| v.tag == tag).then_some(*name))
    }

    /// The type of field `name` in a record value: through the row for a structural
    /// record (open tail grows to include it), or the declared field for a struct.
    pub(super) fn field_type_of(&mut self, base: Type, name: StrId) -> Result<Type> {
        if let TypeNode::Record(_) = self.unifier.head(base) {
            return self
                .unifier
                .record_field(base, name, "in a record update");
        }
        for (fname, fty) in self.record_fields_of(base)? {
            if fname == name {
                return Ok(fty);
            }
        }
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "record update sets `{name}`, which the base record does not have"
        ))
    }

    /// The record row of a value (the inner row of a structural record, possibly
    /// open; or a struct's closed row), for stacking fields onto it.
    pub(super) fn record_row_of(&mut self, base: Type) -> Result<Type> {
        if let TypeNode::Record(row) = self.unifier.head(base) {
            return Ok(row);
        }
        let fields = self.record_fields_of(base)?;
        Ok(fields
            .into_iter()
            .rev()
            .fold(self.unifier.types.row_empty(), |rest, (n, t)| self.unifier.types.row_field(n, t, rest)))
    }

    /// Whether `ty` is a record-shaped target: a structural record row, or a
    /// The nominal struct name `ty` resolves to (possibly applied), if any.
    pub(super) fn struct_name_of(&self, ty: Type) -> Option<StrId> {
        let (head, _) = self.spine(ty);
        if let TypeNode::Con(n) = self.unifier.types.node(head) {
            if let Some((k, _)) = self.structs.get_key_value(&n) {
                return Some(*k);
            }
        }
        None
    }

    /// The union `ty`'s head names, if it is a (possibly applied) declared union that
    /// has a variant `tag`. Lets a BARE `.Tag` resolve type-directedly against the
    /// expected type, so two unions sharing a constructor name (e.g. a user list and
    /// the builtin `List`, both with `Cons`/`Nil`) do not collide.
    pub(super) fn union_head_with_tag(&self, ty: Type, tag: StrId) -> Option<StrId> {
        let (head, _) = self.spine(ty);
        let TypeNode::Con(n) = self.unifier.types.node(head) else {
            return None;
        };
        let (k, info) = self.unions.get_key_value(&n)?;
        info.variants.iter().any(|v| v.tag == tag).then_some(*k)
    }

    /// Whether `ty` is a declared union with no variants, so no value has it.
    pub(super) fn is_variantless_union(&self, ty: Type) -> bool {
        let (head, _) = self.spine(ty);
        let TypeNode::Con(n) = self.unifier.types.node(head) else {
            return false;
        };
        self.unions.get(&n).is_some_and(|u| u.variants.is_empty())
    }

    /// The `(label, type)` fields of a record value: a structural [`TypeNode::Record`]
    /// (its closed row) or a nominal struct (its declared fields, instantiated).
    pub(super) fn record_fields_of(&mut self, ty: Type) -> Result<Vec<(StrId, Type)>> {
        if let TypeNode::Record(row) = self.unifier.head(ty) {
            let mut out = Vec::new();
            let mut cur = row;
            loop {
                match self.unifier.head(cur) {
                    TypeNode::RowField(l, fty, rest) => {
                        out.push((l, fty));
                        cur = rest;
                    }
                    TypeNode::RowEmpty => return Ok(out),
                    _ => {
                        return Err(diag!(
                            Code::TypeMismatch, Span::at(0), 0,
                            "cannot use a record with an unknown (open) row here"
                        ))
                    }
                }
            }
        }
        let (head, args) = self.spine(ty);
        if let TypeNode::Con(name) = self.unifier.types.node(head) {
            if let Some(info) = self.structs.get(&name).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.unifier);
                return Ok(info
                    .fields
                    .iter()
                    .map(|(n, fty)| (*n, self.ty_of_ast(*fty, &mut subst)))
                    .collect());
            }
        }
        Err(diag!(
            Code::TypeMismatch, Span::at(0), 0,
            "expected a record or struct value here"
        ))
    }

    pub(super) fn resolve_struct_by_fields(&self, fields: &[FieldInit]) -> Option<(StrId, StructInfo)> {
        let mut names = Vec::with_capacity(fields.len());
        for f in fields {
            match f {
                FieldInit::Named { name, .. } => names.push(*name),
                FieldInit::Positional(_) => return None,
            }
        }
        self.structs.iter().find_map(|(sname, info)| {
            let same = info.fields.len() == names.len()
                && names
                    .iter()
                    .all(|n| info.fields.iter().any(|(f, _)| f == n));
            same.then(|| (*sname, info.clone()))
        })
    }

    pub(super) fn struct_field_ty(
        &mut self,
        info: &StructInfo,
        subst: &mut HashMap<StrId, Type>,
        name: Option<StrId>,
        index: usize,
    ) -> Option<Type> {
        let ast_ty = match name {
            Some(name) => info
                .fields
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, t)| *t),
            None => info.fields.get(index).map(|(_, t)| *t),
        }?;
        Some(self.ty_of_ast(ast_ty, subst))
    }

    pub(super) fn instantiate_params(&mut self, params: &[StrId]) -> (Vec<Type>, HashMap<StrId, Type>) {
        let mut subst = HashMap::new();
        let mut args = Vec::with_capacity(params.len());
        for p in params {
            let v = self.unifier.fresh();
            subst.insert(*p, v);
            args.push(v);
        }
        (args, subst)
    }

    pub(super) fn spine(&self, ty: Type) -> (Type, Vec<Type>) {
        let mut args = Vec::new();
        let mut cur = self.unifier.resolve(ty);
        while let TypeNode::App(head, arg) = self.unifier.types.node(cur) {
            args.push(self.unifier.resolve(arg));
            cur = self.unifier.resolve(head);
        }
        args.reverse();
        (cur, args)
    }
}

pub(super) fn applied(types: &Types, name: StrId, args: &[Type]) -> Type {
    let mut acc = types.con(name);
    for a in args {
        acc = types.app(acc, *a);
    }
    acc
}

pub(super) fn subst_from_args(
    params: &[StrId],
    args: &[Type],
    eng: &mut Unifier,
) -> HashMap<StrId, Type> {
    let mut subst = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        let a = args.get(i).cloned().unwrap_or_else(|| eng.fresh());
        subst.insert(*p, a);
    }
    subst
}

pub(super) fn collect_tyvars(ast: &Ast, ty: Aol<Ty>, out: &mut Vec<StrId>) {
    match ast.ty(ty) {
        Ty::Var(name) => {
            let name = *name;
            if !out.contains(&name) {
                out.push(name);
            }
        }
        Ty::App(a, b) => {
            collect_tyvars(ast, *a, out);
            collect_tyvars(ast, *b, out);
        }
        Ty::Arrow { from, to, .. } => {
            collect_tyvars(ast, *from, out);
            collect_tyvars(ast, *to, out);
        }
        Ty::Tuple(items) => ast.slice(*items).iter().for_each(|t| collect_tyvars(ast, *t, out)),
        Ty::Record { fields, tail } => {
            ast.slice(*fields).iter().for_each(|f| collect_tyvars(ast, f.ty, out));
            if let Some(t) = tail {
                let name = *t;
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        Ty::Sized { size, elem, .. } => {
            collect_tyvars(ast, *size, out);
            collect_tyvars(ast, *elem, out);
        }
        Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            collect_tyvars(ast, *a, out);
            collect_tyvars(ast, *b, out);
        }
        Ty::Row(row) => {
            for entry in ast.slice(row.entries) {
                ast.slice(entry.args).iter().for_each(|a| collect_tyvars(ast, *a, out));
            }
            if let Some(t) = row.tail {
                let name = t;
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        // A `@e X` type splice contributes no type variables: its type is unknown
        // until the driver expands it, after which this node no longer exists.
        Ty::Con { .. } | Ty::Nat(_) | Ty::Unit | Ty::MetaE(..) => {}
    }
}

/// Collect the type-constructor names `ty` mentions in a STRICT position: one
/// that must be built to build a value of `ty`. Used for the type dependency
/// graph behind the lazy-slot decision.
///
/// An arrow is not descended into. A function is already a suspension, so a
/// recursive occurrence behind one (`Susp: { @Int, {} -> Task }`) costs nothing
/// at construction and must stay a plain function: thunking it again would leave
/// callers applying a thunk. A type variable contributes nothing either, since a
/// parameter cannot make its owner recursive on its own.
pub(super) fn collect_tycons(ast: &Ast, ty: Aol<Ty>, out: &mut Vec<StrId>) {
    match ast.ty(ty) {
        Ty::Con { name, .. } => {
            let name = *name;
            if !out.contains(&name) {
                out.push(name);
            }
        }
        Ty::App(a, b) | Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            collect_tycons(ast, *a, out);
            collect_tycons(ast, *b, out);
        }
        Ty::Sized { size, elem, .. } => {
            collect_tycons(ast, *size, out);
            collect_tycons(ast, *elem, out);
        }
        Ty::Tuple(items) => ast.slice(*items).iter().for_each(|t| collect_tycons(ast, *t, out)),
        Ty::Record { fields, .. } => {
            ast.slice(*fields).iter().for_each(|f| collect_tycons(ast, f.ty, out))
        }
        Ty::Arrow { .. } | Ty::Row(_) | Ty::Var(_) | Ty::Nat(_) | Ty::Unit | Ty::MetaE(..) => {}
    }
}

/// Normalize a variant payload into `(optional-name, type-handle)` pairs.
pub(super) fn payload_fields(ast: &Ast, p: &Payload) -> Vec<(Option<StrId>, Aol<Ty>)> {
    match p {
        Payload::None => vec![],
        Payload::Bare(ty) => vec![(None, *ty)],
        Payload::Fields(fs) => ast
            .slice(*fs)
            .iter()
            .map(|f| (f.name, f.ty))
            .collect(),
    }
}

/// A type that would receive a `{kind}` (`"field"`/`"variant"`) twice: one it
/// declares and one a `with` splice copies in, or one two included types share.
pub(super) fn dup_member(ty: impl Display, member: impl Display, kind: &str) -> Diagnostic {
    diag!(
        Code::TypeMismatch, Span::at(0), 0,
        "type `{ty}` gets a duplicate {kind} `{member}` from a `with` include"
    )
}

/// A type declaration (or `with` splice) uses a type variable it does not declare.
/// Parameters are mandatory, so this reports the fix: list `v` after the keyword.
pub(super) fn undeclared_param(kind: &str, name: StrId, v: &StrId, declared: &[StrId], spliced: bool) -> Diagnostic {
    let how = if spliced {
        format!("`{name}` splices in a field using the type variable `{v}`")
    } else {
        format!("`{name}` uses the type variable `{v}`")
    };
    let msg = if declared.is_empty() {
        format!("{how}, but `{name}` declares no type parameters")
    } else {
        format!(
            "{how}, which is not one of `{name}`'s declared parameters `{}`",
            declared.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(" ")
        )
    };
    Diagnostic::error(Code::TypeUnbound, Span::at(0), 0, msg)
        .with_note("type parameters are mandatory")
        .with_help(format!("declare every one after the keyword, e.g. `@{kind} {v} = ...`"))
}
