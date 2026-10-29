//! Written types: turning type syntax into [`Type`]s.
//!
//! [`Checker::ty_of_ast`] elaborates a type expression from a signature, a
//! field or an alias body. It validates names (an unknown or hidden type is an
//! error with a fix), checks arities, expands aliases, turns `<...>` effect rows
//! into row types (expanding row aliases), and turns tensor shapes into sizes.
//!
//! Entry point: [`Checker::ty_of_ast`].

use super::*;

// ## Type names

impl<'a> Checker<'a> {
    /// Is `name` a usable type: a built-in base type, or a struct/union/alias
    /// declared here or imported? A bare name that is none of these is a typo (a
    /// type variable is a lowercase name).
    pub(super) fn unknown_type(&self, name: StrId) -> Diagnostic {
        let d = diag!(Code::TypeUnbound, Span::at(0), 0, "unknown type `{name}`");
        let known = self
            .structs
            .keys()
            .chain(self.unions.keys())
            .chain(self.aliases.keys())
            .map(|k| k.as_str())
            .chain(BASE_TYPES.iter().copied());
        match utilities::error::closest(name.as_str(), known) {
            Some(n) => d.with_fix(
                format!("did you mean `{n}`?"),
                vec![Edit::new(Span::at(0), n)],
                Applicability::MaybeIncorrect,
            ),
            None => d.with_note(
                "a type variable is written as a lowercase name; a capitalized type must be declared",
            ),
        }
    }

    /// Record `name` as one of this module's own types, displacing a hidden
    /// one of the same name an import brought in.
    pub(super) fn claim_type_name(&mut self, name: StrId) {
        self.type_home.insert(name, self.module_name);
        let aliased = self.alias_types.remove(&name).is_some();
        if self.hidden_types.remove(&name).is_some() || aliased {
            self.structs.remove(&name);
            self.unions.remove(&name);
            self.aliases.remove(&name);
        }
    }

    /// A type this module wrote naming a hidden type: one only an import's
    /// declarations reach. Deferred like the other type-name errors.
    pub(super) fn check_hidden_type(&mut self, ty: Aol<Ty>, name: StrId) {
        if let Some(&alias) = self.alias_types.get(&name) {
            let qualified = matches!(self.tnode(ty), Ty::Con { module: Some(_), .. });
            if qualified || self.foreign_tys.contains(&ty) || self.unknown_type.is_some() {
                return;
            }
            let mut d = diag!(
                Code::TypeUnbound, Span::at(0), 0,
                "type `{name}` is imported qualified-only";
                note: "`$ with {alias} = {}` brings in no bare names", self.alias_modules[&alias]
            );
            if let Some(span) = self.ast.ty_span(ty) {
                d = d.fill_span(span).with_fix(
                    format!("write `{alias}.{name}`"),
                    vec![Edit::new(span, format!("{alias}.{name}"))],
                    Applicability::MachineApplicable,
                );
            }
            self.unknown_type = Some(d);
            return;
        }
        let Some(&home) = self.hidden_types.get(&name) else {
            return;
        };
        if self.foreign_tys.contains(&ty) || self.unknown_type.is_some() {
            return;
        }
        let mut d = diag!(Code::TypeUnbound, Span::at(0), 0, "type `{name}` is not imported");
        d = self.with_import_fix(d, home.as_str());
        if let Some(span) = self.ast.ty_span(ty) {
            d = d.fill_span(span);
        }
        self.unknown_type = Some(d);
    }

    /// `Type.Tag` naming a type an aliased-only import brought in, without the
    /// alias. A bare `.Tag` is fine: the expected type picks the union.
    pub(super) fn unqualified_alias_ctor(&self, module: Option<StrId>, ty: Option<StrId>, tag: StrId) -> Option<Diagnostic> {
        let ty = ty?;
        let alias = *self.alias_types.get(&ty)?;
        if module.is_some() {
            return None;
        }
        Some(diag!(
            Code::TypeUnbound, Span::at(0), 0,
            "type `{ty}` is imported qualified-only";
            note: "`$ with {alias} = {}` brings in no bare names", self.alias_modules[&alias];
            help: "write `{alias}.{ty}.{tag}`"
        ))
    }

    pub(super) fn is_known_type(&self, name: StrId) -> bool {
        is_base_type(name.as_str())
            || self.structs.contains_key(&name)
            || self.unions.contains_key(&name)
            || self.aliases.contains_key(&name)
    }

