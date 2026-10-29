//! Imports: bringing other modules' names into scope.
//!
//! A module checked after its imports copies their exported values, types,
//! effects, instances and context-bearing definitions into its own tables
//! ([`Checker::import_from`], [`Checker::import_as`]). Names reachable only
//! qualified, names two imports both bring in, and private names are recorded so
//! a use can be resolved or rejected with the right message. An unbound name is
//! reported here too, with an import fix when another module defines it.
//!
//! Entry points: [`Checker::import_from`], [`Checker::import_as`],
//! [`Checker::import_qualified`].

use super::*;

// ## Import declarations

impl<'a> Checker<'a> {
    /// Record every name declared under a `$ @private` marker. Symbols are public
    /// by default; a `$ @private` makes every declaration below it, to the end of
    /// the file, module-private. A private name is fully usable inside the module;
    /// it is only withheld from importers.
    pub(super) fn collect_private_names(&mut self, program: &Program) {
        let mut private = false;
        for item in self.ast.slice(program.items).iter() {
            let name = match item {
                Item::Private => {
                    private = true;
                    continue;
                }
                _ if !private => continue,
                Item::Def { name, .. }
                | Item::Struct { name, .. }
                | Item::Union { name, .. }
                | Item::Alias { name, .. } => *name,
                _ => continue,
            };
            self.private_names.insert(name);
        }
    }

    /// Every qualifier a module writes must name one thing: an alias may not
    /// repeat, nor reuse the name of another imported module, this module, or `C`.
    pub(super) fn check_import_aliases(&mut self, program: &Program) {
        let dotted = |parts: Slice<StrId>| {
            self.ast.slice(parts).iter().map(|p| p.as_str()).collect::<Vec<_>>().join(".")
        };
        let mut plain: HashSet<String> = HashSet::new();
        let mut aliased: Vec<(StrId, String, Span)> = Vec::new();
        for item in self.ast.slice(program.items) {
            match item {
                Item::Import { module, alias: None, .. } => {
                    plain.insert(dotted(*module));
                }
                Item::Import { module, alias: Some(alias), span } => {
                    aliased.push((*alias, dotted(*module), *span));
                }
                _ => {}
            }
        }
        let mut seen: HashMap<StrId, String> = HashMap::new();
        for (alias, module, span) in aliased {
            let clash = if let Some(first) = seen.get(&alias) {
                Some(format!("`{alias}` is already an alias of `{first}`"))
            } else if alias != module.as_str() && plain.contains(alias.as_str()) {
                Some(format!("`{alias}` is already the name of an imported module"))
            } else if alias == self.module_name {
                Some(format!("`{alias}` is the name of this module"))
            } else if alias == "C" || alias == "TARGET" {
                Some(format!("`{alias}` is a built-in namespace"))
            } else {
                None
            };
            match clash {
                Some(why) => self.errors.push(diag!(
                    Code::DuplicateSymbol, span, 0,
                    "import alias `{alias}` names more than one thing";
                    note: "{why}";
                    help: "pick another alias for `{module}`"
                )),
                None => {
                    seen.insert(alias, module);
                }
            }
        }
    }

    /// The built-in `TARGET` namespace: facts about the compilation target that
    /// both engines supply as constants (see the machine's `target_value`).
    pub(super) fn register_target_namespace(&mut self) {
        let int = self.unifier.types.con("@Int");
        let str_ = self.unifier.types.con("@Str");
        let members = [
            ("int_bits", int),
            ("ptr_bits", int),
            ("int_max", int),
            ("int_min", int),
            ("arch", str_),
            ("os", str_),
            ("name", str_),
        ];
        let table = self.qualified.entry(StrId::intern("TARGET")).or_default();
        for (name, ty) in members {
            table.insert(StrId::intern(name), vec![ty]);
        }
    }
}

// ## Importing a module

