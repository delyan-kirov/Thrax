//! Algorithm W over the [`syntax`] AST.
//!
//! Inference is driven by [`Checker::infer`] (expressions) and
//! [`Checker::type_pattern`] (patterns), threading the [`Engine`] for
//! unification and the lexical scope stack for variable types. Global
//! definitions are grouped into strongly-connected components (see
//! [`crate::scc`]) and checked in dependency order: members of a component are
//! bound to fresh monomorphic variables while their bodies are inferred (so
//! self- and mutual recursion resolve), then the component is generalized
//! before the components that depend on it see it (let-polymorphism).
//!
//! Structs, unions, aliases, and their generic parameters are registered up
//! front by [`Checker::register_types`], so constructors, field access, variant
//! construction, and struct/variant patterns are typed at their instantiation.
//!
//! Overloaded names (built-in arithmetic on Int/Real, the `array_*` sequence
//! primitives, and any user name defined several times) are resolved at each use
//! site by trial unification against the argument and result types; ambiguous
//! uses are deferred and solved to a fixpoint at the definition boundary, where
//! leftover integer literals default to Int. Definition bodies are checked
//! against their signatures (bidirectional checking) so parameters have known
//! types. The monomorphism restriction keeps overload-constrained variables from
//! being generalized early.
//!
//! Deferred to later increments (each returns a fresh variable rather than
//! failing, so surrounding code still checks): cross-module `with` imports,
//! effect rows, record-parameter and `with` field scoping, type-directed
//! sequence literals (`[..]` as Array vs List), and pattern exhaustiveness. A
//! bare `.Tag` still resolves to a union by name, not from the expected type.

use std::collections::{HashMap, HashSet};

use diag::{Code, Diagnostic, Result, Span};
use syntax::{Binding, Expr, FieldInit, FieldPat, Item, Pattern, Payload, Program, RecField, Ty};

use crate::engine::Engine;
use crate::ty::{self, Type, VarId};

/// A declared struct type. `params` are the implicit type parameters (the type
/// variables appearing in the fields, in order of first appearance); `fields`
/// keeps declaration order (which is also the positional-constructor order).
#[derive(Clone)]
struct StructInfo<'a> {
    params: Vec<&'a str>,
    fields: Vec<(&'a str, &'a Ty<'a>)>,
}

/// A declared union type: implicit `params` and one [`VariantSig`] per variant.
#[derive(Clone)]
struct UnionInfo<'a> {
    params: Vec<&'a str>,
    variants: Vec<VariantSig<'a>>,
}

/// A union variant: its tag and its (normalized) payload fields, each an
/// optional name and its declared type.
#[derive(Clone)]
struct VariantSig<'a> {
    tag: &'a str,
    payload: Vec<(Option<&'a str>, &'a Ty<'a>)>,
}

/// A variant's payload instantiated to concrete types: one `(optional-name,
/// type)` pair per field, in declaration order.
type VariantPayload<'a> = Vec<(Option<&'a str>, Type)>;