    /// The type parameters a user-declared type constructor takes, if `name` is
    /// a struct, union, or alias (the ones whose arity we track).
    pub(super) fn type_params(&self, name: StrId) -> Option<&[StrId]> {
        self.structs
            .get(&name)
            .map(|i| i.params.as_slice())
            .or_else(|| self.unions.get(&name).map(|i| i.params.as_slice()))
            .or_else(|| self.aliases.get(&name).map(|(p, _)| p.as_slice()))
    }

    /// Flag a type constructor applied to the wrong number of arguments
    /// (`Weirdtype Int Int Int` for a two-parameter `Weirdtype`). Under-application
    /// is flagged only when `exact`: there are no higher-kinded parameters, so a
    /// struct or union is only ever a type once fully applied, but an alias may be
    /// left under-applied (its missing parameters become fresh variables).
    /// Deferred like other type-name errors.
    pub(super) fn check_type_arity(&mut self, ty: Aol<Ty>, exact: bool) {
        let mut count = 0;
        let mut cur = ty;
        while let Ty::App(h, _) = self.tnode(cur) {
            count += 1;
            cur = *h;
        }
        if let Ty::Con { name, .. } = self.tnode(cur) {
            let name = *name;
            if let Some(params) = self.type_params(name) {
                let arity = params.len();
                if count < arity && exact && self.unknown_type.is_none() {
                    let mut d = diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "type `{name}` takes {arity} parameter(s) but {count} {} given",
                        if count == 1 { "was" } else { "were" };
                        note: "a type constructor is a type only once it is applied to every parameter";
                        help: "write `{name} {}`, with a type for each parameter",
                            params.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(" ")
                    );
                    if let Some(span) = self.ast.ty_span(ty) {
                        d = d.fill_span(span);
                    }
                    self.unknown_type = Some(d);
                }
                if count > arity && self.unknown_type.is_none() {
                    let mut d = diag!(
                        Code::TypeMismatch, Span::at(0), 0,
                        "type `{name}` takes {arity} parameter(s) but {count} were given"
                    );
                    if let Some(span) = self.ast.ty_span(ty) {
                        d = d.fill_span(span);
                    }
                    self.unknown_type = Some(d);
                }
            }
        }
    }

    /// If `ty` is an alias applied to zero or more arguments, expand it: bind the
    /// alias's parameters to the arguments (a fresh variable for any not supplied,
    /// so an under-applied alias stays polymorphic, as for structs) and elaborate
    /// the body under that binding. Returns None when the spine head is not an alias.
    pub(super) fn try_expand_alias(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<StrId, Type>) -> Option<Type> {
        let mut args = Vec::new();
        let mut cur = ty;
        while let Ty::App(h, a) = self.tnode(cur) {
            args.push(*a);
            cur = *h;
        }
        let name = match self.tnode(cur) {
            Ty::Con { name, .. } => *name,
            _ => return None,
        };
        let (params, body) = self.aliases.get(&name)?.clone();
        self.check_hidden_type(cur, name);
        if matches!(self.tnode(body), Ty::Row(_)) {
            let span = self.ty_span_or_none(ty);
            self.defer_type_error(diag!(
                Code::TypeMismatch, span, 0,
                "`{name}` is an effect row alias, not a type";
                help: "use it inside an effect row, as in `A -> <{name}> B`"
            ));
            return Some(self.unifier.fresh());
        }
        args.reverse();
        self.check_type_arity(ty, false);
        let mut sub: HashMap<StrId, Type> = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            let t = match args.get(i) {
                Some(&a) => self.ty_of_ast(a, tvars),
                None => self.unifier.fresh(),
            };
            sub.insert(*p, t);
        }
        Some(self.ty_of_ast(body, &mut sub))
    }

    /// Keep the first type error met while elaborating a type, reported once the
    /// definition is checked (see `unknown_type`).
    pub(super) fn defer_type_error(&mut self, d: Diagnostic) {
        if self.unknown_type.is_none() {
            self.unknown_type = Some(d);
        }
    }
}

// ## Effect rows