impl<'a> Checker<'a> {
    /// Import another module's public exports into this checker.
    /// Import another module's exports as QUALIFIED-only (`Module.name`), without
    /// adding them to the unqualified namespace. Used for the auto-injected `C`
    /// namespace, which is reachable as `C.foo` everywhere but must not pollute
    /// bare names (a program's own `sqrt` is not libm's).
    pub fn import_qualified(&mut self, other: &Checker<'a>) {
        let module = other.module_name;
        for (name, scheme) in &other.own_values {
            if other.private_names.contains(name) {
                continue;
            }
            let qualified = self.import_scheme(*scheme);
            self.qualified
                .entry(module)
                .or_default()
                .insert(*name, vec![qualified]);
        }
    }

    pub fn import_from(&mut self, other: &Checker<'a>) {
        self.import_as(other, true, &[]);
    }

    /// Import another module's exports: bare when `bare`, and qualified under its
    /// own name then. Each of `aliases` (`$ with V = VEC`) is a further qualifier
    /// for them, and an aliased-only import (`bare` false) brings no name in bare.
    pub fn import_as(&mut self, other: &Checker<'a>, bare: bool, aliases: &[StrId]) {
        assert!(bare || !aliases.is_empty(), "an import is bare or aliased");
        let module = other.module_name;
        let mut scopes: Vec<StrId> = aliases.to_vec();
        if bare {
            scopes.push(module);
        }
        for &alias in aliases {
            self.alias_modules.insert(alias, module);
        }
        self.imported_builtins.extend(other.own_builtins.iter().copied());
        if !other.private_names.is_empty() {
            for &scope in &scopes {
                self.imported_private
                    .entry(scope)
                    .or_default()
                    .extend(other.private_names.iter().copied());
            }
        }
        // Bring the exporter's context-bearing functions in, so a bare or qualified
        // use of an imported one plans its context (the signature handle lives in
        // the shared `Ast`).
        for (name, sig) in &other.own_ctx {
            if other.private_names.contains(name) {
                continue;
            }
            for &scope in &scopes {
                self.qualified_ctx.insert((scope, *name), *sig);
            }
            if bare {
                self.global_ctx.insert(*name, *sig);
            }
        }
        // Bring the exporter's instances in, translating their packed types into
        // this module's unifier so their variables stay generalized.
        for (name, inst) in &other.own_instances {
            if other.private_names.contains(name) {
                continue;
            }
            let mut map = HashMap::new();
            let bundle = self.import_ty(inst.bundle, &mut map);
            self.unifier.note_tensor_sizes(bundle);
            let Some(head) = self.ctx_head(self.unifier.zonk(self.bundle_parts(bundle).0)) else {
                continue;
            };
            self.instances.entry(head).or_default().push(Inst {
                name: inst.name,
                module: Some(other.module_name),
                bundle,
                has_ctx: inst.has_ctx,
            });
        }
        for &name in &other.own_type_names {
            if other.private_names.contains(&name) {
                continue;
            }
            if bare {
                self.hidden_types.remove(&name);
                self.alias_types.remove(&name);
            } else {
                let named_bare = self.is_known_type(name)
                    && !self.hidden_types.contains_key(&name)
                    && !self.alias_types.contains_key(&name);
                if named_bare {
                    continue;
                }
                self.hidden_types.remove(&name);
                self.alias_types.insert(name, aliases[0]);
            }
            self.type_home.insert(name, other.module_name);
            if let Some(s) = other.structs.get(&name) {
                self.structs.insert(name, s.clone());
            }
            if let Some(u) = other.unions.get(&name) {
                self.unions.insert(name, u.clone());
            }
            if let Some(a) = other.aliases.get(&name) {
                self.aliases.insert(name, a.clone());
            }
        }
        // Everything the exporter's declarations can mention, so they elaborate
        // here too (#311). What this module cannot name stays hidden.
        let names: Vec<StrId> = other
            .structs
            .keys()
            .chain(other.unions.keys())
            .chain(other.aliases.keys())
            .copied()
            .collect();
        for name in names {
            if self.is_known_type(name) {
                continue;
            }
            let home = other
                .type_home
                .get(&name)
                .copied()
                .unwrap_or(other.module_name);
            self.hidden_types.insert(name, home);
            self.type_home.insert(name, home);
            if let Some(s) = other.structs.get(&name) {
                self.structs.insert(name, s.clone());
            }
            if let Some(u) = other.unions.get(&name) {
                self.unions.insert(name, u.clone());
            }
            if let Some(a) = other.aliases.get(&name) {
                self.aliases.insert(name, a.clone());
            }
        }
        let mut roots: Vec<Aol<Ty>> = Vec::new();
        for s in other.structs.values() {
            roots.extend(s.fields.iter().map(|(_, t)| *t));
        }
        for u in other.unions.values() {
            for v in &u.variants {
                roots.extend(v.payload.iter().map(|(_, t)| *t));
            }
        }
        roots.extend(other.aliases.values().map(|(_, t)| *t));
        roots.extend(other.own_ctx.iter().map(|(_, t)| *t));
        for root in roots {
            collect_ty_nodes(self.ast, root, &mut self.foreign_tys);
        }
        self.foreign_tys.extend(other.foreign_tys.iter().copied());
        for &effect in &other.own_effects {
            if other.private_names.contains(&effect) {
                continue;
            }
            if let Some(&n) = other.effect_arity.get(&effect) {
                self.effect_arity.insert(effect, n);
            }
            for (&op, &scheme) in &other.effect_ops[&effect] {
                let qualified = self.import_scheme(scheme);
                self.qualified
                    .entry(effect)
                    .or_default()
                    .insert(op, vec![qualified]);
                self.imported_effect_ops
                    .entry(effect)
                    .or_default()
                    .insert(op, qualified);
            }
        }
        for (name, scheme) in &other.own_values {
            if other.private_names.contains(name) {
                continue;
            }
            if bare {
                let unqualified = self.import_scheme(*scheme);
                self.imported
                    .entry(*name)
                    .or_default()
                    .push((unqualified, module));
            } else {
                self.alias_values.entry(*name).or_default().extend(aliases);
            }
            for &scope in &scopes {
                let qualified = self.import_scheme(*scheme);
                self.qualified
                    .entry(scope)
                    .or_default()
                    .insert(*name, vec![qualified]);
            }
        }
    }