pub struct Checker<'a> {
    eng: Engine,
    scopes: Vec<HashMap<&'a str, Type>>,
    structs: HashMap<&'a str, StructInfo<'a>>,
    unions: HashMap<&'a str, UnionInfo<'a>>,
    aliases: HashMap<&'a str, &'a Ty<'a>>,
    /// Names with more than one candidate type (built-in arithmetic and any
    /// user name defined several times). Resolved at each use site by trial
    /// unification against the argument types.
    overloads: HashMap<&'a str, Vec<Type>>,
    /// Overload uses that were ambiguous when first seen (their argument types
    /// were not yet concrete). Solved to a fixpoint at each definition boundary.
    pending: Vec<Pending>,
    /// Type variables introduced by integer literals, which may be Int or Real
    /// (Thrax allows `1 + 1.0`). Context resolves most; any left over default to
    /// Int at the definition boundary.
    numeric: Vec<Type>,
    /// This module's own exports, recorded after checking so importers can pull
    /// them in: single-definition value schemes, overloaded-name candidate sets,
    /// and the names of types declared here.
    own_values: Vec<(&'a str, Type)>,
    own_overloads: Vec<(&'a str, Vec<Type>)>,
    own_type_names: Vec<&'a str>,
    /// Value schemes pulled in from imports, accumulated across all `$ with`
    /// modules and finalized once: a name imported from a single module becomes
    /// a plain binding, one imported with several candidates (or from several
    /// modules) becomes an overload.
    imported: HashMap<&'a str, Vec<Type>>,
    /// Imported names reachable qualified as `MOD.name` (module -> name ->
    /// candidate schemes), so `MAP.new` resolves to MAP's `new` even when a local
    /// `new` shadows it unqualified.
    qualified: HashMap<&'a str, HashMap<&'a str, Vec<Type>>>,
    /// This module's own `@mod` name, used to key its qualified exports.
    module_name: &'a str,
}

/// A deferred overload use: its candidate set, the argument types, and the fresh
/// result variable standing in for the (not-yet-known) result.
struct Pending {
    name: String,
    candidates: Vec<Type>,
    args: Vec<Type>,
    result: Type,
}

/// The outcome of trying a candidate set against argument types.
enum Match {
    Unique(usize),
    None,
    Ambiguous,
}

impl<'a> Checker<'a> {
    pub fn new() -> Checker<'a> {
        let mut c = Checker {
            eng: Engine::new(),
            scopes: vec![HashMap::new()],
            structs: HashMap::new(),
            unions: HashMap::new(),
            aliases: HashMap::new(),
            overloads: HashMap::new(),
            pending: Vec::new(),
            numeric: Vec::new(),
            own_values: Vec::new(),
            own_overloads: Vec::new(),
            own_type_names: Vec::new(),
            imported: HashMap::new(),
            qualified: HashMap::new(),
            module_name: "",
        };
        c.install_builtins();
        c
    }

    /// Check a whole program, returning the inferred (generalized) type of every
    /// global definition, in source order.
    ///
    /// Globals are grouped into strongly-connected components by their reference
    /// graph (see [`crate::scc`]) and checked in dependency order: each component
    /// is generalized before the components that use it (cross-global
    /// let-polymorphism), while mutual recursion inside a component is kept
    /// monomorphic. Self-recursion is the singleton-component case.
    pub fn check_program(&mut self, program: &Program<'a>) -> Result<Vec<(&'a str, Type)>> {
        self.module_name = program.module;
        self.finalize_imports();
        self.register_types(program);

        let defs: Vec<Def<'a>> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Def { name, sig, body } => Some(Def {
                    name,
                    sig: *sig,
                    body,
                }),
                // Struct/union/alias/effect/import/directive items are registered
                // by later increments; they do not yet contribute to a value.
                _ => None,
            })
            .collect();

        // A name is overloaded if it is defined more than once here, or if it
        // adds to an overload already imported (e.g. `filter` from LIST/MAP/STR).
        // The rest are ordinary single definitions checked by SCC.
        let mut counts: HashMap<&'a str, usize> = HashMap::new();
        for d in &defs {
            *counts.entry(d.name).or_insert(0) += 1;
        }
        let mut overloaded_names: HashSet<&'a str> = HashSet::new();
        for d in &defs {
            if counts[d.name] > 1 || self.overloads.contains_key(d.name) {
                overloaded_names.insert(d.name);
            }
        }
        let is_overloaded = |name: &str| overloaded_names.contains(name);

        // Seed each overloaded name's candidates from its declared signatures, so
        // uses (including recursion) resolve before the bodies are checked.
        for d in &defs {
            if is_overloaded(d.name) {
                if let Some(sig) = d.sig {
                    let scheme = self.scheme_of_sig(sig);
                    self.overloads.entry(d.name).or_default().push(scheme);
                }
            }
        }

        let singles: Vec<Def<'a>> = defs
            .iter()
            .filter(|d| !is_overloaded(d.name))
            .cloned()
            .collect();
        let single_index: HashMap<&'a str, usize> = singles
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name, i))
            .collect();
        let graph = dependency_graph(&singles, &single_index);

        let mut types: HashMap<&'a str, Type> = HashMap::new();
        for component in crate::scc::scc(&graph) {
            self.check_component(&component, &singles, &mut types)?;
        }

        // Check the overloaded bodies against their signatures (their candidates
        // are already seeded, and every single definition is now in scope).
        let mut overloaded_out = Vec::new();
        for d in &defs {
            if is_overloaded(d.name) {
                let ty = self.check_overloaded_def(d)?;
                overloaded_out.push((d.name, ty));
            }
        }

        let mut out: Vec<(&'a str, Type)> = defs
            .iter()
            .filter(|d| !is_overloaded(d.name))
            .map(|d| (d.name, types[d.name].clone()))
            .collect();

        // Record this module's own exports for any importer: single value schemes,
        // and (grouped) only the overload candidates defined HERE, not the ones
        // this module itself imported.
        self.own_values = out.clone();
        let mut own_ov: HashMap<&'a str, Vec<Type>> = HashMap::new();
        for (name, ty) in &overloaded_out {
            own_ov.entry(name).or_default().push(ty.clone());
        }
        self.own_overloads = own_ov.into_iter().collect();

        out.extend(overloaded_out);
        Ok(out)
    }

    /// Import another module's public exports into this checker: its declared
    /// types (so their constructors/patterns resolve), its overloaded-name
    /// candidates, and its single value schemes. Names come in unqualified, which
    /// is what `$ with MOD` grants. Value/overload schemes are re-numbered into
    /// this checker's engine by [`Checker::import_scheme`].
    pub fn import_from(&mut self, other: &Checker<'a>) {
        for &name in &other.own_type_names {
            if let Some(s) = other.structs.get(name) {
                self.structs.insert(name, s.clone());
            }
            if let Some(u) = other.unions.get(name) {
                self.unions.insert(name, u.clone());
            }
            if let Some(a) = other.aliases.get(name) {
                self.aliases.insert(name, a);
            }
        }
        let module = other.module_name;
        for (name, cands) in &other.own_overloads {
            let mut qualified = Vec::with_capacity(cands.len());
            for c in cands {
                let unqualified = self.import_scheme(c);
                self.imported.entry(name).or_default().push(unqualified);
                qualified.push(self.import_scheme(c));
            }
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, qualified);
        }
        for (name, scheme) in &other.own_values {
            let unqualified = self.import_scheme(scheme);
            self.imported.entry(name).or_default().push(unqualified);
            let qualified = self.import_scheme(scheme);
            self.qualified
                .entry(module)
                .or_default()
                .insert(name, vec![qualified]);
        }
    }

    /// Turn the accumulated imports into bindings: a name with one candidate is a
    /// plain binding; several candidates (from a module's own overload or from
    /// two modules defining the same name) form an overload. A name that already
    /// overloads a built-in (e.g. arithmetic) gains the imported candidates.
    fn finalize_imports(&mut self) {
        let imported = std::mem::take(&mut self.imported);
        for (name, mut cands) in imported {
            if let Some(existing) = self.overloads.get_mut(name) {
                existing.extend(cands);
            } else if cands.len() == 1 {
                self.bind(name, cands.pop().expect("one candidate"));
            } else {
                self.overloads.insert(name, cands);
            }
        }
    }

    /// Copy a generalized scheme from another checker's engine into this one,
    /// giving each of its (fully generalized, so quantified) variables a fresh
    /// `Generic` variable here, consistently within the scheme.
    fn import_scheme(&mut self, ty: &Type) -> Type {
        let mut map = HashMap::new();
        self.import_ty(ty, &mut map)
    }

    fn import_ty(&mut self, ty: &Type, map: &mut HashMap<VarId, Type>) -> Type {
        match ty {
            Type::Var(id) => map
                .entry(*id)
                .or_insert_with(|| self.eng.fresh_generic())
                .clone(),
            Type::Con(name) => Type::Con(name.clone()),
            Type::App(head, arg) => Type::app(self.import_ty(head, map), self.import_ty(arg, map)),
            Type::Arrow(from, to) => {
                Type::arrow(self.import_ty(from, map), self.import_ty(to, map))
            }
            Type::Tuple(items) => {
                Type::Tuple(items.iter().map(|t| self.import_ty(t, map)).collect())
            }
        }
    }

    /// Check one overloaded definition's body against its signature. The name is
    /// not bound in scope (its uses go through the overload set); recursion
    /// resolves against the already-seeded candidates.
    fn check_overloaded_def(&mut self, def: &Def<'a>) -> Result<Type> {
        self.eng.enter_level();
        let result = if def.sig.is_some() {
            let fresh = self.eng.fresh();
            self.check_def_body(def, &fresh)?;
            fresh
        } else {
            // No signature: its checked body becomes an additional candidate.
            let inferred = self.infer(def.body)?;
            self.overloads
                .entry(def.name)
                .or_default()
                .push(inferred.clone());
            inferred
        };
        self.solve_pending()?;
        self.eng.leave_level();
        let mono = self.pending_vars();
        self.eng.generalize_except(&result, &mono);
        Ok(self.eng.zonk(&result))
    }

    /// Convert a signature into a generalized scheme (its type variables become
    /// quantified), suitable as an overload candidate.
    fn scheme_of_sig(&mut self, sig: &Ty<'a>) -> Type {
        self.eng.enter_level();
        let mut tvars = HashMap::new();
        let ty = self.ty_of_ast(sig, &mut tvars);
        self.eng.leave_level();
        self.eng.generalize(&ty);
        self.eng.zonk(&ty)
    }

    /// Check one strongly-connected component: bind every member monomorphically
    /// at a bumped level so their mutual references resolve, infer and unify each
    /// body (and signature), then generalize the whole component together.
    fn check_component(
        &mut self,
        component: &[usize],
        defs: &[Def<'a>],
        types: &mut HashMap<&'a str, Type>,
    ) -> Result<()> {
        self.eng.enter_level();
        let mut declared = Vec::with_capacity(component.len());
        for &i in component {
            let v = self.eng.fresh();
            self.bind(defs[i].name, v.clone());
            declared.push(v);
        }
        for (&i, decl) in component.iter().zip(&declared) {
            let def = &defs[i];
            self.check_def_body(def, decl)?;
        }
        self.solve_pending()?;
        self.eng.leave_level();
        let mono = self.pending_vars();
        for (&i, decl) in component.iter().zip(&declared) {
            self.eng.generalize_except(decl, &mono);
            types.insert(defs[i].name, self.eng.zonk(decl));
        }
        Ok(())
    }

    /// Check one definition's body against `decl`. When the definition has a
    /// signature, that type is unified into `decl` first and pushed into the body
    /// (bidirectional checking), so lambda parameters get their declared types
    /// before the body is inferred and field access / overloads resolve.
    fn check_def_body(&mut self, def: &Def<'a>, decl: &Type) -> Result<()> {
        if let Some(sig) = def.sig {
            let mut tvars = HashMap::new();
            let sig_ty = self.ty_of_ast(sig, &mut tvars);
            self.eng.unify(
                decl,
                &sig_ty,
                &format!("against the signature of `{}`", def.name),
            )?;
            self.check_body_against_sig(def.body, *sig, &sig_ty)
        } else {
            let inferred = self.infer(def.body)?;
            self.eng.unify(
                decl,
                &inferred,
                &format!("in the definition of `{}`", def.name),
            )
        }
    }

    /// Check a body against its signature, implicitly consuming leading record
    /// parameters. A record-typed parameter (`{x: Int, y: Int}`) is not written
    /// as a lambda: its fields are bound directly in scope (a `with` field also
    /// scopes its struct's fields), and checking continues against the rest of
    /// the signature. Once no record parameter leads, the body is checked
    /// normally (bidirectional, so plain lambda parameters still get their types).
    fn check_body_against_sig(
        &mut self,
        body: &Expr<'a>,
        sig: Ty<'a>,
        sig_ty: &Type,
    ) -> Result<()> {
        let (fields, to) = match sig {
            Ty::Arrow { from, to, .. } if matches!(*from, Ty::Record(_)) => {
                let Ty::Record(fields) = *from else {
                    unreachable!("guarded on a record parameter")
                };
                (fields, to)
            }
            _ => return self.check(body, sig_ty),
        };
        let (param_ty, result_ty) = self.arrow_parts(sig_ty)?;
        self.enter_scope();
        self.bind_record_param(fields, &param_ty)?;
        let out = self.check_body_against_sig(body, *to, &result_ty);
        self.leave_scope();
        out
    }

    /// Bind a record parameter's fields into scope. A one-field record collapses
    /// to a bare parameter (its type is `param_ty` directly); otherwise `param_ty`
    /// is the tuple of field types. A `with` field additionally scopes its
    /// struct's own fields in.
    fn bind_record_param(&mut self, fields: &[RecField<'a>], param_ty: &Type) -> Result<()> {
        if fields.len() == 1 {
            let f = &fields[0];
            self.bind(f.name, param_ty.clone());
            if f.with {
                self.scope_struct_fields(param_ty)?;
            }
            return Ok(());
        }
        let comps = match self.eng.resolve(param_ty) {
            Type::Tuple(v) => v,
            _ => {
                let vs: Vec<Type> = fields.iter().map(|_| self.eng.fresh()).collect();
                self.eng
                    .unify(param_ty, &Type::Tuple(vs.clone()), "in a record parameter")?;
                vs
            }
        };
        for (f, comp) in fields.iter().zip(&comps) {
            self.bind(f.name, comp.clone());
            if f.with {
                self.scope_struct_fields(comp)?;
            }
        }
        Ok(())
    }

    /// Bind every field of a struct type into the current scope (unqualified),
    /// as `with p in ..` and `with`-record parameters do. A non-struct type
    /// scopes nothing.
    fn scope_struct_fields(&mut self, ty: &Type) -> Result<()> {
        let (head, args) = self.spine(ty);
        if let Type::Con(name) = &head {
            if let Some(info) = self.structs.get(name.as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                for (fname, fty) in &info.fields {
                    let field_ty = self.ty_of_ast(fty, &mut subst);
                    self.bind(fname, field_ty);
                }
            }
        }
        Ok(())
    }

    /// Check an expression against an expected type (the checking direction of
    /// bidirectional type checking). A lambda decomposes the expected arrow and
    /// binds each parameter to its expected type before checking the body;
    /// everything else falls back to inference followed by unification.
    fn check(&mut self, e: &Expr<'a>, expected: &Type) -> Result<()> {
        match e {
            Expr::Lambda { params, body } => {
                self.enter_scope();
                let mut exp = expected.clone();
                for p in *params {
                    let (param_ty, rest) = self.arrow_parts(&exp)?;
                    self.type_pattern(p, &param_ty)?;
                    exp = rest;
                }
                let out = self.check(body, &exp);
                self.leave_scope();
                out
            }
            // `[..]` is type-directed: in an Array context it is a byte vector
            // (elements are Int), otherwise it infers to a List.
            Expr::List(items) if self.is_array(expected) => {
                for item in *items {
                    let t = self.infer(item)?;
                    self.eng
                        .unify(&t, &Type::con(ty::INT), "in an array element")?;
                }
                Ok(())
            }
            _ => {
                let got = self.infer(e)?;
                self.eng.unify(&got, expected, "against the expected type")
            }
        }
    }

    /// Whether an expected type has resolved to the built-in `Array`.
    fn is_array(&self, ty: &Type) -> bool {
        matches!(self.eng.resolve(ty), Type::Con(name) if name == ty::ARRAY)
    }

    /// Split an expected type into `(parameter, result)`, forcing it to a
    /// function type (via a fresh arrow) if it is not already one.
    fn arrow_parts(&mut self, ty: &Type) -> Result<(Type, Type)> {
        match self.eng.resolve(ty) {
            Type::Arrow(from, to) => Ok((*from, *to)),
            other => {
                let from = self.eng.fresh();
                let to = self.eng.fresh();
                self.eng.unify(
                    &other,
                    &Type::arrow(from.clone(), to.clone()),
                    "expected a function",
                )?;
                Ok((from, to))
            }
        }
    }

    /// The variables currently constrained by unresolved overloads, which the
    /// monomorphism restriction must keep ungeneralized.
    fn pending_vars(&self) -> HashSet<VarId> {
        let mut out = HashSet::new();
        for p in &self.pending {
            for a in &p.args {
                self.eng.collect_vars(a, &mut out);
            }
            self.eng.collect_vars(&p.result, &mut out);
        }
        // Undefaulted integer literals must also stay monomorphic until the
        // definition boundary defaults them.
        for t in &self.numeric {
            self.eng.collect_vars(t, &mut out);
        }
        out
    }

    // -- type declarations --------------------------------------------------

    /// Record every struct and union declaration before checking any value, so
    /// their constructors, fields, and variants are available regardless of
    /// source order. Type parameters are the type variables appearing in the
    /// fields/payloads, ordered by first appearance.
    fn register_types(&mut self, program: &Program<'a>) {
        for item in program.items {
            match item {
                Item::Struct { name, fields } => {
                    let mut params = Vec::new();
                    for f in *fields {
                        collect_tyvars(f.ty, &mut params);
                    }
                    let fields = fields.iter().map(|f| (f.name, f.ty)).collect();
                    self.structs.insert(name, StructInfo { params, fields });
                    self.own_type_names.push(name);
                }
                Item::Union { name, variants } => {
                    let mut params = Vec::new();
                    let mut vs = Vec::with_capacity(variants.len());
                    for v in *variants {
                        let payload = payload_fields(&v.payload);
                        for (_, ty) in &payload {
                            collect_tyvars(ty, &mut params);
                        }
                        vs.push(VariantSig {
                            tag: v.tag,
                            payload,
                        });
                    }
                    self.unions.insert(
                        name,
                        UnionInfo {
                            params,
                            variants: vs,
                        },
                    );
                    self.own_type_names.push(name);
                }
                Item::Alias { name, ty } => {
                    self.aliases.insert(name, ty);
                    self.own_type_names.push(name);
                }
                _ => {}
            }
        }
    }

    // -- struct / union typing ----------------------------------------------

    /// Type a field access `record.field` given the record's inferred type. A
    /// known struct type yields the field's type at that instantiation; a tuple
    /// type with a numeric field yields that element; anything else is lenient.
    fn infer_field(&mut self, rec_ty: &Type, field: &str) -> Result<Type> {
        let (head, args) = self.spine(rec_ty);
        if let Type::Con(name) = &head {
            if let Some(info) = self.structs.get(name.as_str()).cloned() {
                let mut subst = subst_from_args(&info.params, &args, &mut self.eng);
                if let Some((_, ty)) = info.fields.iter().find(|(n, _)| *n == field) {
                    return Ok(self.ty_of_ast(ty, &mut subst));
                }
            }
        }
        if let Type::Tuple(items) = &head {
            if let Ok(idx) = field.parse::<usize>() {
                if let Some(t) = items.get(idx) {
                    return Ok(t.clone());
                }
            }
        }
        Ok(self.eng.fresh())
    }

    fn infer_struct_lit(
        &mut self,
        ty: Option<&'a str>,
        fields: &[FieldInit<'a>],
        spread: Option<&Expr<'a>>,
    ) -> Result<Type> {
        // Resolve which struct, its result type, and the param substitution.
        let (info, result, mut subst) = if let Some(base) = spread {
            let base_ty = self.infer(base)?;
            let (head, args) = self.spine(&base_ty);
            match &head {
                Type::Con(n) if self.structs.contains_key(n.as_str()) => {
                    let info = self.structs[n.as_str()].clone();
                    let subst = subst_from_args(&info.params, &args, &mut self.eng);
                    (info, base_ty, subst)
                }
                _ => {
                    self.infer_field_inits(fields)?;
                    return Ok(base_ty);
                }
            }
        } else {
            let resolved = match ty {
                Some(n) => self.structs.get(n).cloned().map(|info| (n, info)),
                None => self.resolve_struct_by_fields(fields),
            };
            match resolved {
                Some((name, info)) => {
                    let (args, subst) = self.instantiate_params(&info.params);
                    (info, applied(name, &args), subst)
                }
                None => {
                    self.infer_field_inits(fields)?;
                    return Ok(self.eng.fresh());
                }
            }
        };

        for (i, fi) in fields.iter().enumerate() {
            let (decl_ty, value) = match fi {
                FieldInit::Named { name, value } => {
                    match info.fields.iter().find(|(n, _)| n == name) {
                        Some((_, t)) => (*t, value),
                        None => {
                            self.infer(value)?;
                            continue;
                        }
                    }
                }
                FieldInit::Positional(value) => match info.fields.get(i) {
                    Some((_, t)) => (*t, value),
                    None => {
                        self.infer(value)?;
                        continue;
                    }
                },
            };
            let want = self.ty_of_ast(decl_ty, &mut subst);
            let got = self.infer(value)?;
            self.eng.unify(&got, &want, "in a struct field")?;
        }
        Ok(result)
    }

    fn infer_variant(
        &mut self,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &[FieldInit<'a>],
    ) -> Result<Type> {
        let resolved = match ty {
            Some(n) => self.variant_sig(n, tag),
            None => self
                .find_union_by_tag(tag)
                .and_then(|u| self.variant_sig(u, tag)),
        };
        let (result, payload) = match resolved {
            Some(r) => r,
            None => {
                self.infer_field_inits(fields)?;
                return Ok(self.eng.fresh());
            }
        };
        for (i, fi) in fields.iter().enumerate() {
            let (want, value) = match fi {
                FieldInit::Named { name, value } => {
                    (variant_field_ty(&payload, Some(name), i), value)
                }
                FieldInit::Positional(value) => (variant_field_ty(&payload, None, i), value),
            };
            let got = self.infer(value)?;
            if let Some(want) = want {
                self.eng.unify(&got, &want, "in a variant payload")?;
            }
        }
        Ok(result)
    }

    /// The result type and payload of a variant, instantiated at fresh type
    /// arguments. Payload entries carry their optional field name for matching
    /// named initializers/patterns. Handles the prelude `List` specially.
    fn variant_sig(&mut self, union: &str, tag: &str) -> Option<(Type, VariantPayload<'a>)> {
        if union == ty::LIST {
            let elem = self.eng.fresh();
            let list = Type::app(Type::con(ty::LIST), elem.clone());
            return match tag {
                "Nil" => Some((list, vec![])),
                "Cons" => Some((list.clone(), vec![(None, elem), (None, list)])),
                _ => None,
            };
        }
        let info = self.unions.get(union)?.clone();
        let pos = info.variants.iter().position(|v| v.tag == tag)?;
        let (args, mut subst) = self.instantiate_params(&info.params);
        let result = applied(union, &args);
        let variant = &info.variants[pos];
        let payload = variant
            .payload
            .clone()
            .into_iter()
            .map(|(name, ast_ty)| (name, self.ty_of_ast(ast_ty, &mut subst)))
            .collect();
        Some((result, payload))
    }

    /// Resolve a bare `.Tag` to the union that declares it. A user union wins
    /// over the prelude `List`, whose `Nil`/`Cons` are the fallback for list
    /// sugar. (In general a bare `.Tag` is resolved from the expected type; that
    /// awaits bidirectional checking.)
    fn find_union_by_tag(&self, tag: &str) -> Option<&'a str> {
        let user = self
            .unions
            .iter()
            .find_map(|(name, info)| info.variants.iter().any(|v| v.tag == tag).then_some(*name));
        user.or(match tag {
            "Nil" | "Cons" => Some(ty::LIST),
            _ => None,
        })
    }

    /// Find a struct whose field-name set exactly matches an all-named literal.
    fn resolve_struct_by_fields(
        &self,
        fields: &[FieldInit<'a>],
    ) -> Option<(&'a str, StructInfo<'a>)> {
        let mut names = Vec::with_capacity(fields.len());
        for f in fields {
            match f {
                FieldInit::Named { name, .. } => names.push(*name),
                // A positional field gives no name to match on.
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

    fn struct_field_ty(
        &mut self,
        info: &StructInfo<'a>,
        subst: &mut HashMap<&'a str, Type>,
        name: Option<&str>,
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

    /// Fresh type arguments for a type's parameters, plus the substitution that
    /// maps each parameter name to its argument.
    fn instantiate_params(&mut self, params: &[&'a str]) -> (Vec<Type>, HashMap<&'a str, Type>) {
        let mut subst = HashMap::new();
        let mut args = Vec::with_capacity(params.len());
        for p in params {
            let v = self.eng.fresh();
            subst.insert(*p, v.clone());
            args.push(v);
        }
        (args, subst)
    }

    /// Peel a type application `Head Arg1 Arg2 ...` into its head and arguments.
    fn spine(&self, ty: &Type) -> (Type, Vec<Type>) {
        let mut args = Vec::new();
        let mut cur = self.eng.resolve(ty);
        while let Type::App(head, arg) = cur {
            args.push(self.eng.resolve(&arg));
            cur = self.eng.resolve(&head);
        }
        args.reverse();
        (cur, args)
    }

    // -- environment --------------------------------------------------------

    /// Render a type with named variables (delegates to the engine).
    pub fn show(&self, ty: &Type) -> String {
        self.eng.show(ty)
    }

    fn bind(&mut self, name: &'a str, ty: Type) {
        self.scopes
            .last_mut()
            .expect("a scope is always open")
            .insert(name, ty);
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn leave_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty(), "popped the global scope");
    }

    // -- expression inference ----------------------------------------------

    pub fn infer(&mut self, e: &Expr<'a>) -> Result<Type> {
        match e {
            Expr::Int(_) => {
                // An integer literal is numeric (Int or Real); its type is a
                // fresh variable, resolved by context or defaulted to Int later.
                let t = self.eng.fresh();
                self.numeric.push(t.clone());
                Ok(t)
            }
            Expr::Real(_) => Ok(Type::con(ty::REAL)),
            Expr::Str(_) => Ok(Type::con(ty::STR)),
            Expr::Bool(_) => Ok(Type::con(ty::BOOL)),
            Expr::Unit => Ok(Type::con(ty::UNIT)),

            Expr::Var { module, name } => self.infer_var(*module, name),

            Expr::App(..) => self.infer_app(e),

            Expr::BinOp { op, lhs, rhs } => {
                let tl = self.infer(lhs)?;
                let tr = self.infer(rhs)?;
                if let Some(cands) = self.overloads.get(op).cloned() {
                    return self.resolve_overload(op, &cands, &[tl, tr]);
                }
                let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                let op_ty = self.eng.instantiate(&scheme);
                let result = self.eng.fresh();
                let want = Type::arrow(tl, Type::arrow(tr, result.clone()));
                self.eng
                    .unify(&op_ty, &want, &format!("in operator `{op}`"))?;
                Ok(result)
            }

            Expr::UnOp { op, operand } => {
                let t = self.infer(operand)?;
                if let Some(cands) = self.overloads.get(op).cloned() {
                    return self.resolve_overload(op, &cands, &[t]);
                }
                let scheme = self.lookup(op).ok_or_else(|| unbound(op))?;
                let op_ty = self.eng.instantiate(&scheme);
                let result = self.eng.fresh();
                self.eng.unify(
                    &op_ty,
                    &Type::arrow(t, result.clone()),
                    &format!("in unary `{op}`"),
                )?;
                Ok(result)
            }

            Expr::Tuple(items) => {
                let mut tys = Vec::with_capacity(items.len());
                for item in *items {
                    tys.push(self.infer(item)?);
                }
                Ok(Type::Tuple(tys))
            }

            Expr::List(items) => {
                let elem = self.eng.fresh();
                for item in *items {
                    let t = self.infer(item)?;
                    self.eng.unify(&elem, &t, "in a list literal")?;
                }
                Ok(Type::app(Type::con(ty::LIST), elem))
            }

            Expr::If { cond, then, alt } => {
                let tc = self.infer(cond)?;
                self.eng
                    .unify(&tc, &Type::con(ty::BOOL), "in an 'if' condition")?;
                let tt = self.infer(then)?;
                let ta = self.infer(alt)?;
                self.eng
                    .unify(&tt, &ta, "between the branches of an 'if'")?;
                Ok(tt)
            }

            Expr::Let { bindings, body } => {
                self.enter_scope();
                self.infer_let_group(bindings)?;
                let t = self.infer(body)?;
                self.leave_scope();
                Ok(t)
            }

            Expr::Lambda { params, body } => {
                self.enter_scope();
                let mut param_tys = Vec::with_capacity(params.len());
                for p in *params {
                    let pv = self.eng.fresh();
                    self.type_pattern(p, &pv)?;
                    param_tys.push(pv);
                }
                let body_ty = self.infer(body)?;
                self.leave_scope();
                Ok(Type::arrows(param_tys.into_iter(), body_ty))
            }

            Expr::Match {
                scrut,
                arms,
                default,
            } => {
                let ts = self.infer(scrut)?;
                let result = self.eng.fresh();
                for arm in *arms {
                    self.enter_scope();
                    for pat in arm.patterns {
                        self.type_pattern(pat, &ts)?;
                    }
                    if let Some(guard) = arm.guard {
                        let tg = self.infer(guard)?;
                        self.eng
                            .unify(&tg, &Type::con(ty::BOOL), "in a match guard")?;
                    }
                    let tb = self.infer(arm.body)?;
                    self.eng.unify(&result, &tb, "between match arms")?;
                    self.leave_scope();
                }
                if let Some(d) = default {
                    let td = self.infer(d)?;
                    self.eng.unify(&result, &td, "in a match 'else' branch")?;
                }
                Ok(result)
            }

            Expr::Field { record, name } => {
                let rec_ty = self.infer(record)?;
                self.infer_field(&rec_ty, name)
            }
            Expr::StructLit { ty, fields, spread } => self.infer_struct_lit(*ty, fields, *spread),
            Expr::Variant {
                ty, tag, fields, ..
            } => self.infer_variant(*ty, tag, fields),

            // Forms whose full typing needs the effect tables. Infer children (so
            // errors inside still fire), then hand back a fresh variable.
            Expr::Array { size } => {
                let ts = self.infer(size)?;
                self.eng
                    .unify(&ts, &Type::con(ty::INT), "in an array size")?;
                Ok(Type::con(ty::ARRAY))
            }
            Expr::With { subject, body } => {
                let subject_ty = self.infer(subject)?;
                self.enter_scope();
                self.scope_struct_fields(&subject_ty)?;
                let t = self.infer(body);
                self.leave_scope();
                t
            }
            Expr::Handle { body, .. } => {
                self.infer(body)?;
                Ok(self.eng.fresh())
            }
            Expr::Defer { cleanup, body } => {
                self.infer(cleanup)?;
                self.infer(body)
            }
            Expr::Extern { .. } => Ok(self.eng.fresh()),
        }
    }

    fn infer_field_inits(&mut self, fields: &[FieldInit<'a>]) -> Result<()> {
        for f in fields {
            match f {
                FieldInit::Named { value, .. } => {
                    self.infer(value)?;
                }
                FieldInit::Positional(v) => {
                    self.infer(v)?;
                }
            }
        }
        Ok(())
    }

    // -- overloading --------------------------------------------------------

    /// Infer a variable reference. A qualified `MOD.name` resolves against the
    /// imported module's exports (a single scheme is instantiated; several are a
    /// deferred overload); a qualified name from an unmodelled namespace (e.g.
    /// `C.sqrt`) is lenient. An unqualified name is a local/imported binding, a
    /// deferred bare overload, or unbound.
    fn infer_var(&mut self, module: Option<&'a str>, name: &'a str) -> Result<Type> {
        if let Some(m) = module {
            return match self.qualified_candidates(m, name) {
                Some(cands) if cands.len() == 1 => Ok(self.eng.instantiate(&cands[0])),
                // A qualified overload, or an external namespace: no argument
                // context here, so hand back a fresh variable.
                _ => Ok(self.eng.fresh()),
            };
        }
        if let Some(scheme) = self.lookup(name) {
            Ok(self.eng.instantiate(&scheme))
        } else if self.overloads.contains_key(name) {
            Ok(self.eng.fresh())
        } else {
            Err(unbound(name))
        }
    }

    fn qualified_candidates(&self, module: &str, name: &str) -> Option<Vec<Type>> {
        self.qualified.get(module)?.get(name).cloned()
    }

    /// Infer an application, resolving an overloaded head against its arguments.
    /// The whole application spine `head a1 a2 ...` is collected first, so an
    /// overloaded callee sees every argument at once (overloading is chosen by
    /// the full argument tuple, not one curried step at a time).
    fn infer_app(&mut self, e: &Expr<'a>) -> Result<Type> {
        let mut args_rev = Vec::new();
        let mut head = e;
        while let Expr::App(f, x) = head {
            args_rev.push(*x);
            head = f;
        }
        args_rev.reverse();
        let args = args_rev;

        // An overloaded head (unqualified, or qualified with several candidates)
        // is resolved against all arguments at once.
        if let Expr::Var { module, name } = head {
            let cands = match module {
                Some(m) => self.qualified_candidates(m, name).filter(|c| c.len() > 1),
                None => self.overloads.get(*name).cloned(),
            };
            if let Some(cands) = cands {
                let arg_tys = args
                    .iter()
                    .map(|a| self.infer(a))
                    .collect::<Result<Vec<_>>>()?;
                return self.resolve_overload(name, &cands, &arg_tys);
            }
        }

        // Apply arguments one at a time, checking each against the parameter
        // type (bidirectional application) so a type-directed argument such as an
        // Array-context `[..]` literal sees its expected type.
        let mut tf = self.infer(head)?;
        for a in &args {
            let (param, result) = self.arrow_parts(&tf)?;
            self.check(a, &param)?;
            tf = result;
        }
        Ok(tf)
    }

    /// Resolve an overload use. A unique match is applied immediately; no match
    /// is an error; an ambiguous match (the argument types are not concrete
    /// enough yet) is deferred to [`Checker::solve_pending`], which retries once
    /// the surrounding definition has pinned those types down.
    fn resolve_overload(&mut self, name: &str, candidates: &[Type], args: &[Type]) -> Result<Type> {
        let result = self.eng.fresh();
        match self.match_overload(candidates, args, &result) {
            Match::Unique(idx) => {
                let cand = candidates[idx].clone();
                self.apply_overload(&cand, args, &result)?;
                Ok(result)
            }
            Match::None => Err(self.no_overload(name, args)),
            Match::Ambiguous => {
                self.pending.push(Pending {
                    name: name.to_string(),
                    candidates: candidates.to_vec(),
                    args: args.to_vec(),
                    result: result.clone(),
                });
                Ok(result)
            }
        }
    }

    /// Trial each candidate against the argument types AND the expected result on
    /// a saved engine state, rolling back so no trial leaks bindings. Including
    /// the result lets a known return type (e.g. `... + 4.2` is Real) pin the
    /// choice. The match must be unique (no most-specific-wins tie-breaking).
    fn match_overload(&mut self, candidates: &[Type], args: &[Type], result: &Type) -> Match {
        let mut matched = None;
        let mut count = 0;
        for (idx, cand) in candidates.iter().enumerate() {
            let save = self.eng.save();
            let ok = self.apply_overload(cand, args, result).is_ok();
            self.eng.restore(save);
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

    /// Solve deferred overloads and numeric literals to a fixpoint: re-try each
    /// pending overload now that more types are known, applying any that became
    /// unique. When overloads stall, default the still-unresolved integer
    /// literals to Int (which typically unblocks them) and continue. An overload
    /// that becomes unsatisfiable is an error; one that stays ambiguous after
    /// defaulting is genuinely ambiguous.
    fn solve_pending(&mut self) -> Result<()> {
        loop {
            let batch = std::mem::take(&mut self.pending);
            let mut progress = false;
            let mut still = Vec::new();
            for p in batch {
                match self.match_overload(&p.candidates, &p.args, &p.result) {
                    Match::Unique(idx) => {
                        let cand = p.candidates[idx].clone();
                        self.apply_overload(&cand, &p.args, &p.result)?;
                        progress = true;
                    }
                    Match::None => return Err(self.no_overload(&p.name, &p.args)),
                    Match::Ambiguous => still.push(p),
                }
            }
            self.pending = still;
            if progress {
                continue;
            }
            // Overloads stalled: default leftover integer literals to Int, which
            // usually makes the stalled uses concrete on the next pass.
            if self.default_numerics()? {
                continue;
            }
            if let Some(p) = self.pending.first() {
                return Err(Diagnostic::error(
                    Code::AmbiguousName,
                    Span::at(0),
                    0,
                    format!("ambiguous overloaded use of `{}`", p.name),
                ));
            }
            return Ok(());
        }
    }

    /// Default every still-unbound integer-literal variable to Int, reporting
    /// whether anything changed. A literal already pinned (e.g. to Real by
    /// context) is left as is.
    fn default_numerics(&mut self) -> Result<bool> {
        let vars = std::mem::take(&mut self.numeric);
        let mut changed = false;
        for t in &vars {
            if let Type::Var(_) = self.eng.resolve(t) {
                self.eng
                    .unify(t, &Type::con(ty::INT), "defaulting an integer literal")?;
                changed = true;
            }
        }
        Ok(changed)
    }

    fn no_overload(&self, name: &str, args: &[Type]) -> Diagnostic {
        let shown: Vec<String> = args.iter().map(|a| self.show(a)).collect();
        Diagnostic::error(
            Code::TypeMismatch,
            Span::at(0),
            0,
            format!(
                "no overload of `{name}` matches argument types ({})",
                shown.join(", ")
            ),
        )
    }

    /// Instantiate one candidate, apply it to the argument types in order, and
    /// unify its return type with `result`. Fails if the candidate does not
    /// accept the arguments or produce the expected result.
    fn apply_overload(&mut self, candidate: &Type, args: &[Type], result: &Type) -> Result<()> {
        let mut f = self.eng.instantiate(candidate);
        for a in args {
            let next = self.eng.fresh();
            self.eng.unify(
                &f,
                &Type::arrow(a.clone(), next.clone()),
                "in an overloaded application",
            )?;
            f = next;
        }
        self.eng.unify(&f, result, "in an overloaded application")
    }

    /// A comma-chained `let`. The chain is sequential nesting: `let x = a, y = b
    /// in e` is exactly `let x = a in let y = b in e`, so each binding is in
    /// scope for the ones after it (but not before). A single binding is
    /// self-recursive: a simple (`Var`) name is pre-declared monomorphically so
    /// its own value can refer to it, then generalized and bound.
    fn infer_let_group(&mut self, bindings: &[Binding<'a>]) -> Result<()> {
        for b in bindings {
            self.infer_binding(b)?;
        }
        Ok(())
    }

    /// One `let` binding: infer the value at a bumped level (with the name
    /// pre-declared for self-recursion), unify the optional signature,
    /// generalize, then bind the pattern's variables into the current scope.
    fn infer_binding(&mut self, b: &Binding<'a>) -> Result<()> {
        self.eng.enter_level();
        let declared = match b.pat {
            Pattern::Var(name) => {
                let v = self.eng.fresh();
                self.bind(name, v.clone());
                Some(v)
            }
            _ => None,
        };
        let value_ty = self.infer(b.value)?;
        if let Some(sig) = b.sig {
            let mut tvars = HashMap::new();
            let sig_ty = self.ty_of_ast(sig, &mut tvars);
            self.eng
                .unify(&value_ty, &sig_ty, "against a 'let' signature")?;
        }
        if let Some(decl) = &declared {
            self.eng
                .unify(decl, &value_ty, "in a recursive 'let' binding")?;
        }
        self.eng.leave_level();
        // Monomorphism restriction: an operand still awaiting overload resolution
        // stays a unification variable so a later use can pin it (see
        // `solve_pending`), rather than being generalized here.
        let mono = self.pending_vars();
        match declared {
            // Already bound to the (now generalizable) placeholder.
            Some(decl) => self.eng.generalize_except(&decl, &mono),
            None => {
                self.eng.generalize_except(&value_ty, &mono);
                self.type_pattern(b.pat, &value_ty)?;
            }
        }
        Ok(())
    }

    // -- pattern typing -----------------------------------------------------

    /// Type `pat` against `expected`, binding its variables into the current
    /// scope.
    pub fn type_pattern(&mut self, pat: &Pattern<'a>, expected: &Type) -> Result<()> {
        match pat {
            Pattern::Wild => Ok(()),
            Pattern::Var(name) => {
                self.bind(name, expected.clone());
                Ok(())
            }
            Pattern::Int(_) => {
                self.eng
                    .unify(expected, &Type::con(ty::INT), "in an integer pattern")
            }
            Pattern::Real(_) => self
                .eng
                .unify(expected, &Type::con(ty::REAL), "in a real pattern"),
            Pattern::Str(_) => self
                .eng
                .unify(expected, &Type::con(ty::STR), "in a string pattern"),
            Pattern::Bool(_) => {
                self.eng
                    .unify(expected, &Type::con(ty::BOOL), "in a boolean pattern")
            }
            Pattern::StrPrefix { rest, .. } => {
                self.eng
                    .unify(expected, &Type::con(ty::STR), "in a string-prefix pattern")?;
                self.type_pattern(rest, &Type::con(ty::STR))
            }
            Pattern::Tuple(pats) => {
                let mut vars = Vec::with_capacity(pats.len());
                for _ in *pats {
                    vars.push(self.eng.fresh());
                }
                self.eng
                    .unify(expected, &Type::Tuple(vars.clone()), "in a tuple pattern")?;
                for (p, v) in pats.iter().zip(&vars) {
                    self.type_pattern(p, v)?;
                }
                Ok(())
            }
            Pattern::Cons { head, tail } => {
                let elem = self.eng.fresh();
                let list = Type::app(Type::con(ty::LIST), elem.clone());
                self.eng.unify(expected, &list, "in a '::' pattern")?;
                self.type_pattern(head, &elem)?;
                self.type_pattern(tail, &list)
            }
            // `[..]` patterns are type-directed like the literals: on an Array the
            // elements are bytes (Int) and `..rest` binds an Array; otherwise the
            // scrutinee is a List.
            Pattern::List { elems, rest } if self.is_array(expected) => {
                for e in *elems {
                    self.type_pattern(e, &Type::con(ty::INT))?;
                }
                if let Some(rest) = rest {
                    self.type_pattern(rest, &Type::con(ty::ARRAY))?;
                }
                Ok(())
            }
            Pattern::List { elems, rest } => {
                let elem = self.eng.fresh();
                let list = Type::app(Type::con(ty::LIST), elem.clone());
                self.eng.unify(expected, &list, "in a list pattern")?;
                for e in *elems {
                    self.type_pattern(e, &elem)?;
                }
                if let Some(rest) = rest {
                    self.type_pattern(rest, &list)?;
                }
                Ok(())
            }
            Pattern::Struct { ty, fields } => self.type_struct_pattern(ty, fields, expected),
            Pattern::Variant {
                ty, tag, fields, ..
            } => self.type_variant_pattern(*ty, tag, fields, expected),
        }
    }

    fn type_struct_pattern(
        &mut self,
        ty: &'a str,
        fields: &[FieldPat<'a>],
        expected: &Type,
    ) -> Result<()> {
        let info = match self.structs.get(ty).cloned() {
            Some(info) => info,
            None => return self.bind_field_patterns_loose(fields),
        };
        let (args, mut subst) = self.instantiate_params(&info.params);
        self.eng
            .unify(expected, &applied(ty, &args), "in a struct pattern")?;
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let want = self.struct_field_ty(&info, &mut subst, Some(name), i);
                    self.bind_field_pattern(pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = self.struct_field_ty(&info, &mut subst, None, i);
                    self.bind_field_pattern(pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let want = self
                        .struct_field_ty(&info, &mut subst, Some(name), i)
                        .unwrap_or_else(|| self.eng.fresh());
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    fn type_variant_pattern(
        &mut self,
        ty: Option<&'a str>,
        tag: &'a str,
        fields: &[FieldPat<'a>],
        expected: &Type,
    ) -> Result<()> {
        let resolved = match ty {
            Some(n) => self.variant_sig(n, tag),
            None => self
                .find_union_by_tag(tag)
                .and_then(|u| self.variant_sig(u, tag)),
        };
        let (result, payload) = match resolved {
            Some(r) => r,
            None => return self.bind_field_patterns_loose(fields),
        };
        self.eng.unify(expected, &result, "in a variant pattern")?;
        for (i, f) in fields.iter().enumerate() {
            match f {
                FieldPat::Named { name, pat } => {
                    let want = variant_field_ty(&payload, Some(name), i);
                    self.bind_field_pattern(pat, want)?;
                }
                FieldPat::Positional(pat) => {
                    let want = variant_field_ty(&payload, None, i);
                    self.bind_field_pattern(pat, want)?;
                }
                FieldPat::Shorthand(name) => {
                    let want = variant_field_ty(&payload, Some(name), i)
                        .unwrap_or_else(|| self.eng.fresh());
                    self.bind(name, want);
                }
            }
        }
        Ok(())
    }

    fn bind_field_pattern(&mut self, pat: &Pattern<'a>, want: Option<Type>) -> Result<()> {
        let want = want.unwrap_or_else(|| self.eng.fresh());
        self.type_pattern(pat, &want)
    }

    /// Fallback when a struct/variant pattern's type is unknown: bind inner
    /// variables against fresh types so names still resolve.
    fn bind_field_patterns_loose(&mut self, fields: &[FieldPat<'a>]) -> Result<()> {
        for f in fields {
            match f {
                FieldPat::Named { pat, .. } | FieldPat::Positional(pat) => {
                    let v = self.eng.fresh();
                    self.type_pattern(pat, &v)?;
                }
                FieldPat::Shorthand(name) => {
                    let v = self.eng.fresh();
                    self.bind(name, v);
                }
            }
        }
        Ok(())
    }

    // -- AST types ----------------------------------------------------------

    /// Convert a surface [`Ty`] to an engine [`Type`]. `tvars` maps each `` `a ``
    /// to a consistent fresh variable within one signature.
    fn ty_of_ast(&mut self, ty: &Ty<'a>, tvars: &mut HashMap<&'a str, Type>) -> Type {
        match ty {
            Ty::Con { name, .. } => match self.aliases.get(name).copied() {
                Some(alias) => self.ty_of_ast(alias, tvars),
                None => Type::con(canonical_con(name)),
            },
            Ty::Var(name) => tvars
                .entry(name)
                .or_insert_with(|| self.eng.fresh())
                .clone(),
            Ty::App(head, arg) => {
                Type::app(self.ty_of_ast(head, tvars), self.ty_of_ast(arg, tvars))
            }
            Ty::Arrow { from, to, .. } => {
                Type::arrow(self.ty_of_ast(from, tvars), self.ty_of_ast(to, tvars))
            }
            Ty::Unit => Type::con(ty::UNIT),
            Ty::Tuple(items) => {
                Type::Tuple(items.iter().map(|t| self.ty_of_ast(t, tvars)).collect())
            }
            // A one-field record parameter collapses to a bare parameter of that
            // field's type; several fields form a tuple.
            Ty::Record(fields) => {
                if let [f] = fields {
                    self.ty_of_ast(f.ty, tvars)
                } else {
                    Type::Tuple(fields.iter().map(|f| self.ty_of_ast(f.ty, tvars)).collect())
                }
            }
        }
    }

    // -- built-ins ----------------------------------------------------------

    fn install_builtins(&mut self) {
        let int = || Type::con(ty::INT);
        let real = || Type::con(ty::REAL);
        let bool_ = || Type::con(ty::BOOL);

        // Arithmetic is overloaded on Int and Real; the instance is chosen at
        // each use site from the operand types.
        for op in ["+", "-", "*", "/", "%"] {
            self.overloads.insert(
                op,
                vec![
                    Type::arrow(int(), Type::arrow(int(), int())),
                    Type::arrow(real(), Type::arrow(real(), real())),
                ],
            );
        }
        self.overloads.insert(
            "neg",
            vec![Type::arrow(int(), int()), Type::arrow(real(), real())],
        );
        self.bind("not", Type::arrow(bool_(), bool_()));

        // Sequence primitives, overloaded on byte arrays and strings (both are
        // byte vectors). A `.` in the shape marks the receiver; the reader ops
        // return Int, and the builder ops return the same sequence kind. Each op
        // gets one instance keyed on Array and one on Str.
        let prim = |mids: &[&str], returns_self: bool| {
            [ty::ARRAY, ty::STR].map(|recv| {
                let ret = if returns_self { recv } else { ty::INT };
                let params = std::iter::once(recv).chain(mids.iter().copied());
                Type::arrows(params.map(Type::con), Type::con(ret))
            })
        };
        self.overloads.insert("array_len", prim(&[], false).into());
        self.overloads
            .insert("array_get", prim(&[ty::INT], false).into());
        self.overloads
            .insert("array_push", prim(&[ty::INT], true).into());
        self.overloads
            .insert("array_set", prim(&[ty::INT, ty::INT], true).into());
        self.overloads
            .insert("array_slice", prim(&[ty::INT, ty::INT], true).into());

        // Vec primitives: an opaque, generic growable vector `Vec `T`. Unlike the
        // byte-oriented `array_*` ops, these are polymorphic in the element type,
        // so each is a single generalized binding.
        let vec = |eng: &mut Engine| {
            let t = eng.fresh_generic();
            (t.clone(), Type::app(Type::con(ty::VEC), t))
        };
        let (_t, vt) = vec(&mut self.eng);
        self.bind("vec_new", Type::arrow(Type::con(ty::UNIT), vt));
        let (t, vt) = vec(&mut self.eng);
        self.bind("vec_fill", Type::arrow(int(), Type::arrow(t, vt)));
        let (_t, vt) = vec(&mut self.eng);
        self.bind("vec_len", Type::arrow(vt, int()));
        let (t, vt) = vec(&mut self.eng);
        self.bind("vec_get", Type::arrow(vt, Type::arrow(int(), t)));
        let (t, vt) = vec(&mut self.eng);
        self.bind(
            "vec_set",
            Type::arrow(vt.clone(), Type::arrow(int(), Type::arrow(t, vt))),
        );
        let (t, vt) = vec(&mut self.eng);
        self.bind("vec_push", Type::arrow(vt.clone(), Type::arrow(t, vt)));

        // `true` / `false` are prelude aliases for the built-in Bool literals;
        // written bare they parse as ordinary variables.
        self.bind("true", bool_());
        self.bind("false", bool_());

        // Comparisons: `forall a. a -> a -> Bool`.
        for op in ["?=", "?<", "?>", "<=", ">="] {
            let a = self.eng.fresh_generic();
            let t = Type::arrow(a.clone(), Type::arrow(a, bool_()));
            self.bind(op, t);
        }
        // Concatenation `forall a. a -> a -> a` (lenient until overloading lands).
        {
            let a = self.eng.fresh_generic();
            self.bind("++", Type::arrow(a.clone(), Type::arrow(a.clone(), a)));
        }
        // Cons `forall a. a -> List a -> List a`.
        {
            let a = self.eng.fresh_generic();
            let list = Type::app(Type::con(ty::LIST), a.clone());
            self.bind("::", Type::arrow(a, Type::arrow(list.clone(), list)));
        }
        // Sequencing `forall a b. a -> b -> b`.
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            self.bind(";", Type::arrow(a, Type::arrow(b.clone(), b)));
        }
        // Pipes.
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            self.bind(
                "|>",
                Type::arrow(a.clone(), Type::arrow(Type::arrow(a, b.clone()), b)),
            );
        }
        {
            let a = self.eng.fresh_generic();
            let b = self.eng.fresh_generic();
            self.bind(
                "<|",
                Type::arrow(Type::arrow(a.clone(), b.clone()), Type::arrow(a, b)),
            );
        }
    }
}

impl<'a> Default for Checker<'a> {
    fn default() -> Checker<'a> {
        Checker::new()
    }
}

/// A global `$` definition, extracted from the program for dependency analysis.
#[derive(Clone, Copy)]
struct Def<'a> {
    name: &'a str,
    sig: Option<&'a Ty<'a>>,
    body: &'a Expr<'a>,
}

/// Build the reference graph over globals: `graph[i]` lists the definitions that
/// definition `i` refers to (edges point from a user to what it uses). Local
/// binders shadow globals, so a reference under a same-named binder is not an
/// edge. The analysis over-approximates in a few cases (e.g. `with`-scoped field
/// names): a spurious edge only merges components, which stays sound.
fn dependency_graph<'a>(defs: &[Def<'a>], index: &HashMap<&'a str, usize>) -> Vec<Vec<usize>> {
    defs.iter()
        .map(|def| {
            let mut out = Vec::new();
            let mut bound = Vec::new();
            free_globals(def.body, index, &mut bound, &mut out);
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect()
}

/// Collect the indices of global definitions referenced by `e`, skipping any
/// reference that a local binder in `bound` shadows.
fn free_globals<'a>(
    e: &Expr<'a>,
    globals: &HashMap<&'a str, usize>,
    bound: &mut Vec<&'a str>,
    out: &mut Vec<usize>,
) {
    match e {
        Expr::Var {
            module: None, name, ..
        } => {
            if !bound.contains(name) {
                if let Some(&idx) = globals.get(name) {
                    out.push(idx);
                }
            }
        }
        Expr::Int(_)
        | Expr::Real(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Unit
        | Expr::Var { .. }
        | Expr::Extern { .. } => {}

        Expr::App(f, x) => {
            free_globals(f, globals, bound, out);
            free_globals(x, globals, bound, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            free_globals(lhs, globals, bound, out);
            free_globals(rhs, globals, bound, out);
        }
        Expr::UnOp { operand, .. } => free_globals(operand, globals, bound, out),
        Expr::Tuple(items) | Expr::List(items) => items
            .iter()
            .for_each(|e| free_globals(e, globals, bound, out)),
        Expr::Array { size } => free_globals(size, globals, bound, out),
        Expr::Field { record, .. } => free_globals(record, globals, bound, out),
        Expr::StructLit { fields, spread, .. } => {
            free_globals_field_inits(fields, globals, bound, out);
            if let Some(s) = spread {
                free_globals(s, globals, bound, out);
            }
        }
        Expr::Variant { fields, .. } => free_globals_field_inits(fields, globals, bound, out),

        Expr::Let { bindings, body } => {
            // Recursive group: every binder is in scope for all values and body.
            let mark = bound.len();
            for b in *bindings {
                collect_pattern_binders(b.pat, bound);
            }
            for b in *bindings {
                free_globals(b.value, globals, bound, out);
            }
            free_globals(body, globals, bound, out);
            bound.truncate(mark);
        }
        Expr::If { cond, then, alt } => {
            free_globals(cond, globals, bound, out);
            free_globals(then, globals, bound, out);
            free_globals(alt, globals, bound, out);
        }
        Expr::Match {
            scrut,
            arms,
            default,
        } => {
            free_globals(scrut, globals, bound, out);
            for arm in *arms {
                let mark = bound.len();
                for pat in arm.patterns {
                    collect_pattern_binders(pat, bound);
                }
                if let Some(g) = arm.guard {
                    free_globals(g, globals, bound, out);
                }
                free_globals(arm.body, globals, bound, out);
                bound.truncate(mark);
            }
            if let Some(d) = default {
                free_globals(d, globals, bound, out);
            }
        }
        Expr::Lambda { params, body } => {
            let mark = bound.len();
            for p in *params {
                collect_pattern_binders(p, bound);
            }
            free_globals(body, globals, bound, out);
            bound.truncate(mark);
        }
        Expr::With { subject, body } => {
            free_globals(subject, globals, bound, out);
            free_globals(body, globals, bound, out);
        }
        Expr::Handle { body, .. } => free_globals(body, globals, bound, out),
        Expr::Defer { cleanup, body } => {
            free_globals(cleanup, globals, bound, out);
            free_globals(body, globals, bound, out);
        }
    }
}

fn free_globals_field_inits<'a>(
    fields: &[FieldInit<'a>],
    globals: &HashMap<&'a str, usize>,
    bound: &mut Vec<&'a str>,
    out: &mut Vec<usize>,
) {
    for f in fields {
        match f {
            FieldInit::Named { value, .. } => free_globals(value, globals, bound, out),
            FieldInit::Positional(v) => free_globals(v, globals, bound, out),
        }
    }
}

/// Push every name a pattern binds onto `bound`.
fn collect_pattern_binders<'a>(pat: &Pattern<'a>, bound: &mut Vec<&'a str>) {
    match pat {
        Pattern::Var(name) => bound.push(name),
        Pattern::StrPrefix { rest, .. } => collect_pattern_binders(rest, bound),
        Pattern::Cons { head, tail } => {
            collect_pattern_binders(head, bound);
            collect_pattern_binders(tail, bound);
        }
        Pattern::List { elems, rest } => {
            elems.iter().for_each(|p| collect_pattern_binders(p, bound));
            if let Some(r) = rest {
                collect_pattern_binders(r, bound);
            }
        }
        Pattern::Tuple(pats) => pats.iter().for_each(|p| collect_pattern_binders(p, bound)),
        Pattern::Struct { fields, .. } | Pattern::Variant { fields, .. } => {
            for f in *fields {
                match f {
                    syntax::FieldPat::Named { pat, .. } => collect_pattern_binders(pat, bound),
                    syntax::FieldPat::Positional(pat) => collect_pattern_binders(pat, bound),
                    syntax::FieldPat::Shorthand(name) => bound.push(name),
                }
            }
        }
        Pattern::Wild | Pattern::Int(_) | Pattern::Real(_) | Pattern::Str(_) | Pattern::Bool(_) => {
        }
    }
}

/// Apply a named type constructor to arguments: `applied("List", [Int])` is
/// `List Int`.
fn applied(name: &str, args: &[Type]) -> Type {
    args.iter()
        .fold(Type::con(name), |acc, a| Type::app(acc, a.clone()))
}

/// Map each type parameter to the corresponding argument, padding any missing
/// argument with a fresh variable.
fn subst_from_args<'a>(
    params: &[&'a str],
    args: &[Type],
    eng: &mut Engine,
) -> HashMap<&'a str, Type> {
    let mut subst = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        let a = args.get(i).cloned().unwrap_or_else(|| eng.fresh());
        subst.insert(*p, a);
    }
    subst
}

/// Collect the type variables appearing in `ty`, in order of first appearance.
fn collect_tyvars<'a>(ty: &Ty<'a>, out: &mut Vec<&'a str>) {
    match ty {
        Ty::Var(name) => {
            if !out.contains(name) {
                out.push(name);
            }
        }
        Ty::App(a, b) => {
            collect_tyvars(a, out);
            collect_tyvars(b, out);
        }
        Ty::Arrow { from, to, .. } => {
            collect_tyvars(from, out);
            collect_tyvars(to, out);
        }
        Ty::Tuple(items) => items.iter().for_each(|t| collect_tyvars(t, out)),
        Ty::Record(fields) => fields.iter().for_each(|f| collect_tyvars(f.ty, out)),
        Ty::Con { .. } | Ty::Unit => {}
    }
}

/// Normalize a variant payload into `(optional-name, type)` pairs. A bare single
/// type has no name; braced fields keep theirs; an empty payload is no fields.
fn payload_fields<'a>(p: &Payload<'a>) -> Vec<(Option<&'a str>, &'a Ty<'a>)> {
    match p {
        Payload::None => vec![],
        Payload::Bare(ty) => vec![(None, *ty)],
        Payload::Fields(fs) => fs.iter().map(|f| (f.name, f.ty)).collect(),
    }
}

/// Select a payload field's type by name (if named) or by position.
fn variant_field_ty(payload: &VariantPayload, name: Option<&str>, index: usize) -> Option<Type> {
    match name {
        Some(name) => payload
            .iter()
            .find(|(n, _)| *n == Some(name))
            .map(|(_, t)| t.clone()),
        None => payload.get(index).map(|(_, t)| t.clone()),
    }
}

/// Map the sigil/alias type constructors to their canonical built-in name.
fn canonical_con(name: &str) -> &str {
    match name {
        "@int64" | "@int32" | "Nat" => ty::INT,
        "@float64" | "@float32" => ty::REAL,
        "@str" => ty::STR,
        "@bool" => ty::BOOL,
        other => other,
    }
}

fn unbound(name: &str) -> Diagnostic {
    Diagnostic::error(
        Code::TypeUnbound,
        Span::at(0),
        0,
        format!("unbound name `{name}`"),
    )
}