impl<'a> Checker<'a> {
    /// Elaborate an effect row: each entry's label, or a row alias's labels in its
    /// place, extended onto the tail. A label occurs once when an alias brings it,
    /// since an alias names a set. The tail is the written one together with any
    /// an alias brings (all one row variable), or the closed row when there is none.
    pub(super) fn row_of_ast(
        &mut self,
        entries: Slice<RowEntry>,
        tail: Option<StrId>,
        tvars: &mut HashMap<StrId, Type>,
    ) -> Type {
        let mut labels: Vec<(StrId, bool, Type)> = Vec::new();
        let mut tails: Vec<Type> = Vec::new();
        if let Some(t) = tail {
            tails.push(*tvars.entry(t).or_insert_with(|| self.unifier.fresh()));
        }
        self.expand_row_entries(entries, tvars, false, &mut labels, &mut tails);
        let mut row = match tails.split_first() {
            None => self.unifier.types.row_empty(),
            Some((&first, rest)) => {
                for &other in rest {
                    if let Err(d) = self.unifier.unify(first, other, "in an effect row") {
                        self.defer_type_error(d);
                    }
                }
                first
            }
        };
        for &(label, _, args) in labels.iter().rev() {
            row = self.unifier.types.row_extend_args(label, args, row);
        }
        row
    }

    /// The type arguments of effect label `name` written with `args`. Omitted
    /// arguments of a parameterized effect are fresh variables, except inside
    /// that effect's own declaration, where they are its parameters. A label
    /// whose effect is not visible here keeps what was written, or matches any
    /// arguments when nothing was.
    pub(super) fn effect_label_args(
        &mut self,
        name: StrId,
        args: &[Aol<Ty>],
        span: Span,
        tvars: &mut HashMap<StrId, Type>,
    ) -> Type {
        if args.is_empty() {
            if let Some((own, own_args)) = self.effect_self {
                if own == name {
                    return own_args;
                }
            }
        }
        let arity = self.effect_arity.get(&name).copied();
        match arity {
            Some(0) if !args.is_empty() => self.defer_type_error(diag!(
                Code::TypeMismatch, span, 0,
                "effect `{name}` takes no parameters";
                note: "only an effect or a row alias declared with parameters is applied in a row"
            )),
            Some(n) if !args.is_empty() && args.len() != n => self.defer_type_error(diag!(
                Code::TypeMismatch, span, 0,
                "effect `{name}` takes {n} parameter(s) but {} were given", args.len();
                help: "write all of them, or none to leave them to inference"
            )),
            _ => {}
        }
        let tys: Vec<Type> = if args.is_empty() {
            (0..arity.unwrap_or(0)).map(|_| self.unifier.fresh()).collect()
        } else {
            args.iter().map(|&a| self.ty_of_ast(a, tvars)).collect()
        };
        if tys.is_empty() {
            self.unifier.types.row_empty()
        } else {
            self.unifier.types.tuple(tys)
        }
    }

    /// Append a row's entries to `labels` (each tagged with whether an alias
    /// brought it) and the tails its aliases bring to `tails`.
    pub(super) fn expand_row_entries(
        &mut self,
        entries: Slice<RowEntry>,
        tvars: &mut HashMap<StrId, Type>,
        in_alias: bool,
        labels: &mut Vec<(StrId, bool, Type)>,
        tails: &mut Vec<Type>,
    ) {
        let ast = self.ast;
        for entry in ast.slice(entries) {
            let name = entry.name;
            let args = ast.slice(entry.args);
            let Some((params, body)) = self.aliases.get(&name).cloned() else {
                let dup = labels.iter().any(|&(l, from, _)| l == name && (from || in_alias));
                if !dup {
                    let args = self.effect_label_args(name, args, entry.span, tvars);
                    labels.push((name, in_alias, args));
                }
                continue;
            };
            let Ty::Row(row) = ast.ty(body) else {
                self.defer_type_error(diag!(
                    Code::TypeMismatch, entry.span, 0,
                    "type alias `{name}` is not an effect row";
                    help: "an effect row names effects, or row aliases declared as \
                           `$ {name} : @alias = <...>`"
                ));
                continue;
            };
            if args.len() > params.len() {
                self.defer_type_error(diag!(
                    Code::TypeMismatch, entry.span, 0,
                    "row alias `{name}` takes {} parameter(s) but {} were given",
                    params.len(), args.len()
                ));
                continue;
            }
            let mut sub: HashMap<StrId, Type> = HashMap::new();
            for (i, p) in params.iter().enumerate() {
                let t = match args.get(i) {
                    Some(&a) => self.ty_of_ast(a, tvars),
                    None => self.unifier.fresh(),
                };
                sub.insert(*p, t);
            }
            self.expand_row_entries(row.entries, &mut sub, true, labels, tails);
            if let Some(t) = row.tail {
                tails.push(*sub.entry(t).or_insert_with(|| self.unifier.fresh()));
            }
        }
    }