    pub(super) fn finalize_imports(&mut self) {
        let imported = std::mem::take(&mut self.imported);
        for (name, mut cands) in imported {
            if cands.len() == 1 {
                let (ty, module) = cands.pop().expect("one candidate");
                self.value_module.insert(name, module);
                self.bind(name, ty);
            } else {
                // Two modules export the same name. Global scope is flat, so the bare
                // name cannot pick one; it is an error at the USE site (with the
                // qualified form as the escape), not here, because a module may import
                // both and reference neither bare.
                let mut owners: Vec<StrId> = cands.iter().map(|(_, m)| *m).collect();
                owners.sort_unstable_by_key(|m| m.as_str());
                self.ambiguous_imports.insert(name, owners);
            }
        }
    }

    pub(super) fn import_scheme(&mut self, ty: Type) -> Type {
        let mut map = HashMap::new();
        let imported = self.import_ty(ty, &mut map);
        // `import_ty` makes fresh plain generics, losing the `Nat` kind; re-mark the
        // tensor-size variables so an imported `[n]a -> ...` still kind-checks.
        self.unifier.note_tensor_sizes(imported);
        imported
    }

    pub(super) fn import_ty(&mut self, ty: Type, map: &mut HashMap<VarId, Type>) -> Type {
        match self.unifier.types.node(ty) {
            TypeNode::Var(id) => match map.get(&id) {
                Some(t) => *t,
                None => {
                    let fresh = self.unifier.fresh_generic();
                    map.insert(id, fresh);
                    fresh
                }
            },
            TypeNode::NatAdd(a, b) => {
                let (x, y) = (self.import_ty(a, map), self.import_ty(b, map));
                self.unifier.types.add(TypeNode::NatAdd(x, y))
            }
            TypeNode::NatMul(a, b) => {
                let (x, y) = (self.import_ty(a, map), self.import_ty(b, map));
                self.unifier.types.add(TypeNode::NatMul(x, y))
            }
            TypeNode::App(head, arg) => {
                let (h, a) = (self.import_ty(head, map), self.import_ty(arg, map));
                self.unifier.types.app(h, a)
            }
            TypeNode::Arrow(from, to, eff) => {
                let (f, t, e) = (
                    self.import_ty(from, map),
                    self.import_ty(to, map),
                    self.import_ty(eff, map),
                );
                self.unifier.types.arrow_eff(f, t, e)
            }
            TypeNode::RowExtend(label, args, rest) => {
                let a = self.import_ty(args, map);
                let r = self.import_ty(rest, map);
                self.unifier.types.add(TypeNode::RowExtend(label, a, r))
            }
            TypeNode::Tuple(items) => {
                let items = self.unifier.types.items(items).to_vec();
                let mapped: Vec<Type> = items.into_iter().map(|t| self.import_ty(t, map)).collect();
                self.unifier.types.tuple(mapped)
            }
            TypeNode::Record(row) => {
                let r = self.import_ty(row, map);
                self.unifier.types.record(r)
            }
            TypeNode::RowField(label, fty, rest) => {
                let (f, r) = (self.import_ty(fty, map), self.import_ty(rest, map));
                self.unifier.types.add(TypeNode::RowField(label, f, r))
            }
            // Con, Nat and RowEmpty are already shared nodes.
            TypeNode::Con(_) | TypeNode::Nat(_) | TypeNode::RowEmpty => ty,
        }
    }
}

// ## Names not in scope

impl<'a> Checker<'a> {
    /// `name` is bound nowhere; suggest a near miss among the visible names.
    pub(super) fn unbound_name(&self, name: StrId) -> Diagnostic {
        if let Some(&alias) = self.alias_values.get(&name).and_then(|a| a.first()) {
            return unbound(name)
                .with_note(format!("`$ with {alias} = {}` brings in no bare names", self.alias_modules[&alias]))
                .with_fix(
                    format!("write `{alias}.{name}`"),
                    vec![Edit::new(Span::at(0), format!("{alias}.{name}"))],
                    Applicability::MachineApplicable,
                );
        }
        let visible = self
            .scopes
            .iter()
            .flat_map(|s| s.keys())
            .chain(self.overloads.keys())
            .chain(self.global_ctx.keys())
            .map(|n| n.as_str());
        match utilities::error::closest(name.as_str(), visible) {
            Some(n) => unbound(name).with_fix(
                format!("did you mean `{n}`?"),
                vec![Edit::new(Span::at(0), n)],
                Applicability::MaybeIncorrect,
            ),
            None => match self.import_hints.get(name.as_str()).map(Vec::as_slice) {
                Some([home]) => {
                    let home = home.clone();
                    self.with_import_fix(unbound(name), &home)
                }
                Some(homes) if !homes.is_empty() => unbound(name).with_help(format!(
                    "it is defined in {}; import one of them",
                    homes.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")
                )),
                _ => unbound(name),
            },
        }
    }

    /// Add a fix that imports `module`, inserted next to this module's other
    /// imports (prose only when the file has no item to anchor it to).
    pub(super) fn with_import_fix(&self, d: Diagnostic, module: &str) -> Diagnostic {
        let msg = format!("import it with `$ with {module}`");
        match self.import_anchor {
            Some((at, before, after)) => d.with_fix(
                msg,
                vec![Edit::new(Span::at(at), format!("{before}$ with {module}{after}"))],
                Applicability::MachineApplicable,
            ),
            None => d.with_help(msg),
        }
    }