    /// The labels a parameterless, closed row alias stands for, through any
    /// aliases it names, or None when it is not one (or names something else).
    pub(super) fn closed_row_alias_labels(&self, name: StrId) -> Option<Vec<String>> {
        let (params, body) = self.aliases.get(&name)?;
        let Ty::Row(row) = self.ast.ty(*body) else {
            return None;
        };
        if !params.is_empty() || row.tail.is_some() {
            return None;
        }
        let mut out: Vec<String> = Vec::new();
        for entry in self.ast.slice(row.entries) {
            let label = entry.name;
            let more = if self.aliases.contains_key(&label) {
                if !self.ast.slice(entry.args).is_empty() {
                    return None;
                }
                self.closed_row_alias_labels(label)?
            } else {
                vec![label.to_string()]
            };
            for l in more {
                if !out.contains(&l) {
                    out.push(l);
                }
            }
        }
        Some(out)
    }

    /// Hand the unifier the row aliases a printed row folds back to: every closed,
    /// parameterless one of at least two labels (a one-label alias is a mere
    /// rename, and printing it would hide the effect's own name), largest first.
    pub(super) fn register_row_folds(&mut self) {
        let mut folds: Vec<(String, Vec<String>)> = self
            .aliases
            .keys()
            .filter_map(|name| {
                let labels = self.closed_row_alias_labels(*name)?;
                (labels.len() >= 2).then(|| (name.to_string(), labels))
            })
            .collect();
        folds.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        self.unifier.row_folds = folds;
    }
}

// ## Elaboration

impl<'a> Checker<'a> {
    /// Elaborate a tensor size: a `Nat` literal, or a (Nat-kinded) size variable
    /// bound by name in `tvars` so `[n]T -> [n]U` shares the one size.
    pub(super) fn size_ty_of_ast(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<StrId, Type>) -> Type {
        match self.tnode(ty) {
            Ty::Nat(n) => self.unifier.types.add(TypeNode::Nat(*n)),
            Ty::Var(name) => {
                let eng = &mut self.unifier;
                *tvars.entry(*name).or_insert_with(|| eng.fresh_nat())
            }
            Ty::SizeAdd(a, b) => {
                let (a, b) = (*a, *b);
                let (x, y) = (
                    self.size_ty_of_ast(a, tvars),
                    self.size_ty_of_ast(b, tvars),
                );
                self.unifier.types.add(TypeNode::NatAdd(x, y))
            }
            Ty::SizeMul(a, b) => {
                let (a, b) = (*a, *b);
                let (x, y) = (
                    self.size_ty_of_ast(a, tvars),
                    self.size_ty_of_ast(b, tvars),
                );
                self.unifier.types.add(TypeNode::NatMul(x, y))
            }
            _ => self.ty_of_ast(ty, tvars),
        }
    }