    pub(super) fn qualified_candidates(&self, module: StrId, name: StrId) -> Option<Vec<Type>> {
        self.qualified.get(&module)?.get(&name).cloned()
    }

    /// `module.name` names nothing: either the module exports no such value or
    /// no module or effect of that name is in scope.
    pub(super) fn unknown_qualified(&self, module: StrId, name: StrId) -> Diagnostic {
        let Some(members) = self.qualified.get(&module) else {
            let d = diag!(Code::UnknownModule, Span::at(0), 0, "no module or effect `{module}` is in scope");
            let mut aliases: Vec<&str> =
                self.alias_modules.iter().filter(|(_, &m)| m == module).map(|(a, _)| a.as_str()).collect();
            aliases.sort_unstable();
            if let Some(alias) = aliases.first() {
                return d
                    .with_note(format!("`{module}` is imported as `{alias}`"))
                    .with_fix(
                        format!("write `{alias}.{name}`"),
                        vec![Edit::new(Span::at(0), format!("{alias}.{name}"))],
                        Applicability::MachineApplicable,
                    );
            }
            return match utilities::error::closest(module.as_str(), self.qualified.keys().map(|k| k.as_str())) {
                Some(m) => d.with_help(format!("did you mean `{m}.{name}`?")),
                None => d.with_help(format!("import it with `$ with {module}`")),
            };
        };
        let d = diag!(Code::TypeUnbound, Span::at(0), 0, "`{module}` has no value named `{name}`");
        match utilities::error::closest(name.as_str(), members.keys().map(|k| k.as_str())) {
            Some(n) => d.with_help(format!("did you mean `{module}.{n}`?")),
            None => d,
        }
    }
}

/// Every type node in `ty`, itself included.
pub(super) fn collect_ty_nodes(ast: &Ast, ty: Aol<Ty>, out: &mut HashSet<Aol<Ty>>) {
    if !out.insert(ty) {
        return;
    }
    let row = |row: &EffectRow, out: &mut HashSet<Aol<Ty>>| {
        for entry in ast.slice(row.entries) {
            ast.slice(entry.args)
                .iter()
                .for_each(|a| collect_ty_nodes(ast, *a, out));
        }
    };
    match ast.ty(ty) {
        Ty::Con { .. } | Ty::Var(_) | Ty::Nat(_) | Ty::MetaE(_) | Ty::Unit => {}
        Ty::App(a, b) | Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) => {
            collect_ty_nodes(ast, *a, out);
            collect_ty_nodes(ast, *b, out);
        }
        Ty::Sized { size, elem, .. } => {
            collect_ty_nodes(ast, *size, out);
            collect_ty_nodes(ast, *elem, out);
        }
        Ty::Arrow { from, effect, to } => {
            collect_ty_nodes(ast, *from, out);
            collect_ty_nodes(ast, *to, out);
            if let Some(r) = effect {
                row(r, out);
            }
        }
        Ty::Row(r) => row(r, out),
        Ty::Tuple(items) => ast
            .slice(*items)
            .iter()
            .for_each(|t| collect_ty_nodes(ast, *t, out)),
        Ty::Record { fields, .. } => ast
            .slice(*fields)
            .iter()
            .for_each(|f| collect_ty_nodes(ast, f.ty, out)),
    }
}

/// Where an added `$ with M` line goes: after the last import, or else before the
/// first item. `None` for a file with no items to place it by.
pub(super) fn import_anchor(ast: &Ast, program: &Program) -> Option<(usize, &'static str, &'static str)> {
    let items = ast.slice(program.items);
    let spans = ast.slice(program.spans);
    let last_import = items
        .iter()
        .zip(spans)
        .filter(|(item, _)| matches!(item, Item::Import { .. }))
        .last();
    match last_import {
        Some((_, span)) => Some((span.full.end, "\n", "")),
        None if program.header != Span::at(0) => Some((program.header.end, "\n\n", "")),
        None => spans.first().map(|s| (s.full.start, "", "\n\n")),
    }
}