    /// The value of an `Int` literal expression, else `None` (used to read the
    /// compile-time bounds of a range that builds a sized tensor).
    pub(super) fn int_literal(&self, e: Aol<Expr>) -> Option<i64> {
        match self.node(e) {
            Expr::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Peel a sized-tensor type `@tensor variance size elem` into `(size, elem)`,
    /// discarding the variance (callers here only need the length and element).
    pub(super) fn tensor_parts(&self, ty: Type) -> Option<(Type, Type)> {
        if let TypeNode::App(head, elem) = self.unifier.head(ty) {
            if let TypeNode::App(head2, size) = self.unifier.head(head) {
                if let TypeNode::App(con, _variance) = self.unifier.head(head2) {
                    if matches!(self.unifier.head(con), TypeNode::Con(n) if self.unifier.types.name(n) == TENSOR)
                    {
                        return Some((size, elem));
                    }
                }
            }
        }
        None
    }

    /// An application spine whose arity the caller has already checked: the
    /// partial applications inside it (`Pair @Int` in `Pair @Int @Str`) are not
    /// types of their own, so they are not checked again.
    pub(super) fn spine_of_ast(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<StrId, Type>) -> Type {
        match self.tnode(ty) {
            Ty::App(head, arg) => {
                let (head, arg) = (*head, *arg);
                let h = self.spine_of_ast(head, tvars);
                let a = self.ty_of_ast(arg, tvars);
                self.unifier.types.app(h, a)
            }
            Ty::Con { .. } => self.con_of_ast(ty),
            _ => self.ty_of_ast(ty, tvars),
        }
    }

    pub(super) fn con_of_ast(&mut self, ty: Aol<Ty>) -> Type {
        let Ty::Con { name, .. } = self.tnode(ty) else {
            unreachable!("con_of_ast on a non-constructor");
        };
        let name = *name;
        if !self.is_known_type(name) && self.unknown_type.is_none() {
            let mut d = self.unknown_type(name);
            if let Some(span) = self.ast.ty_span(ty) {
                d = d.fill_span(span);
            }
            self.unknown_type = Some(d);
        }
        self.check_hidden_type(ty, name);
        self.unifier.types.con(name)
    }

    pub(super) fn ty_of_ast(&mut self, ty: Aol<Ty>, tvars: &mut HashMap<StrId, Type>) -> Type {
        // An alias at the head of an application spine expands first, so `MapInt Bool`
        // substitutes into `Map Int Bool` rather than forming `App(alias, Bool)`.
        if let Some(t) = self.try_expand_alias(ty, tvars) {
            return t;
        }
        match self.tnode(ty) {
            Ty::Con { .. } => {
                self.check_type_arity(ty, true);
                self.con_of_ast(ty)
            }
            Ty::Var(name) => {
                *tvars
                    .entry(*name)
                    .or_insert_with(|| self.unifier.fresh())
            }
            Ty::App(..) => {
                self.check_type_arity(ty, true);
                self.spine_of_ast(ty, tvars)
            }
            // `@e X` in type position: infer `X` (so its calls/overloads resolve
            // for lowering) and require it to build `@Code`; the spliced-in type is
            // unknown until the driver's expand loop runs `X`, so it stands as a
            // fresh variable here. Only reachable in a lenient (pre-expansion)
            // round; the final strict compile sees the substituted concrete type.
            Ty::MetaE(expr) => {
                let expr = *expr;
                let ambient = self.meta_ambient();
                let saved = std::mem::replace(&mut self.ambient, ambient);
                if let Ok(t) = self.infer(expr) {
                    let _ = self.unifier.unify(t, self.unifier.types.con("@Code"), "in a `@e` type splice");
                }
                self.ambient = saved;
                self.unifier.fresh()
            }
            Ty::Nat(n) => self.unifier.types.add(TypeNode::Nat(*n)),
            // A size expression written in type position (only well-formed inside a
            // `[..]`); elaborate it as a size so kind-checking flags any misuse.
            Ty::SizeAdd(..) | Ty::SizeMul(..) => self.size_ty_of_ast(ty, tvars),
            Ty::Sized {
                variance,
                size,
                elem,
            } => {
                let (variance, size, elem) = (*variance, *size, *elem);
                let size_ty = self.size_ty_of_ast(size, tvars);
                let elem_ty = self.ty_of_ast(elem, tvars);
                tensor_type(&self.unifier.types, variance_con(&self.unifier.types, variance), size_ty, elem_ty)
            }
            Ty::Arrow { from, effect, to } => {
                let (from, to) = (*from, *to);
                // An unannotated arrow is pure.
                let eff = match effect {
                    Some(row) => self.row_of_ast(row.entries, row.tail, tvars),
                    None => self.unifier.types.row_empty(),
                };
                let (f, t) = (self.ty_of_ast(from, tvars), self.ty_of_ast(to, tvars));
                self.unifier.types.arrow_eff(f, t, eff)
            }
            Ty::Row(_) => self.unifier.fresh(),
            Ty::Unit => self.unifier.types.con(ty::UNIT),
            Ty::Tuple(items) => {
                let items = self.ast.slice(*items);
                let mapped: Vec<Type> =
                    items.iter().map(|t| self.ty_of_ast(*t, tvars)).collect();
                self.unifier.types.tuple(mapped)
            }
            Ty::Record { fields, tail } => {
                // A record type: `{ x: A | r }` is open (row variable tail), `{ x: A,
                // y: B }` is closed (empty tail). Records are real, name-keyed types;
                // positional/scalar values promote to them at call sites.
                let rest = match tail {
                    Some(tvar) => *tvars.entry(*tvar).or_insert_with(|| self.unifier.fresh()),
                    None => self.unifier.types.row_empty(),
                };
                let decls: Vec<_> = self.ast.slice(*fields).to_vec();
                let mut row = rest;
                for f in decls.iter().rev() {
                    let fty = self.ty_of_ast(f.ty, tvars);
                    row = self.unifier.types.row_field(f.name, fty, row);
                }
                self.unifier.types.record(row)
            }
        }
    }
}
